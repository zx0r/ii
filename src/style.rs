//! Native terminal presentation for Markdown-like prose.
//!
//! This module owns ANSI styling only. It never scans graphical spans, touches KGP,
//! or performs I/O. `CommonMark` owns inline syntax and reference resolution.

use memchr::memchr;

use crate::{
    err::Error,
    theme::{Palette, SGR_RESET, SemanticRole},
};

const MAX_TABLE_COLS: usize = 32;
const MAX_LINE_BYTES: usize = 1024 * 1024;

const fn check_line(line: &[u8], limit: usize) -> Result<(), Error> {
    if limit != usize::MAX && line.len() > MAX_LINE_BYTES {
        return Err(Error::ResourceLimit);
    }
    Ok(())
}

/// Stateful fence marker for an incremental prose stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    in_fence: bool,
    cols: u16,
    fence_len: usize,
    fence_byte: u8,
    fence_indent: Vec<u8>,
    syntax: CodeSyntax,
    references: References,
    table_ranges: Vec<core::ops::Range<usize>>,
    prepared: bool,
    pub full_width_tables: bool,
}

pub(crate) type References = std::collections::BTreeMap<String, (String, String)>;
type Cell<'a> =
    dyn FnMut(&[u8], usize, &References, &mut Vec<u8>, usize) -> Result<usize, Error> + 'a;
type InlineMath<'a> = dyn FnMut(core::ops::Range<usize>, &mut Vec<u8>) -> Result<bool, Error> + 'a;

impl State {
    #[must_use]
    pub const fn with_width(cols: u16) -> Self {
        Self {
            in_fence: false,
            cols,
            fence_len: 0,
            fence_byte: 0,
            fence_indent: Vec::new(),
            syntax: CodeSyntax::new(CodeLanguage::Plain),
            references: std::collections::BTreeMap::new(),
            table_ranges: Vec::new(),
            prepared: false,
            full_width_tables: false,
        }
    }
    pub fn prepare(&mut self, src: &[u8]) {
        if self.prepared {
            return;
        }
        self.prepared = true;
        if memchr::memchr2(b'[', b'|', src).is_none() {
            return;
        }
        if let Ok(text) = core::str::from_utf8(src) {
            let parser = pulldown_cmark::Parser::new_ext(text, pulldown_cmark::Options::all());
            for (name, definition) in parser.reference_definitions().iter() {
                self.references.insert(
                    name.to_lowercase(),
                    (
                        definition.dest.to_string(),
                        definition
                            .title
                            .as_ref()
                            .map_or_else(String::new, ToString::to_string),
                    ),
                );
            }
            if memchr(b'|', src).is_none() {
                return;
            }
            for (event, range) in parser.into_offset_iter() {
                if matches!(
                    event,
                    pulldown_cmark::Event::Start(pulldown_cmark::Tag::Table(_))
                ) {
                    self.table_ranges.push(range);
                }
            }
        }
    }

    pub(crate) fn overlaps_table(&self, start: usize, end: usize) -> bool {
        let index = self
            .table_ranges
            .partition_point(|range| range.end <= start);
        self.table_ranges
            .get(index)
            .is_some_and(|range| range.start < end)
    }
}

#[derive(Clone, Copy)]
struct Callout {
    tag: &'static [u8],
    role: SemanticRole,
    label: &'static [u8],
}

const CALLOUTS: [Callout; 17] = [
    Callout {
        tag: b"[!NOTE]",
        role: SemanticRole::Info,
        label: "󰋽 Note".as_bytes(),
    },
    Callout {
        tag: b"[!TIP]",
        role: SemanticRole::Success,
        label: "󰌶 Tip".as_bytes(),
    },
    Callout {
        tag: b"[!IMPORTANT]",
        role: SemanticRole::Highlight,
        label: "󰅾 Important".as_bytes(),
    },
    Callout {
        tag: b"[!WARNING]",
        role: SemanticRole::Warn,
        label: "󰀪 Warning".as_bytes(),
    },
    Callout {
        tag: b"[!CAUTION]",
        role: SemanticRole::Danger,
        label: "󰳦 Caution".as_bytes(),
    },
    Callout {
        tag: b"[!ABSTRACT]",
        role: SemanticRole::Info,
        label: "󰨸 Abstract".as_bytes(),
    },
    Callout {
        tag: b"[!SUMMARY]",
        role: SemanticRole::Info,
        label: "󰨸 Summary".as_bytes(),
    },
    Callout {
        tag: b"[!TLDR]",
        role: SemanticRole::Info,
        label: "󰨸 Tldr".as_bytes(),
    },
    Callout {
        tag: b"[!INFO]",
        role: SemanticRole::Info,
        label: "󰋽 Info".as_bytes(),
    },
    Callout {
        tag: b"[!TODO]",
        role: SemanticRole::Warn,
        label: "󰗡 Todo".as_bytes(),
    },
    Callout {
        tag: b"[!SUCCESS]",
        role: SemanticRole::Success,
        label: "󰄬 Success".as_bytes(),
    },
    Callout {
        tag: b"[!QUESTION]",
        role: SemanticRole::Warn,
        label: "󰘥 Question".as_bytes(),
    },
    Callout {
        tag: b"[!FAILURE]",
        role: SemanticRole::Danger,
        label: "󰅖 Failure".as_bytes(),
    },
    Callout {
        tag: b"[!DANGER]",
        role: SemanticRole::Danger,
        label: "󱐌 Danger".as_bytes(),
    },
    Callout {
        tag: b"[!BUG]",
        role: SemanticRole::Danger,
        label: "󰨰 Bug".as_bytes(),
    },
    Callout {
        tag: b"[!EXAMPLE]",
        role: SemanticRole::Highlight,
        label: "󰉹 Example".as_bytes(),
    },
    Callout {
        tag: b"[!QUOTE]",
        role: SemanticRole::TextMuted,
        label: "󱆨 Quote".as_bytes(),
    },
];

