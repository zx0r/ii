//! Static palette tables, 1-byte `ColorToken`, and compile-time SGR escape sequences.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct ColorToken(u8);

const _: () = assert!(size_of::<ColorToken>() == 1);

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

/// 32-bit packed straight RGBA color in LE byte order: `[r, g, b, a]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct Rgba(pub u32);

const _: () = assert!(size_of::<Rgba>() == 4);

impl Rgba {
    #[inline]
    #[must_use]
    pub const fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self(u32::from_le_bytes([r, g, b, a]))
    }

    #[inline]
    #[must_use]
    pub const fn r(self) -> u8 {
        self.0.to_le_bytes()[0]
    }

    #[inline]
    #[must_use]
    pub const fn g(self) -> u8 {
        self.0.to_le_bytes()[1]
    }

    #[inline]
    #[must_use]
    pub const fn b(self) -> u8 {
        self.0.to_le_bytes()[2]
    }

    #[inline]
    #[must_use]
    pub const fn a(self) -> u8 {
        self.0.to_le_bytes()[3]
    }

    #[inline]
    #[must_use]
    pub const fn to_array(self) -> [u8; 4] {
        self.0.to_le_bytes()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SgrSeq {
    pub buf: [u8; 20],
    pub len: u8,
}

const fn write_u8(val: u8, buf: &mut [u8; 20], mut idx: usize) -> usize {
    if val >= 100 {
        buf[idx] = b'0' + val / 100;
        idx += 1;
        buf[idx] = b'0' + (val / 10) % 10;
        idx += 1;
    } else if val >= 10 {
        buf[idx] = b'0' + val / 10;
        idx += 1;
    }
    buf[idx] = b'0' + val % 10;
    idx + 1
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
    let i1 = write_u8(color.r(), &mut buf, 7);
    buf[i1] = b';';
    let i2 = write_u8(color.g(), &mut buf, i1 + 1);
    buf[i2] = b';';
    let i3 = write_u8(color.b(), &mut buf, i2 + 1);
    buf[i3] = b'm';
    #[expect(clippy::cast_possible_truncation, reason = "SGR sequence <= 19 bytes")]
    let len = (i3 + 1) as u8;
    SgrSeq { buf, len }
}

const fn hash_palette(colors: &[Rgba; 32]) -> u32 {
    let mut h = 0x811c_9dc5_u32;
    let mut i = 0;
    while i < 32 {
        h = (h ^ colors[i].0).wrapping_mul(0x0100_0193);
        i += 1;
    }
    h
}

/// 64-byte aligned palette table (3 cache lines = 192 B).
#[derive(Clone, Copy, Debug)]
#[repr(C, align(64))]
pub struct Palette {
    pub colors: [Rgba; 32],
    pub theme_hash: u32,
    pub is_dark: bool,
    _pad: [u8; 3],
}

const _: () = assert!(size_of::<Palette>() == 192);
const _: () = assert!(align_of::<Palette>() == 64);

impl Palette {
    #[inline]
    #[must_use]
    pub const fn color(&self, token: ColorToken) -> Rgba {
        self.colors[token.index()]
    }

    #[inline]
    #[must_use]
    pub const fn hex_str(&self, idx: Idx) -> &'static str {
        if self.is_dark {
            DARK_HEX[idx as usize]
        } else {
            LIGHT_HEX[idx as usize]
        }
    }

    #[inline]
    #[must_use]
    pub const fn series_hex_str(&self, idx: usize) -> &'static str {
        if self.is_dark {
            DARK_SERIES_HEX[idx & 15]
        } else {
            LIGHT_SERIES_HEX[idx & 15]
        }
    }

    #[inline]
    #[must_use]
    pub fn get(&self, idx: Idx) -> ColorEntry {
        let c = self.colors[idx as usize];
        ColorEntry {
            hex: self.hex_str(idx),
            sgr: sgr(idx.to_token(), self.is_dark),
            rgb: (c.r(), c.g(), c.b()),
        }
    }
}

#[rustfmt::skip]
const DARK_HEX: [&str; 10] = [
    "#18181b", "#27272a", "#f4f4f5", "#71717a", "#3f3f46",
    "#52525b", "#38bdf8", "#fb923c", "#4ade80", "#f87171",
];
#[rustfmt::skip]
const LIGHT_HEX: [&str; 10] = [
    "#ffffff", "#f4f4f5", "#18181b", "#a1a1aa", "#e4e4e7",
    "#d4d4d8", "#0284c7", "#ea580c", "#16a34a", "#dc2626",
];
#[rustfmt::skip]
const DARK_SERIES_HEX: [&str; 16] = [
    "#38bdf8", "#fb923c", "#4ade80", "#c084fc", "#f472b6", "#2dd4bf", "#facc15", "#a78bfa",
    "#fb7185", "#60a5fa", "#34d399", "#f87171", "#818cf8", "#fbbf24", "#e879f9", "#94a3b8",
];
#[rustfmt::skip]
const LIGHT_SERIES_HEX: [&str; 16] = [
    "#0284c7", "#ea580c", "#16a34a", "#9333ea", "#db2777", "#0d9488", "#ca8a04", "#7c3aed",
    "#e11d48", "#2563eb", "#059669", "#dc2626", "#4f46e5", "#d97706", "#c026d3", "#64748b",
];

