//! Static palette tables, 1-byte `ColorToken`, `SemanticRole`, and compile-time SGR/Hex escape sequences.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    Dark,
    Light,
}

/// Semantic color role in the visual interface hierarchy.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticRole {
    TextPrimary = 0,
    TextMuted = 1,
    Surface = 2,
    Border = 3,
    Accent = 4,
    Info = 5,
    Success = 6,
    Warn = 7,
    Danger = 8,
    Highlight = 9,
}

const _: () = assert!(size_of::<SemanticRole>() == 1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct ColorToken(u8);

const _: () = assert!(size_of::<ColorToken>() == 1);

impl ColorToken {
    pub const TEXT_PRIMARY: Self = Self(0);
    pub const TEXT_MUTED: Self = Self(1);
    pub const SURFACE: Self = Self(2);
    pub const BORDER: Self = Self(3);
    pub const ACCENT: Self = Self(4);
    pub const INFO: Self = Self(5);
    pub const SUCCESS: Self = Self(6);
    pub const WARN: Self = Self(7);
    pub const DANGER: Self = Self(8);

    pub const BG_PRIMARY: Self = Self::SURFACE;
    pub const BG_SURFACE: Self = Self::SURFACE;
    pub const FG_PRIMARY: Self = Self::TEXT_PRIMARY;
    pub const FG_MUTED: Self = Self::TEXT_MUTED;
    pub const GRID_LINE: Self = Self::BORDER;
    pub const ACCENT_ORANGE: Self = Self::ACCENT;
    pub const ACCENT_GREEN: Self = Self::SUCCESS;
    pub const ACCENT_RED: Self = Self::DANGER;

    #[inline]
    #[must_use]
    pub const fn from_role(role: SemanticRole) -> Self {
        Self(role as u8)
    }

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

/// 32-bit packed straight sRGB RGBA color in LE byte order: `[r, g, b, a]`.
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

#[derive(Clone, Copy, Debug)]
pub struct HexSeq(pub [u8; 7]);

impl HexSeq {
    #[inline]
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match core::str::from_utf8(&self.0) {
            Ok(s) => s,
            Err(_) => "",
        }
    }
}

const HEX_LUT: &[u8; 16] = b"0123456789ABCDEF";

const fn make_hex(c: Rgba) -> HexSeq {
    let [r, g, b, _] = c.to_array();
    HexSeq([
        b'#',
        HEX_LUT[(r >> 4) as usize],
        HEX_LUT[(r & 0x0F) as usize],
        HEX_LUT[(g >> 4) as usize],
        HEX_LUT[(g & 0x0F) as usize],
        HEX_LUT[(b >> 4) as usize],
        HEX_LUT[(b & 0x0F) as usize],
    ])
}

const fn build_hex_table(colors: &[Rgba; 32]) -> [HexSeq; 32] {
    let mut table = [HexSeq(*b"#000000"); 32];
    let mut i = 0;
    while i < 32 {
        table[i] = make_hex(colors[i]);
        i += 1;
    }
    table
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
    pub sgr: &'static [SgrSeq; 32],
    pub hex: &'static [HexSeq; 32],
    pub theme_hash: u32,
    pub is_dark: bool,
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
    pub const fn role_color(&self, role: SemanticRole) -> Rgba {
        self.colors[role as usize]
    }

    #[inline]
    #[must_use]
    pub const fn hex_str(&self, role: SemanticRole) -> &'static str {
        self.hex[role as usize].as_str()
    }

    #[inline]
    #[must_use]
    pub const fn series_hex_str(&self, idx: usize) -> &'static str {
        self.hex[16 + (idx & 15)].as_str()
    }

    #[inline]
    #[must_use]
    pub const fn sgr_fg(&self, role: SemanticRole) -> &'static [u8] {
        let entry = &self.sgr[role as usize];
        entry.buf.split_at(entry.len as usize).0
    }

    /// SVG surfaces are transparent whenever the theme surface alpha is zero.
    #[must_use]
    pub const fn svg_surface_fill(&self) -> &'static str {
        if self.color(ColorToken::SURFACE).a() == 0 {
            "none"
        } else {
            self.hex_str(SemanticRole::Surface)
        }
    }

    #[inline]
    pub fn write_sgr_fg(&self, out: &mut Vec<u8>, role: SemanticRole) {
        out.extend_from_slice(self.sgr_fg(role));
    }
}

