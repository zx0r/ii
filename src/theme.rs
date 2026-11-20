//! Static palette tables and 1-byte ColorToken definitions.
//! Zero runtime color parsing; O(1) theme swap via pointer dereference.
//! Precomputed const SGR escape sequences: zero runtime formatting.

/// 1-byte opaque color token pointing into theme palette.
/// Inner field is private to guarantee branchless, panicking-free [0..=31] palette indexing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct ColorToken(u8);

impl ColorToken {
    pub const BG_PRIMARY: Self = Self(0);
    pub const BG_SURFACE: Self = Self(1);
    pub const FG_PRIMARY: Self = Self(2);
    pub const FG_MUTED: Self = Self(3);
    pub const GRID_LINE: Self = Self(4);
    pub const BORDER: Self = Self(5);
    pub const ACCENT: Self = Self(6);
    pub const ACCENT_ORANGE: Self = Self(7);
    pub const ACCENT_GREEN: Self = Self(8);
    pub const ACCENT_RED: Self = Self(9);

    #[inline]
    #[must_use]
    pub const fn series(idx: u8) -> Self {
        Self(16 + (idx & 0x0F))
    }

    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        (self.0 & 31) as usize
    }
}

/// Palette index identifier used by format adapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Idx {
    Bg = 0,
    BgSurface = 1,
    Fg = 2,
    FgMuted = 3,
    GridLine = 4,
    Border = 5,
    Accent = 6,
    Heading = 7,
    HeadingSub = 8,
    Error = 9,
}

impl Idx {
    #[inline]
    #[must_use]
    pub const fn to_token(self) -> ColorToken {
        ColorToken(self as u8)
    }
}

/// Hex and SGR entry for formatted outputs.
#[derive(Clone, Copy, Debug)]
pub struct ColorEntry {
    pub hex: &'static str,
    pub sgr: &'static [u8],
    pub rgb: (u8, u8, u8),
}

impl ColorEntry {
    #[inline]
    #[must_use]
    pub const fn hex(self) -> &'static str {
        self.hex
    }

    #[inline]
    #[must_use]
    pub const fn sgr(self) -> &'static [u8] {
        self.sgr
    }
}

/// 32-bit packed RGBA color for tiny-skia (premultiplied LE: [r, g, b, a]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct Rgba(pub u32);

impl Rgba {
    /// Const constructor with compile-time alpha premultiplication.
    #[inline]
    #[must_use]
    pub const fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        if a == 255 {
            Self(((a as u32) << 24) | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32))
        } else if a == 0 {
            Self(0)
        } else {
            let pr = ((r as u32 * a as u32 + 127) / 255) as u8;
            let pg = ((g as u32 * a as u32 + 127) / 255) as u8;
            let pb = ((b as u32 * a as u32 + 127) / 255) as u8;
            Self(((a as u32) << 24) | ((pb as u32) << 16) | ((pg as u32) << 8) | (pr as u32))
        }
    }

    #[inline]
    #[must_use]
    pub const fn r(self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    #[inline]
    #[must_use]
    pub const fn g(self) -> u8 {
        ((self.0 >> 8) & 0xFF) as u8
    }

    #[inline]
    #[must_use]
    pub const fn b(self) -> u8 {
        ((self.0 >> 16) & 0xFF) as u8
    }

    #[inline]
    #[must_use]
    pub const fn a(self) -> u8 {
        ((self.0 >> 24) & 0xFF) as u8
    }

    #[inline]
    #[must_use]
    pub const fn to_array(self) -> [u8; 4] {
        [self.r(), self.g(), self.b(), self.a()]
    }
}

#[derive(Debug)]
pub struct SgrSeq {
    pub buf: [u8; 20],
    pub len: u8,
}

const fn format_u8(mut val: u8, buf: &mut [u8; 20], mut idx: usize) -> usize {
    if val >= 100 {
        buf[idx] = b'0' + (val / 100);
        idx += 1;
        val %= 100;
        buf[idx] = b'0' + (val / 10);
        idx += 1;
        buf[idx] = b'0' + (val % 10);
        idx += 1;
    } else if val >= 10 {
        buf[idx] = b'0' + (val / 10);
        idx += 1;
        buf[idx] = b'0' + (val % 10);
        idx += 1;
    } else {
        buf[idx] = b'0' + val;
        idx += 1;
    }
    idx
}

