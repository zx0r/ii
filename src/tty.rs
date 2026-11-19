//! Terminal geometry. Probing lands in the I/O boundary phase.

const FLAG_REMOTE: u16 = 1;
const FLAG_TMUX: u16 = 2;

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
    pub flags: u16,
    pub dpr_q8: u16,
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
        flags: 0,
        dpr_q8: 256,
    };

    #[inline]
    #[must_use]
    pub const fn is_remote(self) -> bool {
        self.flags & FLAG_REMOTE != 0
    }

    #[inline]
    #[must_use]
    pub const fn is_tmux(self) -> bool {
        self.flags & FLAG_TMUX != 0
    }

    #[inline]
    #[must_use]
    pub const fn with_env(mut self, remote: bool, tmux: bool) -> Self {
        self.flags = (remote as u16 * FLAG_REMOTE) | (tmux as u16 * FLAG_TMUX);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::Metrics;

    #[test]
    fn flags_roundtrip() {
        let m = Metrics::FALLBACK.with_env(true, false);
        assert!(m.is_remote() && !m.is_tmux());
        let m = m.with_env(false, true);
        assert!(!m.is_remote() && m.is_tmux());
    }
}
