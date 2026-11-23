//! Complete SVG & `Scene` IR -> cell-snapped RGBA8 rasterizer, PNG decoder, and Q24 unpremultiplier.

use crate::{
    err::Error,
    layout::StyleMetrics,
    scene::{GlyphRun, Path, Rect, Scene, TableGrid, Verb},
    theme::{Palette, Rgba},
};

// Each process scans system fonts at most once. Math outlines need no system lookup.
static FONTS: std::sync::OnceLock<std::sync::Arc<resvg::usvg::fontdb::Database>> =
    std::sync::OnceLock::new();

fn svg_options(src: &[u8]) -> resvg::usvg::Options<'static> {
    let mut options = resvg::usvg::Options::default();
    if memchr::memmem::find(src, b"<text").is_some() {
        options.fontdb = FONTS
            .get_or_init(|| {
                let mut fonts = resvg::usvg::fontdb::Database::new();
                fonts.load_system_fonts();
                std::sync::Arc::new(fonts)
            })
            .clone();
    }
    options
}

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
    if raw_w > MAX_DIM || raw_h > MAX_DIM {
        return Err(Error::ImageTooLarge { w: raw_w, h: raw_h });
    }
    let (cw, ch) = (u32::from(cell_w.max(1)), u32::from(cell_h.max(1)));
    let max_cols = (MAX_DIM / cw).max(1);
    let max_rows = (MAX_DIM / ch).max(1);
    let cols = raw_w.max(1).div_ceil(cw).min(max_cols);
    let rows = raw_h.max(1).div_ceil(ch).min(max_rows);
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

/// Vector path command for building in-memory icon shapes.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub enum PathCmd {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo(f32, f32, f32, f32),
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

/// Warning icon (Lucide `AlertTriangle`, 16x16 grid).
pub const ICON_WARN: &[PathCmd] = &[
    PathCmd::MoveTo(8.0, 2.0),
    PathCmd::LineTo(14.5, 13.5),
    PathCmd::LineTo(1.5, 13.5),
    PathCmd::Close,
    PathCmd::MoveTo(8.0, 5.5),
    PathCmd::LineTo(8.0, 8.8),
    PathCmd::MoveTo(8.0, 10.8),
    PathCmd::LineTo(8.0, 11.0),
];

/// Information icon (Lucide Info, 16x16 grid).
pub const ICON_INFO: &[PathCmd] = &[
    PathCmd::MoveTo(8.0, 1.5),
    PathCmd::CubicTo(4.41, 1.5, 1.5, 4.41, 1.5, 8.0),
    PathCmd::CubicTo(1.5, 11.59, 4.41, 14.5, 8.0, 14.5),
    PathCmd::CubicTo(11.59, 14.5, 14.5, 11.59, 14.5, 8.0),
    PathCmd::CubicTo(14.5, 4.41, 11.59, 1.5, 8.0, 1.5),
    PathCmd::Close,
    PathCmd::MoveTo(8.0, 5.0),
    PathCmd::LineTo(8.0, 5.5),
    PathCmd::MoveTo(8.0, 7.5),
    PathCmd::LineTo(8.0, 11.0),
];

/// Checkmark icon (Lucide Check, 16x16 grid).
pub const ICON_CHECK: &[PathCmd] = &[
    PathCmd::MoveTo(3.0, 8.0),
    PathCmd::LineTo(6.5, 11.5),
    PathCmd::LineTo(13.0, 4.5),
];

/// 1-byte icon identifier.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconId {
    Note = 0,
    Tip = 1,
    Warn = 2,
    Caution = 3,
}

const _: () = assert!(size_of::<IconId>() == 1);

#[inline]
#[must_use]
pub const fn icon_cmds(id: IconId) -> &'static [PathCmd] {
    match id {
        IconId::Note => ICON_INFO,
        IconId::Tip => ICON_CHECK,
        IconId::Warn | IconId::Caution => ICON_WARN,
    }
}

/// Build a `tiny_skia::Path` directly from an in-memory command slice without parsing allocations.
#[inline]
#[must_use]
pub fn build_icon_path(cmds: &[PathCmd]) -> Option<tiny_skia::Path> {
    let mut pb = tiny_skia::PathBuilder::new();
    for cmd in cmds {
        match *cmd {
            PathCmd::MoveTo(x, y) => pb.move_to(x, y),
            PathCmd::LineTo(x, y) => pb.line_to(x, y),
            PathCmd::QuadTo(x1, y1, x, y) => pb.quad_to(x1, y1, x, y),
            PathCmd::CubicTo(x1, y1, x2, y2, x, y) => pb.cubic_to(x1, y1, x2, y2, x, y),
            PathCmd::Close => pb.close(),
        }
    }
    pb.finish()
}

