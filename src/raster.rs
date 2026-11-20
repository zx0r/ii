//! Complete SVG & `Scene` IR -> cell-snapped RGBA8 rasterizer, PNG decoder, and Q24 unpremultiplier.

use crate::{
    err::Error,
    scene::{GlyphRun, Path, Rect, Scene, TableGrid, Verb},
    theme::{ColorToken, Palette, Rgba},
};

pub const MAX_DIM: u32 = 4096;
const GRID_ROW_H: f32 = 18.0;

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

#[inline]
fn make_paint(c: Rgba) -> tiny_skia::Paint<'static> {
    let mut paint = tiny_skia::Paint {
        anti_alias: true,
        ..tiny_skia::Paint::default()
    };
    paint.set_color_rgba8(c.r(), c.g(), c.b(), c.a());
    paint
}

/// Minimalist geometric vector stroke definitions on a `4x6` grid (`(x0<<4|y0, x1<<4|y1)`).
#[rustfmt::skip]
const fn ascii_strokes(ch: u8) -> &'static [(u8, u8)] {
    match ch {
        b'!' => &[(0x20,0x24),(0x26,0x26)],
        b'"' => &[(0x10,0x12),(0x30,0x32)],
        b'#' => &[(0x10,0x16),(0x30,0x36),(0x02,0x42),(0x04,0x44)],
        b'$' => &[(0x20,0x26),(0x41,0x11),(0x11,0x02),(0x02,0x44),(0x44,0x35),(0x35,0x05)],
        b'%' => &[(0x06,0x40),(0x01,0x11),(0x35,0x45)],
        b'&' => &[(0x46,0x02),(0x02,0x10),(0x10,0x31),(0x31,0x04),(0x04,0x16),(0x16,0x44)],
        b'\'' => &[(0x20,0x22)],
        b'(' => &[(0x30,0x12),(0x12,0x14),(0x14,0x36)],
        b')' => &[(0x10,0x32),(0x32,0x34),(0x34,0x16)],
        b'*' => &[(0x21,0x25),(0x02,0x44),(0x42,0x04)],
        b'+' => &[(0x21,0x25),(0x03,0x43)],
        b',' => &[(0x25,0x17)],
        b'-' => &[(0x03,0x43)],
        b'.' => &[(0x25,0x26)],
        b'/' => &[(0x06,0x40)],
        b'0' => &[(0x10,0x30),(0x30,0x41),(0x41,0x45),(0x45,0x36),(0x36,0x16),(0x16,0x05),(0x05,0x01),(0x01,0x10),(0x15,0x31)],
        b'1' => &[(0x11,0x20),(0x20,0x26),(0x16,0x36)],
        b'2' => &[(0x01,0x10),(0x10,0x30),(0x30,0x41),(0x41,0x42),(0x42,0x06),(0x06,0x46)],
        b'3' => &[(0x00,0x30),(0x30,0x41),(0x41,0x33),(0x13,0x33),(0x33,0x44),(0x44,0x36),(0x36,0x06)],
        b'4' => &[(0x36,0x30),(0x30,0x04),(0x04,0x44)],
        b'5' => &[(0x40,0x00),(0x00,0x03),(0x03,0x33),(0x33,0x44),(0x44,0x36),(0x36,0x06)],
        b'6' => &[(0x30,0x10),(0x10,0x02),(0x02,0x05),(0x05,0x16),(0x16,0x36),(0x36,0x45),(0x45,0x33),(0x33,0x03)],
        b'7' => &[(0x00,0x40),(0x40,0x16)],
        b'8' => &[(0x10,0x30),(0x30,0x41),(0x41,0x33),(0x13,0x33),(0x13,0x01),(0x01,0x10),(0x13,0x05),(0x05,0x16),(0x16,0x36),(0x36,0x45),(0x45,0x33)],
        b'9' => &[(0x16,0x36),(0x36,0x44),(0x44,0x41),(0x41,0x30),(0x30,0x10),(0x10,0x01),(0x01,0x13),(0x13,0x43)],
        b':' => &[(0x22,0x22),(0x25,0x25)],
        b';' => &[(0x22,0x22),(0x25,0x17)],
        b'<' => &[(0x41,0x03),(0x03,0x45)],
        b'=' => &[(0x02,0x42),(0x04,0x44)],
        b'>' => &[(0x01,0x43),(0x43,0x05)],
        b'?' => &[(0x01,0x10),(0x10,0x30),(0x30,0x41),(0x41,0x23),(0x23,0x24),(0x26,0x26)],
        b'@' => &[(0x34,0x24),(0x24,0x13),(0x13,0x22),(0x22,0x32),(0x32,0x34),(0x34,0x44),(0x44,0x41),(0x41,0x30),(0x30,0x10),(0x10,0x01),(0x01,0x05),(0x05,0x16),(0x16,0x46)],
        b'A' => &[(0x06,0x20),(0x20,0x46),(0x14,0x34)],
        b'B' => &[(0x00,0x06),(0x00,0x30),(0x30,0x41),(0x41,0x33),(0x03,0x33),(0x33,0x44),(0x44,0x36),(0x36,0x06)],
        b'C' => &[(0x41,0x30),(0x30,0x10),(0x10,0x01),(0x01,0x05),(0x05,0x16),(0x16,0x36),(0x36,0x45)],
        b'D' => &[(0x00,0x06),(0x00,0x20),(0x20,0x42),(0x42,0x44),(0x44,0x26),(0x26,0x06)],
        b'E' => &[(0x40,0x00),(0x00,0x06),(0x06,0x46),(0x03,0x33)],
        b'F' => &[(0x40,0x00),(0x00,0x06),(0x03,0x33)],
        b'G' => &[(0x41,0x30),(0x30,0x10),(0x10,0x01),(0x01,0x05),(0x05,0x16),(0x16,0x36),(0x36,0x45),(0x45,0x43),(0x43,0x23)],
        b'H' => &[(0x00,0x06),(0x40,0x46),(0x03,0x43)],
        b'I' => &[(0x10,0x30),(0x20,0x26),(0x16,0x36)],
        b'J' => &[(0x10,0x40),(0x30,0x35),(0x35,0x26),(0x26,0x16),(0x16,0x05)],
        b'K' => &[(0x00,0x06),(0x40,0x03),(0x13,0x46)],
        b'L' => &[(0x00,0x06),(0x06,0x46)],
        b'M' => &[(0x06,0x00),(0x00,0x23),(0x23,0x40),(0x40,0x46)],
        b'N' => &[(0x06,0x00),(0x00,0x46),(0x46,0x40)],
        b'O' => &[(0x10,0x30),(0x30,0x41),(0x41,0x45),(0x45,0x36),(0x36,0x16),(0x16,0x05),(0x05,0x01),(0x01,0x10)],
        b'P' => &[(0x06,0x00),(0x00,0x30),(0x30,0x41),(0x41,0x42),(0x42,0x33),(0x33,0x03)],
        b'Q' => &[(0x10,0x30),(0x30,0x41),(0x41,0x45),(0x45,0x36),(0x36,0x16),(0x16,0x05),(0x05,0x01),(0x01,0x10),(0x24,0x46)],
        b'R' => &[(0x06,0x00),(0x00,0x30),(0x30,0x41),(0x41,0x33),(0x33,0x03),(0x23,0x46)],
        b'S' => &[(0x41,0x30),(0x30,0x10),(0x10,0x01),(0x01,0x02),(0x02,0x44),(0x44,0x45),(0x45,0x36),(0x36,0x16),(0x16,0x05)],
        b'T' => &[(0x00,0x40),(0x20,0x26)],
        b'U' => &[(0x00,0x05),(0x05,0x16),(0x16,0x36),(0x36,0x45),(0x45,0x40)],
        b'V' => &[(0x00,0x26),(0x26,0x40)],
        b'W' => &[(0x00,0x06),(0x06,0x23),(0x23,0x46),(0x46,0x40)],
        b'X' => &[(0x00,0x46),(0x40,0x06)],
        b'Y' => &[(0x00,0x23),(0x40,0x23),(0x23,0x26)],
        b'Z' => &[(0x00,0x40),(0x40,0x06),(0x06,0x46)],
        b'[' => &[(0x30,0x10),(0x10,0x16),(0x16,0x36)],
        b'\\' => &[(0x00,0x46)],
        b']' => &[(0x10,0x30),(0x30,0x36),(0x36,0x16)],
        b'^' => &[(0x02,0x20),(0x20,0x42)],
        b'_' => &[(0x06,0x46)],
        b'`' => &[(0x10,0x32)],
        b'a' => &[(0x12,0x32),(0x32,0x43),(0x43,0x46),(0x44,0x14),(0x14,0x05),(0x05,0x16),(0x16,0x46)],
        b'b' => &[(0x00,0x06),(0x03,0x12),(0x12,0x32),(0x32,0x43),(0x43,0x45),(0x45,0x36),(0x36,0x06)],
        b'c' => &[(0x43,0x32),(0x32,0x12),(0x12,0x03),(0x03,0x05),(0x05,0x16),(0x16,0x36),(0x36,0x45)],
        b'd' => &[(0x40,0x46),(0x43,0x32),(0x32,0x12),(0x12,0x03),(0x03,0x05),(0x05,0x16),(0x16,0x46)],
        b'e' => &[(0x04,0x44),(0x44,0x43),(0x43,0x32),(0x32,0x12),(0x12,0x03),(0x03,0x05),(0x05,0x16),(0x16,0x46)],
        b'f' => &[(0x30,0x20),(0x20,0x11),(0x11,0x16),(0x03,0x33)],
        b'g' => &[(0x42,0x46),(0x46,0x37),(0x37,0x17),(0x43,0x32),(0x32,0x12),(0x12,0x03),(0x03,0x05),(0x05,0x16),(0x16,0x46)],
        b'h' => &[(0x00,0x06),(0x03,0x12),(0x12,0x32),(0x32,0x43),(0x43,0x46)],
        b'i' => &[(0x20,0x20),(0x22,0x26)],
        b'j' => &[(0x30,0x30),(0x32,0x36),(0x36,0x27),(0x27,0x17)],
        b'k' => &[(0x00,0x06),(0x42,0x04),(0x14,0x46)],
        b'l' => &[(0x20,0x26),(0x26,0x36)],
        b'm' => &[(0x06,0x02),(0x02,0x12),(0x12,0x23),(0x23,0x26),(0x23,0x32),(0x32,0x43),(0x43,0x46)],
        b'n' => &[(0x06,0x02),(0x03,0x12),(0x12,0x32),(0x32,0x43),(0x43,0x46)],
        b'o' => &[(0x12,0x32),(0x32,0x43),(0x43,0x45),(0x45,0x36),(0x36,0x16),(0x16,0x05),(0x05,0x03),(0x03,0x12)],
        b'p' => &[(0x02,0x07),(0x03,0x12),(0x12,0x32),(0x32,0x43),(0x43,0x45),(0x45,0x36),(0x36,0x06)],
        b'q' => &[(0x42,0x47),(0x43,0x32),(0x32,0x12),(0x12,0x03),(0x03,0x05),(0x05,0x16),(0x16,0x46)],
        b'r' => &[(0x02,0x06),(0x03,0x12),(0x12,0x42)],
        b's' => &[(0x42,0x12),(0x12,0x03),(0x03,0x44),(0x44,0x36),(0x36,0x06)],
        b't' => &[(0x10,0x15),(0x15,0x26),(0x26,0x46),(0x02,0x32)],
        b'u' => &[(0x02,0x05),(0x05,0x16),(0x16,0x36),(0x36,0x45),(0x42,0x46)],
        b'v' => &[(0x02,0x26),(0x26,0x42)],
        b'w' => &[(0x02,0x16),(0x16,0x23),(0x23,0x36),(0x36,0x42)],
        b'x' => &[(0x02,0x46),(0x42,0x06)],
        b'y' => &[(0x02,0x25),(0x42,0x17)],
        b'z' => &[(0x02,0x42),(0x42,0x06),(0x06,0x46)],
        b'{' => &[(0x30,0x21),(0x21,0x22),(0x22,0x13),(0x13,0x24),(0x24,0x25),(0x25,0x36)],
        b'|' => &[(0x20,0x26)],
        b'}' => &[(0x10,0x21),(0x21,0x22),(0x22,0x33),(0x33,0x24),(0x24,0x25),(0x25,0x16)],
        b'~' => &[(0x03,0x12),(0x12,0x34),(0x34,0x43)],
        _ => &[(0x01,0x41),(0x41,0x45),(0x45,0x05),(0x05,0x01)],
    }
}

