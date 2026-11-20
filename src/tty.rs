//! Terminal geometry (`TIOCGWINSZ`), OSC 11 background probe, and Rec. 709 luminance.

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fd::{AsFd, BorrowedFd, OwnedFd},
    fs::{Mode, OFlags, open},
    io::{Errno, read, write},
    termios::{OptionalActions, Termios, tcgetattr, tcgetwinsize, tcsetattr},
};

use crate::theme::{self, Palette};

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

    #[inline]
    #[must_use]
    pub const fn max_width_px(&self) -> u16 {
        self.cols.saturating_mul(self.cell_w)
    }
}

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

struct RawModeGuard<'fd> {
    fd: BorrowedFd<'fd>,
    orig: Termios,
}

impl<'fd> RawModeGuard<'fd> {
    fn enter(fd: BorrowedFd<'fd>) -> Option<Self> {
        let orig = tcgetattr(fd).ok()?;
        let mut raw = orig.clone();
        raw.make_raw();
        tcsetattr(fd, OptionalActions::Now, &raw).ok()?;
        Some(Self { fd, orig })
    }
}

impl Drop for RawModeGuard<'_> {
    fn drop(&mut self) {
        let _ = tcsetattr(self.fd, OptionalActions::Now, &self.orig);
    }
}

#[inline]
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "(54+183+19)*255 >> 8 <= 255"
)]
pub const fn luminance(r: u8, g: u8, b: u8) -> u8 {
    (((54 * (r as u32)) + (183 * (g as u32)) + (19 * (b as u32))) >> 8) as u8
}

#[inline]
fn parse_hex_channel(s: &[u8]) -> Option<u8> {
    let hi = theme::hex_digit(*s.first()?)?;
    let lo = s.get(1).and_then(|&b| theme::hex_digit(b)).unwrap_or(hi);
    Some((hi << 4) | lo)
}

#[must_use]
pub fn parse_osc_rgb(buf: &[u8]) -> Option<(u8, u8, u8)> {
    let pos = buf.windows(4).position(|w| w == b"rgb:")?;
    let tail = &buf[pos + 4..];
    let term = tail.iter().enumerate().find_map(|(i, &b)| {
        (b == 0x07 || (b == 0x1b && tail.get(i + 1) == Some(&b'\\'))).then_some(i)
    })?;
    let mut parts = tail[..term].split(|&b| b == b'/');
    Some((
        parse_hex_channel(parts.next()?)?,
        parse_hex_channel(parts.next()?)?,
        parse_hex_channel(parts.next()?)?,
    ))
}

fn query_osc_color(fd: BorrowedFd<'_>, mut query: &[u8]) -> Option<(u8, u8, u8)> {
    let _raw = RawModeGuard::enter(fd)?;
    while !query.is_empty() {
        match write(fd, query) {
            Ok(0) => return None,
            Ok(n) => query = &query[n..],
            Err(Errno::INTR) => {}
            Err(_) => return None,
        }
    }

    let mut buf = [0u8; 64];
    let mut filled = 0usize;
    let timeout = Timespec {
        tv_sec: 0,
        tv_nsec: 25_000_000,
    };
    for _ in 0..4 {
        let ready = loop {
            let mut pfd = [PollFd::new(&fd, PollFlags::IN)];
            match poll(&mut pfd, Some(&timeout)) {
                Ok(n) => break n,
                Err(Errno::INTR) => {}
                Err(_) => return None,
            }
        };
        if ready == 0 {
            break;
        }
        let n = loop {
            match read(fd, &mut buf[filled..]) {
                Ok(n) => break n,
                Err(Errno::INTR) => {}
                Err(_) => return None,
            }
        };
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
}

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
        if !is_remote && let Some((r, g, b)) = query_osc_color(fd.as_fd(), b"\x1b]11;?\x1b\\") {
            palette = if luminance(r, g, b) < 128 {
                &theme::DARK
            } else {
                &theme::LIGHT
            };
        }
    }
    (metrics, palette, tty_fd)
}

pub fn render_ansi(src: &[u8], spans: &[crate::ipc::SpanCoord], dark: bool, out: &mut Vec<u8>) {
    out.reserve(src.len());
    let mut cursor = 0usize;
    let fg_sgr = theme::sgr(theme::ColorToken::FG_PRIMARY, dark);
    for sp in spans {
        let start = (sp.outer_start as usize).min(src.len());
        let end = (sp.outer_end as usize).clamp(start, src.len());
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
