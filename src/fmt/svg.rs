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

fn attr<'a>(header: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    for pos in memchr::memmem::find_iter(header, key) {
        if pos > 0 && !header[pos - 1].is_ascii_whitespace() {
            continue;
        }
        let rest = &header[pos + key.len()..];
        let quote = *rest.first()?;
        if !matches!(quote, b'"' | b'\'') {
            continue;
        }
        let end = memchr::memchr(quote, &rest[1..])?;
        return Some(&rest[1..=end]);
    }
    None
}

fn parse_attr_f32(header: &[u8], key: &[u8]) -> Option<f32> {
    let raw = core::str::from_utf8(attr(header, key)?).ok()?.trim();
    let end = raw
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')))
        .unwrap_or(raw.len());
    let factor = match raw[end..].trim() {
        "" | "px" => 1.0,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        _ => return None,
    };
    let value = raw[..end].parse::<f32>().ok()? * factor;
    (value.is_finite() && value > 0.0).then_some(value)
}

fn parse_viewbox(header: &[u8]) -> Option<(f32, f32)> {
    let text = core::str::from_utf8(attr(header, b"viewBox=")?).ok()?;
    let mut nums = text
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .map(str::parse::<f32>);
    let _x = nums.next()?.ok()?;
    let _y = nums.next()?.ok()?;
    let width = nums.next()?.ok()?;
    let height = nums.next()?.ok()?;
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
    let viewbox = parse_viewbox(header);
    let width = parse_attr_f32(header, b"width=");
    let height = parse_attr_f32(header, b"height=");
    match (width, height, viewbox) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some((vw, vh))) => (w, w * vh / vw),
        (None, Some(h), Some((vw, vh))) => (h * vw / vh, h),
        (None, None, Some(dims)) => dims,
        (w, h, _) => (w.unwrap_or(320.0), h.unwrap_or(180.0)),
    }
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
    const GZIP_MAGIC: [u8; 2] = [0x1F, 0x8B];
    let decompressed: Vec<u8>;
    let payload = if src.starts_with(&GZIP_MAGIC) {
        use std::io::Read;
        decompressed = {
            let mut out = Vec::new();
            let decoder = flate2::read::MultiGzDecoder::new(src);
            decoder
                .take(64 * 1024 * 1024)
                .read_to_end(&mut out)
                .map_err(|_| Error::SvgParse)?;
            out
        };
        &decompressed[..]
    } else {
        src
    };

    if !payload.windows(4).any(|w| w == b"<svg") {
        return Err(Error::SvgParse);
    }
    let start = scene.text_pool.len();
    rewrite_colors(payload, palette, &mut scene.text_pool);
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
