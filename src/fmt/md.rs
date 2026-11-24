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
fn fence_line_start<const CLOSE: bool>(src: &[u8], pos: usize) -> Option<usize> {
    let start = src[..pos]
        .iter()
        .rposition(|&b| {
            !(matches!(b, b' ' | b'\t') || CLOSE && b != b'\n' && b.is_ascii_whitespace())
        })
        .map_or(0, |idx| idx + 1);
    (start == 0 || src[start - 1] == b'\n').then_some(start)
}

#[inline]
fn scan_fence(src: &[u8], pos: usize, out: &mut Vec<Span>) -> usize {
    let Some(line_start) = fence_line_start::<false>(src, pos) else {
        return pos + 1;
    };
    let delimiter = src[pos];
    let count = src[pos..].iter().take_while(|&&b| b == delimiter).count();
    let line_end = newline(&src[pos + count..]).map_or(src.len(), |i| pos + count + i);
    let kind = fence_kind(&src[pos + count..line_end]);
    let inner_start = (line_end + 1).min(src.len());
    let mut search = inner_start;
    while let Some(off) = memchr::memchr(delimiter, &src[search..]) {
        let close = search + off;
        let run = src[close..].iter().take_while(|&&b| b == delimiter).count();
        if run >= count
            && let Some(start) = fence_line_start::<true>(src, close)
        {
            let after = close + run;
            let end = newline(&src[after..]).map_or(src.len(), |i| after + i);
            if src[after..end].iter().all(u8::is_ascii_whitespace) {
                let outer_end = (end + 1).min(src.len());
                if let Some(kind) = kind {
                    push_span(out, (line_start, outer_end), (inner_start, start), kind);
                }
                return outer_end;
            }
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

#[inline]
const fn is_token(b: u8) -> bool {
    matches!(b, b'`' | b'$' | b'\\' | b'~')
}

#[cfg(all(target_arch = "aarch64", target_feature = "neon", not(miri)))]
#[inline]
fn token<const BULK: bool>(src: &[u8]) -> Option<usize> {
    // SAFETY: this branch is only compiled when NEON is a target feature.
    unsafe { neon::<BULK>(src) }
}

#[cfg(all(target_arch = "aarch64", target_feature = "neon", not(miri)))]
#[inline]
#[target_feature(enable = "neon")]
fn neon<const BULK: bool>(src: &[u8]) -> Option<usize> {
    use core::arch::aarch64::{
        vandq_u8, vceqq_u8, vdupq_n_u8, vld1q_u8, vmaxvq_u8, vorrq_u8, vqtbl1q_u8,
    };

    // Low nibbles are unique. Zero in unused slots cannot match any input with
    // those nibbles, and the comparison checks the complete byte.
    const TOKENS: [u8; 16] = [b'`', 0, 0, 0, b'$', 0, 0, 0, 0, 0, 0, 0, b'\\', 0, b'~', 0];

    // SAFETY: the table contains all 16 readable bytes.
    let table = unsafe { vld1q_u8(TOKENS.as_ptr()) };
    let mask = vdupq_n_u8(15);
    let hits = |block: &[u8; 16]| {
        // SAFETY: a fixed-size slice guarantees 16 readable bytes. The load
        // accepts unaligned addresses.
        let bytes = unsafe { vld1q_u8(block.as_ptr()) };
        vceqq_u8(bytes, vqtbl1q_u8(table, vandq_u8(bytes, mask)))
    };

    let (blocks, tail) = src.as_chunks::<16>();
    let mut off = 0;
    let rest = if BULK {
        let (wide, last) = blocks.as_chunks::<4>();
        for [a, b, c, d] in wide {
            let any = vorrq_u8(vorrq_u8(hits(a), hits(b)), vorrq_u8(hits(c), hits(d)));
            if vmaxvq_u8(any) != 0 {
                return src[off..off + 64]
                    .iter()
                    .position(|&b| is_token(b))
                    .map(|i| off + i);
            }
            off += 64;
        }
        last
    } else {
        blocks
    };
    for block in rest {
        if vmaxvq_u8(hits(block)) != 0 {
            return block.iter().position(|&b| is_token(b)).map(|i| off + i);
        }
        off += 16;
    }
    tail.iter().position(|&b| is_token(b)).map(|i| off + i)
}

#[cfg(all(target_arch = "x86_64", target_feature = "sse2", not(miri)))]
#[inline]
fn token<const BULK: bool>(src: &[u8]) -> Option<usize> {
    // SAFETY: this branch is only compiled when SSE2 is a target feature.
    unsafe { sse2::<BULK>(src) }
}

#[cfg(all(target_arch = "x86_64", target_feature = "sse2", not(miri)))]
#[inline]
#[target_feature(enable = "sse2")]
fn sse2<const BULK: bool>(src: &[u8]) -> Option<usize> {
    use core::arch::x86_64::{
        _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_or_si128, _mm_set1_epi8,
    };

    let backtick = _mm_set1_epi8(i8::from_ne_bytes(*b"`"));
    let dollar = _mm_set1_epi8(i8::from_ne_bytes(*b"$"));
    let slash = _mm_set1_epi8(i8::from_ne_bytes(*b"\\"));
    let tilde = _mm_set1_epi8(i8::from_ne_bytes(*b"~"));
    let (blocks, tail) = src.as_chunks::<16>();
    for (i, block) in blocks.iter().enumerate() {
        // SAFETY: a fixed-size slice guarantees 16 readable bytes. The load
        // accepts unaligned addresses.
        let bytes = unsafe { _mm_loadu_si128(block.as_ptr().cast()) };
        let hits = _mm_or_si128(
            _mm_or_si128(
                _mm_cmpeq_epi8(bytes, backtick),
                _mm_cmpeq_epi8(bytes, dollar),
            ),
            _mm_or_si128(_mm_cmpeq_epi8(bytes, slash), _mm_cmpeq_epi8(bytes, tilde)),
        );
        let mask = _mm_movemask_epi8(hits);
        if mask != 0 {
            return Some(i * 16 + mask.trailing_zeros() as usize);
        }
    }
    tail.iter()
        .position(|&b| is_token(b))
        .map(|i| blocks.len() * 16 + i)
}

#[cfg(any(
    miri,
    not(any(
        all(target_arch = "aarch64", target_feature = "neon"),
        all(target_arch = "x86_64", target_feature = "sse2"),
    ))
))]
#[inline]
fn token<const BULK: bool>(src: &[u8]) -> Option<usize> {
    src.iter().position(|&b| is_token(b))
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

    let Some(rel) = token::<true>(&src[pos..]) else {
        return;
    };
    pos += rel;

    loop {
        match src[pos] {
            delimiter @ (b'`' | b'~') if src.get(pos + 1..pos + 3) == Some(&[delimiter; 2]) => {
                pos = scan_fence(src, pos, out);
            }
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
        let Some(rel) = token::<false>(&src[pos..]) else {
            break;
        };
        pos += rel;
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

#[cfg(test)]
mod tests {
    use super::{is_token, token};

    #[test]
    fn token_kernels_match_scalar_across_lanes_and_tail() {
        let mut src = [b'a'; 97];
        for byte in u8::MIN..=u8::MAX {
            for at in [0, 15, 16, 31, 32, 63, 64, 95, 96] {
                src[at] = byte;
                for start in [0, 1, 15] {
                    let suffix = &src[start..];
                    let expected = suffix.iter().position(|&byte| is_token(byte));
                    assert_eq!(token::<false>(suffix), expected);
                    assert_eq!(token::<true>(suffix), expected);
                }
                src[at] = b'a';
            }
        }
    }
}
