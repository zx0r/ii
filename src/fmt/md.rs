//! Single-pass SIMD (`memchr`) scanner & `Scene` compiler for `.md`, `.qmd`, `.mdx`, `.rmd`, `.typ`.
//! Zero horizontal imports of sibling format parsers (routes via `fmt::dispatch_kind`).

use crate::theme;
use crate::{
    err::Error,
    fmt::{self, Kind, trim_ascii},
    scene::Scene,
};

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
            12 => Kind::Png,
            13 => Kind::Typst,
            14 => Kind::Mdx,
            _ => Kind::Md,
        }
    }
}

#[inline]
fn fence_kind(info: &[u8]) -> Option<Kind> {
    let clean = trim_ascii(info);
    match clean {
        b"mermaid" => return Some(Kind::Mermaid),
        b"svg" => return Some(Kind::Svg),
        b"math" | b"tex" | b"latex" | b"katex" => return Some(Kind::Tex),
        _ => {}
    }
    let tag = clean
        .strip_prefix(b"{")
        .unwrap_or(clean)
        .split(|&byte| byte.is_ascii_whitespace() || byte == b',' || byte == b'}')
        .next()
        .and_then(|bytes| core::str::from_utf8(bytes).ok())?;
    if tag.eq_ignore_ascii_case("mermaid") {
        return Some(Kind::Mermaid);
    }
    if tag.eq_ignore_ascii_case("svg") {
        return Some(Kind::Svg);
    }
    if tag.eq_ignore_ascii_case("math") || tag.eq_ignore_ascii_case("katex") {
        return Some(Kind::Tex);
    }
    if tag.eq_ignore_ascii_case("graphviz") {
        return Some(Kind::Dot);
    }
    fmt::kind_from_ext(tag)
        .filter(|kind| !matches!(kind, Kind::Md | Kind::Mdx | Kind::Ipynb | Kind::Archive))
}

#[inline]
#[expect(
    clippy::cast_possible_truncation,
    reason = "scan rejects inputs above the packed 28-bit offset limit before invoking helpers"
)]
fn push_span(out: &mut Vec<Span>, outer: (usize, usize), inner: (usize, usize), kind: Kind) {
    out.push(Span {
        outer_start: outer.0 as u32,
        outer_end: outer.1 as u32,
        inner_start: inner.0 as u32,
        payload_end_and_kind: inner.1 as u32 | (u32::from(kind as u8) << 28),
    });
}

#[inline]
fn newline(src: &[u8]) -> Option<usize> {
    let head = src.len().min(16);
    src[..head]
        .iter()
        .position(|&b| b == b'\n')
        .or_else(|| memchr::memchr(b'\n', &src[head..]).map(|i| head + i))
}

#[inline]
fn scan_fence(src: &[u8], pos: usize, out: &mut Vec<Span>) -> usize {
    let line_start = memchr::memrchr(b'\n', &src[..pos]).map_or(0, |i| i + 1);
    if !src[line_start..pos]
        .iter()
        .all(|&b| b == b' ' || b == b'\t')
    {
        return pos + 1;
    }
    let delimiter = src[pos];
    let count = src[pos..].iter().take_while(|&&b| b == delimiter).count();
    let line_end = newline(&src[pos + count..]).map_or(src.len(), |i| pos + count + i);
    let kind = fence_kind(&src[pos + count..line_end]);
    let inner_start = (line_end + 1).min(src.len());
    let mut search = inner_start;
    while let Some(off) = memchr::memchr(delimiter, &src[search..]) {
        let close = search + off;
        let start = memchr::memrchr(b'\n', &src[..close]).map_or(0, |i| i + 1);
        let run = src[close..].iter().take_while(|&&b| b == delimiter).count();
        let end = newline(&src[close + run..]).map_or(src.len(), |i| close + run + i);
        if run >= count
            && src[start..close].iter().all(u8::is_ascii_whitespace)
            && src[close + run..end].iter().all(u8::is_ascii_whitespace)
        {
            let outer_end = (end + 1).min(src.len());
            if let Some(kind) = kind {
                push_span(out, (line_start, outer_end), (inner_start, start), kind);
            }
            return outer_end;
        }
        search = close + run;
    }
    src.len()
}

