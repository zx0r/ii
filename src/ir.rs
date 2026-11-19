//! Hourglass IR: 16 B packed `Span`, 4-bit `Kind`, and frame-local `Scratch` arena.

use crate::ipynb::Cell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    MathDisplay = 0,
    MathInline = 1,
    Mermaid = 2,
    RawSvg = 3,
    VegaLite = 4,
    Plotly = 5,
    CsvTsv = 6,
    Dot = 7,
    Quarto = 8,
    QmdCallout = 9,
    NotebookOut = 10,
    PipeTable = 11,
    Chem = 12,
    TypstMath = 13,
}

impl Kind {
    #[inline]
    #[must_use]
    pub const fn from_u4(v: u8) -> Self {
        const TABLE: [Kind; 16] = [
            Kind::MathDisplay,
            Kind::MathInline,
            Kind::Mermaid,
            Kind::RawSvg,
            Kind::VegaLite,
            Kind::Plotly,
            Kind::CsvTsv,
            Kind::Dot,
            Kind::Quarto,
            Kind::QmdCallout,
            Kind::NotebookOut,
            Kind::PipeTable,
            Kind::Chem,
            Kind::TypstMath,
            Kind::MathDisplay,
            Kind::MathDisplay,
        ];
        TABLE[(v & 0x0F) as usize]
    }
}

/// 16-byte lexer token. 4 per 64 B x86 cacheline, 8 per 128 B aarch64.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Span {
    pub outer_start: u32,
    pub outer_end: u32,
    pub payload_start: u32,
    pub payload_end_and_kind: u32,
}

const _: () = assert!(size_of::<Span>() == 16);
const _: () = assert!(align_of::<Span>() == 4);

pub const OFFSET_MASK: u32 = 0x0FFF_FFFF;
pub const KIND_SHIFT: u32 = 28;

impl Span {
    #[inline]
    #[must_use]
    pub const fn new(
        outer_start: u32,
        outer_end: u32,
        payload_start: u32,
        payload_end: u32,
        kind: Kind,
    ) -> Self {
        Self {
            outer_start,
            outer_end,
            payload_start,
            payload_end_and_kind: (payload_end & OFFSET_MASK) | ((kind as u32) << KIND_SHIFT),
        }
    }

    #[inline]
    #[must_use]
    pub const fn payload_end(self) -> u32 {
        self.payload_end_and_kind & OFFSET_MASK
    }

    #[inline]
    #[must_use]
    pub const fn kind(self) -> Kind {
        Kind::from_u4((self.payload_end_and_kind >> KIND_SHIFT) as u8)
    }

    #[inline]
    #[must_use]
    pub fn payload<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        src.get(self.payload_start as usize..self.payload_end() as usize)
            .unwrap_or(&[])
    }

    #[inline]
    #[must_use]
    pub fn outer<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        src.get(self.outer_start as usize..self.outer_end as usize)
            .unwrap_or(&[])
    }
}

/// Disjoint frame-local working arena. Zero steady-state heap allocation.
pub struct Scratch {
    pub spans: Vec<Span>,
    pub apc_buf: Vec<u8>,
    pub text_buf: Vec<u8>,
    pub svg_buf: String,
    pub aux_buf: Vec<u8>,
    pub rgba_buf: Vec<u8>,
    pub lz4_buf: Vec<u8>,
    pub b64_buf: Vec<u8>,
    pub line_buf: Vec<u8>,
    pub cells: Vec<Cell>,
}

const RGBA_HWM: usize = 4 << 20;
const TEXT_HWM: usize = 1 << 20;
const RGBA_BASE: usize = 1 << 19;
const TEXT_BASE: usize = 1 << 16;

impl Scratch {
    #[must_use]
    pub fn new() -> Self {
        Self {
            spans: Vec::with_capacity(256),
            apc_buf: Vec::with_capacity(RGBA_BASE),
            text_buf: Vec::with_capacity(TEXT_BASE),
            svg_buf: String::with_capacity(TEXT_BASE),
            aux_buf: Vec::with_capacity(TEXT_BASE),
            rgba_buf: Vec::with_capacity(RGBA_BASE),
            lz4_buf: Vec::with_capacity(TEXT_BASE),
            b64_buf: Vec::with_capacity(1 << 12),
            line_buf: Vec::with_capacity(1 << 14),
            cells: Vec::with_capacity(64),
        }
    }

    /// Reset per-frame cursors while preserving capacity. `line_buf` persists across stream frames.
    #[inline]
    pub fn reset_frame(&mut self) {
        self.spans.clear();
        self.apc_buf.clear();
        self.text_buf.clear();
        self.svg_buf.clear();
        self.aux_buf.clear();
        self.rgba_buf.clear();
        self.lz4_buf.clear();
        self.b64_buf.clear();
        self.cells.clear();
    }

    /// Reclaim bloated buffers in-place without double-allocation peak spikes.
    pub fn shrink_if_bloated(&mut self) {
        if self.rgba_buf.capacity() > RGBA_HWM {
            self.rgba_buf.clear();
            self.rgba_buf.shrink_to(RGBA_BASE);
        }
        if self.svg_buf.capacity() > TEXT_HWM {
            self.svg_buf.clear();
            self.svg_buf.shrink_to(TEXT_BASE);
        }
        if self.aux_buf.capacity() > TEXT_HWM {
            self.aux_buf.clear();
            self.aux_buf.shrink_to(TEXT_BASE);
        }
    }
}

impl Default for Scratch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_pack_unpack_all_kinds() {
        let kinds = [
            Kind::MathDisplay,
            Kind::MathInline,
            Kind::Mermaid,
            Kind::RawSvg,
            Kind::VegaLite,
            Kind::Plotly,
            Kind::CsvTsv,
            Kind::Dot,
            Kind::Quarto,
            Kind::QmdCallout,
            Kind::NotebookOut,
            Kind::PipeTable,
            Kind::Chem,
            Kind::TypstMath,
        ];
        let src = b"$$x^2 + y^2 = z^2$$";
        for k in kinds {
            let s = Span::new(0, 19, 2, 17, k);
            assert_eq!(s.kind(), k);
            assert_eq!(s.payload_end(), 17);
            assert_eq!(s.payload(src), b"x^2 + y^2 = z^2");
            assert_eq!(s.outer(src), src);
        }
        assert_eq!(Kind::from_u4(14), Kind::MathDisplay);
        assert_eq!(Kind::from_u4(15), Kind::MathDisplay);
    }

    #[test]
    fn scratch_reset_and_shrink_in_place() {
        let mut sc = Scratch::new();
        let cap_before = sc.spans.capacity();
        sc.spans.push(Span::new(0, 4, 1, 3, Kind::MathInline));
        sc.line_buf.extend_from_slice(b"partial");
        sc.reset_frame();
        assert_eq!(sc.spans.len(), 0);
        assert_eq!(sc.spans.capacity(), cap_before);
        assert_eq!(sc.line_buf, b"partial");

        sc.rgba_buf.reserve(RGBA_HWM + 1024);
        sc.svg_buf.reserve(TEXT_HWM + 1024);
        sc.aux_buf.reserve(TEXT_HWM + 1024);
        sc.shrink_if_bloated();
        assert!(sc.rgba_buf.capacity() <= RGBA_HWM);
        assert!(sc.svg_buf.capacity() <= TEXT_HWM);
        assert!(sc.aux_buf.capacity() <= TEXT_HWM);
    }
}