#[derive(Clone, Copy)]
struct TextDraw {
    x: f32,
    baseline_y: f32,
    font_px: f32,
    max_x: f32,
    flags: u8,
    color: Rgba,
}

fn draw_vector_text(
    text: &str,
    cfg: TextDraw,
    pixmap: &mut tiny_skia::PixmapMut<'_>,
) {
    if text.is_empty() || cfg.font_px <= 0.0 {
        return;
    }
    let glyph_h = (cfg.font_px * 0.68).max(5.0);
    let glyph_w = glyph_h * 0.54;
    let adv = glyph_w * 1.32;
    let sx = glyph_w * 0.25;
    let sy = glyph_h / 6.0;
    let top_y = cfg.baseline_y - glyph_h;
    let italic = (cfg.flags & GlyphRun::ITALIC) != 0;

    let mut pb = tiny_skia::PathBuilder::new();
    let mut cx = cfg.x;
    for ch in text.chars() {
        if cx + glyph_w > cfg.max_x {
            break;
        }
        if ch == ' ' || ch == '\t' {
            cx += adv;
            continue;
        }
        let code = u8::try_from(ch as u32).unwrap_or(0);
        for &(p0, p1) in ascii_strokes(code) {
            let (gx0, gy0) = (f32::from(p0 >> 4), f32::from(p0 & 0x0F));
            let (gx1, gy1) = (f32::from(p1 >> 4), f32::from(p1 & 0x0F));
            let sh0 = if italic { (6.0 - gy0) * 0.22 } else { 0.0 };
            let sh1 = if italic { (6.0 - gy1) * 0.22 } else { 0.0 };
            let x0 = sx.mul_add(gx0 + sh0, cx);
            let y0 = sy.mul_add(gy0, top_y);
            let x1 = sx.mul_add(gx1 + sh1, cx);
            let y1 = sy.mul_add(gy1, top_y);
            pb.move_to(x0, y0);
            if (p0 == p1) || ((x1 - x0).abs() < 0.05 && (y1 - y0).abs() < 0.05) {
                pb.line_to(x0 + 0.5, y0);
            } else {
                pb.line_to(x1, y1);
            }
        }
        cx += adv;
    }

    let Some(path) = pb.finish() else { return };
    let paint = make_paint(cfg.color);
    let bold_factor = if (cfg.flags & GlyphRun::BOLD) != 0 {
        0.14
    } else {
        0.09
    };
    let stroke = tiny_skia::Stroke {
        width: (glyph_h * bold_factor).max(1.0),
        line_cap: tiny_skia::LineCap::Round,
        line_join: tiny_skia::LineJoin::Round,
        ..tiny_skia::Stroke::default()
    };
    pixmap.stroke_path(
        &path,
        &paint,
        &stroke,
        tiny_skia::Transform::identity(),
        None,
    );
}

