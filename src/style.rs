//! Native terminal presentation for Markdown-like prose.
//!
//! This module owns ANSI styling only. It never scans graphical spans, touches KGP,
//! allocates per line, or performs I/O.

use memchr::memchr;

use crate::theme::{Palette, SGR_RESET, SemanticRole};

const CANDIDATE: [bool; 256] = {
    let mut t = [false; 256];
    t[b'#' as usize] = true;
    t[b'=' as usize] = true;
    t[b'>' as usize] = true;
    t[b'-' as usize] = true;
    t[b'*' as usize] = true;
    t[b'`' as usize] = true;
    t[b'|' as usize] = true;
    t[b'%' as usize] = true;
    t[b'/' as usize] = true;
    t
};
const MAX_TABLE_COLS: usize = 32;

/// Stateful fence marker for an incremental prose stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    in_fence: bool,
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
        role: SemanticRole::Accent,
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
        label: "󰨸 TL;DR".as_bytes(),
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
        label: "󱐌 Failure".as_bytes(),
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
        role: SemanticRole::Accent,
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

fn inline(src: &[u8], palette: &Palette, out: &mut Vec<u8>) {
    let mut pos = 0;
    while pos < src.len() {
        let Some(rel) = memchr(b'`', &src[pos..]) else {
            out.extend_from_slice(&src[pos..]);
            return;
        };
        let at = pos + rel;
        out.extend_from_slice(&src[pos..at]);
        if let Some(end) = memchr(b'`', &src[at + 1..]) {
            role(out, palette, SemanticRole::Accent);
            out.extend_from_slice(&src[at..at + end + 2]);
            out.extend_from_slice(SGR_RESET);
            pos = at + end + 2;
        } else {
            out.extend_from_slice(&src[at..]);
            return;
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
    let line = trim_start(line);
    let Some(mut rest) = line.strip_prefix(b"|") else {
        return 0;
    };
    let mut len = 0;
    while len < cells.len() {
        let Some(at) = memchr(b'|', rest) else {
            break;
        };
        let raw = &rest[..at];
        cells[len] = raw.trim_ascii();
        len += 1;
        rest = &rest[at + 1..];
        if rest.is_empty() {
            break;
        }
    }
    len
}

fn table_sep(line: &[u8]) -> bool {
    let line = trim_start(line);
    line.starts_with(b"|")
        && line
            .iter()
            .copied()
            .all(|b| matches!(b, b'|' | b'-' | b':' | b' ' | b'\t' | b'\r'))
}

fn width(src: &[u8]) -> usize {
    src.iter().filter(|b| (**b & 0xc0) != 0x80).count()
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

fn table_row(
    indent: &[u8],
    cells: &[&[u8]],
    widths: &[usize],
    aligns: &[Align],
    header: bool,
    palette: &Palette,
    out: &mut Vec<u8>,
) {
    out.extend_from_slice(indent);
    role(out, palette, SemanticRole::Border);
    out.extend_from_slice("│".as_bytes());
    for idx in 0..widths.len() {
        let cell = cells.get(idx).copied().unwrap_or(b"");
        let pad = widths[idx].saturating_sub(width(cell));
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
        inline(cell, palette, out);
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

fn emit_table(src: &[u8], pos: &mut usize, palette: &Palette, out: &mut Vec<u8>) -> bool {
    let first_end = memchr(b'\n', &src[*pos..]).map_or(src.len(), |n| *pos + n);
    let second_start = first_end.saturating_add(1);
    if second_start >= src.len() {
        return false;
    }
    let second_end = memchr(b'\n', &src[second_start..]).map_or(src.len(), |n| second_start + n);
    let first = &src[*pos..first_end];
    if !trim_start(first).starts_with(b"|") || !table_sep(&src[second_start..second_end]) {
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
        if !trim_start(line).starts_with(b"|") || table_sep(line) {
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
        ((1..=6).contains(&marker) && line.get(marker) == Some(&b' '))
            .then_some((marker, &line[marker + 1..]))
    }
}

fn emit_heading(indent_: &[u8], level: usize, body: &[u8], palette: &Palette, out: &mut Vec<u8>) {
    const ICONS: [&[u8]; 6] = [
        "󰲡 ".as_bytes(),
        "󰲣 ".as_bytes(),
        "󰲥 ".as_bytes(),
        "󰲧 ".as_bytes(),
        "󰲩 ".as_bytes(),
        "󰲫 ".as_bytes(),
    ];
    let role_ = match level {
        1 => SemanticRole::Info,
        2 => SemanticRole::Accent,
        3 => SemanticRole::Success,
        4 => SemanticRole::Warn,
        _ => SemanticRole::TextMuted,
    };
    out.extend_from_slice(indent_);
    bold(out, palette, role_);
    out.extend_from_slice(ICONS[level - 1]);
    inline(body, palette, out);
    out.extend_from_slice(SGR_RESET);
}

fn callout(body: &[u8], indent_: &[u8], palette: &Palette, out: &mut Vec<u8>) {
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
            inline(tail, palette, out);
        }
    } else {
        role(out, palette, SemanticRole::Info);
        out.extend_from_slice("│ \x1b[3m".as_bytes());
        inline(body, palette, out);
    }
    out.extend_from_slice(SGR_RESET);
}

fn task(line: &[u8]) -> Option<(&[u8], SemanticRole, bool)> {
    let rest = line
        .strip_prefix(b"- ")
        .or_else(|| line.strip_prefix(b"* "))?;
    let (needle, icon, role_) = if let Some(x) = rest
        .strip_prefix(b"[x] ")
        .or_else(|| rest.strip_prefix(b"[X] "))
    {
        (x, "󰄵 ".as_bytes(), SemanticRole::Success)
    } else if let Some(x) = rest.strip_prefix(b"[ ] ") {
        (x, "󰄱 ".as_bytes(), SemanticRole::TextMuted)
    } else if let Some(x) = rest.strip_prefix(b"[-] ") {
        (x, "󰥔 ".as_bytes(), SemanticRole::Warn)
    } else if let Some(x) = rest.strip_prefix(b"[!] ") {
        (x, "󰀨 ".as_bytes(), SemanticRole::Danger)
    } else {
        let x = rest.strip_prefix(b"[~] ")?;
        (x, "󰰱 ".as_bytes(), SemanticRole::TextMuted)
    };
    let _ = icon;
    Some((
        needle,
        role_,
        matches!(rest.get(1), Some(b'x' | b'X' | b'-' | b'!')),
    ))
}

fn emit_line(line: &[u8], palette: &Palette, state: &mut State, out: &mut Vec<u8>) {
    let (indent_, clean) = indent(line);
    if clean.starts_with(b"```") {
        out.extend_from_slice(indent_);
        role(out, palette, SemanticRole::Border);
        if state.in_fence {
            out.extend_from_slice(
                "╰────────────────────────────────────────────────────────".as_bytes(),
            );
        } else {
            let lang = trim_start(&clean[3..]);
            out.extend_from_slice("╭─".as_bytes());
            if !lang.is_empty() {
                out.extend_from_slice(b" [ ");
                bold(out, palette, SemanticRole::Info);
                out.extend_from_slice(lang);
                out.extend_from_slice(SGR_RESET);
                role(out, palette, SemanticRole::Border);
                out.extend_from_slice(b" ] ");
            }
            out.extend_from_slice(
                "────────────────────────────────────────────────────────".as_bytes(),
            );
        }
        state.in_fence = !state.in_fence;
        out.extend_from_slice(SGR_RESET);
    } else if state.in_fence {
        out.extend_from_slice(indent_);
        role(out, palette, SemanticRole::Border);
        out.extend_from_slice("│ ".as_bytes());
        role(out, palette, SemanticRole::TextMuted);
        out.extend_from_slice(clean);
        out.extend_from_slice(SGR_RESET);
    } else if let Some((level, body)) = heading(clean) {
        emit_heading(indent_, level, body, palette, out);
    } else if let Some(body) = clean.strip_prefix(b">") {
        callout(body, indent_, palette, out);
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
        inline(body, palette, out);
    } else if let Some(body) = clean
        .strip_prefix(b"- ")
        .or_else(|| clean.strip_prefix(b"* "))
    {
        out.extend_from_slice(indent_);
        bold(out, palette, SemanticRole::TextMuted);
        out.extend_from_slice("• ".as_bytes());
        out.extend_from_slice(SGR_RESET);
        inline(body, palette, out);
    } else if clean.len() >= 3
        && clean
            .iter()
            .all(|b| matches!(*b, b'-' | b'*' | b'_' | b' '))
    {
        out.extend_from_slice(indent_);
        role(out, palette, SemanticRole::Border);
        out.extend_from_slice(
            "────────────────────────────────────────────────────────".as_bytes(),
        );
        out.extend_from_slice(SGR_RESET);
    } else if clean.starts_with(b"%") || clean.starts_with(b"//") {
        out.extend_from_slice(indent_);
        role(out, palette, SemanticRole::TextMuted);
        out.extend_from_slice(clean);
        out.extend_from_slice(SGR_RESET);
    } else {
        inline(line, palette, out);
    }
}

/// Append styled prose to `out`. The caller owns the reusable `State`.
pub fn emit_prose(src: &[u8], palette: &Palette, state: &mut State, out: &mut Vec<u8>) {
    if src.is_empty() {
        return;
    }
    if !src.iter().any(|b| CANDIDATE[*b as usize]) {
        out.extend_from_slice(src);
        return;
    }
    let mut pos = 0;
    while pos < src.len() {
        if !state.in_fence && emit_table(src, &mut pos, palette, out) {
            continue;
        }
        let end = memchr(b'\n', &src[pos..]).map_or(src.len(), |x| pos + x);
        emit_line(&src[pos..end], palette, state, out);
        if end < src.len() {
            out.push(b'\n');
        }
        pos = end.saturating_add(1);
    }
}