#[inline]
fn make_paint(c: Rgba) -> tiny_skia::Paint<'static> {
    tiny_skia::Paint {
        shader: tiny_skia::Shader::SolidColor(tiny_skia::Color::from_rgba8(
            c.r(),
            c.g(),
            c.b(),
            c.a(),
        )),
        anti_alias: true,
        ..Default::default()
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

fn append_text(text: &str, cfg: TextDraw, svg: &mut String) {
    use std::fmt::Write as _;
    let width = cfg.max_x - cfg.x;
    if text.is_empty() || width <= 0.0 || cfg.font_px <= 0.0 {
        return;
    }
    let family = if cfg.flags & GlyphRun::MONO != 0 {
        "monospace"
    } else {
        "sans-serif"
    };
    let weight = if cfg.flags & GlyphRun::BOLD != 0 {
        "bold"
    } else {
        "normal"
    };
    let style = if cfg.flags & GlyphRun::ITALIC != 0 {
        "italic"
    } else {
        "normal"
    };
    let _ = write!(
        svg,
        "<svg x=\"{}\" y=\"{}\" width=\"{width}\" height=\"{}\"><text y=\"{}\" font-size=\"{}\" font-family=\"{family}\" font-weight=\"{weight}\" font-style=\"{style}\" fill=\"#{:02x}{:02x}{:02x}\" fill-opacity=\"{}\">",
        cfg.x,
        cfg.baseline_y - cfg.font_px,
        cfg.font_px * 1.5,
        cfg.font_px,
        cfg.font_px,
        cfg.color.r(),
        cfg.color.g(),
        cfg.color.b(),
        f32::from(cfg.color.a()) / 255.0
    );
    for ch in text.chars() {
        match ch {
            '&' => svg.push_str("&amp;"),
            '<' => svg.push_str("&lt;"),
            '>' => svg.push_str("&gt;"),
            _ => svg.push(ch),
        }
    }
    svg.push_str("</text></svg>");
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
                ..StyleMetrics::BASE_STROKE
            };
            let border = if r.color == crate::theme::ColorToken::SURFACE {
                make_paint(palette.role_color(crate::theme::SemanticRole::Border))
            } else {
                paint.clone()
            };
            pixmap.stroke_path(&path, &border, &stroke, ts, None);
        }
    }
}

fn draw_scene_grid(
    scene: &Scene,
    grid: &TableGrid,
    palette: &Palette,
    scale: f32,
    metrics: &StyleMetrics,
    pixmap: &mut tiny_skia::PixmapMut<'_>,
    labels: &mut String,
) {
    let ts = tiny_skia::Transform::from_scale(scale, scale);
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
    let border_stroke = metrics.grid_stroke();
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
            }
            let text = scene.cell_text(cell);
            if !text.is_empty() {
                let pad = 4.0 * scale;
                let tx = cx.mul_add(scale, pad);
                let max_x = (cx + col_w).mul_add(scale, -pad);
                let baseline = GRID_ROW_H.mul_add(0.74, cy) * scale;
                let flags = if cell.flags != 0 { GlyphRun::BOLD } else { 0 };
                append_text(
                    text,
                    TextDraw {
                        x: tx,
                        baseline_y: baseline,
                        font_px,
                        max_x,
                        flags,
                        color: palette.color(cell.color),
                    },
                    labels,
                );
            }
            cx += col_w;
        }
    }

    let total_w: f32 = widths.iter().sum();
    let x0 = (grid.x * scale).floor() + 0.5;
    let x_end = ((grid.x + total_w) * scale).floor() + 0.5;
    let rows_f = f32::from(grid.rows);
    let y0 = (grid.y * scale).floor() + 0.5;
    let y_end = (rows_f.mul_add(GRID_ROW_H, grid.y) * scale).floor() + 0.5;

    let mut grid_pb = tiny_skia::PathBuilder::new();
    for r in 0..=rows {
        let r_f = f32::from(u16::try_from(r).unwrap_or(u16::MAX));
        let y = (r_f.mul_add(GRID_ROW_H, grid.y) * scale).floor() + 0.5;
        grid_pb.move_to(x0, y);
        grid_pb.line_to(x_end, y);
    }
    let mut cur_x = grid.x;
    for &col_w in widths {
        let x = (cur_x * scale).floor() + 0.5;
        grid_pb.move_to(x, y0);
        grid_pb.line_to(x, y_end);
        cur_x += col_w;
    }
    grid_pb.move_to(x_end, y0);
    grid_pb.line_to(x_end, y_end);

    if let Some(grid_path) = grid_pb.finish() {
        pixmap.stroke_path(
            &grid_path,
            &border_paint,
            &border_stroke,
            tiny_skia::Transform::identity(),
            None,
        );
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
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..StyleMetrics::BASE_STROKE
        };
        pixmap.stroke_path(&path, &paint, &stroke, ts, None);
    }
}