fn rounded_rect_path(r: &Rect) -> Option<tiny_skia::Path> {
    let rad = f32::from(r.radius).min(r.w * 0.5).min(r.h * 0.5);
    if rad <= 0.5 {
        return tiny_skia::Rect::from_xywh(r.x, r.y, r.w, r.h)
            .map(tiny_skia::PathBuilder::from_rect);
    }
    let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x0 + rad, y0);
    pb.line_to(x1 - rad, y0);
    pb.quad_to(x1, y0, x1, y0 + rad);
    pb.line_to(x1, y1 - rad);
    pb.quad_to(x1, y1, x1 - rad, y1);
    pb.line_to(x0 + rad, y1);
    pb.quad_to(x0, y1, x0, y1 - rad);
    pb.line_to(x0, y0 + rad);
    pb.quad_to(x0, y0, x0 + rad, y0);
    pb.close();
    pb.finish()
}

fn draw_scene_rects(
    scene: &Scene,
    palette: &Palette,
    ts: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut<'_>,
) {
    for r in &scene.rects {
        let Some(path) = rounded_rect_path(r) else {
            continue;
        };
        let paint = make_paint(palette.color(r.color));
        if (r.flags & Rect::FILL) != 0 || r.flags == 0 {
            pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, ts, None);
        }
        if (r.flags & Rect::STROKE) != 0 {
            let stroke = tiny_skia::Stroke {
                width: f32::from(r.stroke_width.max(1)),
                ..tiny_skia::Stroke::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, ts, None);
        }
    }
}

