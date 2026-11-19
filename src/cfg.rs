//! Immutable session configuration.

use rustix::fd::OwnedFd;

use crate::tty::Metrics;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Viewport {
    pub skip_lines: u16,
    pub max_lines: u16,
    pub width_cells: u16,
    pub height_cells: u16,
}

const _: () = assert!(size_of::<Viewport>() == 8);

impl Viewport {
    pub const DEFAULT: Self = Self {
        skip_lines: 0,
        max_lines: u16::MAX,
        width_cells: 0,
        height_cells: 0,
    };
}

impl Default for Viewport {
    fn default() -> Self {
        Self::DEFAULT
    }
}

pub struct Cfg {
    pub tty: Metrics,
    pub viewport: Viewport,
    pub cache_dir: Option<OwnedFd>,
    pub is_stdout_tty: bool,
}

impl Cfg {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tty: Metrics::FALLBACK,
            viewport: Viewport::DEFAULT,
            cache_dir: None,
            is_stdout_tty: false,
        }
    }

    /// Drawable width in pixels, saturating at `u16::MAX`.
    #[inline]
    #[must_use]
    pub const fn max_width_px(&self) -> u16 {
        self.tty.cols.saturating_mul(self.tty.cell_w)
    }
}

impl Default for Cfg {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Cfg;

    #[test]
    fn width_px_fallback() {
        assert_eq!(Cfg::new().max_width_px(), 640);
    }

    #[test]
    fn width_px_saturates() {
        let mut c = Cfg::new();
        c.tty.cols = u16::MAX;
        c.tty.cell_w = 2;
        assert_eq!(c.max_width_px(), u16::MAX);
    }
}