// Historical ii_old palettes (a2d84066); surfaces retain RGB identity but have zero alpha.
const DARK_MUTED: Rgba = Rgba::from_rgba(161, 167, 194, 255);
const LIGHT_MUTED: Rgba = Rgba::from_rgba(116, 119, 140, 255);
#[rustfmt::skip]
const DARK_COLORS: [Rgba; 32] = [
    Rgba::from_rgba(205, 214, 244, 255),
    DARK_MUTED,
    Rgba::from_rgba(30, 30, 46, 0),
    DARK_MUTED,
    Rgba::from_rgba(203, 166, 247, 255),
    Rgba::from_rgba(137, 180, 250, 255),
    Rgba::from_rgba(166, 227, 161, 255),
    Rgba::from_rgba(249, 226, 175, 255),
    Rgba::from_rgba(243, 139, 168, 255),
    Rgba::from_rgba(148, 226, 213, 255),
    Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(137, 180, 250, 255),
    Rgba::from_rgba(166, 227, 161, 255),
    Rgba::from_rgba(203, 166, 247, 255),
    Rgba::from_rgba(250, 179, 135, 255),
    Rgba::from_rgba(148, 226, 213, 255),
    Rgba::from_rgba(249, 226, 175, 255),
    Rgba::from_rgba(243, 139, 168, 255),
    Rgba::from_rgba(137, 220, 235, 255),
    Rgba::from_rgba(137, 180, 250, 255),
    Rgba::from_rgba(166, 227, 161, 255),
    Rgba::from_rgba(203, 166, 247, 255),
    Rgba::from_rgba(250, 179, 135, 255),
    Rgba::from_rgba(148, 226, 213, 255),
    Rgba::from_rgba(249, 226, 175, 255),
    Rgba::from_rgba(243, 139, 168, 255),
    Rgba::from_rgba(137, 220, 235, 255),
];
#[rustfmt::skip]
const LIGHT_COLORS: [Rgba; 32] = [
    Rgba::from_rgba(76, 79, 105, 255),
    LIGHT_MUTED,
    Rgba::from_rgba(239, 241, 245, 0),
    LIGHT_MUTED,
    Rgba::from_rgba(136, 57, 239, 255),
    Rgba::from_rgba(30, 102, 245, 255),
    Rgba::from_rgba(64, 160, 43, 255),
    Rgba::from_rgba(223, 142, 29, 255),
    Rgba::from_rgba(210, 15, 57, 255),
    Rgba::from_rgba(23, 146, 153, 255),
    Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(0, 0, 0, 0),
    Rgba::from_rgba(30, 102, 245, 255),
    Rgba::from_rgba(64, 160, 43, 255),
    Rgba::from_rgba(136, 57, 239, 255),
    Rgba::from_rgba(254, 100, 11, 255),
    Rgba::from_rgba(23, 146, 153, 255),
    Rgba::from_rgba(223, 142, 29, 255),
    Rgba::from_rgba(210, 15, 57, 255),
    Rgba::from_rgba(4, 165, 229, 255),
    Rgba::from_rgba(30, 102, 245, 255),
    Rgba::from_rgba(64, 160, 43, 255),
    Rgba::from_rgba(136, 57, 239, 255),
    Rgba::from_rgba(254, 100, 11, 255),
    Rgba::from_rgba(23, 146, 153, 255),
    Rgba::from_rgba(223, 142, 29, 255),
    Rgba::from_rgba(210, 15, 57, 255),
    Rgba::from_rgba(4, 165, 229, 255),
];

pub static DARK_SGR: [SgrSeq; 32] = build_sgr_table(&DARK_COLORS);
pub static LIGHT_SGR: [SgrSeq; 32] = build_sgr_table(&LIGHT_COLORS);
pub static DARK_HEX: [HexSeq; 32] = build_hex_table(&DARK_COLORS);
pub static LIGHT_HEX: [HexSeq; 32] = build_hex_table(&LIGHT_COLORS);

pub static DARK: Palette = Palette {
    colors: DARK_COLORS,
    sgr: &DARK_SGR,
    hex: &DARK_HEX,
    theme_hash: hash_palette(&DARK_COLORS),
    is_dark: true,
};

pub static LIGHT: Palette = Palette {
    colors: LIGHT_COLORS,
    sgr: &LIGHT_SGR,
    hex: &LIGHT_HEX,
    theme_hash: hash_palette(&LIGHT_COLORS),
    is_dark: false,
};

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
    let palette = if dark { &DARK } else { &LIGHT };
    let entry = &palette.sgr[token.index()];
    entry.buf.split_at(entry.len as usize).0
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
