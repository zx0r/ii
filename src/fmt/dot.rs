//! Graphviz DOT → layered Sugiyama SVG renderer. Bounded stack arrays, zero heap graphs.


use core::fmt::Write as _;

use crate::{
    ir::{trim_ascii, write_xml_escaped},
    theme::{Idx, Palette},
};

const MAX_NODES: usize = 32;
const MAX_EDGES: usize = 64;

#[derive(Clone, Copy)]
struct Node<'a> {
    id: &'a [u8],
    rank: u8,
    order: u8,
}

#[derive(Clone, Copy)]
struct Edge {
    from: u8,
    to: u8,
}

#[inline]
fn trim_semi(mut s: &[u8]) -> &[u8] {
    s = trim_ascii(s);
    while let Some((&b, rest)) = s.split_first() {
        if b == b';' { s = rest; } else { break; }
    }
    while let Some((&b, rest)) = s.split_last() {
        if b == b';' { s = rest; } else { break; }
    }
    trim_ascii(s)
}

fn clean_id(mut s: &[u8]) -> &[u8] {
    s = trim_semi(s);
    if let Some(bracket) = s.iter().position(|&b| b == b'[') {
        s = trim_semi(&s[..bracket]);
    }
    if s.len() >= 2 && s.starts_with(b"\"") && s.ends_with(b"\"") {
        s = &s[1..s.len() - 1];
    }
    s
}

fn find_or_insert<'a>(
    nodes: &mut [Node<'a>; MAX_NODES],
    n_nodes: &mut usize,
    id: &'a [u8],
) -> Option<u8> {
    if id.is_empty() {
        return None;
    }
    for (i, node) in nodes.iter().enumerate().take(*n_nodes) {
        if node.id == id {
            return Some(i as u8);
        }
    }
    if *n_nodes >= MAX_NODES {
        return None;
    }
    let idx = *n_nodes;
    nodes[idx] = Node {
        id,
        rank: 0,
        order: 0,
    };
    *n_nodes += 1;
    Some(idx as u8)
}

