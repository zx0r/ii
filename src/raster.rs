//! SVG → cell-snapped RGBA rasterizer, PNG decoder, and Q24 fixed-point alpha unpremultiplier.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "bounded pixel dimensions (<= 4096) and Q24 fixed-point unpremultiply arithmetic"
)]

use crate::err::Error;

const MAX_DIM: u32 = 4096;

/// Compile-time Q24 reciprocal LUT: `UNPREMUL_LUT[a] = ((255 << 24) + (a >> 1)) / a`.
const UNPREMUL_LUT: [u32; 256] = {
    let mut lut = [0u32; 256];
    let mut a = 1usize;
    while a < 256 {
        lut[a] = (((255u64 << 24) + (a as u64 >> 1)) / (a as u64)) as u32;
        a += 1;
    }
    lut
};

/// In-place Q24 fixed-point unpremultiplication of premultiplied RGBA8 pixels.
/// Zero integer division on the hot loop.
#[inline]
pub fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3];
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else if a < 255 {
            let inv = UNPREMUL_LUT[a as usize];
            px[0] = ((u32::from(px[0]) * inv + (1 << 23)) >> 24).min(255) as u8;
            px[1] = ((u32::from(px[1]) * inv + (1 << 23)) >> 24).min(255) as u8;
            px[2] = ((u32::from(px[2]) * inv + (1 << 23)) >> 24).min(255) as u8;
        }
    }
}

/// Rasterize `svg` into cell-snapped unpremultiplied RGBA8 inside `rgba`.
/// Returns `(w_px, h_px, cols, rows)`.
pub fn rasterize(
    svg: &[u8],
    cell_w: u16,
    cell_h: u16,
    max_width_px: u16,
    rgba: &mut Vec<u8>,
) -> Result<(u16, u16, u16, u16), Error> {
    let cw = u32::from(cell_w.max(1));
    let ch = u32::from(cell_h.max(1));
    let max_w = u32::from(max_width_px.max(cell_w.max(1))).min(MAX_DIM);

    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(svg, &opt).map_err(|_| Error::SvgParse)?;
    let sz = tree.size();
    let src_w = sz.width();
    let src_h = sz.height();
    if !(src_w > 0.0 && src_h > 0.0) {
        return Err(Error::SvgParse);
    }

    let scale = if src_w > max_w as f32 {
        (max_w as f32) / src_w
    } else {
        1.0
    };
    let raw_w = ((src_w * scale).ceil() as u32).max(1);
    let raw_h = ((src_h * scale).ceil() as u32).max(1);

    let cols = raw_w.div_ceil(cw).max(1);
    let rows = raw_h.div_ceil(ch).max(1);
    let snap_w = cols.saturating_mul(cw);
    let snap_h = rows.saturating_mul(ch);

    if snap_w > MAX_DIM || snap_h > MAX_DIM {
        return Err(Error::ImageTooLarge {
            w: snap_w,
            h: snap_h,
        });
    }

    let total_bytes = (snap_w as usize)
        .checked_mul(snap_h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(Error::ImageTooLarge {
            w: snap_w,
            h: snap_h,
        })?;

    rgba.clear();
    rgba.resize(total_bytes, 0);

    let mut pixmap =
        tiny_skia::PixmapMut::from_bytes(rgba.as_mut_slice(), snap_w, snap_h).ok_or(Error::SvgParse)?;
    let ts = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, ts, &mut pixmap);

    unpremultiply(rgba.as_mut_slice());

    Ok((snap_w as u16, snap_h as u16, cols as u16, rows as u16))
}

/// Decode a PNG byte slice `src` (or base64-encoded PNG) into `rgba` (RGBA8).
/// Returns `(width, height)`.
pub fn decode_png(src: &[u8], rgba: &mut Vec<u8>) -> Result<(u32, u32), Error> {
    let mut decoder = png::Decoder::new(core::io::BorrowedBuf::from(src).into_reader());
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| Error::PngDecode)?;
    let info = reader.info();
    let w = info.width;
    let h = info.height;
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
        return Err(Error::ImageTooLarge { w, h });
    }
    let buf_size = reader.output_buffer_size().ok_or(Error::PngDecode)?;
    rgba.clear();
    rgba.resize(buf_size, 0);
    let frame = reader
        .next_frame(rgba.as_mut_slice())
        .map_err(|_| Error::PngDecode)?;

    match frame.color_type {
        png::ColorType::Rgba => {
            rgba.truncate(frame.buffer_size());
        }
        png::ColorType::Rgb => {
            let n_px = (w as usize) * (h as usize);
            rgba.resize(n_px * 4, 255);
            for i in (0..n_px).rev() {
                let r = rgba[i * 3];
                let g = rgba[i * 3 + 1];
                let b = rgba[i * 3 + 2];
                rgba[i * 4] = r;
                rgba[i * 4 + 1] = g;
                rgba[i * 4 + 2] = b;
                rgba[i * 4 + 3] = 255;
            }
        }
        png::ColorType::Grayscale => {
            let n_px = (w as usize) * (h as usize);
            rgba.resize(n_px * 4, 255);
            for i in (0..n_px).rev() {
                let y = rgba[i];
                rgba[i * 4] = y;
                rgba[i * 4 + 1] = y;
                rgba[i * 4 + 2] = y;
                rgba[i * 4 + 3] = 255;
            }
        }
        png::ColorType::GrayscaleAlpha => {
            let n_px = (w as usize) * (h as usize);
            rgba.resize(n_px * 4, 255);
            for i in (0..n_px).rev() {
                let y = rgba[i * 2];
                let a = rgba[i * 2 + 1];
                rgba[i * 4] = y;
                rgba[i * 4 + 1] = y;
                rgba[i * 4 + 2] = y;
                rgba[i * 4 + 3] = a;
            }
        }
        png::ColorType::Indexed => return Err(Error::PngDecode),
    }

    Ok((w, h))
}
