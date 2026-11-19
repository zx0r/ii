//! Compile-time palettes. Every token is precomputed: hex, SGR and raw RGB.

#![expect(
    clippy::cast_possible_truncation,
    reason = "channel extraction from packed 0xRRGGBB and bounded SGR length (<= 19)"
)]

/// 32 B: `#RRGGBB`, `ESC[38;2;R;G;Bm`, raw channels for tag folding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Token {
    pub hex: [u8; 7],
    pub hex_len: u8,
    pub sgr: [u8; 20],
    pub sgr_len: u8,
    pub rgb: [u8; 3],
}

const _: () = assert!(size_of::<Token>() == 32);

impl Token {
    #[inline]
    #[must_use]
    pub fn hex(&self) -> &[u8] {
        self.hex.split_at(self.hex_len as usize).0
    }

    #[inline]
    #[must_use]
    pub fn sgr(&self) -> &[u8] {
        self.sgr.split_at(self.sgr_len as usize).0
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

const fn put_dec(buf: &mut [u8; 20], mut n: usize, v: u8) -> usize {
    if v >= 100 {
        buf[n] = b'0' + v / 100;
        n += 1;
    }
    if v >= 10 {
        buf[n] = b'0' + (v / 10) % 10;
        n += 1;
    }
    buf[n] = b'0' + v % 10;
    n + 1
}

#[must_use]
pub const fn token_from_rgb(r: u8, g: u8, b: u8) -> Token {
    let hex = [
        b'#',
        HEX[(r >> 4) as usize],
        HEX[(r & 15) as usize],
        HEX[(g >> 4) as usize],
        HEX[(g & 15) as usize],
        HEX[(b >> 4) as usize],
        HEX[(b & 15) as usize],
    ];
    let mut sgr = [0u8; 20];
    let head = b"\x1b[38;2;";
    let mut n = 0;
    while n < head.len() {
        sgr[n] = head[n];
        n += 1;
    }
    n = put_dec(&mut sgr, n, r);
    sgr[n] = b';';
    n = put_dec(&mut sgr, n + 1, g);
    sgr[n] = b';';
    n = put_dec(&mut sgr, n + 1, b);
    sgr[n] = b'm';
    n += 1;
    Token {
        hex,
        hex_len: 7,
        sgr,
        sgr_len: n as u8,
        rgb: [r, g, b],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Idx {
    Fg = 0,
    FgMuted,
    Bg,
    BgSurface,
    Heading,
    HeadingSub,
    Link,
    Code,
    Error,
    GridLine,
    Series0,
    Series1,
    Series2,
    Series3,
    Series4,
    Series5,
    Series6,
    Series7,
}

pub const N_TOKENS: usize = 18;

#[derive(Debug)]
pub struct Palette {
    pub tokens: [Token; N_TOKENS],
    pub is_dark: bool,
}

impl Palette {
    #[inline]
    #[must_use]
    pub const fn get(&self, idx: Idx) -> &Token {
        &self.tokens[idx as usize]
    }

    /// `i` wraps over the 8 series colours.
    #[inline]
    #[must_use]
    pub const fn series(&self, i: usize) -> &Token {
        &self.tokens[Idx::Series0 as usize + (i & 7)]
    }

    #[inline]
    #[must_use]
    pub fn pick(dark: bool) -> &'static Self {
        if dark { &DARK } else { &LIGHT }
    }
}

const fn build(rgb: [u32; N_TOKENS], is_dark: bool) -> Palette {
    let mut tokens = [token_from_rgb(0, 0, 0); N_TOKENS];
    let mut i = 0;
    while i < N_TOKENS {
        let c = rgb[i];
        tokens[i] = token_from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
        i += 1;
    }
    Palette { tokens, is_dark }
}

/// Catppuccin Mocha.
pub static DARK: Palette = build(
    [
        0xcdd6f4, 0xa6adc8, 0x1e1e2e, 0x313244, 0xcba6f7, 0x89b4fa, 0x74c7ec, 0xa6e3a1, 0xf38ba8,
        0x45475a, 0x89b4fa, 0xa6e3a1, 0xfab387, 0xcba6f7, 0xf9e2af, 0x94e2d5, 0xf5c2e7, 0xf38ba8,
    ],
    true,
);

/// Catppuccin Latte.
pub static LIGHT: Palette = build(
    [
        0x4c4f69, 0x6c6f85, 0xeff1f5, 0xccd0da, 0x8839ef, 0x1e66f5, 0x209fb5, 0x40a02b, 0xd20f39,
        0xbcc0cc, 0x1e66f5, 0x40a02b, 0xfe640b, 0x8839ef, 0xdf8e1d, 0x179299, 0xea76cb, 0xd20f39,
    ],
    false,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrip() {
        let t = token_from_rgb(0xcd, 5, 255);
        assert_eq!(t.hex(), b"#cd05ff");
        assert_eq!(t.sgr(), b"\x1b[38;2;205;5;255m");
        assert_eq!(t.rgb, [0xcd, 5, 255]);
    }

    #[test]
    fn sgr_extremes_fit() {
        assert_eq!(token_from_rgb(0, 0, 0).sgr(), b"\x1b[38;2;0;0;0m");
        assert_eq!(token_from_rgb(255, 255, 255).sgr_len, 19);
    }

    #[test]
    fn palettes_distinct_and_indexed() {
        assert_eq!(DARK.get(Idx::Bg).hex(), b"#1e1e2e");
        assert_eq!(LIGHT.get(Idx::Bg).hex(), b"#eff1f5");
        assert_eq!(DARK.series(8), DARK.series(0));
        assert!(Palette::pick(true).is_dark && !Palette::pick(false).is_dark);
    }
}
