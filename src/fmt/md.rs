//! Single-pass SIMD (`memchr`) scanner & `Scene` compiler for `.md`, `.qmd`, `.mdx`, `.rmd`, `.typ`.
//! Zero horizontal imports of sibling format parsers (routes via `fmt::dispatch_kind`).

use crate::{
    err::Error,
    fmt::{self, Kind, trim_ascii},
    scene::Scene,
};

const MATH_FENCE: &[u8; 2] = b"\x24\x24";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Span {
    pub outer_start: u32,
    pub outer_end: u32,
    pub inner_start: u32,
    payload_end_and_kind: u32,
}
const _: () = assert!(size_of::<Span>() == 16);

impl Span {
    const END_MASK: u32 = 0x0fff_ffff;

    #[inline]
    #[must_use]
    pub const fn inner_end(self) -> u32 {
        self.payload_end_and_kind & Self::END_MASK
    }

    #[inline]
    #[must_use]
    pub const fn kind(self) -> Kind {
        match self.payload_end_and_kind >> 28 {
            1 => Kind::Ipynb,
            2 => Kind::Tex,
            3 => Kind::TexInline,
            4 => Kind::Chem,
            5 => Kind::Mermaid,
            6 => Kind::Dot,
            7 => Kind::Svg,
            8 => Kind::Csv,
            9 => Kind::Tsv,
            10 => Kind::Chart,
            11 => Kind::Archive,
            _ => Kind::Md,
        }
    }
}

#[inline]
fn fence_kind(info: &[u8]) -> Option<Kind> {
    let clean = trim_ascii(info);
    let tag = clean
        .strip_prefix(b"{")
        .unwrap_or(clean)
        .split(|&byte| byte.is_ascii_whitespace() || byte == b',' || byte == b'}')
        .next()
        .and_then(|bytes| core::str::from_utf8(bytes).ok())?;
    if tag.eq_ignore_ascii_case("math") {
        return Some(Kind::Tex);
    }
    if tag.eq_ignore_ascii_case("graphviz") {
        return Some(Kind::Dot);
    }
    fmt::kind_from_ext(tag).filter(|kind| !matches!(kind, Kind::Md | Kind::Ipynb | Kind::Archive))
}

#[inline]
fn push_span(out: &mut Vec<Span>, outer: (usize, usize), inner: (usize, usize), kind: Kind) {
    let (Ok(outer_start), Ok(outer_end), Ok(inner_start), Ok(inner_end)) = (
        u32::try_from(outer.0),
        u32::try_from(outer.1),
        u32::try_from(inner.0),
        u32::try_from(inner.1),
    ) else {
        return;
    };
    if inner_end > Span::END_MASK || (kind as u8) > 0x0f {
        return;
    }
    out.push(Span {
        outer_start,
        outer_end,
        inner_start,
        payload_end_and_kind: inner_end | (u32::from(kind as u8) << 28),
    });
}

fn scan_fence(src: &[u8], pos: usize, out: &mut Vec<Span>) -> usize {
    let len = src.len();
    let mut ls = pos;
    while ls > 0 && (src[ls - 1] == b' ' || src[ls - 1] == b'\t') {
        ls -= 1;
    }
    let line_start = if ls == 0 || src[ls - 1] == b'\n' {
        ls
    } else {
        pos
    };
    let delim = &src[pos..pos + 3];
    let line_end = memchr::memchr(b'\n', &src[pos..]).map_or(len, |o| pos + o);
    let info = trim_ascii(&src[pos + 3..line_end]);
    let kind_opt = fence_kind(info);

    let inner_start = (line_end + 1).min(len);
    let mut cur = inner_start;
    let mut inner_end = len;
    let mut outer_end = len;

    while cur < len {
        let eol = memchr::memchr(b'\n', &src[cur..]).map_or(len, |o| cur + o);
        if trim_ascii(&src[cur..eol]).starts_with(delim) {
            inner_end = cur;
            outer_end = (eol + 1).min(len);
            break;
        }
        cur = (eol + 1).min(len);
    }

    if let Some(kind) = kind_opt {
        push_span(out, (line_start, outer_end), (inner_start, inner_end), kind);
    }
    outer_end
}

fn scan_display_math(src: &[u8], pos: usize, out: &mut Vec<Span>) -> usize {
    let len = src.len();
    let mut ls = pos;
    while ls > 0 && (src[ls - 1] == b' ' || src[ls - 1] == b'\t') {
        ls -= 1;
    }
    let is_line_start = ls == 0 || src[ls - 1] == b'\n';
    let line_end = memchr::memchr(b'\n', &src[pos..]).map_or(len, |o| pos + o);
    let line_start = if is_line_start { ls } else { pos };
    let trimmed = trim_ascii(&src[line_start..line_end]);

    if is_line_start && trimmed == MATH_FENCE {
        let inner_start = (line_end + 1).min(len);
        let mut cur = inner_start;
        while cur < len {
            let end_nl = memchr::memchr(b'\n', &src[cur..]).map_or(len, |off| cur + off);
            if trim_ascii(&src[cur..end_nl]) == MATH_FENCE {
                let outer_end = (end_nl + 1).min(len);
                push_span(out, (line_start, outer_end), (inner_start, cur), Kind::Tex);
                return outer_end;
            }
            cur = (end_nl + 1).min(len);
        }
        return (line_end + 1).min(len);
    }

    let inner_s = pos + 2;
    if let Some(close_rel) = src[inner_s..line_end]
        .windows(2)
        .position(|win| win == MATH_FENCE)
    {
        let inner_e = inner_s + close_rel;
        if inner_e > inner_s {
            push_span(out, (pos, inner_e + 2), (inner_s, inner_e), Kind::Tex);
        }
        return inner_e + 2;
    }
    inner_s
}