fn draw_scene_grid(
    scene: &Scene,
    grid: &TableGrid,
    palette: &Palette,
    scale: f32,
    ts: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut<'_>,
) {
    let cols = usize::from(grid.cols);
    let rows = usize::from(grid.rows);
    if cols == 0 || rows == 0 {
        return;
    }
    let w_start = grid.width_start as usize;
    let Some(widths) = scene.col_widths.get(w_start..w_start.saturating_add(cols)) else {
        return;
    };
    let border_paint = make_paint(palette.color(grid.border_color));
    let border_stroke = tiny_skia::Stroke {
        width: 1.0,
        ..tiny_skia::Stroke::default()
    };
    let font_px = 11.5 * scale;

    for row in 0..rows {
        let row_u16 = u16::try_from(row).unwrap_or(u16::MAX);
        let cy = f32::from(row_u16).mul_add(GRID_ROW_H, grid.y);
        let mut cx = grid.x;
        for (col, &col_w) in widths.iter().enumerate() {
            let cell_idx = (grid.cell_start as usize).saturating_add(row * cols + col);
            let Some(cell) = scene.cells.get(cell_idx) else {
                cx += col_w;
                continue;
            };
            if let Some(rect) = tiny_skia::Rect::from_xywh(cx, cy, col_w, GRID_ROW_H) {
                let bg = make_paint(palette.color(cell.bg));
                pixmap.fill_rect(rect, &bg, ts, None);
                let path = tiny_skia::PathBuilder::from_rect(rect);
                pixmap.stroke_path(&path, &border_paint, &border_stroke, ts, None);
            }
            let text = scene.cell_text(cell);
            if !text.is_empty() {
                let pad = 4.0 * scale;
                let tx = cx.mul_add(scale, pad);
                let max_x = (cx + col_w).mul_add(scale, -pad);
                let baseline = GRID_ROW_H.mul_add(0.74, cy) * scale;
                let flags = if cell.flags != 0 { GlyphRun::BOLD } else { 0 };
                draw_vector_text(
                    text,
                    TextDraw {
                        x: tx,
                        baseline_y: baseline,
                        font_px,
                        max_x,
                        flags,
                        color: palette.color(cell.color),
                    },
                    pixmap,
                );
            }
            cx += col_w;
        }
    }
}

