//! Zero-DOM SVG theme color rewriter and header dimension extractor.

use crate::{
    err::Error,
    scene::Scene,
    theme::{self, Palette, SemanticRole},
};

#[inline]
const fn has_byte(word: u64, byte: u8) -> bool {
    let x = word ^ u64::from_ne_bytes([byte; 8]);
    x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080 != 0
}

#[inline]
fn candidate(src: &[u8], mut pos: usize) -> usize {
    while pos + 8 <= src.len() {
        let bytes: [u8; 8] = match src[pos..pos + 8].try_into() {
            Ok(bytes) => bytes,
            Err(_) => break,
        };
        let word = u64::from_ne_bytes(bytes);
        if has_byte(word, b'c') || has_byte(word, b'\"') {
            break;
        }
        pos += 8;
    }
    while pos < src.len() && !matches!(src[pos], b'c' | b'\"') {
        pos += 1;
    }
    pos
}

/// Rewrite `currentColor` and hardcoded black strokes/fills to match active `palette`.
pub fn rewrite_colors(src: &[u8], palette: &Palette, dst: &mut Vec<u8>) {
    rewrite_colors_as(src, palette, SemanticRole::TextPrimary, dst);
}

pub(crate) fn rewrite_colors_as(
    src: &[u8],
    palette: &Palette,
    role: SemanticRole,
    dst: &mut Vec<u8>,
) {
    dst.reserve(src.len() + 64);
    let fg = palette.hex_str(role).as_bytes();
    let mut pos = 0usize;

    while pos < src.len() {
        let next = candidate(src, pos);
        if next > pos {
            dst.extend_from_slice(&src[pos..next]);
            pos = next;
            continue;
        }
        let rest = &src[pos..];
        if rest.starts_with(b"currentColor") {
            dst.extend_from_slice(fg);
            pos += 12;
        } else if rest.starts_with(b"\"rgb(0,0,0)\"") {
            dst.push(b'"');
            dst.extend_from_slice(fg);
            dst.push(b'"');
            pos += 12;
        } else if palette.is_dark && rest.starts_with(b"\"#000000\"") {
            dst.push(b'"');
            dst.extend_from_slice(fg);
            dst.push(b'"');
            pos += 9;
        } else if palette.is_dark && rest.starts_with(b"\"#000\"") {
            dst.push(b'"');
            dst.extend_from_slice(fg);
            dst.push(b'"');
            pos += 6;
        } else if palette.is_dark && rest.starts_with(b"\"black\"") {
            dst.push(b'"');
            dst.extend_from_slice(fg);
            dst.push(b'"');
            pos += 7;
        } else {
            dst.push(src[pos]);
            pos += 1;
        }
    }
}

#[inline]
fn parse_attr_f32(header: &[u8], key: &[u8]) -> Option<f32> {
    let pos = header.windows(key.len()).position(|w| w == key)?;
    let rest = &header[pos + key.len()..];
    let quote = *rest.first()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let val_bytes = &rest[1..];
    let end = memchr::memchr(quote, val_bytes)?;
    let num_end = val_bytes[..end]
        .iter()
        .position(|&b| !(b.is_ascii_digit() || b == b'.' || b == b'-'))
        .unwrap_or(end);
    core::str::from_utf8(&val_bytes[..num_end])
        .ok()?
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
}

#[inline]
fn parse_viewbox(header: &[u8]) -> Option<(f32, f32)> {
    let pos = header.windows(9).position(|w| w == b"viewBox=\"")?;
    let rest = &header[pos + 9..];
    let end = memchr::memchr(b'"', rest)?;
    let text = core::str::from_utf8(&rest[..end]).ok()?;
    let mut nums = text
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<f32>().ok());
    let _x = nums.next()?;
    let _y = nums.next()?;
    let width = nums.next()?;
    let height = nums.next()?;
    (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0)
        .then_some((width, height))
}

/// Extract `(width, height)` from `<svg ...>` header (`viewBox` or `width`/`height`).
#[must_use]
pub fn extract_svg_dimensions(src: &[u8]) -> (f32, f32) {
    let start = memchr::memmem::find(src, b"<svg").unwrap_or(0);
    let src = &src[start..];
    let tag_end = memchr::memchr(b'>', src).unwrap_or_else(|| src.len().min(512));
    let header = &src[..tag_end.min(src.len())];
    if let Some(dims) = parse_viewbox(header) {
        return dims;
    }
    let width = parse_attr_f32(header, b"width=").unwrap_or(320.0);
    let height = parse_attr_f32(header, b"height=").unwrap_or(180.0);
    (width, height)
}

/// Stage color-normalized SVG payload directly into `Scene::svgs` using caller-provided palette.
///
/// # Errors
/// Returns `Error::SvgParse` if `src` contains no `<svg` marker or pool overflows `u32`.
pub fn parse_into_scene_with_palette(
    src: &[u8],
    palette: &Palette,
    scene: &mut Scene,
) -> Result<(), Error> {
    if !src.windows(4).any(|w| w == b"<svg") {
        return Err(Error::SvgParse);
    }
    let start = scene.text_pool.len();
    rewrite_colors(src, palette, &mut scene.text_pool);
    let (svg_w, svg_h) = extract_svg_dimensions(&scene.text_pool[start..]);
    if !scene.commit_svg(0.0, scene.height, svg_w, svg_h, start) {
        scene.text_pool.truncate(start);
        return Err(Error::SvgParse);
    }
    scene.width = scene.width.max(svg_w);
    scene.height += svg_h;
    Ok(())
}

/// Stage color-normalized SVG payload directly into `Scene::svgs` without 64 `KiB` truncation.
///
/// # Errors
/// Returns `Error::SvgParse` if `src` contains no `<svg` marker or pool overflows `u32`.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    parse_into_scene_with_palette(src, &theme::DARK, scene)
}