fn draw_scene_svgs(scene: &Scene, scale: f32, pixmap: &mut tiny_skia::PixmapMut<'_>) {
    if scene.svgs.is_empty() {
        return;
    }
    for emb in &scene.svgs {
        let raw = scene.svg_bytes(emb);
        let Ok(tree) = resvg::usvg::Tree::from_data(raw, &svg_options(raw)) else {
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
    let tree = resvg::usvg::Tree::from_data(svg, &svg_options(svg)).map_err(|_| Error::SvgParse)?;
    let (src_w, src_h) = (tree.size().width(), tree.size().height());
    if !(src_w.is_finite() && src_h.is_finite() && src_w > 0.0 && src_h > 0.0) {
        return Err(Error::SvgParse);
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "max_w <= MAX_DIM (4096) exact in f32"
    )]
    let max_w_f = max_w as f32;
    // Fit both axes rather than clipping the bottom. KGP row diacritics encode
    // at most 297 rows; account for cell snapping before choosing the scale.
    #[expect(
        clippy::cast_precision_loss,
        reason = "bounded pixel dimensions are exact in f32"
    )]
    let max_h = ((MAX_DIM / u32::from(cell_h.max(1))).min(297) * u32::from(cell_h.max(1))) as f32;
    let scale = 1.0_f32.min(max_w_f / src_w).min(max_h / src_h);

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
    rasterize_scene_scaled(
        scene,
        palette,
        cell_w,
        cell_h,
        max_width_px,
        (f32::from(cell_h.max(1)) / 20.0).clamp(1.0, 4.0),
        rgba,
    )
}

/// Rasterize with a format-specific preferred scale, still fitting both axes.
///
/// # Errors
/// Propagates SVG parsing and bounded image allocation errors.
pub fn rasterize_scene_scaled(
    scene: &Scene,
    palette: &Palette,
    cell_w: u16,
    cell_h: u16,
    max_width_px: u16,
    preferred_scale: f32,
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
    // Fit both axes rather than clipping the bottom. KGP row diacritics encode
    // at most 297 rows; account for cell snapping before choosing the scale.
    #[expect(
        clippy::cast_precision_loss,
        reason = "bounded pixel dimensions are exact in f32"
    )]
    let max_h = ((MAX_DIM / u32::from(cell_h.max(1))).min(297) * u32::from(cell_h.max(1))) as f32;
    if !preferred_scale.is_finite() || preferred_scale <= 0.0 {
        return Err(Error::SvgParse);
    }
    let scale = preferred_scale
        .min(4.0)
        .min(max_w_f / src_w)
        .min(max_h / src_h);

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

    let metrics = StyleMetrics::from_cell_geometry(cell_w, cell_h);
    let ts = tiny_skia::Transform::from_scale(scale, scale);
    let mut labels =
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\">");
    draw_scene_rects(scene, palette, ts, &mut pixmap);
    for grid in &scene.grids {
        draw_scene_grid(
            scene,
            grid,
            palette,
            scale,
            &metrics,
            &mut pixmap,
            &mut labels,
        );
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
        append_text(
            scene.text(glyph),
            TextDraw {
                x: glyph.x * scale,
                baseline_y: glyph.y * scale,
                font_px: f32::from(glyph.font_size) * scale,
                max_x: canvas_w - 4.0,
                flags: glyph.flags,
                color: palette.color(glyph.color),
            },
            &mut labels,
        );
    }

    labels.push_str("</svg>");
    if memchr::memmem::find(labels.as_bytes(), b"<text").is_some() {
        let tree = resvg::usvg::Tree::from_data(labels.as_bytes(), &svg_options(labels.as_bytes()))
            .map_err(|_| Error::SvgParse)?;
        resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap);
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

/// Layout metrics computed for quantized cell-snapped rasterization.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasLayout {
    pub scale: f32,
    pub grid_cols: u16,
    pub grid_rows: u16,
    pub snap_w: u32,
    pub snap_h: u32,
    pub off_x: f32,
    pub off_y: f32,
}

