//! Terminal geometry. Probing lands in the I/O boundary phase.

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
    pub _pad: [u8; 2],
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
}

#[cfg(test)]
mod tests {
    use super::Metrics;

    #[test]
    fn flags_roundtrip() {
        let m = Metrics::FALLBACK.with_env(true, false);
        assert_eq!((m.is_remote, m.is_tmux), (1, 0));
        let m = m.with_env(false, true);
        assert_eq!((m.is_remote, m.is_tmux), (0, 1));
    }
}