#[inline]
fn escaped(src: &[u8], pos: usize) -> bool {
    src[..pos].iter().rev().take_while(|&&b| b == b'\\').count() % 2 != 0
}

#[inline]
fn scan_display_math(src: &[u8], pos: usize, out: &mut Vec<Span>) -> usize {
    let start = pos + 2;
    let mut search = start;
    while let Some(off) = memchr::memchr(b'$', &src[search..]) {
        let close = search + off;
        if src.get(close + 1) == Some(&b'$') && !escaped(src, close) {
            if close > start {
                push_span(out, (pos, close + 2), (start, close), Kind::Tex);
            }
            return close + 2;
        }
        search = close + 1;
    }
    start
}

fn scan_bracket_math(src: &[u8], pos: usize, out: &mut Vec<Span>) -> usize {
    let display = src.get(pos + 1) == Some(&b'[');
    let closing = if display { b']' } else { b')' };
    let start = pos + 2;
    let mut search = start;
    while let Some(off) = memchr::memchr2(b'\\', b'\n', &src[search..]) {
        let close = search + off;
        if src[close] == b'\n' && !display {
            break;
        }
        if src[close] == b'\\' && src.get(close + 1) == Some(&closing) && !escaped(src, close) {
            push_span(
                out,
                (pos, close + 2),
                (start, close),
                if display { Kind::Tex } else { Kind::TexInline },
            );
            return close + 2;
        }
        search = close + 1;
    }
    start
}

fn scan_math_environment(src: &[u8], pos: usize, out: &mut Vec<Span>) -> Option<usize> {
    let rest = src.get(pos..)?.strip_prefix(b"\\begin{")?;
    let name_len = memchr::memchr(b'}', rest)?;
    let name = &rest[..name_len];
    if !matches!(
        name,
        b"equation"
            | b"equation*"
            | b"align"
            | b"align*"
            | b"aligned"
            | b"alignedat"
            | b"gather"
            | b"gather*"
            | b"gathered"
            | b"multline"
            | b"multline*"
            | b"flalign"
            | b"flalign*"
            | b"split"
            | b"displaymath"
            | b"math"
            | b"matrix"
            | b"pmatrix"
            | b"bmatrix"
            | b"Bmatrix"
            | b"vmatrix"
            | b"Vmatrix"
            | b"cases"
            | b"dcases"
    ) {
        return None;
    }
    let mut cursor = pos + 8 + name_len;
    let mut depth = 1usize;
    while let Some(off) = memchr::memchr(b'\\', &src[cursor..]) {
        let at = cursor + off;
        let tail = &src[at..];
        let (prefix, opening) = if tail.starts_with(b"\\begin{") {
            (7, true)
        } else if tail.starts_with(b"\\end{") {
            (5, false)
        } else {
            cursor = at + 1;
            continue;
        };
        if tail.get(prefix..prefix + name_len) == Some(name)
            && tail.get(prefix + name_len) == Some(&b'}')
        {
            if opening {
                depth += 1;
            } else {
                depth -= 1;
            }
            let end = at + prefix + name_len + 1;
            if depth == 0 {
                push_span(out, (pos, end), (pos, end), Kind::Tex);
                return Some(end);
            }
            cursor = end;
        } else {
            cursor = at + 1;
        }
    }
    None
}