fn draw_scene_path(
    scene: &Scene,
    p: &Path,
    palette: &Palette,
    ts: tiny_skia::Transform,
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
    let paint = make_paint(palette.color(p.color));
    if (p.flags & Path::FILL) != 0 {
        pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, ts, None);
    }
    if (p.flags & Path::STROKE) != 0 || (p.flags & Path::FILL) == 0 {
        let stroke = tiny_skia::Stroke {
            width: f32::from(p.stroke_width.max(1)),
            ..tiny_skia::Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, ts, None);
    }
}

fn draw_scene_svgs(scene: &Scene, scale: f32, pixmap: &mut tiny_skia::PixmapMut<'_>) {
    if scene.svgs.is_empty() {
        return;
    }
    let opts = resvg::usvg::Options::default();
    for emb in &scene.svgs {
        let raw = scene.svg_bytes(emb);
        let Ok(tree) = resvg::usvg::Tree::from_data(raw, &opts) else {
            continue;
        };
        let (tw, th) = (tree.size().width(), tree.size().height());
        let sx = if tw.is_finite() && tw > 0.0 && emb.w > 0.0 {
            (emb.w / tw) * scale
        } else {
            scale
        };
        let sy = if th.is_finite() && th > 0.0 && emb.h > 0.0 {
            (emb.h / th) * scale
        } else {
            scale
        };
        let ts = tiny_skia::Transform::from_row(sx, 0.0, 0.0, sy, emb.x * scale, emb.y * scale);
        resvg::render(&tree, ts, pixmap);
    }
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

/// Rasterize complete `Scene` (`BG_PRIMARY`, `rects`, `grids`, `paths`, `svgs`, `glyphs`) into RGBA8.
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
        clippy::cast_precision_loss,
        reason = "max_w <= MAX_DIM (4096) exact in f32"
    )]
    let max_w_f = max_w as f32;
    let src_w = if scene.width.is_finite() && scene.width > 0.0 {
        scene.width
    } else {
        1.0
    };
    let src_h = if scene.height.is_finite() && scene.height > 0.0 {
        scene.height
    } else {
        1.0
    };
    let scale = if src_w > max_w_f {
        max_w_f / src_w
    } else {
        1.0
    };

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped positive finite f32 <= MAX_DIM"
    )]
    let raw_w = ((src_w * scale).ceil() as u32).clamp(1, max_w);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped positive finite f32 <= MAX_DIM"
    )]
    let raw_h = ((src_h * scale).ceil() as u32).clamp(1, MAX_DIM);

    let (w, h, cols, rows, bytes) = snap_dims(raw_w, raw_h, cell_w, cell_h)?;
    rgba.clear();
    rgba.resize(bytes, 0);

    let mut pixmap = tiny_skia::PixmapMut::from_bytes(rgba, u32::from(w), u32::from(h))
        .ok_or(Error::SvgParse)?;
    let bg = palette.color(ColorToken::BG_PRIMARY);
    pixmap.fill(tiny_skia::Color::from_rgba8(bg.r(), bg.g(), bg.b(), bg.a()));

    let ts = tiny_skia::Transform::from_scale(scale, scale);
    draw_scene_rects(scene, palette, ts, &mut pixmap);
    for grid in &scene.grids {
        draw_scene_grid(scene, grid, palette, scale, ts, &mut pixmap);
    }
    for p in &scene.paths {
        draw_scene_path(scene, p, palette, ts, &mut pixmap);
    }
    draw_scene_svgs(scene, scale, &mut pixmap);

    let canvas_w = f32::from(w);
    for glyph in &scene.glyphs {
        if glyph.font_size == 0 {
            continue;
        }
        draw_vector_text(
            scene.text(glyph),
            TextDraw {
                x: glyph.x * scale,
                baseline_y: glyph.y * scale,
                font_px: f32::from(glyph.font_size) * scale,
                max_x: canvas_w - 4.0,
                flags: glyph.flags,
                color: palette.color(glyph.color),
            },
            &mut pixmap,
        );
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
