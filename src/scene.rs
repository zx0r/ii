//! Hourglass Scene IR: flat `SoA` primitives (`GlyphRun`, `Path`, `Rect`, `TableGrid`).

use crate::theme::ColorToken;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
#[expect(clippy::pub_underscore_fields, reason = "explicit C-ABI padding")]
pub struct GlyphRun {
    pub x: f32,
    pub y: f32,
    pub text_off: u32,
    pub text_len: u16,
    pub font_size: u16,
    pub color: ColorToken,
    pub flags: u8,
    pub _pad: [u8; 2],
}
const _: () = assert!(size_of::<GlyphRun>() == 20);

impl GlyphRun {
    pub const BOLD: u8 = 1 << 0;
    pub const ITALIC: u8 = 1 << 1;
    pub const MONO: u8 = 1 << 2;
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: ColorToken,
    pub radius: u8,
    pub stroke_width: u8,
    pub flags: u8,
}
const _: () = assert!(size_of::<Rect>() == 20);

impl Rect {
    pub const FILL: u8 = 1 << 0;
    pub const STROKE: u8 = 1 << 1;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Verb {
    MoveTo = 0,
    LineTo = 1,
    QuadTo = 2,
    CubicTo = 3,
    Close = 4,
}
const _: () = assert!(size_of::<Verb>() == 1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
#[expect(clippy::pub_underscore_fields, reason = "explicit C-ABI padding")]
pub struct Path {
    pub pt_start: u32,
    pub pt_count: u32,
    pub verb_start: u32,
    pub verb_count: u32,
    pub color: ColorToken,
    pub stroke_width: u8,
    pub flags: u8,
    pub _pad: u8,
}
const _: () = assert!(size_of::<Path>() == 20);

impl Path {
    pub const FILL: u8 = 1 << 0;
    pub const STROKE: u8 = 1 << 1;
    pub const CLOSED: u8 = 1 << 2;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct CellSpan {
    pub text_off: u32,
    pub text_len: u16,
    pub col_span: u8,
    pub row_span: u8,
    pub color: ColorToken,
    pub bg: ColorToken,
    pub align: u8,
    pub flags: u8,
}
const _: () = assert!(size_of::<CellSpan>() == 12);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
#[expect(clippy::pub_underscore_fields, reason = "explicit C-ABI padding")]
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

#[derive(Debug, Default)]
pub struct Scene {
    pub rects: Vec<Rect>,
    pub paths: Vec<Path>,
    pub glyphs: Vec<GlyphRun>,
    pub grids: Vec<TableGrid>,
    pub points: Vec<[f32; 2]>,
    pub verbs: Vec<Verb>,
    pub text_pool: Vec<u8>,
    pub cells: Vec<CellSpan>,
    pub col_widths: Vec<f32>,
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
            points: Vec::with_capacity(prims.saturating_mul(4)),
            verbs: Vec::with_capacity(prims.saturating_mul(2)),
            text_pool: Vec::with_capacity(text_bytes),
            cells: Vec::with_capacity(128),
            col_widths: Vec::with_capacity(32),
            width: 0.0,
            height: 0.0,
        }
    }

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
    pub fn intern_text(&mut self, text: &str) -> Option<(u32, u16)> {
        let clamped = floor_utf8(text, usize::from(u16::MAX));
        let off = u32::try_from(self.text_pool.len()).ok()?;
        let len = u16::try_from(clamped.len()).ok()?;
        off.checked_add(u32::from(len))?;
        self.text_pool.extend_from_slice(clamped.as_bytes());
        Some((off, len))
    }

    #[inline]
    pub fn push_text(&mut self, x: f32, y: f32, text: &str, color: ColorToken, font_size: u16) {
        self.push_text_styled(x, y, text, color, font_size, 0);
    }

    #[inline]
    pub fn push_text_styled(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        color: ColorToken,
        font_size: u16,
        flags: u8,
    ) {
        let Some((text_off, text_len)) = self.intern_text(text) else {
            return;
        };
        self.glyphs.push(GlyphRun {
            x,
            y,
            text_off,
            text_len,
            font_size,
            color,
            flags,
            _pad: [0; 2],
        });
    }

    #[inline]
    #[must_use]
    pub fn slice_text(&self, off: u32, len: u16) -> &str {
        let start = off as usize;
        let Some(end) = start.checked_add(usize::from(len)) else {
            return "";
        };
        self.text_pool
            .get(start..end)
            .and_then(|b| core::str::from_utf8(b).ok())
            .unwrap_or("")
    }

    #[inline]
    #[must_use]
    pub fn text(&self, glyph: &GlyphRun) -> &str {
        self.slice_text(glyph.text_off, glyph.text_len)
    }

    #[inline]
    #[must_use]
    pub fn cell_text(&self, cell: &CellSpan) -> &str {
        self.slice_text(cell.text_off, cell.text_len)
    }

    #[inline]
    pub fn push_series_line(&mut self, pts: &[[f32; 2]], color: ColorToken, stroke_width: u8) {
        if pts.is_empty() {
            return;
        }
        let (Ok(pt_start), Ok(verb_start), Ok(count)) = (
            u32::try_from(self.points.len()),
            u32::try_from(self.verbs.len()),
            u32::try_from(pts.len()),
        ) else {
            return;
        };
        if pt_start.checked_add(count).is_none() || verb_start.checked_add(count).is_none() {
            return;
        }
        self.points.extend_from_slice(pts);
        self.verbs.reserve(pts.len());
        self.verbs.push(Verb::MoveTo);
        self.verbs
            .extend(core::iter::repeat_n(Verb::LineTo, pts.len() - 1));
        self.paths.push(Path {
            pt_start,
            pt_count: count,
            verb_start,
            verb_count: count,
            color,
            stroke_width,
            flags: Path::STROKE,
            _pad: 0,
        });
    }
}

#[inline]
fn floor_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or("")
}

#[derive(Debug)]
pub struct RenderScratch {
    pub scene: Scene,
    pub rgba_buf: Vec<u8>,
    pub out_buf: Vec<u8>,
}

impl RenderScratch {
    pub const DEFAULT_RGBA_CAP: usize = 4 * 1024 * 1024;
    pub const DEFAULT_OUT_CAP: usize = 1024 * 1024;
    pub const MAX_RETAINED_RGBA: usize = 16 * 1024 * 1024;
    pub const MAX_RETAINED_OUT: usize = 4 * 1024 * 1024;

    #[must_use]
    pub fn new() -> Self {
        Self {
            scene: Scene::with_capacity(1024, 64 * 1024),
            rgba_buf: Vec::with_capacity(Self::DEFAULT_RGBA_CAP),
            out_buf: Vec::with_capacity(Self::DEFAULT_OUT_CAP),
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.scene.clear();
        self.rgba_buf.clear();
        self.out_buf.clear();
        if self.rgba_buf.capacity() > Self::MAX_RETAINED_RGBA {
            self.rgba_buf.shrink_to(Self::DEFAULT_RGBA_CAP);
        }
        if self.out_buf.capacity() > Self::MAX_RETAINED_OUT {
            self.out_buf.shrink_to(Self::DEFAULT_OUT_CAP);
        }
    }
}

impl Default for RenderScratch {
    fn default() -> Self {
        Self::new()
    }
}
