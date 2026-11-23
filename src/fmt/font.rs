//! Allocation-free width estimates for initial scene layout.
//! SVG text shaping and final clipping are performed by usvg.

#[rustfmt::skip]
const ADVANCE_PCT: [u8; 128] = [
    0,0,0,0,0,0,0,0,0,240,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    30,33,40,60,56,84,68,24,34,34,44,60,28,36,28,34,
    56,56,56,56,56,56,56,56,56,56,30,30,60,60,60,52,
    92,66,64,66,70,60,58,72,70,30,48,64,56,82,70,74,
    62,74,66,60,60,70,64,90,64,62,60,34,34,34,54,56,
    34,54,58,52,58,54,34,56,58,26,28,52,26,84,58,56,
    58,58,38,50,36,58,52,74,52,52,48,38,28,38,60,0,
];

/// Stroke glyph cap height and cell width as fractions of the font size (`raster` grid is 4x6).
pub const GLYPH_H_EM: f32 = 0.68;
pub const GLYPH_W_EM: f32 = GLYPH_H_EM * 0.54;
/// Monospace advance of the stroke font; proportional advances are blended toward it so that
/// narrow/wide glyphs keep visual rhythm without overlapping the fixed 4-unit stroke box.
const MONO_EM: f32 = GLYPH_W_EM * 1.32;
const NON_ASCII_EM: f32 = 1.0;

/// Advance of `ch` in em units (`font_px` multiplier).
#[inline]
#[must_use]
pub fn advance_em(ch: char) -> f32 {
    let code = ch as u32;
    if code >= 128 {
        return NON_ASCII_EM;
    }
    let pct = ADVANCE_PCT[code as usize];
    if pct == 0 {
        return 0.0;
    }
    MONO_EM.mul_add(0.75, f32::from(pct) * 0.0025)
}

/// Estimate pixel width of UTF-8 text at `font_px` for initial layout.
#[inline]
#[must_use]
pub fn measure_str(text: &str, font_px: f32) -> f32 {
    text.chars().map(advance_em).sum::<f32>() * font_px
}

#[inline]
#[must_use]
pub fn measure_bytes(bytes: &[u8], font_px: f32) -> f32 {
    measure_str(core::str::from_utf8(bytes).unwrap_or(""), font_px)
}