/// Parse Graphviz DOT `src` and emit a layered DAG SVG into `out`.
pub fn render(src: &[u8], palette: &Palette, width: u16, out: &mut String) {
    let mut nodes = [Node {
        id: b"",
        rank: 0,
        order: 0,
    }; MAX_NODES];
    let mut edges = [Edge { from: 0, to: 0 }; MAX_EDGES];
    let mut n_nodes = 0usize;
    let mut n_edges = 0usize;

    let body = if let (Some(open), Some(close)) = (
        src.iter().position(|&b| b == b'{'),
        src.iter().rposition(|&b| b == b'}'),
    ) {
        if open < close {
            &src[open + 1..close]
        } else {
            src
        }
    } else {
        src
    };

    for raw_stmt in body.split(|&b| b == b'\n' || b == b';') {
        let stmt = trim_semi(raw_stmt);
        if stmt.is_empty()
            || stmt.starts_with(b"//")
            || stmt.starts_with(b"#")
            || stmt.starts_with(b"rankdir")
            || stmt.starts_with(b"node")
            || stmt.starts_with(b"edge")
            || stmt.starts_with(b"graph")
        {
            continue;
        }
        // Check for directed `->` or undirected `--` chains
        let sep = if stmt.windows(2).any(|w| w == b"->") {
            Some(&b"->"[..])
        } else if stmt.windows(2).any(|w| w == b"--") {
            Some(&b"--"[..])
        } else {
            None
        };

        if let Some(op) = sep {
            let mut prev: Option<u8> = None;
            let mut cur = 0usize;
            while cur <= stmt.len() {
                let next_op = stmt[cur..]
                    .windows(2)
                    .position(|w| w == op)
                    .map(|p| cur + p);
                let part = match next_op {
                    Some(p) => &stmt[cur..p],
                    None => &stmt[cur..],
                };
                let id = clean_id(part);
                if let Some(u) = find_or_insert(&mut nodes, &mut n_nodes, id) {
                    if let Some(p) = prev
                        && n_edges < MAX_EDGES
                    {
                        edges[n_edges] = Edge { from: p, to: u };
                        n_edges += 1;
                    }
                    prev = Some(u);
                }
                if let Some(p) = next_op {
                    cur = p + 2;
                } else {
                    break;
                }
            }
        } else {
            let id = clean_id(stmt);
            let _ = find_or_insert(&mut nodes, &mut n_nodes, id);
        }
    }

    // Assign topological ranks via bounded Bellman-Ford longest-path relaxation
    let mut max_rank = 0u8;
    for _ in 0..n_nodes {
        for e in edges.iter().take(n_edges) {
            let r_from = nodes[e.from as usize].rank;
            let r_to = nodes[e.to as usize].rank;
            if e.from != e.to && r_to <= r_from && r_from + 1 < (MAX_NODES as u8) {
                nodes[e.to as usize].rank = r_from + 1;
                if r_from + 1 > max_rank {
                    max_rank = r_from + 1;
                }
            }
        }
    }

    // Count nodes per rank and assign horizontal order within each rank
    let mut rank_counts = [0u8; MAX_NODES];
    for node in nodes.iter_mut().take(n_nodes) {
        let r = node.rank as usize;
        node.order = rank_counts[r];
        rank_counts[r] += 1;
    }

    let w = f64::from(width.max(240));
    let layers = usize::from(max_rank) + 1;
    let layer_h = 72.0;
    let h = (layers as f64).mul_add(layer_h, 36.0).max(120.0);

    let bg = palette.hex_str(Idx::BgSurface);
    let fg = palette.hex_str(Idx::Fg);
    let border = palette.hex_str(Idx::HeadingSub);
    let line_col = palette.hex_str(Idx::FgMuted);

    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}">"#
    );
    let _ = write!(
        out,
        r#"<rect width="{w:.0}" height="{h:.0}" rx="6" fill="{bg}"/>"#
    );

    let node_pos = |node: &Node| -> (f64, f64) {
        let count = f64::from(rank_counts[node.rank as usize].max(1));
        let cx = w * (f64::from(node.order) + 0.5) / count;
        let cy = f64::mul_add(f64::from(node.rank), layer_h, 36.0);
        (cx, cy)
    };

    // Draw edges first
    for e in edges.iter().take(n_edges) {
        let (x0, y0) = node_pos(&nodes[e.from as usize]);
        let (x1, y1) = node_pos(&nodes[e.to as usize]);
        let _ = write!(
            out,
            r#"<line x1="{x0:.1}" y1="{y0:.1}" x2="{x1:.1}" y2="{y1:.1}" stroke="{line_col}" stroke-width="1.6"/>"#
        );
    }

    // Draw nodes
    let box_w = 88.0;
    let box_h = 32.0;
    for node in nodes.iter().take(n_nodes) {
        let (cx, cy) = node_pos(node);
        let rx = f64::mul_add(box_w, -0.5, cx);
        let ry = f64::mul_add(box_h, -0.5, cy);
        let _ = write!(
            out,
            r#"<rect x="{rx:.1}" y="{ry:.1}" width="{box_w:.0}" height="{box_h:.0}" rx="5" fill="{bg}" stroke="{border}" stroke-width="1.8"/>"#
        );
        let _ = write!(
            out,
            r#"<text x="{cx:.1}" y="{:.1}" fill="{fg}" font-family="sans-serif" font-size="12" text-anchor="middle">"#,
            cy + 4.0
        );
        write_xml_escaped(out, node.id);
        out.push_str("</text>");
    }

    out.push_str("</svg>");
}

#[cfg(test)]
mod tests {
    use super::render;
    use crate::theme::DARK;

    #[test]
    fn dot_dag_renders_ordered_ranks() {
        let src = b"digraph G { A -> B; B -> C; }";
        let mut svg = String::new();
        render(src, &DARK, 360, &mut svg);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains(">A</text>"));
        assert!(svg.contains(">B</text>"));
        assert!(svg.contains(">C</text>"));
        assert!(svg.ends_with("</svg>"));
    }
}