fn scan_inline_math(src: &[u8], pos: usize, out: &mut Vec<Span>) -> usize {
    let len = src.len();
    if pos + 1 >= len || src[pos + 1].is_ascii_whitespace() {
        return pos + 1;
    }
    if pos > 0 && src[pos - 1] == b'\\' {
        return pos + 1;
    }
    if src[pos + 1].is_ascii_digit() {
        let mut p = pos + 1;
        while p < len && (src[p].is_ascii_digit() || src[p] == b'.' || src[p] == b',') {
            p += 1;
        }
        return p;
    }

    let line_end = memchr::memchr(b'\n', &src[pos..]).map_or(len, |o| pos + o);
    let inner_s = pos + 1;
    let mut search = inner_s;

    while search < line_end {
        let Some(close_rel) = memchr::memchr(b'$', &src[search..line_end]) else {
            break;
        };
        let close = search + close_rel;
        if src[close - 1] == b'\\' {
            search = close + 1;
            continue;
        }
        let valid_close = !src[close - 1].is_ascii_whitespace()
            && !src.get(close + 1).is_some_and(u8::is_ascii_digit);
        if valid_close && close > inner_s {
            push_span(out, (pos, close + 1), (inner_s, close), Kind::TexInline);
            return close + 1;
        }
        break;
    }
    inner_s
}

fn scan_table_row(src: &[u8], pos: usize, out: &mut Vec<Span>) -> usize {
    let len = src.len();
    let mut ls = pos;
    while ls > 0 && (src[ls - 1] == b' ' || src[ls - 1] == b'\t') {
        ls -= 1;
    }
    if ls == 0 || src[ls - 1] == b'\n' {
        let line_end = memchr::memchr(b'\n', &src[pos..]).map_or(len, |o| pos + o);
        let row = trim_ascii(&src[ls..line_end]);
        if row.starts_with(b"|") && row.ends_with(b"|") {
            let table_start = ls;
            let mut cur = (line_end + 1).min(len);
            let mut table_end = cur;
            while cur < len {
                let eol = memchr::memchr(b'\n', &src[cur..]).map_or(len, |o| cur + o);
                let r = trim_ascii(&src[cur..eol]);
                if !(r.starts_with(b"|") && r.ends_with(b"|")) {
                    break;
                }
                cur = (eol + 1).min(len);
                table_end = cur;
            }
            push_span(
                out,
                (table_start, table_end),
                (table_start, table_end),
                Kind::Csv,
            );
            return table_end;
        }
    }
    pos + 1
}

/// Scan document bytes for renderable graphical/math/table spans (zero alloc after warmup).
pub fn scan(src: &[u8], out: &mut Vec<Span>) {
    out.clear();
    let len = src.len();
    if len == 0 {
        return;
    }

    let mut pos = if (src.starts_with(b"---\n") || src.starts_with(b"---\r\n"))
        && let Some(close) = src[4..].windows(4).position(|win| win == b"\n---")
    {
        let after = 4 + close + 4;
        memchr::memchr(b'\n', &src[after..]).map_or(len, |nl| after + nl + 1)
    } else {
        0usize
    };

    while pos < len {
        let Some(rel) = memchr::memchr3(b'`', b'$', b'|', &src[pos..]) else {
            break;
        };
        pos += rel;
        match src[pos] {
            b'`' if src[pos..].starts_with(b"```") => pos = scan_fence(src, pos, out),
            b'`' => {
                pos += 1;
                if let Some(close) = memchr::memchr2(b'`', b'\n', &src[pos..])
                    && src[pos + close] == b'`'
                {
                    pos += close + 1;
                }
            }
            b'$' if src.get(pos + 1) == Some(&b'$') => {
                pos = scan_display_math(src, pos, out);
            }
            b'$' => pos = scan_inline_math(src, pos, out),
            b'|' => pos = scan_table_row(src, pos, out),
            _ => pos += 1,
        }
    }
}

/// Compile only graphical Markdown spans into `Scene`.
///
/// Native prose belongs to `style`; keeping it out of the pixel IR prevents
/// accidental whole-document bitmap rendering.
///
/// # Errors
/// Degrades gracefully on individual sub-block errors; returns `Ok(())`.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    let mut spans = Vec::with_capacity(16);
    scan(src, &mut spans);

    for sp in &spans {
        let payload = &src[sp.inner_start as usize..sp.inner_end() as usize];
        let _ = fmt::dispatch_kind(sp.kind(), payload, scene);
    }
    Ok(())
}