#[inline]
fn trim_start(src: &[u8]) -> &[u8] {
    let mut n = 0;
    while n < src.len() && matches!(src[n], b' ' | b'\t') {
        n += 1;
    }
    &src[n..]
}

#[inline]
fn indent(src: &[u8]) -> (&[u8], &[u8]) {
    let rest = trim_start(src);
    (&src[..src.len() - rest.len()], rest)
}

#[inline]
fn role(out: &mut Vec<u8>, palette: &Palette, role: SemanticRole) {
    palette.write_sgr_fg(out, role);
}

#[inline]
fn bold(out: &mut Vec<u8>, palette: &Palette, role_: SemanticRole) {
    role(out, palette, role_);
    out.extend_from_slice(b"\x1b[1m");
}

fn inline(src: &[u8], palette: &Palette, out: &mut Vec<u8>, references: &References) {
    let _ = inline_with_math(src, palette, out, references, None);
}

pub(crate) fn inline_with_math(
    src: &[u8],
    palette: &Palette,
    out: &mut Vec<u8>,
    references: &References,
    mut math: Option<&mut InlineMath<'_>>,
) -> Result<(), Error> {
    use pulldown_cmark::{BrokenLink, Event, Options, Parser, Tag, TagEnd};
    if !src.iter().any(|&b| {
        matches!(
            b,
            b'*' | b'_' | b'~' | b'`' | b'[' | b'<' | b'&' | b'\\' | 127
        ) || b < 32 && b != b'\t'
            || b == b'$' && math.is_some()
    }) {
        out.extend_from_slice(src);
        return Ok(());
    }
    let Ok(text) = core::str::from_utf8(src) else {
        emit_literal(src, out);
        return Ok(());
    };
    let resolver = |broken: BrokenLink<'_>| {
        references
            .get(&broken.reference.to_lowercase())
            .map(|(url, title)| (url.clone().into(), title.clone().into()))
    };
    for (event, range) in
        Parser::new_with_broken_link_callback(text, Options::all(), Some(resolver))
            .into_offset_iter()
    {
        match event {
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => {
                emit_literal(t.as_bytes(), out);
            }
            Event::Code(t) => {
                role(out, palette, SemanticRole::Warn);
                emit_literal(t.as_bytes(), out);
                out.extend_from_slice(SGR_RESET);
            }
            Event::Start(Tag::Strong) => out.extend_from_slice(b"\x1b[1m"),
            Event::End(TagEnd::Strong) => out.extend_from_slice(b"\x1b[22m"),
            Event::Start(Tag::Emphasis) => out.extend_from_slice(b"\x1b[3m"),
            Event::End(TagEnd::Emphasis) => out.extend_from_slice(b"\x1b[23m"),
            Event::Start(Tag::Strikethrough) => out.extend_from_slice(b"\x1b[9m"),
            Event::End(TagEnd::Strikethrough) => out.extend_from_slice(b"\x1b[29m"),
            Event::Start(Tag::Link { .. }) => {
                role(out, palette, SemanticRole::Info);
                out.extend_from_slice(b"\x1b[4m");
            }
            Event::End(TagEnd::Link) => out.extend_from_slice(b"\x1b[24;39m"),
            Event::Start(Tag::Image { .. }) => out.extend_from_slice("▧ ".as_bytes()),
            Event::FootnoteReference(t) => {
                out.push(b'[');
                emit_literal(t.as_bytes(), out);
                out.push(b']');
            }
            Event::Start(Tag::FootnoteDefinition(t)) => {
                out.push(b'[');
                emit_literal(t.as_bytes(), out);
                out.extend_from_slice(b"] ");
            }
            Event::Start(Tag::List(Some(n))) => {
                crate::fmt::push_u64(out, n);
                out.extend_from_slice(b". ");
            }
            Event::SoftBreak => out.push(b' '),
            Event::HardBreak => out.push(b'\n'),
            Event::InlineMath(t) | Event::DisplayMath(t) => {
                if let Some(render) = &mut math
                    && render(range, out)?
                {
                    continue;
                }
                out.push(b'$');
                emit_literal(t.as_bytes(), out);
                out.push(b'$');
            }
            Event::TaskListMarker(yes) => out.extend_from_slice(if yes {
                "󰄵 ".as_bytes()
            } else {
                "󰄱 ".as_bytes()
            }),
            _ => {}
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
}

fn table_cells<'a>(line: &'a [u8], cells: &mut [&'a [u8]; MAX_TABLE_COLS]) -> usize {
    let mut line = line.trim_ascii();
    if let Some(rest) = line.strip_prefix(b"|") {
        line = rest;
    }
    if line.ends_with(b"|")
        && line[..line.len() - 1]
            .iter()
            .rev()
            .take_while(|&&b| b == b'\\')
            .count()
            % 2
            == 0
    {
        line = &line[..line.len() - 1];
    }
    let mut count = 0;
    let mut start = 0;
    let mut escaped = false;
    for (i, &byte) in line.iter().enumerate() {
        if byte == b'|' && !escaped {
            if count == cells.len() {
                return 0;
            }
            cells[count] = line[start..i].trim_ascii();
            count += 1;
            start = i + 1;
        }
        escaped = byte == b'\\' && !escaped;
    }
    if count == cells.len() {
        return 0;
    }
    cells[count] = line[start..].trim_ascii();
    count + 1
}

fn table_sep(line: &[u8]) -> bool {
    let mut cells = [&b""[..]; MAX_TABLE_COLS];
    let count = table_cells(line, &mut cells);
    count > 0
        && cells[..count].iter().all(|cell| {
            let dashes = cell.strip_prefix(b":").unwrap_or(cell);
            let dashes = dashes.strip_suffix(b":").unwrap_or(dashes);
            !dashes.is_empty() && dashes.iter().all(|&b| b == b'-')
        })
}

pub(crate) fn width(src: &[u8]) -> usize {
    let mut visible = 0;
    let mut pos = 0;
    while pos < src.len() {
        if src[pos..].starts_with(b"\x1b[")
            && let Some(end) = memchr(b'm', &src[pos + 2..])
        {
            pos += end + 3;
        } else {
            let end = memchr(0x1b, &src[pos..])
                .map_or(src.len(), |i| pos + i)
                .max(pos + 1);
            visible += core::str::from_utf8(&src[pos..end]).map_or(end - pos, |text| {
                text.chars().map(crate::viewport::cells).sum()
            });
            pos = end;
        }
    }
    visible
}

fn border(
    indent: &[u8],
    widths: &[usize],
    left: &[u8],
    mid: &[u8],
    right: &[u8],
    palette: &Palette,
    out: &mut Vec<u8>,
) {
    out.extend_from_slice(indent);
    role(out, palette, SemanticRole::Border);
    out.extend_from_slice(left);
    for (idx, width) in widths.iter().enumerate() {
        for _ in 0..width.saturating_add(2) {
            out.extend_from_slice("─".as_bytes());
        }
        out.extend_from_slice(if idx + 1 == widths.len() { right } else { mid });
    }
    out.extend_from_slice(SGR_RESET);
    out.push(b'\n');
}

#[expect(
    clippy::too_many_arguments,
    reason = "row layout, content and styling remain explicit without an extra context type"
)]
fn table_row(
    indent: &[u8],
    cells: &[&[u8]],
    widths: &[usize],
    aligns: &[Align],
    header: bool,
    references: &References,
    palette: &Palette,
    out: &mut Vec<u8>,
    cell: &mut Cell<'_>,
    limit: &mut usize,
) -> Result<(), Error> {
    out.extend_from_slice(indent);
    role(out, palette, SemanticRole::Border);
    out.extend_from_slice("│".as_bytes());
    let mut rendered = Vec::new();
    let mut clipped = Vec::new();
    for idx in 0..widths.len() {
        let raw = cells.get(idx).copied().unwrap_or(b"");
        rendered.clear();
        clipped.clear();
        *limit = (*limit).min(cell(
            raw,
            widths[idx],
            references,
            &mut rendered,
            out.len(),
        )?);
        crate::viewport::append(
            &rendered,
            u16::try_from(widths[idx]).unwrap_or(u16::MAX),
            0,
            1,
            &mut clipped,
        );
        if clipped.last() == Some(&b'\n') {
            clipped.pop();
        }
        let pad = widths[idx].saturating_sub(width(&clipped));
        let (before, after) = match aligns[idx] {
            Align::Left => (0, pad),
            Align::Center => (pad / 2, pad - pad / 2),
            Align::Right => (pad, 0),
        };
        // One cell also emits borders, padding, and up to three SGR changes.
        if out
            .len()
            .saturating_add(pad)
            .saturating_add(clipped.len())
            .saturating_add(80)
            > *limit
        {
            return Err(Error::ResourceLimit);
        }
        out.push(b' ');
        for _ in 0..before {
            out.push(b' ');
        }
        if header {
            bold(out, palette, SemanticRole::Info);
        } else {
            role(out, palette, SemanticRole::TextPrimary);
        }
        out.extend_from_slice(&clipped);
        out.extend_from_slice(SGR_RESET);
        role(out, palette, SemanticRole::Border);
        for _ in 0..after {
            out.push(b' ');
        }
        out.push(b' ');
        out.extend_from_slice("│".as_bytes());
    }
    out.extend_from_slice(SGR_RESET);
    out.push(b'\n');
    Ok(())
}