#[rustfmt::skip]
const DARK_COLORS: [Rgba; 32] = [
    Rgba::from_rgba(0x18,0x18,0x1B,0xFF), Rgba::from_rgba(0x27,0x27,0x2A,0xFF), Rgba::from_rgba(0xF4,0xF4,0xF5,0xFF), Rgba::from_rgba(0x71,0x71,0x7A,0xFF),
    Rgba::from_rgba(0x3F,0x3F,0x46,0x80), Rgba::from_rgba(0x52,0x52,0x5B,0xFF), Rgba::from_rgba(0x38,0xBD,0xF8,0xFF), Rgba::from_rgba(0xFB,0x92,0x3C,0xFF),
    Rgba::from_rgba(0x4A,0xDE,0x80,0xFF), Rgba::from_rgba(0xF8,0x71,0x71,0xFF), Rgba(0), Rgba(0), Rgba(0), Rgba(0), Rgba(0), Rgba(0),
    Rgba::from_rgba(0x38,0xBD,0xF8,0xFF), Rgba::from_rgba(0xFB,0x92,0x3C,0xFF), Rgba::from_rgba(0x4A,0xDE,0x80,0xFF), Rgba::from_rgba(0xC0,0x84,0xFC,0xFF),
    Rgba::from_rgba(0xF4,0x72,0xB6,0xFF), Rgba::from_rgba(0x2D,0xD4,0xBF,0xFF), Rgba::from_rgba(0xFA,0xCC,0x15,0xFF), Rgba::from_rgba(0xA7,0x8B,0xFA,0xFF),
    Rgba::from_rgba(0xFB,0x71,0x85,0xFF), Rgba::from_rgba(0x60,0xA5,0xFA,0xFF), Rgba::from_rgba(0x34,0xD3,0x99,0xFF), Rgba::from_rgba(0xF8,0x71,0x71,0xFF),
    Rgba::from_rgba(0x81,0x8C,0xF8,0xFF), Rgba::from_rgba(0xFB,0xBF,0x24,0xFF), Rgba::from_rgba(0xE8,0x79,0xF9,0xFF), Rgba::from_rgba(0x94,0xA3,0xB8,0xFF),
];

#[rustfmt::skip]
const LIGHT_COLORS: [Rgba; 32] = [
    Rgba::from_rgba(0xFF,0xFF,0xFF,0xFF), Rgba::from_rgba(0xF4,0xF4,0xF5,0xFF), Rgba::from_rgba(0x18,0x18,0x1B,0xFF), Rgba::from_rgba(0xA1,0xA1,0xAA,0xFF),
    Rgba::from_rgba(0xE4,0xE4,0xE7,0x80), Rgba::from_rgba(0xD4,0xD4,0xD8,0xFF), Rgba::from_rgba(0x02,0x84,0xC7,0xFF), Rgba::from_rgba(0xEA,0x58,0x0C,0xFF),
    Rgba::from_rgba(0x16,0xA3,0x4A,0xFF), Rgba::from_rgba(0xDC,0x26,0x26,0xFF), Rgba(0), Rgba(0), Rgba(0), Rgba(0), Rgba(0), Rgba(0),
    Rgba::from_rgba(0x02,0x84,0xC7,0xFF), Rgba::from_rgba(0xEA,0x58,0x0C,0xFF), Rgba::from_rgba(0x16,0xA3,0x4A,0xFF), Rgba::from_rgba(0x93,0x33,0xEA,0xFF),
    Rgba::from_rgba(0xDB,0x27,0x77,0xFF), Rgba::from_rgba(0x0D,0x94,0x88,0xFF), Rgba::from_rgba(0xCA,0x8A,0x04,0xFF), Rgba::from_rgba(0x7C,0x3A,0xED,0xFF),
    Rgba::from_rgba(0xE1,0x1D,0x48,0xFF), Rgba::from_rgba(0x25,0x63,0xEB,0xFF), Rgba::from_rgba(0x05,0x96,0x69,0xFF), Rgba::from_rgba(0xDC,0x26,0x26,0xFF),
    Rgba::from_rgba(0x4F,0x46,0xE5,0xFF), Rgba::from_rgba(0xD9,0x77,0x06,0xFF), Rgba::from_rgba(0xC0,0x26,0xD3,0xFF), Rgba::from_rgba(0x64,0x74,0x8B,0xFF),
];

pub static DARK: Palette = Palette {
    colors: DARK_COLORS,
    theme_hash: hash_palette(&DARK_COLORS),
    is_dark: true,
    _pad: [0; 3],
};
pub static LIGHT: Palette = Palette {
    colors: LIGHT_COLORS,
    theme_hash: hash_palette(&LIGHT_COLORS),
    is_dark: false,
    _pad: [0; 3],
};

const fn build_sgr_table(colors: &[Rgba; 32]) -> [SgrSeq; 32] {
    let mut table = [const {
        SgrSeq {
            buf: [0; 20],
            len: 0,
        }
    }; 32];
    let mut i = 0;
    while i < 32 {
        table[i] = make_sgr(colors[i]);
        i += 1;
    }
    table
}

pub static DARK_SGR: [SgrSeq; 32] = build_sgr_table(&DARK_COLORS);
pub static LIGHT_SGR: [SgrSeq; 32] = build_sgr_table(&LIGHT_COLORS);
pub const SGR_RESET: &[u8] = b"\x1b[0m";

#[inline]
#[must_use]
pub fn resolve_color(token: ColorToken, dark: bool) -> Rgba {
    if dark {
        DARK.color(token)
    } else {
        LIGHT.color(token)
    }
}

#[inline]
#[must_use]
pub fn sgr(token: ColorToken, dark: bool) -> &'static [u8] {
    let entry = if dark {
        &DARK_SGR[token.index()]
    } else {
        &LIGHT_SGR[token.index()]
    };
    &entry.buf[..usize::from(entry.len)]
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
