//! SVG & Scene IR -> cell-snapped RGBA8 rasterizer, PNG decoder, and Q24 unpremultiplier.

use crate::{
    err::Error,
    scene::{Path, Rect, Scene, Verb},
    theme::Palette,
};

pub const MAX_DIM: u32 = 4096;

const UNPREMUL_LUT: [u32; 256] = {
    let mut lut = [0u32; 256];
    let mut a = 1usize;
    while a < 256 {
        #[expect(clippy::cast_possible_truncation, reason = "max (255<<24) < u32::MAX")]
        let q24 = (((255_u64 << 24) + (a as u64 >> 1)) / (a as u64)) as u32;
        lut[a] = q24;
        a += 1;
    }
    lut
};

#[inline]
pub fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = px[3];
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else if a < 255 {
            let inv = UNPREMUL_LUT[usize::from(a)];
            px[0] = ((u32::from(px[0].min(a)) * inv + (1 << 23)) >> 24).min(255) as u8;
            px[1] = ((u32::from(px[1].min(a)) * inv + (1 << 23)) >> 24).min(255) as u8;
            px[2] = ((u32::from(px[2].min(a)) * inv + (1 << 23)) >> 24).min(255) as u8;
        }
    }
}

#[inline]
fn snap_dims(
    raw_w: u32,
    raw_h: u32,
    cell_w: u16,
    cell_h: u16,
) -> Result<(u16, u16, u16, u16, usize), Error> {
    let (cw, ch) = (u32::from(cell_w.max(1)), u32::from(cell_h.max(1)));
    let (cols, rows) = (
        raw_w.max(1).div_ceil(cw).max(1),
        raw_h.max(1).div_ceil(ch).max(1),
    );
    let (snap_w, snap_h) = (cols.saturating_mul(cw), rows.saturating_mul(ch));
    let too_big = Error::ImageTooLarge {
        w: snap_w,
        h: snap_h,
    };
    if snap_w > MAX_DIM || snap_h > MAX_DIM {
        return Err(too_big);
    }
    let bytes = (snap_w as usize)
        .checked_mul(snap_h as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or(too_big)?;
    Ok((
        u16::try_from(snap_w).map_err(|_| too_big)?,
        u16::try_from(snap_h).map_err(|_| too_big)?,
        u16::try_from(cols).map_err(|_| too_big)?,
        u16::try_from(rows).map_err(|_| too_big)?,
        bytes,
    ))
}

/// Rasterize `svg` into cell-snapped unpremultiplied RGBA8. Returns `(w, h, cols, rows)`.
///
/// # Errors
/// Returns `Error::SvgParse` or `Error::ImageTooLarge`.
pub fn rasterize(
    svg: &[u8],
    cell_w: u16,
    cell_h: u16,
    max_width_px: u16,
    rgba: &mut Vec<u8>,
) -> Result<(u16, u16, u16, u16), Error> {
    let max_w = u32::from(max_width_px.max(cell_w.max(1))).min(MAX_DIM);
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default())
        .map_err(|_| Error::SvgParse)?;
    let (src_w, src_h) = (tree.size().width(), tree.size().height());
    if !(src_w.is_finite() && src_h.is_finite() && src_w > 0.0 && src_h > 0.0) {
        return Err(Error::SvgParse);
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "max_w <= MAX_DIM (4096) exact in f32"
    )]
    let max_w_f = max_w as f32;
    let scale = if src_w > max_w_f {
        max_w_f / src_w
    } else {
        1.0
    };

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "positive scaled dims validated in snap_dims"
    )]
    let (raw_w, raw_h) = (
        ((src_w * scale).ceil() as u32).max(1),
        ((src_h * scale).ceil() as u32).max(1),
    );
    let (w, h, cols, rows, bytes) = snap_dims(raw_w, raw_h, cell_w, cell_h)?;
    rgba.clear();
    rgba.resize(bytes, 0);

    let mut pixmap = tiny_skia::PixmapMut::from_bytes(rgba, u32::from(w), u32::from(h))
        .ok_or(Error::SvgParse)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap,
    );
    unpremultiply(rgba);
    Ok((w, h, cols, rows))
}

fn draw_scene_path(
    scene: &Scene,
    p: &Path,
    palette: &Palette,
    pixmap: &mut tiny_skia::PixmapMut<'_>,
) {
    let (Some(pt_end), Some(v_end)) = (
        (p.pt_start as usize).checked_add(p.pt_count as usize),
        (p.verb_start as usize).checked_add(p.verb_count as usize),
    ) else {
        return;
    };
    let (Some(pts), Some(verbs)) = (
        scene.points.get(p.pt_start as usize..pt_end),
        scene.verbs.get(p.verb_start as usize..v_end),
    ) else {
        return;
    };

    let mut pb = tiny_skia::PathBuilder::new();
    let mut idx = 0usize;
    for &v in verbs {
        match v {
            Verb::MoveTo => {
                if let Some(&[x, y]) = pts.get(idx) {
                    pb.move_to(x, y);
                    idx += 1;
                }
            }
            Verb::LineTo => {
                if let Some(&[x, y]) = pts.get(idx) {
                    pb.line_to(x, y);
                    idx += 1;
                }
            }
            Verb::QuadTo => {
                if let (Some(&[x1, y1]), Some(&[x2, y2])) = (pts.get(idx), pts.get(idx + 1)) {
                    pb.quad_to(x1, y1, x2, y2);
                    idx += 2;
                }
            }
            Verb::CubicTo => {
                if let (Some(&[x1, y1]), Some(&[x2, y2]), Some(&[x3, y3])) =
                    (pts.get(idx), pts.get(idx + 1), pts.get(idx + 2))
                {
                    pb.cubic_to(x1, y1, x2, y2, x3, y3);
                    idx += 3;
                }
            }
            Verb::Close => pb.close(),
        }
    }
    let Some(path) = pb.finish() else { return };
    let c = palette.color(p.color);
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(c.r(), c.g(), c.b(), c.a());
    let id = tiny_skia::Transform::identity();

    if (p.flags & Path::FILL) != 0 {
        pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, id, None);
    }
    if (p.flags & Path::STROKE) != 0 || (p.flags & Path::FILL) == 0 {
        let stroke = tiny_skia::Stroke {
            width: f32::from(p.stroke_width.max(1)),
            ..tiny_skia::Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, id, None);
    }
}

