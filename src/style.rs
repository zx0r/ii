//! Native terminal presentation for Markdown-like prose.
//!
//! This module owns ANSI styling only. It never scans graphical spans, touches KGP,
//! or performs I/O. `CommonMark` owns inline syntax and reference resolution.

use memchr::memchr;

use crate::theme::{Palette, SGR_RESET, SemanticRole};

const MAX_TABLE_COLS: usize = 32;

/// Stateful fence marker for an incremental prose stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    in_fence: bool,
    cols: u16,
    fence_len: usize,
    fence_byte: u8,
    fence_indent: Vec<u8>,
    references: References,
    table_ranges: Vec<core::ops::Range<usize>>,
    prepared: bool,
    pub full_width_tables: bool,
}

type References = std::collections::BTreeMap<String, (String, String)>;

impl State {
    #[must_use]
    pub const fn with_width(cols: u16) -> Self {
        Self {
            in_fence: false,
            cols,
            fence_len: 0,
            fence_byte: 0,
            fence_indent: Vec::new(),
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
    use pulldown_cmark::{BrokenLink, Event, Options, Parser, Tag, TagEnd};
    if !src
        .iter()
        .any(|b| matches!(*b, b'*' | b'_' | b'~' | b'`' | b'[' | b'<' | b'&' | b'\\'))
    {
        out.extend_from_slice(src);
        return;
    }
    let Ok(text) = core::str::from_utf8(src) else {
        emit_literal(src, out);
        return;
    };
    let resolver = |broken: BrokenLink<'_>| {
        references
            .get(&broken.reference.to_lowercase())
            .map(|(url, title)| (url.clone().into(), title.clone().into()))
    };
    for event in Parser::new_with_broken_link_callback(text, Options::all(), Some(resolver)) {
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

fn width(src: &[u8]) -> usize {
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
) {
    out.extend_from_slice(indent);
    role(out, palette, SemanticRole::Border);
    out.extend_from_slice("│".as_bytes());
    let mut rendered = Vec::new();
    let mut clipped = Vec::new();
    for idx in 0..widths.len() {
        let raw = cells.get(idx).copied().unwrap_or(b"");
        rendered.clear();
        clipped.clear();
        inline(raw, palette, &mut rendered, references);
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

fn emit_table(
    src: &[u8],
    pos: &mut usize,
    cols_limit: u16,
    full_width: bool,
    references: &References,
    palette: &Palette,
    out: &mut Vec<u8>,
) -> bool {
    let first_end = memchr(b'\n', &src[*pos..]).map_or(src.len(), |n| *pos + n);
    let second_start = first_end.saturating_add(1);
    if second_start >= src.len() {
        return false;
    }
    let second_end = memchr(b'\n', &src[second_start..]).map_or(src.len(), |n| second_start + n);
    let first = &src[*pos..first_end];
    if !first.contains(&b'|') || !table_sep(&src[second_start..second_end]) {
        return false;
    }
    let (indent_, _) = indent(first);
    let mut header = [&b""[..]; MAX_TABLE_COLS];
    let mut separator = [&b""[..]; MAX_TABLE_COLS];
    let cols = table_cells(first, &mut header);
    let sep_cols = table_cells(&src[second_start..second_end], &mut separator);
    if cols == 0 || cols != sep_cols {
        return false;
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
    );
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
        );
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
    *pos = end;
    true
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
        2..=4 => crate::theme::ColorToken::series(u8::try_from(level).unwrap_or(2)),
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

fn code_line(src: &[u8], palette: &Palette, out: &mut Vec<u8>) {
    let mut pos = 0;
    while pos < src.len() {
        let start = pos;
        let token = if src[pos..].starts_with(b"//") || src[pos] == b'#' {
            pos = src.len();
            Some(SemanticRole::TextMuted)
        } else if matches!(src[pos], b'\'' | b'"') {
            let quote = src[pos];
            pos += 1;
            while pos < src.len() {
                let b = src[pos];
                pos += 1;
                if b == b'\\' {
                    pos = (pos + 1).min(src.len());
                } else if b == quote {
                    break;
                }
            }
            Some(SemanticRole::Success)
        } else if src[pos].is_ascii_digit() {
            pos += 1;
            while pos < src.len() && (src[pos].is_ascii_alphanumeric() || src[pos] == b'.') {
                pos += 1;
            }
            Some(SemanticRole::Warn)
        } else if src[pos].is_ascii_alphabetic() || src[pos] == b'_' {
            pos += 1;
            while pos < src.len() && (src[pos].is_ascii_alphanumeric() || src[pos] == b'_') {
                pos += 1;
            }
            matches!(
                &src[start..pos],
                b"def"
                    | b"class"
                    | b"import"
                    | b"from"
                    | b"as"
                    | b"return"
                    | b"for"
                    | b"in"
                    | b"if"
                    | b"else"
                    | b"elif"
                    | b"while"
                    | b"with"
                    | b"try"
                    | b"except"
                    | b"fn"
                    | b"let"
                    | b"mut"
                    | b"pub"
                    | b"use"
                    | b"struct"
                    | b"enum"
                    | b"impl"
                    | b"match"
                    | b"const"
                    | b"async"
                    | b"await"
                    | b"SELECT"
                    | b"FROM"
                    | b"WHERE"
                    | b"GROUP"
                    | b"BY"
                    | b"ORDER"
                    | b"LIMIT"
                    | b"True"
                    | b"False"
                    | b"None"
                    | b"true"
                    | b"false"
                    | b"null"
            )
            .then_some(SemanticRole::Accent)
        } else {
            pos += 1;
            None
        };
        if let Some(color) = token {
            role(out, palette, color);
        }
        out.extend_from_slice(&src[start..pos]);
        if token.is_some() {
            out.extend_from_slice(SGR_RESET);
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

fn emit_code(clean: &[u8], indent_: &[u8], palette: &Palette, cols_limit: u16, out: &mut Vec<u8>) {
    let budget = if cols_limit == 0 {
        58
    } else {
        usize::from(cols_limit)
            .saturating_sub(width(indent_))
            .max(4)
    };
    let inner = budget.saturating_sub(4).max(1);
    let mut styled = Vec::new();
    code_line(clean, palette, &mut styled);
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
        out.extend_from_slice(indent_);
        role(out, palette, SemanticRole::Border);
        out.extend_from_slice("│ ".as_bytes());
        role(out, palette, SemanticRole::TextMuted);
        out.extend_from_slice(line);
        out.extend_from_slice(SGR_RESET);
        out.resize(out.len() + inner.saturating_sub(width(line)), b' ');
        role(out, palette, SemanticRole::Border);
        out.extend_from_slice(" │".as_bytes());
        out.extend_from_slice(SGR_RESET);
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
        emit_code(code, &state.fence_indent, palette, state.cols, out);
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
    mut block: impl FnMut(usize, usize, usize, usize),
) {
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
            )
        {
            block(start, pos.min(src.len()), output, out.len());
            continue;
        }
        let end = memchr(b'\n', &src[pos..]).map_or(src.len(), |n| pos + n);
        let line = &src[pos..end];
        if line.iter().any(|&b| b < 32 && b != b'\t') {
            let mut safe = Vec::new();
            emit_literal(line, &mut safe);
            emit_line(&safe, palette, state, out);
        } else {
            emit_line(line, palette, state, out);
        }
        if end < src.len() {
            out.push(b'\n');
        }
        pos = (end + 1).min(src.len());
        block(start, pos, output, out.len());
    }
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