fn fit_widths(widths: &mut [usize], cols_limit: u16, indent: usize) {
    if cols_limit == 0 || widths.is_empty() {
        return;
    }
    let budget = usize::from(cols_limit)
        .saturating_sub(indent + widths.len() * 3 + 1)
        .max(widths.len());
    for width in widths.iter_mut() {
        *width = (*width).max(1);
    }
    if widths.iter().sum::<usize>() <= budget {
        return;
    }
    // Water-fill by a shared cap: O(columns * log(max_width)), independent of
    // how many millions of source cells have to be removed.
    let mut low = 1;
    let mut high = widths.iter().copied().max().unwrap_or(1);
    while low < high {
        let cap = low + (high - low).div_ceil(2);
        if widths.iter().map(|&w| w.min(cap)).sum::<usize>() <= budget {
            low = cap;
        } else {
            high = cap - 1;
        }
    }
    let mut spare = budget - widths.iter().map(|&w| w.min(low)).sum::<usize>();
    for width in widths {
        let extra = usize::from(*width > low && spare > 0);
        *width = (*width).min(low) + extra;
        spare -= extra;
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "table shares the caller's styling and byte budget"
)]
#[expect(
    clippy::too_many_lines,
    reason = "width measurement and row emission share one parsed table"
)]
fn emit_table(
    src: &[u8],
    pos: &mut usize,
    cols_limit: u16,
    full_width: bool,
    references: &References,
    palette: &Palette,
    out: &mut Vec<u8>,
    limit: &mut usize,
    cell: &mut Cell<'_>,
) -> Result<bool, Error> {
    let first_end = memchr(b'\n', &src[*pos..]).map_or(src.len(), |n| *pos + n);
    let second_start = first_end.saturating_add(1);
    if second_start >= src.len() {
        return Ok(false);
    }
    let second_end = memchr(b'\n', &src[second_start..]).map_or(src.len(), |n| second_start + n);
    let first = &src[*pos..first_end];
    check_line(first, *limit)?;
    check_line(&src[second_start..second_end], *limit)?;
    if !first.contains(&b'|') || !table_sep(&src[second_start..second_end]) {
        return Ok(false);
    }
    let (indent_, _) = indent(first);
    let mut header = [&b""[..]; MAX_TABLE_COLS];
    let mut separator = [&b""[..]; MAX_TABLE_COLS];
    let cols = table_cells(first, &mut header);
    let sep_cols = table_cells(&src[second_start..second_end], &mut separator);
    if cols == 0 || cols != sep_cols {
        return Ok(false);
    }
    let mut widths = [0usize; MAX_TABLE_COLS];
    let mut aligns = [Align::Left; MAX_TABLE_COLS];
    for idx in 0..cols {
        widths[idx] = width(header[idx]);
        aligns[idx] = match (
            separator[idx].starts_with(b":"),
            separator[idx].ends_with(b":"),
        ) {
            (true, true) => Align::Center,
            (false, true) => Align::Right,
            _ => Align::Left,
        };
    }
    let data_start = second_end.saturating_add(1);
    let mut end = data_start;
    let mut cells = [&b""[..]; MAX_TABLE_COLS];
    while end <= src.len() {
        let line_end = memchr(b'\n', &src[end..]).map_or(src.len(), |n| end + n);
        let line = &src[end..line_end];
        check_line(line, *limit)?;
        if !line.contains(&b'|') || table_sep(line) {
            break;
        }
        let count = table_cells(line, &mut cells);
        if count != cols {
            break;
        }
        for idx in 0..cols {
            widths[idx] = widths[idx].max(width(cells[idx]));
        }
        end = line_end.saturating_add(1);
        if line_end == src.len() {
            break;
        }
    }
    fit_widths(&mut widths[..cols], cols_limit, width(indent_));
    if full_width && cols_limit > 0 {
        let budget = usize::from(cols_limit).saturating_sub(width(indent_) + cols * 3 + 1);
        let spare = budget.saturating_sub(widths[..cols].iter().sum::<usize>());
        for (index, width) in widths[..cols].iter_mut().enumerate() {
            *width += spare / cols + usize::from(index < spare % cols);
        }
    }
    border(
        indent_,
        &widths[..cols],
        "┌".as_bytes(),
        "┬".as_bytes(),
        "┐".as_bytes(),
        palette,
        out,
    );
    table_row(
        indent_,
        &header[..cols],
        &widths[..cols],
        &aligns[..cols],
        true,
        references,
        palette,
        out,
        cell,
        limit,
    )?;
    border(
        indent_,
        &widths[..cols],
        "├".as_bytes(),
        "┼".as_bytes(),
        "┤".as_bytes(),
        palette,
        out,
    );
    let mut row_start = data_start;
    while row_start < end {
        if out.len() > *limit {
            return Err(Error::ResourceLimit);
        }
        let row_end = memchr(b'\n', &src[row_start..]).map_or(src.len(), |n| row_start + n);
        let count = table_cells(&src[row_start..row_end], &mut cells);
        if count != cols {
            break;
        }
        table_row(
            indent_,
            &cells[..cols],
            &widths[..cols],
            &aligns[..cols],
            false,
            references,
            palette,
            out,
            cell,
            limit,
        )?;
        row_start = row_end.saturating_add(1);
    }
    border(
        indent_,
        &widths[..cols],
        "└".as_bytes(),
        "┴".as_bytes(),
        "┘".as_bytes(),
        palette,
        out,
    );
    if out.len() > *limit {
        return Err(Error::ResourceLimit);
    }
    *pos = end;
    Ok(true)
}