const fn make_sgr(color: Rgba) -> SgrSeq {
    let mut buf = [0u8; 20];
    buf[0] = 0x1b;
    buf[1] = b'[';
    buf[2] = b'3';
    buf[3] = b'8';
    buf[4] = b';';
    buf[5] = b'2';
    buf[6] = b';';
    let idx = format_u8(color.r(), &mut buf, 7);
    buf[idx] = b';';
    let idx2 = format_u8(color.g(), &mut buf, idx + 1);
    buf[idx2] = b';';
    let idx3 = format_u8(color.b(), &mut buf, idx2 + 1);
    buf[idx3] = b'm';
    SgrSeq {
        buf,
        len: (idx3 + 1) as u8,
    }
}

const fn compute_palette_hash(colors: &[Rgba; 32]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    let mut i = 0;
    while i < 32 {
        h ^= colors[i].0;
        h = h.wrapping_mul(0x0100_0193);
        i += 1;
    }
    h
}

/// Theme palette table aligned to 64 bytes (exactly 3 cachelines = 192 bytes).
#[derive(Clone, Copy, Debug)]
#[repr(C, align(64))]
pub struct Palette {
    pub colors: [Rgba; 32],
    pub theme_hash: u32,
    pub is_dark: bool,
    pub _pad: [u8; 3],
}

const _: () = assert!(size_of::<Palette>() == 192);
const _: () = assert!(align_of::<Palette>() == 64);

impl Palette {
    #[inline]
    #[must_use]
    pub fn hex_str(&self, idx: Idx) -> &'static str {
        DARK_HEX[idx as usize]
    }

    #[inline]
    #[must_use]
    pub fn series_hex_str(&self, idx: usize) -> &'static str {
        SERIES_HEX[idx & 0x0F]
    }

    #[inline]
    #[must_use]
    pub fn get(&self, idx: Idx) -> ColorEntry {
        let i = idx as usize;
        let c = self.colors[i];
        ColorEntry {
            hex: DARK_HEX[i],
            sgr: sgr(ColorToken(i as u8), self.is_dark),
            rgb: (c.r(), c.g(), c.b()),
        }
    }
}

const DARK_HEX: [&str; 16] = [
    "#18181b", "#27272a", "#f4f4f5", "#71717a", "#3f3f46", "#52525b",
    "#38bdf8", "#fb923c", "#4ade80", "#f87171", "#000000", "#000000",
    "#000000", "#000000", "#000000", "#000000",
];

const SERIES_HEX: [&str; 16] = [
    "#38bdf8", "#fb923c", "#4ade80", "#c084fc", "#f472b6", "#2dd4bf",
    "#facc15", "#a78bfa", "#fb7185", "#60a5fa", "#34d399", "#f87171",
    "#818cf8", "#fbbf24", "#e879f9", "#94a3b8",
];

const DARK_COLORS: [Rgba; 32] = [
    Rgba::from_rgba(0x18, 0x18, 0x1B, 0xFF), Rgba::from_rgba(0x27, 0x27, 0x2A, 0xFF), Rgba::from_rgba(0xF4, 0xF4, 0xF5, 0xFF), Rgba::from_rgba(0x71, 0x71, 0x7A, 0xFF),
    Rgba::from_rgba(0x3F, 0x3F, 0x46, 0x80), Rgba::from_rgba(0x52, 0x52, 0x5B, 0xFF), Rgba::from_rgba(0x38, 0xBD, 0xF8, 0xFF), Rgba::from_rgba(0xFB, 0x92, 0x3C, 0xFF),
    Rgba::from_rgba(0x4A, 0xDE, 0x80, 0xFF), Rgba::from_rgba(0xF8, 0x71, 0x71, 0xFF), Rgba::from_rgba(0, 0, 0, 0), Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0, 0, 0, 0), Rgba::from_rgba(0, 0, 0, 0), Rgba::from_rgba(0, 0, 0, 0), Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0x38, 0xBD, 0xF8, 0xFF), Rgba::from_rgba(0xFB, 0x92, 0x3C, 0xFF), Rgba::from_rgba(0x4A, 0xDE, 0x80, 0xFF), Rgba::from_rgba(0xC0, 0x84, 0xFC, 0xFF),
    Rgba::from_rgba(0xF4, 0xF4, 0xB6, 0xFF), Rgba::from_rgba(0x2D, 0xD4, 0xBF, 0xFF), Rgba::from_rgba(0xFA, 0xCC, 0x15, 0xFF), Rgba::from_rgba(0xA7, 0x8B, 0xFA, 0xFF),
    Rgba::from_rgba(0xFB, 0x71, 0x85, 0xFF), Rgba::from_rgba(0x60, 0xA5, 0xFA, 0xFF), Rgba::from_rgba(0x34, 0xD3, 0x99, 0xFF), Rgba::from_rgba(0xF8, 0x71, 0x71, 0xFF),
    Rgba::from_rgba(0x81, 0x8C, 0xF8, 0xFF), Rgba::from_rgba(0xFB, 0xBF, 0x24, 0xFF), Rgba::from_rgba(0xE8, 0x79, 0xF9, 0xFF), Rgba::from_rgba(0x94, 0xA3, 0xB8, 0xFF),
];

