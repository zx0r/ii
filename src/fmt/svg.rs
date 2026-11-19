//! Single-pass O(N) streaming SVG theme rewriter (`src -> dst`, zero in-place `memmove`).

use memchr::memchr3;

use crate::theme::{Idx, Palette};

#[inline]
fn is_black_hex(rem: &[u8]) -> Option<usize> {
    if rem.len() >= 7 && rem[..7].eq_ignore_ascii_case(b"#000000") {
        Some(7)
    } else if rem.len() >= 4
        && rem[..4].eq_ignore_ascii_case(b"#000")
        && (rem.len() == 4 || !rem[4].is_ascii_hexdigit())
    {
        Some(4)
    } else {
        None
    }
}

#[inline]
fn is_white_hex(rem: &[u8]) -> Option<usize> {
    if rem.len() >= 7 && rem[..7].eq_ignore_ascii_case(b"#ffffff") {
        Some(7)
    } else if rem.len() >= 4
        && rem[..4].eq_ignore_ascii_case(b"#fff")
        && (rem.len() == 4 || !rem[4].is_ascii_hexdigit())
    {
        Some(4)
    } else {
        None
    }
}

#[inline]
fn match_rgb_zero(rem: &[u8]) -> Option<usize> {
    if !rem.starts_with(b"rgb(") {
        return None;
    }
    let close = rem[..rem.len().min(32)].iter().position(|&b| b == b')')?;
    let inner = &rem[4..close];
    if inner
        .iter()
        .all(|&b| matches!(b, b'0' | b',' | b' ' | b'%' | b'.'))
        && inner.contains(&b'0')
    {
        Some(close + 1)
    } else {
        None
    }
}

/// Stream `src` into `dst`, substituting hardcoded `#000`/`#000000`, `currentColor`,
/// `rgb(0,0,0)`, and `#fff`/`#ffffff` with palette foreground/background tokens in O(N).
pub fn rewrite_colors(src: &[u8], palette: &Palette, dst: &mut Vec<u8>) {
    let fg = palette.get(Idx::Fg).hex();
    let bg = palette.get(Idx::Bg).hex();
    let len = src.len();
    let mut pos = 0usize;

    while pos < len {
        let rem = &src[pos..];
        let Some(off) = memchr3(b'#', b'c', b'r', rem) else {
            dst.extend_from_slice(rem);
            break;
        };
        dst.extend_from_slice(&rem[..off]);
        pos += off;
        let tail = &src[pos..];

        match tail[0] {
            b'#' => {
                if let Some(n) = is_black_hex(tail) {
                    dst.extend_from_slice(fg);
                    pos += n;
                } else if let Some(n) = is_white_hex(tail) {
                    dst.extend_from_slice(bg);
                    pos += n;
                } else {
                    dst.push(b'#');
                    pos += 1;
                }
            }
            b'c' => {
                if tail.len() >= 12 && &tail[..12] == b"currentColor" {
                    dst.extend_from_slice(fg);
                    pos += 12;
                } else {
                    dst.push(b'c');
                    pos += 1;
                }
            }
            b'r' => {
                if let Some(n) = match_rgb_zero(tail) {
                    dst.extend_from_slice(fg);
                    pos += n;
                } else {
                    dst.push(b'r');
                    pos += 1;
                }
            }
            _ => {
                dst.push(tail[0]);
                pos += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::rewrite_colors;
    use crate::theme::{DARK, Idx};

    #[test]
    fn rewrites_black_current_color_and_rgb_in_single_pass() {
        let src = b"<svg fill=\"#000\" stroke=\"currentColor\"><path fill=\"rgb(0, 0, 0)\" bg=\"#ffffff\"/></svg>";
        let mut dst = Vec::new();
        rewrite_colors(src, &DARK, &mut dst);
        let fg = core::str::from_utf8(DARK.get(Idx::Fg).hex()).unwrap_or("");
        let bg = core::str::from_utf8(DARK.get(Idx::Bg).hex()).unwrap_or("");
        let expected =
            format!("<svg fill=\"{fg}\" stroke=\"{fg}\"><path fill=\"{fg}\" bg=\"{bg}\"/></svg>");
        assert_eq!(dst, expected.as_bytes());
    }
}