fn heading(line: &[u8]) -> Option<(usize, &[u8])> {
    let marker = line.iter().take_while(|&&b| b == b'#').count();
    if (1..=6).contains(&marker) && line.get(marker) == Some(&b' ') {
        Some((marker, &line[marker + 1..]))
    } else {
        let marker = line.iter().take_while(|&&b| b == b'=').count();
        if (1..=6).contains(&marker) && line.get(marker) == Some(&b' ') {
            Some((marker, &line[marker + 1..]))
        } else {
            None
        }
    }
}

fn emit_heading(
    indent_: &[u8],
    level: usize,
    body: &[u8],
    palette: &Palette,
    out: &mut Vec<u8>,
    references: &References,
) {
    const ICONS: [&[u8]; 6] = [
        "󰲡 ".as_bytes(),
        "󰲣 ".as_bytes(),
        "󰲥 ".as_bytes(),
        "󰲧 ".as_bytes(),
        "󰲩 ".as_bytes(),
        "󰲫 ".as_bytes(),
    ];
    out.extend_from_slice(indent_);
    let token = match level {
        1 => crate::theme::ColorToken::INFO,
        2 => crate::theme::ColorToken::SUCCESS,
        3 => crate::theme::ColorToken::WARN,
        4 => crate::theme::ColorToken::DANGER,
        _ => crate::theme::ColorToken::TEXT_MUTED,
    };
    let seq = &palette.sgr[token.index()];
    out.extend_from_slice(&seq.buf[..usize::from(seq.len)]);
    out.extend_from_slice(b"\x1b[1m");
    out.extend_from_slice(ICONS[level - 1]);
    inline(body, palette, out, references);
    out.extend_from_slice(SGR_RESET);
}

