//! Shared layout & geometry engine: axis scales, table column distribution,
//! and Sugiyama DAG coordinate assignment. Zero frontend pixel awareness.

/// 1D linear scale for charts and plots (Domain -> Range mapping).
#[derive(Clone, Copy, Debug)]
pub struct LinearScale {
    pub domain_min: f32,
    pub domain_max: f32,
    pub range_min: f32,
    pub range_max: f32,
}

impl LinearScale {
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
        if span.abs() < 1e-6 {
            return self.range_min;
        }
        let t = (val - self.domain_min) / span;
        self.range_min + t * (self.range_max - self.range_min)
    }

    /// Compute "nice" human-readable axis ticks.
    pub fn compute_ticks(&self, target_count: usize) -> Vec<f32> {
        let span = self.domain_max - self.domain_min;
        if span <= 0.0 || target_count == 0 {
            return vec![self.domain_min];
        }

        let raw_step = span / (target_count as f32);
        let step = nice_num(raw_step, true);
        let start = (self.domain_min / step).ceil() * step;
        let mut ticks = Vec::with_capacity(target_count + 2);
        let mut cur = start;
        while cur <= self.domain_max + step * 0.5 {
            ticks.push(cur);
            cur += step;
        }
        ticks
    }
}

fn nice_num(range: f32, round: bool) -> f32 {
    let exponent = range.log10().floor();
    let fraction = range / 10.0f32.powf(exponent);
    let nice_fraction = if round {
        if fraction < 1.5 {
            1.0
        } else if fraction < 3.0 {
            2.0
        } else if fraction < 7.0 {
            5.0
        } else {
            10.0
        }
    } else if fraction <= 1.0 {
        1.0
    } else if fraction <= 2.0 {
        2.0
    } else if fraction <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice_fraction * 10.0f32.powf(exponent)
}

/// Table layout calculator: distributes column widths based on cell content lengths.
#[derive(Debug)]
pub struct TableLayout {
    pub col_widths: Vec<f32>,
    pub total_width: f32,
    pub row_height: f32,
}

impl TableLayout {
    /// Compute column widths with min/max clamps and padding.
    pub fn compute(
        cols: usize,
        content_lengths: &[usize], // flattened row-major or per-column max chars
        char_width: f32,
        padding: f32,
        max_total_w: f32,
    ) -> Self {
        let mut max_chars = vec![0usize; cols];
        for (i, &len) in content_lengths.iter().enumerate() {
            let col = i % cols;
            if len > max_chars[col] {
                max_chars[col] = len;
            }
        }

        let mut col_widths = Vec::with_capacity(cols);
        let mut total = 0.0;
        for &chars in &max_chars {
            let w = (chars as f32 * char_width + padding * 2.0).clamp(32.0, max_total_w * 0.6);
            col_widths.push(w);
            total += w;
        }

        // Scale down proportionally if exceeding max_total_w
        if total > max_total_w && total > 0.0 {
            let scale = max_total_w / total;
            for w in &mut col_widths {
                *w = (*w * scale).max(24.0);
            }
            total = max_total_w;
        }

        Self {
            col_widths,
            total_width: total,
            row_height: char_width * 2.2, // ~16-20px default
        }
    }
}

/// Sugiyama layered graph layout: assigns vertical rank and horizontal barycenter order.
#[derive(Debug)]
pub struct DagNode {
    pub id: u32,
    pub rank: u16,
    pub order: u16,
    pub x: f32,
    pub y: f32,
}

#[derive(Debug)]
pub struct DagEdge {
    pub from: u32,
    pub to: u32,
}

/// Assigns ranks to nodes using longest-path layering.
pub fn assign_ranks(nodes: &mut [DagNode], edges: &[DagEdge]) {
    // Zero in-degree roots start at rank 0
    let mut changed = true;
    let mut passes = 0;
    while changed && passes < nodes.len() {
        changed = false;
        passes += 1;
        for e in edges {
            let from_rank = nodes.iter().find(|n| n.id == e.from).map_or(0, |n| n.rank);
            if let Some(to_node) = nodes.iter_mut().find(|n| n.id == e.to) {
                if to_node.rank <= from_rank {
                    to_node.rank = from_rank + 1;
                    changed = true;
                }
            }
        }
    }
}

/// Compute node coordinates from ranks and layer orders.
pub fn layout_dag_coordinates(
    nodes: &mut [DagNode],
    layer_gap_y: f32,
    node_gap_x: f32,
    node_w: f32,
) {
    let max_rank = nodes.iter().map(|n| n.rank).max().unwrap_or(0);
    for r in 0..=max_rank {
        let count = nodes.iter().filter(|n| n.rank == r).count();
        let row_width = count as f32 * (node_w + node_gap_x) - node_gap_x;
        let start_x = -row_width * 0.5;

        let mut idx = 0;
        for n in nodes.iter_mut().filter(|n| n.rank == r) {
            n.x = start_x + (idx as f32) * (node_w + node_gap_x);
            n.y = (r as f32) * layer_gap_y;
            idx += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linear_scale() {
        let scale = LinearScale::new(0.0, 100.0, 0.0, 500.0);
        assert_eq!(scale.map(50.0), 250.0);
        assert_eq!(scale.map(0.0), 0.0);
        assert_eq!(scale.map(100.0), 500.0);

        let ticks = scale.compute_ticks(5);
        assert!(ticks.len() >= 4);
        assert_eq!(ticks[0], 0.0);
    }

    #[test]
    fn test_table_layout() {
        let lengths = [4, 10, 8, 20];
        let layout = TableLayout::compute(2, &lengths, 8.0, 4.0, 400.0);
        assert_eq!(layout.col_widths.len(), 2);
        assert!(layout.total_width <= 400.0);
    }
}
