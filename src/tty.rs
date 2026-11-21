//! Terminal geometry (`TIOCGWINSZ`), OSC 11 background probe, and Rec. 709 luminance.

use rustix::{
    fd::{AsFd, OwnedFd},
    termios::{Winsize, tcgetwinsize},
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

#[cfg(target_os = "macos")]
pub const DEFAULT_CELL: (u16, u16) = (18, 36);
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_CELL: (u16, u16) = (9, 18);

#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "px_w and px_h are clamped to u16::MAX"
)]
pub fn resolve_metrics(
    size_arg: Option<(u16, u16)>,
    cell_size_arg: Option<(u16, u16)>,
    is_interactive: bool,
) -> Metrics {
    let is_remote = std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_CLIENT").is_some()
        || std::env::var_os("SSH_TTY").is_some();
    let is_tmux = std::env::var_os("TMUX").is_some();

    let (cols, rows, cell_w, cell_h) = if is_interactive {
        let ws = tcgetwinsize(std::io::stdout().as_fd()).unwrap_or(Winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        });

        let cols = size_arg.map_or(if ws.ws_col > 0 { ws.ws_col } else { 80 }, |(w, _)| w);
        let rows = size_arg.map_or(if ws.ws_row > 0 { ws.ws_row } else { 24 }, |(_, h)| h);

        let (cell_w, cell_h) = cell_size_arg.unwrap_or_else(|| {
            if !is_tmux && ws.ws_col > 0 && ws.ws_xpixel > 0 && ws.ws_row > 0 && ws.ws_ypixel > 0 {
                (
                    (ws.ws_xpixel / ws.ws_col).max(4),
                    (ws.ws_ypixel / ws.ws_row).max(8),
                )
            } else {
                DEFAULT_CELL
            }
        });
        (cols, rows, cell_w, cell_h)
    } else {
        let (cols, rows) = size_arg.unwrap_or((80, 24));
        let (cell_w, cell_h) = cell_size_arg.unwrap_or(DEFAULT_CELL);
        (cols, rows, cell_w, cell_h)
    };

    let px_w = (u32::from(cols) * u32::from(cell_w)).min(u32::from(u16::MAX)) as u16;
    let px_h = (u32::from(rows) * u32::from(cell_h)).min(u32::from(u16::MAX)) as u16;

    Metrics {
        cols,
        rows,
        cell_w,
        cell_h,
        px_w,
        px_h,
        is_remote: u8::from(is_remote),
        is_tmux: u8::from(is_tmux),
        _pad: [0; 2],
    }
}

#[must_use]
pub fn resolve_theme(theme_arg: Option<theme::ThemeChoice>) -> &'static Palette {
    if let Some(choice) = theme_arg {
        return match choice {
            theme::ThemeChoice::Dark => &theme::DARK,
            theme::ThemeChoice::Light => &theme::LIGHT,
        };
    }
    if let Some(colorfgbg) = std::env::var_os("COLORFGBG").and_then(|v| v.into_string().ok())
        && let Some((_, bg)) = colorfgbg.rsplit_once(';')
        && let Ok(bg_num) = bg.trim().parse::<u8>()
        && (bg_num == 15 || bg_num == 7)
    {
        return &theme::LIGHT;
    }
    &theme::DARK
}

#[must_use]
pub fn probe() -> (Metrics, &'static Palette, Option<OwnedFd>) {
    let is_interactive = rustix::termios::isatty(std::io::stdout());
    let metrics = resolve_metrics(None, None, is_interactive);
    let palette = resolve_theme(None);
    (metrics, palette, None)
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