#[inline]
fn scan_inline_math(src: &[u8], pos: usize, out: &mut Vec<Span>) -> usize {
    let len = src.len();
    if pos + 1 >= len || src[pos + 1].is_ascii_whitespace() {
        return pos + 1;
    }
    if escaped(src, pos) {
        return pos + 1;
    }
    if src[pos + 1].is_ascii_digit() {
        let mut p = pos + 1;
        while p < len && (src[p].is_ascii_digit() || src[p] == b'.' || src[p] == b',') {
            p += 1;
        }
        let currency = match src.get(p) {
            None | Some(b'\n' | b'\r' | b')' | b']' | b';' | b':') => true,
            Some(b' ' | b'\t') => {
                let mut next = p + 1;
                while matches!(src.get(next), Some(b' ' | b'\t')) {
                    next += 1;
                }
                !matches!(
                    src.get(next),
                    Some(b'+' | b'-' | b'=' | b'<' | b'>' | b'*' | b'/' | b'^' | b'_' | b'\\')
                )
            }
            _ => false,
        };
        if currency {
            return p;
        }
    }

    let inner_s = pos + 1;
    let mut search = inner_s;
    while let Some(off) = memchr::memchr3(b'$', b'\n', b'`', &src[search..]) {
        let close = search + off;
        if matches!(src[close], b'\n' | b'`') {
            break;
        }
        if escaped(src, close) {
            search = close + 1;
            continue;
        }
        if !src[close - 1].is_ascii_whitespace()
            && !src.get(close + 1).is_some_and(u8::is_ascii_digit)
            && close > inner_s
        {
            push_span(out, (pos, close + 1), (inner_s, close), Kind::TexInline);
            return close + 1;
        }
        break;
    }
    inner_s
}

/// Scan document bytes for renderable graphical/math spans (zero alloc after warmup).
pub fn scan(src: &[u8], out: &mut Vec<Span>) {
    out.clear();
    let len = src.len();
    if len == 0 || len > Span::END_MASK as usize {
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
        let remaining = &src[pos..];
        let head = remaining.len().min(16);
        let token = remaining[..head]
            .iter()
            .position(|&b| matches!(b, b'`' | b'$' | b'\\'))
            .or_else(|| memchr::memchr3(b'`', b'$', b'\\', &remaining[head..]).map(|i| head + i));
        let Some(rel) = token else {
            break;
        };
        pos += rel;
        match src[pos] {
            b'`' if src[pos..].starts_with(b"```") => pos = scan_fence(src, pos, out),
            b'`' => {
                pos += 1;
                let remaining = &src[pos..];
                let head = remaining.len().min(32);
                let close = remaining[..head]
                    .iter()
                    .position(|&b| b == b'`' || b == b'\n')
                    .or_else(|| memchr::memchr2(b'`', b'\n', &remaining[head..]).map(|i| head + i));
                if let Some(close) = close {
                    pos += close + 1;
                }
            }
            b'$' if src.get(pos + 1) == Some(&b'$') => {
                pos = scan_display_math(src, pos, out);
            }
            b'$' => pos = scan_inline_math(src, pos, out),
            b'\\' if matches!(src.get(pos + 1), Some(b'[' | b'(')) && !escaped(src, pos) => {
                pos = scan_bracket_math(src, pos, out);
            }
            b'\\' if src.get(pos + 1) == Some(&b'b') => {
                pos = scan_math_environment(src, pos, out).unwrap_or_else(|| (pos + 2).min(len));
            }
            b'\\' => pos = (pos + 2).min(len),
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
    parse_with_palette(src, &theme::DARK, scene)
}

/// Parse with an explicit receiving palette.
///
/// # Errors
/// Propagates format decoding errors.
pub fn parse_with_palette(
    src: &[u8],
    palette: &theme::Palette,
    scene: &mut Scene,
) -> Result<(), Error> {
    let mut spans = Vec::with_capacity(16);
    scan(src, &mut spans);

    for sp in &spans {
        let payload = &src[sp.inner_start as usize..sp.inner_end() as usize];
        let _ = fmt::dispatch_with_palette(sp.kind(), payload, palette, scene);
    }
    Ok(())
}