const LIGHT_COLORS: [Rgba; 32] = [
    Rgba::from_rgba(0xFF, 0xFF, 0xFF, 0xFF), Rgba::from_rgba(0xF4, 0xF4, 0xF5, 0xFF), Rgba::from_rgba(0x18, 0x18, 0x1B, 0xFF), Rgba::from_rgba(0xA1, 0xA1, 0xAA, 0xFF),
    Rgba::from_rgba(0xE4, 0xE4, 0xE7, 0x80), Rgba::from_rgba(0xD4, 0xD4, 0xD8, 0xFF), Rgba::from_rgba(0x02, 0x84, 0xC7, 0xFF), Rgba::from_rgba(0xEA, 0x58, 0x0C, 0xFF),
    Rgba::from_rgba(0x16, 0xA3, 0x4A, 0xFF), Rgba::from_rgba(0xDC, 0x26, 0x26, 0xFF), Rgba::from_rgba(0, 0, 0, 0), Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0, 0, 0, 0), Rgba::from_rgba(0, 0, 0, 0), Rgba::from_rgba(0, 0, 0, 0), Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0x02, 0x84, 0xC7, 0xFF), Rgba::from_rgba(0xEA, 0x58, 0x0C, 0xFF), Rgba::from_rgba(0x16, 0xA3, 0x4A, 0xFF), Rgba::from_rgba(0x93, 0x33, 0xEA, 0xFF),
    Rgba::from_rgba(0xDB, 0x27, 0x77, 0xFF), Rgba::from_rgba(0x0D, 0x94, 0x88, 0xFF), Rgba::from_rgba(0xCA, 0x8A, 0x04, 0xFF), Rgba::from_rgba(0x7C, 0x3A, 0xED, 0xFF),
    Rgba::from_rgba(0xE1, 0x1D, 0x48, 0xFF), Rgba::from_rgba(0x25, 0x63, 0xEB, 0xFF), Rgba::from_rgba(0x05, 0x96, 0x69, 0xFF), Rgba::from_rgba(0xDC, 0x26, 0x26, 0xFF),
    Rgba::from_rgba(0x4F, 0x46, 0xE5, 0xFF), Rgba::from_rgba(0xD9, 0xF7, 0x06, 0xFF), Rgba::from_rgba(0xC0, 0x26, 0xD3, 0xFF), Rgba::from_rgba(0x64, 0x74, 0x8B, 0xFF),
];

pub static DARK: Palette = Palette { colors: DARK_COLORS, theme_hash: compute_palette_hash(&DARK_COLORS), is_dark: true, _pad: [0; 3] };
pub static LIGHT: Palette = Palette { colors: LIGHT_COLORS, theme_hash: compute_palette_hash(&LIGHT_COLORS), is_dark: false, _pad: [0; 3] };
pub static DARK_PALETTE: Palette = DARK;
pub static LIGHT_PALETTE: Palette = LIGHT;

const fn build_sgr_table(colors: &[Rgba; 32]) -> [SgrSeq; 32] {
    let mut table = [const { SgrSeq { buf: [0; 20], len: 0 } }; 32];
    let mut i = 0;
    while i < 32 {
        table[i] = make_sgr(colors[i]);
        i += 1;
    }
    table
}

pub static DARK_SGR: [SgrSeq; 32] = build_sgr_table(&DARK_COLORS);
pub static LIGHT_SGR: [SgrSeq; 32] = build_sgr_table(&LIGHT_COLORS);
pub static SGR_RESET: &[u8] = b"\x1b[0m";

#[inline]
#[must_use]
pub fn resolve_color(token: ColorToken, dark: bool) -> Rgba {
    let pal = if dark { &DARK_PALETTE } else { &LIGHT_PALETTE };
    pal.colors[token.index()]
}

#[inline]
#[must_use]
pub fn sgr(token: ColorToken, dark: bool) -> &'static [u8] {
    let table = if dark { &DARK_SGR } else { &LIGHT_SGR };
    let entry = &table[token.index()];
    &entry.buf[..entry.len as usize]
}

#[inline]
#[must_use]
pub const fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