/// Compute scale-down layout snapped to terminal character cell dimensions.
///
/// # Errors
/// Returns `Error::SvgParse` if intrinsic dimensions are not finite and positive,
/// or `Error::ImageTooLarge` if snapped bounds exceed `MAX_DIM`.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "bounded positive dimension conversions"
)]
pub fn scale_down_and_snap(
    intrinsic_w: f32,
    intrinsic_h: f32,
    cols: u16,
    rows: u16,
    cell_w: u16,
    cell_h: u16,
) -> Result<CanvasLayout, Error> {
    if !(intrinsic_w.is_finite()
        && intrinsic_h.is_finite()
        && intrinsic_w > 0.0
        && intrinsic_h > 0.0)
    {
        return Err(Error::SvgParse);
    }

    let max_px_w = f32::from(cols) * f32::from(cell_w);
    let max_px_h = f32::from(rows) * f32::from(cell_h);

    let scale_x = max_px_w / intrinsic_w;
    let scale_y = max_px_h / intrinsic_h;
    let scale = 1.0_f32.min(scale_x).min(scale_y);

    let content_w = ((intrinsic_w * scale).ceil() as u32)
        .min(max_px_w as u32)
        .max(1);
    let content_h = ((intrinsic_h * scale).ceil() as u32)
        .min(max_px_h as u32)
        .max(1);

    let grid_cols = content_w.div_ceil(u32::from(cell_w)).max(1) as u16;
    let grid_rows = content_h.div_ceil(u32::from(cell_h)).max(1) as u16;

    let snap_w = u32::from(grid_cols) * u32::from(cell_w);
    let snap_h = u32::from(grid_rows) * u32::from(cell_h);

    if snap_w > MAX_DIM || snap_h > MAX_DIM {
        return Err(Error::ImageTooLarge {
            w: snap_w,
            h: snap_h,
        });
    }

    let off_x = ((snap_w - content_w) / 2) as f32;
    let off_y = ((snap_h - content_h) / 2) as f32;

    Ok(CanvasLayout {
        scale,
        grid_cols,
        grid_rows,
        snap_w,
        snap_h,
        off_x,
        off_y,
    })
}

/// Rasterize SVG into cell-snapped transparent RGBA8 with aspect-ratio preservation.
///
/// # Errors
/// Returns `Error::SvgParse` or `Error::ImageTooLarge`.
pub fn rasterize_svg_snapped(
    svg_data: &[u8],
    cols: u16,
    rows: u16,
    cell_w: u16,
    cell_h: u16,
    rgba_out: &mut Vec<u8>,
) -> Result<(u16, u16), Error> {
    let tree = resvg::usvg::Tree::from_data(svg_data, &svg_options(svg_data))
        .map_err(|_| Error::SvgParse)?;
    let (src_w, src_h) = (tree.size().width(), tree.size().height());

    let layout = scale_down_and_snap(src_w, src_h, cols, rows, cell_w, cell_h)?;
    let byte_len = (layout.snap_w as usize)
        .checked_mul(layout.snap_h as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or(Error::ImageTooLarge {
            w: layout.snap_w,
            h: layout.snap_h,
        })?;

    rgba_out.clear();
    rgba_out.resize(byte_len, 0);

    let mut pixmap = tiny_skia::PixmapMut::from_bytes(rgba_out, layout.snap_w, layout.snap_h)
        .ok_or(Error::SvgParse)?;

    let ts = tiny_skia::Transform::from_scale(layout.scale, layout.scale)
        .post_translate(layout.off_x, layout.off_y);
    resvg::render(&tree, ts, &mut pixmap);

    unpremultiply(rgba_out);

    Ok((layout.grid_cols, layout.grid_rows))
}

/// Fit decoded straight RGBA into the terminal grid, preserving all source rows.
///
/// # Errors
/// Rejects inconsistent dimensions or an image exceeding the raster budget.
pub fn fit_rgba(
    w: u32,
    h: u32,
    cell_w: u16,
    cell_h: u16,
    max_w: u16,
    rgba: &mut Vec<u8>,
) -> Result<(u16, u16), Error> {
    let cw = u32::from(cell_w.max(1));
    let ch = u32::from(cell_h.max(1));
    if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 {
        return Err(Error::PngDecode);
    }
    let bound_w = u32::from(max_w).min(MAX_DIM) / cw * cw;
    let bound_h = (MAX_DIM / ch).min(297) * ch;
    if bound_w == 0 || bound_h == 0 {
        return Err(Error::ImageTooLarge { w, h });
    }
    let mut dw = w.min(bound_w);
    let mut dh = u32::try_from(u64::from(h) * u64::from(dw) / u64::from(w))
        .unwrap_or(bound_h)
        .max(1);
    if dh > bound_h {
        dh = bound_h;
        dw = u32::try_from(u64::from(w) * u64::from(dh) / u64::from(h))
            .unwrap_or(bound_w)
            .max(1);
    }
    let (sw, sh, _, _, bytes) = snap_dims(dw, dh, cell_w, cell_h)?;
    let mut dst = vec![0; bytes];
    for y in 0..dh {
        let sy = u64::from(y) * u64::from(h) / u64::from(dh);
        for x in 0..dw {
            let sx = u64::from(x) * u64::from(w) / u64::from(dw);
            let from =
                usize::try_from((sy * u64::from(w) + sx) * 4).map_err(|_| Error::PngDecode)?;
            let to = (y as usize * usize::from(sw) + x as usize) * 4;
            dst[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
        }
    }
    *rgba = dst;
    Ok((sw, sh))
}