/// Rasterize `Scene` into cell-snapped RGBA8 using `palette`.
///
/// # Errors
/// Returns `Error::ImageTooLarge` or `Error::SvgParse`.
pub fn rasterize_scene(
    scene: &Scene,
    palette: &Palette,
    cell_w: u16,
    cell_h: u16,
    max_width_px: u16,
    rgba: &mut Vec<u8>,
) -> Result<(u16, u16, u16, u16), Error> {
    let max_w = u32::from(max_width_px.max(cell_w.max(1))).min(MAX_DIM);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped positive finite f32 <= MAX_DIM"
    )]
    let raw_w = if scene.width.is_finite() && scene.width > 0.0 {
        (scene.width.ceil() as u32).clamp(1, max_w)
    } else {
        1
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped positive finite f32 <= MAX_DIM"
    )]
    let raw_h = if scene.height.is_finite() && scene.height > 0.0 {
        (scene.height.ceil() as u32).clamp(1, MAX_DIM)
    } else {
        1
    };

    let (w, h, cols, rows, bytes) = snap_dims(raw_w, raw_h, cell_w, cell_h)?;
    rgba.clear();
    rgba.resize(bytes, 0);

    let mut pixmap = tiny_skia::PixmapMut::from_bytes(rgba, u32::from(w), u32::from(h))
        .ok_or(Error::SvgParse)?;
    let id = tiny_skia::Transform::identity();
    for r in &scene.rects {
        let Some(rect) = tiny_skia::Rect::from_xywh(r.x, r.y, r.w, r.h) else {
            continue;
        };
        let c = palette.color(r.color);
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(c.r(), c.g(), c.b(), c.a());
        if (r.flags & Rect::FILL) != 0 || r.flags == 0 {
            pixmap.fill_rect(rect, &paint, id, None);
        }
        if (r.flags & Rect::STROKE) != 0 {
            let path = tiny_skia::PathBuilder::from_rect(rect);
            let stroke = tiny_skia::Stroke {
                width: f32::from(r.stroke_width.max(1)),
                ..tiny_skia::Stroke::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, id, None);
        }
    }
    for p in &scene.paths {
        draw_scene_path(scene, p, palette, &mut pixmap);
    }
    unpremultiply(rgba);
    Ok((w, h, cols, rows))
}

/// Decode a PNG slice into straight RGBA8 in-place.
///
/// # Errors
/// Returns `Error::ImageTooLarge` or `Error::PngDecode`.
pub fn decode_png(src: &[u8], rgba: &mut Vec<u8>) -> Result<(u32, u32), Error> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(src));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|_| Error::PngDecode)?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width == 0 || height == 0 || width > MAX_DIM || height > MAX_DIM {
        return Err(Error::ImageTooLarge {
            w: width,
            h: height,
        });
    }
    let px_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or(Error::ImageTooLarge {
            w: width,
            h: height,
        })?;
    let rgba_len = px_count.checked_mul(4).ok_or(Error::ImageTooLarge {
        w: width,
        h: height,
    })?;
    let buf_size = reader.output_buffer_size().ok_or(Error::PngDecode)?;

    rgba.clear();
    rgba.resize(buf_size.max(rgba_len), 0);
    let frame = reader
        .next_frame(&mut rgba[..buf_size])
        .map_err(|_| Error::PngDecode)?;
    match frame.color_type {
        png::ColorType::Rgba if frame.buffer_size() == rgba_len => {}
        png::ColorType::Rgb if frame.buffer_size() == px_count * 3 => {
            for idx in (0..px_count).rev() {
                let (src_off, dst_off) = (idx * 3, idx * 4);
                let px = [rgba[src_off], rgba[src_off + 1], rgba[src_off + 2], 0xFF];
                rgba[dst_off..dst_off + 4].copy_from_slice(&px);
            }
        }
        png::ColorType::GrayscaleAlpha if frame.buffer_size() == px_count * 2 => {
            for idx in (0..px_count).rev() {
                let (src_off, dst_off) = (idx * 2, idx * 4);
                let (luma, alpha) = (rgba[src_off], rgba[src_off + 1]);
                rgba[dst_off..dst_off + 4].copy_from_slice(&[luma, luma, luma, alpha]);
            }
        }
        png::ColorType::Grayscale if frame.buffer_size() == px_count => {
            for idx in (0..px_count).rev() {
                let (dst_off, luma) = (idx * 4, rgba[idx]);
                rgba[dst_off..dst_off + 4].copy_from_slice(&[luma, luma, luma, 0xFF]);
            }
        }
        _ => return Err(Error::PngDecode),
    }
    rgba.truncate(rgba_len);
    Ok((width, height))
}
