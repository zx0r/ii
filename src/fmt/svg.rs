//! SVG ingest adapter: theme rewriter + scene ingest entry point.
//! Single-pass O(N) streaming SVG theme rewriter (`src → dst`, zero in-place `memmove`).
//! parse_into_scene stores the raw (or rewritten) SVG bytes into scene.text_pool.

use memchr::memchr3;

use crate::{
    err::Error,
    scene::{GlyphRun, Scene},
    theme::{ColorToken, Idx, Palette},
};

// ── Black/white hex matchers ──────────────────────────────────────────────────

#[inline]
fn is_black_hex(rem: &[u8]) -> Option<usize> {
    if rem.len() >= 7 && rem[..7].eq_ignore_ascii_case(b"#000000") {
        Some(7)
    } else if rem.len() >= 4
        && rem[..4].eq_ignore_ascii_case(b"#000")
        && (rem.len() == 4 || !rem[4].is_ascii_hexdigit())
    {
        Some(4)
    } else {
        None
    }
}

#[inline]
fn is_white_hex(rem: &[u8]) -> Option<usize> {
    if rem.len() >= 7 && rem[..7].eq_ignore_ascii_case(b"#ffffff") {
        Some(7)
    } else if rem.len() >= 4
        && rem[..4].eq_ignore_ascii_case(b"#fff")
        && (rem.len() == 4 || !rem[4].is_ascii_hexdigit())
    {
        Some(4)
    } else {
        None
    }
}

#[inline]
fn match_rgb_zero(rem: &[u8]) -> Option<usize> {
    if !rem.starts_with(b"rgb(") {
        return None;
    }
    let close = rem[..rem.len().min(32)].iter().position(|&b| b == b')')?;
    let inner = &rem[4..close];
    if inner.iter().all(|&b| matches!(b, b'0' | b',' | b' ' | b'%' | b'.'))
        && inner.contains(&b'0')
    {
        Some(close + 1)
    } else {
        None
    }
}

// ── Color rewriter ────────────────────────────────────────────────────────────

/// Stream `src` into `dst`, substituting hardcoded black/white values with
/// palette foreground/background tokens in a single O(N) pass.
/// Handles: `#000`/`#000000`, `currentColor`, `rgb(0,0,0)`, `#fff`/`#ffffff`.
pub fn rewrite_colors(src: &[u8], palette: &Palette, dst: &mut Vec<u8>) {
    let fg  = palette.get(Idx::Fg).hex().as_bytes();
    let bg  = palette.get(Idx::Bg).hex().as_bytes();
    let len = src.len();
    let mut pos = 0usize;

    while pos < len {
        let rem = &src[pos..];
        let Some(off) = memchr3(b'#', b'c', b'r', rem) else {
            dst.extend_from_slice(rem);
            break;
        };
        dst.extend_from_slice(&rem[..off]);
        pos += off;
        let tail = &src[pos..];

        match tail[0] {
            b'#' => {
                if let Some(n) = is_black_hex(tail) {
                    dst.extend_from_slice(fg);
                    pos += n;
                } else if let Some(n) = is_white_hex(tail) {
                    dst.extend_from_slice(bg);
                    pos += n;
                } else {
                    dst.push(b'#');
                    pos += 1;
                }
            }
            b'c' => {
                if tail.len() >= 12 && &tail[..12] == b"currentColor" {
                    dst.extend_from_slice(fg);
                    pos += 12;
                } else {
                    dst.push(b'c');
                    pos += 1;
                }
            }
            b'r' => {
                if let Some(n) = match_rgb_zero(tail) {
                    dst.extend_from_slice(fg);
                    pos += n;
                } else {
                    dst.push(b'r');
                    pos += 1;
                }
            }
            _ => {
                dst.push(tail[0]);
                pos += 1;
            }
        }
    }
}

// ── Scene ingest ──────────────────────────────────────────────────────────────

