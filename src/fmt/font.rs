//! Fast ASCII proportional glyph advance-width LUT (`0.01 em` units). Zero allocations.

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

/// Estimate pixel width of UTF-8 text at `font_px` without shaping overhead.
#[inline]
#[must_use]
pub fn measure_str(text: &str, font_px: f32) -> f32 {
    let mut sum = 0u32;
    for ch in text.chars() {
        let code = ch as u32;
        sum = sum.saturating_add(if code < 128 {
            u32::from(ADVANCE_PCT[code as usize])
        } else {
            100
        });
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "display string pixel width fits f32 precision"
    )]
    let width = (sum as f32) * font_px * 0.01;
    width
}

#[inline]
#[must_use]
pub fn measure_bytes(bytes: &[u8], font_px: f32) -> f32 {
    measure_str(core::str::from_utf8(bytes).unwrap_or(""), font_px)
}
