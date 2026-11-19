//! Terminal geometry probing (`TIOCGWINSZ`), OSC 10/11 background/foreground query, and Rec. 709 luminance.

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fd::{AsFd, BorrowedFd, OwnedFd},
    fs::{Mode, OFlags, open},
    io::{read, write},
    termios::{OptionalActions, tcgetattr, tcgetwinsize, tcsetattr},
};

use crate::theme::{self, Palette};

/// Cell/pixel geometry. 16 B, no padding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Metrics {
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
    pub px_w: u16,
    pub px_h: u16,
    pub is_remote: u8,
    pub is_tmux: u8,
    _pad: [u8; 2],
}

const _: () = assert!(size_of::<Metrics>() == 16);
const _: () = assert!(align_of::<Metrics>() == 2);

impl Metrics {
    pub const FALLBACK: Self = Self {
        cols: 80,
        rows: 24,
        cell_w: 8,
        cell_h: 16,
        px_w: 640,
        px_h: 384,
        is_remote: 0,
        is_tmux: 0,
        _pad: [0; 2],
    };

    #[inline]
    #[must_use]
    pub const fn with_env(mut self, remote: bool, tmux: bool) -> Self {
        self.is_remote = remote as u8;
        self.is_tmux = tmux as u8;
        self
    }

    /// Drawable width in pixels, saturating at `u16::MAX`.
    #[inline]
    #[must_use]
    pub const fn max_width_px(&self) -> u16 {
        self.cols.saturating_mul(self.cell_w)
    }
}

/// Terminal session environment.
#[derive(Debug)]
pub struct Term {
    pub metrics: Metrics,
    pub palette: &'static Palette,
    pub cache: Option<OwnedFd>,
}

impl Term {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            metrics: Metrics::FALLBACK,
            palette: &theme::DARK,
            cache: None,
        }
    }
}

impl Default for Term {
    fn default() -> Self {
        Self::new()
    }
}

/// Integer-scaled Rec. 709 perceived luminance (`(54*R + 183*G + 19*B) >> 8`).
#[inline]
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "Rec. 709 luminance coefficients (54+183+19=256) ensure product >> 8 fits in u8"
)]
pub const fn luminance(r: u8, g: u8, b: u8) -> u8 {
    (((54 * (r as u32)) + (183 * (g as u32)) + (19 * (b as u32))) >> 8) as u8
}


#[inline]
fn parse_hex_channel(s: &[u8]) -> Option<u8> {
    if s.len() >= 2 {
        let hi = theme::hex_digit(s[0])?;
        let lo = theme::hex_digit(s[1])?;
        Some((hi << 4) | lo)
    } else if s.len() == 1 {
        let h = theme::hex_digit(s[0])?;
        Some((h << 4) | h)
    } else {
        None
    }
}

/// Parse an OSC 10/11 response containing `rgb:RRRR/GGGG/BBBB` into 8-bit `(R, G, B)`.
#[must_use]
pub fn parse_osc_rgb(buf: &[u8]) -> Option<(u8, u8, u8)> {
    let pos = buf.windows(4).position(|w| w == b"rgb:")?;
    let tail = &buf[pos + 4..];
    let mut parts = tail.split(|&b| b == b'/' || b == 0x07 || b == 0x1b || b == b'\\');
    let r = parse_hex_channel(parts.next()?)?;
    let g = parse_hex_channel(parts.next()?)?;
    let b = parse_hex_channel(parts.next()?)?;
    Some((r, g, b))
}

fn query_osc_color(fd: BorrowedFd<'_>, query: &[u8]) -> Option<(u8, u8, u8)> {
    let orig = tcgetattr(fd).ok()?;
    let mut raw = orig.clone();
    raw.make_raw();
    tcsetattr(fd, OptionalActions::Now, &raw).ok()?;

    let res = (|| {
        let _ = write(fd, query).ok()?;
        let mut buf = [0u8; 64];
        let mut filled = 0usize;
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: 25_000_000, // 25 ms bounded probe
        };
        for _ in 0..4 {
            let mut pfd = [PollFd::new(&fd, PollFlags::IN)];
            if poll(&mut pfd, Some(&timeout)).ok()? == 0 {
                break;
            }
            let n = read(fd, &mut buf[filled..]).ok()?;
            if n == 0 {
                break;
            }
            filled += n;
            if let Some(rgb) = parse_osc_rgb(&buf[..filled]) {
                return Some(rgb);
            }
            if filled >= buf.len() {
                break;
            }
        }
        parse_osc_rgb(&buf[..filled])
    })();

    let _ = tcsetattr(fd, OptionalActions::Now, &orig);
    res
}

