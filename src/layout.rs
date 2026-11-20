//! Pure zero-allocation layout math: axis scales and table column distribution.

const MIN_COL_W: f32 = 32.0;
const MIN_SCALED_COL_W: f32 = 24.0;
const MAX_TICKS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct LinearScale {
    pub domain_min: f32,
    pub domain_max: f32,
    pub range_min: f32,
    pub range_max: f32,
}
const _: () = assert!(size_of::<LinearScale>() == 16);

impl LinearScale {
    #[inline]
    #[must_use]
    pub const fn new(d_min: f32, d_max: f32, r_min: f32, r_max: f32) -> Self {
        Self {
            domain_min: d_min,
            domain_max: d_max,
            range_min: r_min,
            range_max: r_max,
        }
    }

    #[inline]
    #[must_use]
    pub fn map(&self, val: f32) -> f32 {
        let span = self.domain_max - self.domain_min;
        if !span.is_finite() || span.abs() < 1e-6 {
            return self.range_min;
        }
        let t = (val - self.domain_min) / span;
        t.mul_add(self.range_max - self.range_min, self.range_min)
    }

    pub fn compute_ticks(&self, target_count: usize, out: &mut Vec<f32>) {
        out.clear();
        let span = self.domain_max - self.domain_min;
        if !span.is_finite() || span <= 0.0 || target_count == 0 {
            if self.domain_min.is_finite() {
                out.push(self.domain_min);
            }
            return;
        }
        let clamped = target_count.min(MAX_TICKS);
        #[expect(clippy::cast_precision_loss, reason = "clamped <= MAX_TICKS (64)")]
        let step = nice_num(span / (clamped as f32), true);
        if !step.is_finite() || step <= 0.0 {
            out.push(self.domain_min);
            return;
        }
        let limit = step.mul_add(0.5, self.domain_max);
        let mut cur = (self.domain_min / step).ceil() * step;
        out.reserve(clamped + 2);
        for _ in 0..MAX_TICKS {
            if cur > limit {
                break;
            }
            out.push(cur);
            let next = cur + step;
            if next <= cur {
                break;
            }
            cur = next;
        }
    }
}

#[inline]
fn nice_num(range: f32, round: bool) -> f32 {
    if !range.is_finite() || range <= 0.0 {
        return 1.0;
    }
    let pow10 = 10.0_f32.powf(range.log10().floor());
    let frac = range / pow10;
    let nice = if round {
        if frac < 1.5 {
            1.0
        } else if frac < 3.0 {
            2.0
        } else if frac < 7.0 {
            5.0
        } else {
            10.0
        }
    } else if frac <= 1.0 {
        1.0
    } else if frac <= 2.0 {
        2.0
    } else if frac <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * pow10
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct TableMetrics {
    pub total_width: f32,
    pub row_height: f32,
}
const _: () = assert!(size_of::<TableMetrics>() == 8);

pub fn compute_table_widths(
    cols: usize,
    content_lengths: &[usize],
    char_width: f32,
    padding: f32,
    max_total_w: f32,
    out_widths: &mut Vec<f32>,
) -> TableMetrics {
    out_widths.clear();
    if cols == 0 || !char_width.is_finite() || !padding.is_finite() || !max_total_w.is_finite() {
        return TableMetrics::default();
    }
    let cw = char_width.max(1.0);
    let pad2 = padding.max(0.0) * 2.0;
    let max_w = max_total_w.max(MIN_COL_W);
    let col_cap = (max_w * 0.6).max(MIN_COL_W);

    out_widths.resize(cols, 0.0);
    for (i, &len) in content_lengths.iter().enumerate() {
        let len_f = f32::from(u16::try_from(len).unwrap_or(u16::MAX));
        let slot = &mut out_widths[i % cols];
        if len_f > *slot {
            *slot = len_f;
        }
    }

    let mut total = 0.0_f32;
    for w in out_widths.iter_mut() {
        *w = (*w).mul_add(cw, pad2).clamp(MIN_COL_W, col_cap);
        total += *w;
    }
    if total > max_w {
        let scale = max_w / total;
        total = 0.0;
        for w in out_widths.iter_mut() {
            *w = (*w * scale).max(MIN_SCALED_COL_W);
            total += *w;
        }
    }
    TableMetrics {
        total_width: total,
        row_height: cw * 2.2,
    }
}