/// Parse SVG `src` into `scene`.
/// Stores the (palette-rewritten) SVG bytes in `scene.text_pool` referenced by
/// a sentinel `GlyphRun` with `flags = 0x80` (SVG marker) so backends can
/// rasterize via `raster::render_svg`.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    // Rewrite colors into a temporary region of text_pool.
    let start = scene.text_pool.len() as u32;
    // Use dark palette — theme is resolved at render time; we just strip
    // hardcoded black/white. We need any Palette; use the static DARK constant.
    rewrite_colors(src, &crate::theme::DARK, &mut scene.text_pool);
    let end = scene.text_pool.len();
    let len = end - start as usize;

    if len == 0 {
        return Ok(());
    }

    // Approximate SVG viewport from width= / height= attributes.
    let (svg_w, svg_h) = extract_svg_dimensions(&scene.text_pool[start as usize..end]);

    scene.glyphs.push(GlyphRun {
        x: 0.0,
        y: scene.height,
        text_off: start,
        text_len: len.min(u16::MAX as usize) as u16,
        font_size: 0,          // unused for SVG marker
        color: ColorToken::FG_PRIMARY,
        flags: 0x80,           // 0x80 = SVG blob marker
        _pad: [0; 2],
    });

    scene.height += svg_h;
    if svg_w > scene.width { scene.width = svg_w; }

    Ok(())
}

/// Extract `width` and `height` from an SVG `<svg ...>` opening tag.
/// Returns `(800.0, 600.0)` as fallback.
pub fn extract_svg_dimensions(src: &[u8]) -> (f32, f32) {
    let tag_end = src.iter().position(|&b| b == b'>').unwrap_or(src.len().min(512));
    let tag = &src[..tag_end];

    let w = extract_attr(tag, b"width=")
        .or_else(|| extract_viewbox_dim(tag, 2))
        .unwrap_or(800.0);
    let h = extract_attr(tag, b"height=")
        .or_else(|| extract_viewbox_dim(tag, 3))
        .unwrap_or(600.0);
    (w, h)
}

fn extract_attr(tag: &[u8], name: &[u8]) -> Option<f32> {
    let pos = tag.windows(name.len()).position(|w| w.eq_ignore_ascii_case(name))?;
    let after = &tag[pos + name.len()..];
    // Skip quote character
    let after = after.strip_prefix(b"\"").or_else(|| after.strip_prefix(b"'")).unwrap_or(after);
    let end = after.iter().position(|&b| !b.is_ascii_digit() && b != b'.').unwrap_or(after.len());
    core::str::from_utf8(&after[..end]).ok()?.parse().ok()
}

fn extract_viewbox_dim(tag: &[u8], idx: usize) -> Option<f32> {
    let pos = tag.windows(8).position(|w| w.eq_ignore_ascii_case(b"viewBox="))?;
    let after = &tag[pos + 8..];
    let after = after.strip_prefix(b"\"").or_else(|| after.strip_prefix(b"'")).unwrap_or(after);
    let end = after.iter().position(|&b| b == b'"' || b == b'\'').unwrap_or(after.len());
    let vb = &after[..end];
    let mut parts = vb.split(|&b| b == b' ' || b == b',');
    for _ in 0..idx { parts.next()?; }
    core::str::from_utf8(parts.next()?).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::DARK;

    #[test]
    fn rewrites_black_current_color_and_rgb_in_single_pass() {
        let src = b"<svg fill=\"#000\" stroke=\"currentColor\"><path fill=\"rgb(0, 0, 0)\" bg=\"#ffffff\"/></svg>";
        let mut dst = Vec::new();
        rewrite_colors(src, &DARK, &mut dst);
        let fg = DARK.get(Idx::Fg).hex();
        let bg = DARK.get(Idx::Bg).hex();
        let s = core::str::from_utf8(&dst).unwrap();
        assert!(s.contains(fg));
        assert!(s.contains(bg));
        assert!(!s.contains("#000"));
        assert!(!s.contains("currentColor"));
        assert!(!s.contains("rgb(0, 0, 0)"));
    }

    #[test]
    fn parse_into_scene_stores_svg_glyph_marker() {
        let src = b"<svg width=\"400\" height=\"300\"><rect/></svg>";
        let mut scene = Scene::with_capacity(16, 256);
        parse_into_scene(src, &mut scene).unwrap();
        assert_eq!(scene.glyphs.len(), 1);
        assert_eq!(scene.glyphs[0].flags, 0x80);
        assert!(scene.height > 0.0);
    }
}