fn query_bg(fd: BorrowedFd<'_>) -> Option<(u8, u8, u8)> {
    query_osc_color(fd, b"\x1b]11;?\x1b\\")
}

#[expect(dead_code, reason = "architectural §3.13 symmetry with query_bg")]
fn query_fg(fd: BorrowedFd<'_>) -> Option<(u8, u8, u8)> {
    query_osc_color(fd, b"\x1b]10;?\x1b\\")
}

/// Probe `/dev/tty` for window geometry (`TIOCGWINSZ`), SSH/tmux environment, and OSC 11 background luminance.
#[must_use]
pub fn probe() -> (Metrics, &'static Palette, Option<OwnedFd>) {
    let is_remote = std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_CLIENT").is_some()
        || std::env::var_os("SSH_TTY").is_some();
    let is_tmux = std::env::var_os("TMUX").is_some();

    let tty_fd = open(
        "/dev/tty",
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok();

    let mut metrics = Metrics::FALLBACK.with_env(is_remote, is_tmux);
    let mut palette: &'static Palette = &theme::DARK;

    if let Some(ref fd) = tty_fd {
        if let Ok(ws) = tcgetwinsize(fd)
            && ws.ws_col > 0
            && ws.ws_row > 0
        {
            metrics.cols = ws.ws_col;
            metrics.rows = ws.ws_row;
            if ws.ws_xpixel > 0 && ws.ws_ypixel > 0 {
                metrics.px_w = ws.ws_xpixel;
                metrics.px_h = ws.ws_ypixel;
                metrics.cell_w = (ws.ws_xpixel / ws.ws_col).max(1);
                metrics.cell_h = (ws.ws_ypixel / ws.ws_row).max(1);
            } else {
                metrics.px_w = metrics.cols.saturating_mul(metrics.cell_w);
                metrics.px_h = metrics.rows.saturating_mul(metrics.cell_h);
            }
        }
        if !is_remote && let Some((r, g, b)) = query_bg(fd.as_fd()) {
            palette = if luminance(r, g, b) < 128 {
                &theme::DARK_PALETTE
            } else {
                &theme::LIGHT_PALETTE
            };
        }
    }

    (metrics, palette, tty_fd)
}

/// Compose mixed terminal document: ANSI styled text interleaved with Kitty graphics placeholders.
pub fn render_ansi(
    src: &[u8],
    spans: &[crate::ipc::SpanCoord],
    dark: bool,
    out: &mut Vec<u8>,
) {
    let mut cursor = 0usize;
    let fg_sgr = theme::sgr(theme::ColorToken::FG_PRIMARY, dark);

    for sp in spans {
        let start = (sp.outer_start as usize).min(src.len());
        let end = (sp.outer_end as usize).min(src.len());

        if start > cursor {
            out.extend_from_slice(fg_sgr);
            out.extend_from_slice(&src[cursor..start]);
            out.extend_from_slice(theme::SGR_RESET);
        }

        if !out.is_empty() && out.last() != Some(&b'\n') {
            out.push(b'\n');
        }
        crate::kitty::emit_placeholder(sp.image_id, sp.cols, sp.rows, out);

        cursor = end.max(cursor);
    }

    if cursor < src.len() {
        out.extend_from_slice(fg_sgr);
        out.extend_from_slice(&src[cursor..]);
        out.extend_from_slice(theme::SGR_RESET);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_roundtrip() {
        let metrics = Metrics::FALLBACK.with_env(true, false);
        assert_eq!((metrics.is_remote, metrics.is_tmux), (1, 0));
        let metrics = metrics.with_env(false, true);
        assert_eq!((metrics.is_remote, metrics.is_tmux), (0, 1));
    }

    #[test]
    fn osc_11_parse_and_luminance() {
        let resp = b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\";
        let (r, g, b) = parse_osc_rgb(resp).unwrap_or((0, 0, 0));
        assert_eq!((r, g, b), (0x1e, 0x1e, 0x2e));
        assert!(luminance(r, g, b) < 128);
        assert!(luminance(0xef, 0xf1, 0xf5) >= 128);
    }

    #[test]
    fn width_px_fallback() {
        assert_eq!(Term::new().metrics.max_width_px(), 640);
    }

    #[test]
    fn width_px_saturates() {
        let mut term = Term::new();
        term.metrics.cols = u16::MAX;
        term.metrics.cell_w = 2;
        assert_eq!(term.metrics.max_width_px(), u16::MAX);
    }
}