fn callout(
    body: &[u8],
    indent_: &[u8],
    palette: &Palette,
    out: &mut Vec<u8>,
    references: &References,
) {
    let body = body.strip_prefix(b" ").unwrap_or(body);
    let found = CALLOUTS
        .iter()
        .find(|x| body.len() >= x.tag.len() && body[..x.tag.len()].eq_ignore_ascii_case(x.tag));
    out.extend_from_slice(indent_);
    if let Some(x) = found {
        role(out, palette, x.role);
        out.extend_from_slice("│ ".as_bytes());
        bold(out, palette, x.role);
        out.extend_from_slice(x.label);
        let tail = &body[x.tag.len()..];
        if !tail.is_empty() {
            out.extend_from_slice(b"\x1b[3m");
            inline(tail, palette, out, references);
        }
    } else {
        role(out, palette, SemanticRole::Info);
        out.extend_from_slice("│ \x1b[3m".as_bytes());
        inline(body, palette, out, references);
    }
    out.extend_from_slice(SGR_RESET);
}

fn task(line: &[u8]) -> Option<(&[u8], SemanticRole, bool)> {
    let rest = line
        .strip_prefix(b"- ")
        .or_else(|| line.strip_prefix(b"* "))?;
    let (needle, role_) = if let Some(x) = rest
        .strip_prefix(b"[x] ")
        .or_else(|| rest.strip_prefix(b"[X] "))
    {
        (x, SemanticRole::Success)
    } else if let Some(x) = rest.strip_prefix(b"[ ] ") {
        (x, SemanticRole::TextMuted)
    } else if let Some(x) = rest.strip_prefix(b"[-] ") {
        (x, SemanticRole::Warn)
    } else if let Some(x) = rest.strip_prefix(b"[!] ") {
        (x, SemanticRole::Danger)
    } else {
        let x = rest.strip_prefix(b"[~] ")?;
        (x, SemanticRole::TextMuted)
    };
    Some((
        needle,
        role_,
        matches!(rest.get(1), Some(b'x' | b'X' | b'-' | b'!')),
    ))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CodeLanguage {
    #[default]
    Plain,
    Python,
    Rust,
    JavaScript,
    Shell,
    Sql,
    Json,
}

impl CodeLanguage {
    const fn from_tag(tag: &[u8]) -> Self {
        match tag {
            b"python" | b"py" => Self::Python,
            b"rust" | b"rs" => Self::Rust,
            b"js" | b"javascript" | b"jsx" | b"ts" | b"typescript" | b"tsx" => Self::JavaScript,
            b"sh" | b"bash" | b"zsh" | b"shell" => Self::Shell,
            b"sql" => Self::Sql,
            b"json" => Self::Json,
            _ => Self::Plain,
        }
    }

    fn keyword(self, word: &[u8]) -> bool {
        let keywords: &[&[u8]] = match self {
            Self::Python => &[
                b"def", b"class", b"import", b"from", b"as", b"return", b"for", b"in", b"if",
                b"else", b"elif", b"while", b"with", b"try", b"except", b"finally", b"True",
                b"False", b"None", b"async", b"await", b"lambda", b"yield", b"raise", b"pass",
                b"not", b"and", b"or",
            ],
            Self::Rust => &[
                b"fn", b"let", b"mut", b"pub", b"use", b"struct", b"enum", b"impl", b"trait",
                b"match", b"const", b"async", b"await", b"return", b"if", b"else", b"for", b"in",
                b"while", b"loop", b"move", b"unsafe", b"mod", b"true", b"false", b"Self", b"self",
            ],
            Self::JavaScript => &[
                b"function",
                b"let",
                b"const",
                b"var",
                b"class",
                b"return",
                b"import",
                b"export",
                b"from",
                b"if",
                b"else",
                b"for",
                b"in",
                b"of",
                b"new",
                b"async",
                b"await",
                b"true",
                b"false",
                b"null",
                b"undefined",
                b"interface",
                b"type",
            ],
            Self::Shell => &[
                b"if",
                b"then",
                b"elif",
                b"else",
                b"fi",
                b"for",
                b"in",
                b"do",
                b"done",
                b"while",
                b"case",
                b"esac",
                b"function",
            ],
            Self::Sql => &[
                b"select", b"from", b"where", b"group", b"by", b"order", b"limit", b"join", b"on",
                b"having", b"as", b"insert", b"into", b"update", b"delete", b"create", b"table",
                b"null", b"and", b"or",
            ],
            Self::Json => &[b"true", b"false", b"null"],
            Self::Plain => &[],
        };
        keywords.iter().any(|key| {
            if self == Self::Sql {
                word.eq_ignore_ascii_case(key)
            } else {
                word == *key
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CodeSyntax {
    language: CodeLanguage,
    closing: &'static [u8],
    color: Option<SemanticRole>,
}

fn quoted_end(src: &[u8], mut pos: usize, delimiter: &[u8]) -> (usize, bool) {
    while pos < src.len() {
        if src[pos..].starts_with(delimiter) {
            return (pos + delimiter.len(), true);
        }
        pos += if src[pos] == b'\\' && pos + 1 < src.len() {
            2
        } else {
            1
        };
    }
    (pos, false)
}

impl CodeSyntax {
    const fn new(language: CodeLanguage) -> Self {
        Self {
            language,
            closing: b"",
            color: None,
        }
    }

    fn line(&mut self, src: &[u8], palette: &Palette, out: &mut Vec<u8>) {
        let lang = self.language;
        if lang == CodeLanguage::Plain {
            out.extend_from_slice(src);
            return;
        }
        let mut pos = 0;
        while pos < src.len() {
            let start = pos;
            let tail = &src[pos..];
            let byte = tail[0];
            let color = if let Some(color) = self.color {
                let closed;
                (pos, closed) = if color == SemanticRole::TextMuted {
                    memchr::memmem::find(tail, self.closing).map_or((src.len(), false), |end| {
                        (pos + end + self.closing.len(), true)
                    })
                } else {
                    quoted_end(src, pos, self.closing)
                };
                if closed {
                    self.color = None;
                }
                Some(color)
            } else if (matches!(lang, CodeLanguage::Rust | CodeLanguage::JavaScript)
                && tail.starts_with(b"//"))
                || (lang == CodeLanguage::Sql && tail.starts_with(b"--"))
                || (byte == b'#'
                    && (lang == CodeLanguage::Python
                        || (lang == CodeLanguage::Shell
                            && (pos == 0 || src[pos - 1].is_ascii_whitespace()))))
            {
                pos = src.len();
                Some(SemanticRole::TextMuted)
            } else if matches!(
                lang,
                CodeLanguage::Rust | CodeLanguage::JavaScript | CodeLanguage::Sql
            ) && tail.starts_with(b"/*")
            {
                self.closing = b"*/";
                self.color = Some(SemanticRole::TextMuted);
                pos += 2;
                role(out, palette, SemanticRole::TextMuted);
                out.extend_from_slice(b"/*");
                out.extend_from_slice(SGR_RESET);
                continue;
            } else if lang == CodeLanguage::Rust
                && byte == b'\''
                && tail
                    .get(1)
                    .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
                && tail.get(2) != Some(&b'\'')
            {
                pos += 1;
                None
            } else if byte == b'"'
                || (byte == b'\'' && lang != CodeLanguage::Json)
                || (byte == b'`' && matches!(lang, CodeLanguage::JavaScript | CodeLanguage::Shell))
            {
                let delimiter: &'static [u8] = match byte {
                    b'"' if lang == CodeLanguage::Python && tail.starts_with(b"\"\"\"") => {
                        b"\"\"\""
                    }
                    b'\'' if lang == CodeLanguage::Python && tail.starts_with(b"'''") => b"'''",
                    b'"' => b"\"",
                    b'\'' => b"'",
                    _ => b"`",
                };
                let (end, closed) = quoted_end(src, pos + delimiter.len(), delimiter);
                pos = end;
                if !closed && (delimiter.len() > 1 || byte == b'`') {
                    self.closing = delimiter;
                    self.color = Some(SemanticRole::Success);
                }
                (closed || lang != CodeLanguage::Rust).then_some(SemanticRole::Success)
            } else if byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 128 {
                pos += 1;
                while src.get(pos).is_some_and(|b| {
                    b.is_ascii_alphanumeric()
                        || *b == b'_'
                        || *b >= 128
                        || (byte.is_ascii_digit() && *b == b'.')
                }) {
                    pos += 1;
                }
                if byte.is_ascii_digit() {
                    Some(SemanticRole::Warn)
                } else {
                    lang.keyword(&src[start..pos])
                        .then_some(SemanticRole::Accent)
                }
            } else {
                pos += 1;
                None
            };
            if let Some(color) = color {
                role(out, palette, color);
            }
            out.extend_from_slice(&src[start..pos]);
            if color.is_some() {
                out.extend_from_slice(SGR_RESET);
            }
        }
    }
}

fn prefix_cells(src: &[u8], limit: usize) -> (usize, usize) {
    let Ok(text) = core::str::from_utf8(src) else {
        return (0, 0);
    };
    let mut used = 0;
    let mut end = 0;
    for (off, ch) in text.char_indices() {
        let cells = crate::viewport::cells(ch);
        if used + cells > limit {
            break;
        }
        used += cells;
        end = off + ch.len_utf8();
    }
    (end, used)
}

fn emit_code(clean: &[u8], palette: &Palette, state: &mut State, out: &mut Vec<u8>) {
    let indent_ = state.fence_indent.as_slice();
    let cols_limit = state.cols;
    let budget = if cols_limit == 0 {
        58
    } else {
        usize::from(cols_limit)
            .saturating_sub(width(indent_))
            .max(4)
    };
    let inner = budget.saturating_sub(4).max(1);
    let start_row = |out: &mut Vec<u8>| {
        out.extend_from_slice(indent_);
        role(out, palette, SemanticRole::Border);
        out.extend_from_slice("│ ".as_bytes());
        role(out, palette, SemanticRole::TextMuted);
    };
    let end_row = |out: &mut Vec<u8>, cells| {
        out.extend_from_slice(SGR_RESET);
        out.resize(out.len() + inner.saturating_sub(cells), b' ');
        role(out, palette, SemanticRole::Border);
        out.extend_from_slice(" │".as_bytes());
        out.extend_from_slice(SGR_RESET);
    };
    let cells = width(clean);
    if cells <= inner && memchr(b'\t', clean).is_none() {
        start_row(out);
        state.syntax.line(clean, palette, out);
        end_row(out, cells);
        return;
    }
    let mut styled = Vec::new();
    state.syntax.line(clean, palette, &mut styled);
    let mut wrapped = Vec::new();
    crate::viewport::append(
        &styled,
        u16::try_from(inner).unwrap_or(u16::MAX),
        0,
        0,
        &mut wrapped,
    );
    for (index, line) in wrapped.split(|&b| b == b'\n').enumerate() {
        if index > 0 {
            out.push(b'\n');
        }
        start_row(out);
        out.extend_from_slice(line);
        end_row(out, width(line));
    }
}

fn emit_fence(
    indent_: &[u8],
    clean: &[u8],
    count: usize,
    palette: &Palette,
    state: &mut State,
    out: &mut Vec<u8>,
) {
    if !state.in_fence {
        state.fence_indent.extend_from_slice(indent_);
    }
    let indent_ = state.fence_indent.as_slice();
    out.extend_from_slice(indent_);
    role(out, palette, SemanticRole::Border);
    let budget = if state.cols == 0 {
        58
    } else {
        usize::from(state.cols)
            .saturating_sub(width(indent_))
            .max(1)
    };
    let mut used = 1;
    out.extend_from_slice(if state.in_fence {
        "╰".as_bytes()
    } else {
        "╭".as_bytes()
    });
    if !state.in_fence {
        state.fence_len = count;
        state.fence_byte = clean[0];
        let info = trim_start(&clean[count..]);
        let lang = info
            .strip_prefix(b"{")
            .unwrap_or(info)
            .split(|b| b.is_ascii_whitespace() || matches!(*b, b',' | b'}'))
            .next()
            .unwrap_or(b"");
        state.syntax = CodeSyntax::new(CodeLanguage::from_tag(lang));
        if !lang.is_empty() && budget >= 8 {
            out.extend_from_slice("─[".as_bytes());
            used += 2;
            bold(out, palette, SemanticRole::Info);
            let (end, cells) = prefix_cells(lang, budget.saturating_sub(used + 2));
            used += cells;
            out.extend_from_slice(&lang[..end]);
            out.extend_from_slice(SGR_RESET);
            role(out, palette, SemanticRole::Border);
            out.extend_from_slice(b"]");
            used += 1;
        }
    }
    for _ in used..budget.saturating_sub(1) {
        out.extend_from_slice("─".as_bytes());
    }
    out.extend_from_slice(if state.in_fence {
        "╯".as_bytes()
    } else {
        "╮".as_bytes()
    });
    state.in_fence = !state.in_fence;
    if !state.in_fence {
        state.fence_indent.clear();
        state.syntax = CodeSyntax::default();
    }
    out.extend_from_slice(SGR_RESET);
}

fn emit_line(line: &[u8], palette: &Palette, state: &mut State, out: &mut Vec<u8>) {
    let (indent_, clean) = indent(line);
    let marker = clean.first().copied().unwrap_or(0);
    let count = clean.iter().take_while(|&&b| b == marker).count();
    let fence = matches!(marker, b'`' | b'~')
        && count >= 3
        && (!state.in_fence
            || (marker == state.fence_byte
                && count >= state.fence_len
                && clean[count..].trim_ascii().is_empty()));
    if fence {
        emit_fence(indent_, clean, count, palette, state, out);
    } else if state.in_fence {
        let code = line
            .strip_prefix(state.fence_indent.as_slice())
            .unwrap_or(line);
        emit_code(code, palette, state, out);
    } else if let Some((level, body)) = heading(clean) {
        emit_heading(indent_, level, body, palette, out, &state.references);
    } else if let Some(body) = clean.strip_prefix(b">") {
        callout(body, indent_, palette, out, &state.references);
    } else if let Some((body, role_, emph)) = task(clean) {
        out.extend_from_slice(indent_);
        if emph {
            bold(out, palette, role_);
        } else {
            role(out, palette, role_);
        }
        let icon = match clean.get(3) {
            Some(b'x' | b'X') => "󰄵 ",
            Some(b'-') => "󰥔 ",
            Some(b'!') => "󰀨 ",
            Some(b'~') => "󰰱 ",
            _ => "󰄱 ",
        };
        out.extend_from_slice(icon.as_bytes());
        out.extend_from_slice(SGR_RESET);
        inline(body, palette, out, &state.references);
    } else if let Some(body) = clean
        .strip_prefix(b"- ")
        .or_else(|| clean.strip_prefix(b"* "))
    {
        out.extend_from_slice(indent_);
        bold(out, palette, SemanticRole::Info);
        out.extend_from_slice("● ".as_bytes());
        out.extend_from_slice(SGR_RESET);
        inline(body, palette, out, &state.references);
    } else if clean.len() >= 3
        && clean
            .iter()
            .all(|b| matches!(*b, b'-' | b'*' | b'_' | b' '))
    {
        out.extend_from_slice(indent_);
        role(out, palette, SemanticRole::Border);
        let cols = if state.cols == 0 {
            56
        } else {
            usize::from(state.cols).saturating_sub(width(indent_))
        };
        for _ in 0..cols {
            out.extend_from_slice("─".as_bytes());
        }
        out.extend_from_slice(SGR_RESET);
    } else if clean.starts_with(b"%") || clean.starts_with(b"//") {
        out.extend_from_slice(indent_);
        role(out, palette, SemanticRole::TextMuted);
        out.extend_from_slice(clean);
        out.extend_from_slice(SGR_RESET);
    } else {
        inline(line, palette, out, &state.references);
    }
}

/// Append styled prose to `out`. The caller owns the reusable `State`.
pub fn emit_prose(src: &[u8], palette: &Palette, state: &mut State, out: &mut Vec<u8>) {
    if src
        .iter()
        .any(|&b| b < 32 && b != b'\n' && b != b'\t' || b == 127)
    {
        let mut safe = Vec::with_capacity(src.len());
        emit_literal(src, &mut safe);
        emit_safe_prose(&safe, palette, state, out);
    } else {
        emit_safe_prose(src, palette, state, out);
    }
}

fn emit_safe_prose(src: &[u8], palette: &Palette, state: &mut State, out: &mut Vec<u8>) {
    emit_blocks(src, palette, state, out, |_, _, _, _| {});
}

/// Emit source-anchored native blocks; a table is one block, not independent rows.
pub fn emit_blocks(
    src: &[u8],
    palette: &Palette,
    state: &mut State,
    out: &mut Vec<u8>,
    block: impl FnMut(usize, usize, usize, usize),
) {
    let _ = emit_blocks_bounded(src, palette, state, out, usize::MAX, block);
}

pub(crate) fn emit_blocks_bounded(
    src: &[u8],
    palette: &Palette,
    state: &mut State,
    out: &mut Vec<u8>,
    limit: usize,
    block: impl FnMut(usize, usize, usize, usize),
) -> Result<(), Error> {
    emit_blocks_with_cells(
        src,
        palette,
        state,
        out,
        limit,
        block,
        &mut |raw, _, refs, rendered, _| {
            inline(raw, palette, rendered, refs);
            Ok(limit)
        },
    )
}

pub(crate) fn emit_blocks_with_cells(
    src: &[u8],
    palette: &Palette,
    state: &mut State,
    out: &mut Vec<u8>,
    mut limit: usize,
    mut block: impl FnMut(usize, usize, usize, usize),
    cell: &mut Cell<'_>,
) -> Result<(), Error> {
    state.prepare(src);
    let mut pos = 0;
    while pos < src.len() {
        let start = pos;
        let output = out.len();
        if !state.in_fence
            && emit_table(
                src,
                &mut pos,
                state.cols,
                state.full_width_tables,
                &state.references,
                palette,
                out,
                &mut limit,
                cell,
            )?
        {
            block(start, pos.min(src.len()), output, out.len());
            continue;
        }
        let end = memchr(b'\n', &src[pos..]).map_or(src.len(), |n| pos + n);
        let line = &src[pos..end];
        check_line(line, limit)?;
        if line.iter().any(|&b| b < 32 && b != b'\t' || b == 127) {
            let mut safe = Vec::new();
            emit_literal(line, &mut safe);
            emit_line(&safe, palette, state, out);
        } else {
            emit_line(line, palette, state, out);
        }
        if end < src.len() {
            out.push(b'\n');
        }
        if out.len() > limit {
            return Err(Error::ResourceLimit);
        }
        pos = (end + 1).min(src.len());
        block(start, pos, output, out.len());
    }
    Ok(())
}

/// Emit source text while neutralizing terminal control bytes.
pub fn emit_literal(src: &[u8], out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in src {
        match byte {
            b'\n' | b'\t' | 0x20..=0x7e | 0x80..=0xff => out.push(byte),
            _ => {
                out.extend_from_slice(b"\\x");
                out.push(HEX[usize::from(byte >> 4)]);
                out.push(HEX[usize::from(byte & 15)]);
            }
        }
    }
}
