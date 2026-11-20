//! Hourglass Scene IR: Flat SoA primitives (GlyphRun, Path, Rect, TableGrid).
//! Target compile destination for ALL ingest parsers. Zero frontend pixel awareness.

use crate::theme::ColorToken;

/// Compact 20-byte packed glyph/text run.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct GlyphRun {
    pub x: f32,
    pub y: f32,
    pub text_off: u32,
    pub text_len: u16,
    pub font_size: u16,
    pub color: ColorToken,
    pub flags: u8, // bit 0: bold, 1: italic, 2: mono
    pub _pad: [u8; 2],
}
const _: () = assert!(size_of::<GlyphRun>() == 20);

/// Compact 20-byte rectangle primitive.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: ColorToken,
    pub radius: u8,
    pub stroke_width: u8, // 0 = fill, >0 = stroke
    pub flags: u8,        // bit 0: fill, bit 1: stroke
}
const _: () = assert!(size_of::<Rect>() == 20);

/// Vector path verb descriptor (MoveTo, LineTo, QuadTo, CubicTo, Close).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Verb {
    MoveTo = 0,
    LineTo = 1,
    QuadTo = 2,
    CubicTo = 3,
    Close = 4,
}

/// Compact vector path reference pointing into flat point/verb pools.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Path {
    pub pt_start: u32,
    pub pt_count: u32,
    pub verb_start: u32,
    pub verb_count: u32,
    pub color: ColorToken,
    pub stroke_width: u8,
    pub flags: u8, // bit 0: fill, bit 1: stroke, bit 2: closed
    pub _pad: u8,
}
const _: () = assert!(size_of::<Path>() == 20);

/// Cell span for structured tables and matrices.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct CellSpan {
    pub text_off: u32,
    pub text_len: u16,
    pub col_span: u8,
    pub row_span: u8,
    pub color: ColorToken,
    pub bg: ColorToken,
    pub align: u8, // 0: left, 1: center, 2: right
    pub flags: u8,
}
const _: () = assert!(size_of::<CellSpan>() == 12);

/// TableGrid primitive for CSV, TSV, Parquet, and Markdown pipe tables.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct TableGrid {
    pub x: f32,
    pub y: f32,
    pub cell_start: u32,
    pub width_start: u32,
    pub rows: u16,
    pub cols: u16,
    pub border_color: ColorToken,
    pub _pad: [u8; 3],
}
const _: () = assert!(size_of::<TableGrid>() == 24);

/// Flat SoA Scene Arena: zero steady-state allocation.
#[derive(Debug, Default)]
pub struct Scene {
    pub rects: Vec<Rect>,
    pub paths: Vec<Path>,
    pub glyphs: Vec<GlyphRun>,
    pub grids: Vec<TableGrid>,
    // Shared flat buffers
    pub points: Vec<[f32; 2]>,
    pub verbs: Vec<u8>,
    pub text_pool: Vec<u8>,
    pub cells: Vec<CellSpan>,
    pub col_widths: Vec<f32>,
    // Scene bounds
    pub width: f32,
    pub height: f32,
}

impl Scene {
    #[must_use]
    pub fn with_capacity(prims: usize, text_bytes: usize) -> Self {
        Self {
            rects: Vec::with_capacity(prims),
            paths: Vec::with_capacity(prims),
            glyphs: Vec::with_capacity(prims),
            grids: Vec::with_capacity(16),
            points: Vec::with_capacity(prims * 4),
            verbs: Vec::with_capacity(prims * 2),
            text_pool: Vec::with_capacity(text_bytes),
            cells: Vec::with_capacity(128),
            col_widths: Vec::with_capacity(32),
            width: 0.0,
            height: 0.0,
        }
    }

    /// O(1) reset of all cursors while preserving underlying allocations.
    #[inline]
    pub fn clear(&mut self) {
        self.rects.clear();
        self.paths.clear();
        self.glyphs.clear();
        self.grids.clear();
        self.points.clear();
        self.verbs.clear();
        self.text_pool.clear();
        self.cells.clear();
        self.col_widths.clear();
        self.width = 0.0;
        self.height = 0.0;
    }

    #[inline]
    pub fn push_rect(&mut self, rect: Rect) {
        self.rects.push(rect);
    }

    #[inline]
    pub fn push_text(&mut self, x: f32, y: f32, text: &str, color: ColorToken, font_size: u16) {
        let text_off = self.text_pool.len() as u32;
        let text_len = text.len() as u16;
        self.text_pool.extend_from_slice(text.as_bytes());
        self.glyphs.push(GlyphRun {
            x,
            y,
            text_off,
            text_len,
            font_size,
            color,
            flags: 0,
            _pad: [0; 2],
        });
    }

    #[inline]
    pub fn text(&self, glyph: &GlyphRun) -> &str {
        core::str::from_utf8(self.text_pool.get(glyph.text_off as usize..glyph.text_off as usize + glyph.text_len as usize).unwrap_or(&[])).unwrap_or("")
    }

    #[inline]
    pub fn cell_text(&self, cell: &CellSpan) -> &str {
        core::str::from_utf8(self.text_pool.get(cell.text_off as usize..cell.text_off as usize + cell.text_len as usize).unwrap_or(&[])).unwrap_or("")
    }

    #[inline]
    pub fn push_series_line(&mut self, pts: &[[f32; 2]], color: ColorToken, stroke_width: u8) {
        if pts.is_empty() { return; }
        let (pt_start, pt_count) = (self.points.len() as u32, pts.len() as u32);
        self.points.extend_from_slice(pts);
        let verb_start = self.verbs.len() as u32;
        self.verbs.push(Verb::MoveTo as u8);
        self.verbs.resize(self.verbs.len() + pts.len() - 1, Verb::LineTo as u8);
        self.paths.push(Path {
            pt_start, pt_count, verb_start, verb_count: pts.len() as u32,
            color, stroke_width, flags: 2, _pad: 0,
        });
    }
}

/// Scratchpad frame context passed down to all pipelines.
#[derive(Debug)]
pub struct RenderScratch {
    pub scene: Scene,
    pub rgba_buf: Vec<u8>,
    pub out_buf: Vec<u8>,
}

impl RenderScratch {
    #[must_use]
    pub fn new() -> Self {
        Self {
            scene: Scene::with_capacity(1024, 64 * 1024),
            rgba_buf: Vec::with_capacity(4 * 1024 * 1024), // 4 MB default frame buffer
            out_buf: Vec::with_capacity(1024 * 1024),
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.scene.clear();
        self.rgba_buf.clear();
        self.out_buf.clear();
    }
}

impl Default for RenderScratch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scene_clear_and_reuse() {
        let mut scene = Scene::with_capacity(16, 64);
        scene.push_rect(Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 50.0,
            color: ColorToken::BG_PRIMARY,
            radius: 0,
            stroke_width: 0,
            flags: 1,
        });
        scene.push_text(10.0, 10.0, "hello", ColorToken::FG_PRIMARY, 14);
        assert_eq!(scene.rects.len(), 1);
        assert_eq!(scene.glyphs.len(), 1);
        assert_eq!(scene.text(&scene.glyphs[0]), "hello");

        scene.clear();
        assert_eq!(scene.rects.len(), 0);
        assert_eq!(scene.glyphs.len(), 0);
        assert_eq!(scene.text_pool.len(), 0);
    }
}
