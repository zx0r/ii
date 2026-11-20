//! `Graphviz` DOT → layered Sugiyama SVG renderer.
//!
//! Bounded stack arrays, zero heap graphs.
//! Zero `core::fmt::Write` — all SVG emitted via `fmt::push_f64_*` / `push_u32`.

use crate::{
    err::Error,
    fmt::{push_f64_0, push_f64_1, trim_ascii, write_xml_escaped},
    scene::{GlyphRun, Scene},
    theme::{ColorToken, Idx, Palette},
};

const MAX_NODES: usize = 32;
const MAX_EDGES: usize = 64;

// ── DOT graph primitives ──────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Node<'a> {
    id:    &'a [u8],
    rank:  u8,
    order: u8,
}

#[derive(Clone, Copy)]
struct Edge {
    from: u8,
    to:   u8,
}

#[inline]
const fn trim_semi(mut s: &[u8]) -> &[u8] {
    s = trim_ascii(s);
    while let Some((&b, rest)) = s.split_first()  { if b == b';' { s = rest; } else { break; } }
    while let Some((&b, rest)) = s.split_last()   { if b == b';' { s = rest; } else { break; } }
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
    if id.is_empty() { return None; }
    for (i, node) in nodes.iter().enumerate().take(*n_nodes) {
        if node.id == id { return u8::try_from(i).ok(); }
    }
    if *n_nodes >= MAX_NODES { return None; }
    let idx = *n_nodes;
    nodes[idx] = Node { id, rank: 0, order: 0 };
    *n_nodes += 1;
    u8::try_from(idx).ok()
}

// ── SVG emitter (zero `core::fmt`) ───────────────────────────────────────────

/// Parse `Graphviz` DOT `src` and emit a layered DAG SVG into `out`.
pub fn render(src: &[u8], palette: &Palette, width: u16, out: &mut Vec<u8>) {
    let mut nodes: [Node<'_>; MAX_NODES] = [Node { id: b"", rank: 0, order: 0 }; MAX_NODES];
    let mut edges: [Edge; MAX_EDGES]     = [Edge { from: 0, to: 0 }; MAX_EDGES];
    let mut n_nodes = 0usize;
    let mut n_edges = 0usize;

    let body = match (
        src.iter().position(|&b| b == b'{'),
        src.iter().rposition(|&b| b == b'}'),
    ) {
        (Some(open), Some(close)) if open < close => &src[open + 1..close],
        _ => src,
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

        let sep: Option<&[u8]> = if stmt.windows(2).any(|w| w == b"->") {
            Some(b"->")
        } else if stmt.windows(2).any(|w| w == b"--") {
            Some(b"--")
        } else {
            None
        };

        if let Some(op) = sep {
            let mut prev: Option<u8> = None;
            let mut cur = 0usize;
            while cur <= stmt.len() {
                let next_op = stmt[cur..].windows(2).position(|w| w == op).map(|p| cur + p);
                let part = next_op.map_or(&stmt[cur..], |p| &stmt[cur..p]);
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
                match next_op {
                    Some(p) => cur = p + 2,
                    None    => break,
                }
            }
        } else {
            let id = clean_id(stmt);
            let _ = find_or_insert(&mut nodes, &mut n_nodes, id);
        }
    }

    // Topological ranks: bounded Bellman-Ford longest-path
    let mut max_rank = 0u8;
    for _ in 0..n_nodes {
        for e in edges.iter().take(n_edges) {
            let r_from = nodes[e.from as usize].rank;
            let r_to   = nodes[e.to   as usize].rank;
            if e.from != e.to && r_to <= r_from && r_from + 1 < MAX_NODES as u8 {
                nodes[e.to as usize].rank = r_from + 1;
                if r_from + 1 > max_rank { max_rank = r_from + 1; }
            }
        }
    }

    // Horizontal order within each rank
    let mut rank_counts = [0u8; MAX_NODES];
    for node in nodes.iter_mut().take(n_nodes) {
        let r    = node.rank as usize;
        node.order = rank_counts[r];
        rank_counts[r] += 1;
    }

    let w       = f64::from(width.max(240));
    let layers  = usize::from(max_rank) + 1;
    let layer_h = 72.0_f64;
    let h       = (layers as f64).mul_add(layer_h, 36.0).max(120.0);

    let bg       = palette.hex_str(Idx::BgSurface).as_bytes();
    let fg       = palette.hex_str(Idx::Fg).as_bytes();
    let border   = palette.hex_str(Idx::HeadingSub).as_bytes();
    let line_col = palette.hex_str(Idx::FgMuted).as_bytes();

    // <svg>
    out.extend_from_slice(b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"");
    push_f64_0(out, w);
    out.extend_from_slice(b"\" height=\"");
    push_f64_0(out, h);
    out.extend_from_slice(b"\" viewBox=\"0 0 ");
    push_f64_0(out, w);
    out.push(b' ');
    push_f64_0(out, h);
    out.extend_from_slice(b"\"><rect width=\"");
    push_f64_0(out, w);
    out.extend_from_slice(b"\" height=\"");
    push_f64_0(out, h);
    out.extend_from_slice(b"\" rx=\"6\" fill=\"");
    out.extend_from_slice(bg);
    out.extend_from_slice(b"\"/>");

    let node_pos = |node: &Node<'_>| -> (f64, f64) {
        let count = f64::from(rank_counts[node.rank as usize].max(1));
        let cx = w * (f64::from(node.order) + 0.5) / count;
        let cy = f64::mul_add(f64::from(node.rank), layer_h, 36.0);
        (cx, cy)
    };

    // Edges
    for e in edges.iter().take(n_edges) {
        let (x0, y0) = node_pos(&nodes[e.from as usize]);
        let (x1, y1) = node_pos(&nodes[e.to as usize]);
        out.extend_from_slice(b"<line x1=\"");
        push_f64_1(out, x0);
        out.extend_from_slice(b"\" y1=\"");
        push_f64_1(out, y0);
        out.extend_from_slice(b"\" x2=\"");
        push_f64_1(out, x1);
        out.extend_from_slice(b"\" y2=\"");
        push_f64_1(out, y1);
        out.extend_from_slice(b"\" stroke=\"");
        out.extend_from_slice(line_col);
        out.extend_from_slice(b"\" stroke-width=\"1.6\"/>");
    }

    // Nodes
    let (box_w, box_h) = (88.0_f64, 32.0_f64);
    for node in nodes.iter().take(n_nodes) {
        let (cx, cy) = node_pos(node);
        let rx = f64::mul_add(box_w, -0.5, cx);
        let ry = f64::mul_add(box_h, -0.5, cy);

        out.extend_from_slice(b"<rect x=\"");
        push_f64_1(out, rx);
        out.extend_from_slice(b"\" y=\"");
        push_f64_1(out, ry);
        out.extend_from_slice(b"\" width=\"88\" height=\"32\" rx=\"5\" fill=\"");
        out.extend_from_slice(bg);
        out.extend_from_slice(b"\" stroke=\"");
        out.extend_from_slice(border);
        out.extend_from_slice(b"\" stroke-width=\"1.8\"/><text x=\"");
        push_f64_1(out, cx);
        out.extend_from_slice(b"\" y=\"");
        push_f64_1(out, cy + 4.0);
        out.extend_from_slice(b"\" fill=\"");
        out.extend_from_slice(fg);
        out.extend_from_slice(b"\" font-family=\"sans-serif\" font-size=\"12\" text-anchor=\"middle\">");
        write_xml_escaped(out, node.id);
        out.extend_from_slice(b"</text>");
    }

    out.extend_from_slice(b"</svg>");
}

// ── Scene ingest ──────────────────────────────────────────────────────────────

/// Parse DOT `src`, emit SVG, and store it as a `GlyphRun` SVG blob in `scene`.
///
/// # Errors
///
/// Returns `Err` on malformed input (currently infallible).
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    let svg_start = u32::try_from(scene.text_pool.len()).unwrap_or(u32::MAX);
    // We need Metrics to get width; use 800px as default.
    render(src, &crate::theme::DARK, 800, &mut scene.text_pool);
    let svg_len = scene.text_pool.len() - usize::try_from(svg_start).unwrap_or(0);
    if svg_len == 0 { return Ok(()); }

    let (svg_w, svg_h) = crate::fmt::svg::extract_svg_dimensions(
        &scene.text_pool[usize::try_from(svg_start).unwrap_or(0)..],
    );

    scene.glyphs.push(GlyphRun {
        x: 0.0,
        y: scene.height,
        text_off: svg_start,
        text_len: u16::try_from(svg_len.min(usize::from(u16::MAX))).unwrap_or(u16::MAX),
        font_size: 0,
        color: ColorToken::FG_PRIMARY,
        flags: 0x80, // SVG blob marker
        _pad: [0; 2],
    });

    scene.height += svg_h;
    if svg_w > scene.width { scene.width = svg_w; }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::DARK;

    #[test]
    fn dot_dag_renders_ordered_ranks() {
        let src = b"digraph G { A -> B; B -> C; }";
        let mut out = Vec::new();
        render(src, &DARK, 360, &mut out);
        let s = core::str::from_utf8(&out).unwrap();
        assert!(s.starts_with("<svg"));
        assert!(s.contains(">A<"));
        assert!(s.contains(">B<"));
        assert!(s.contains(">C<"));
        assert!(s.ends_with("</svg>"));
    }

    #[test]
    fn parse_into_scene_builds_svg_glyph() {
        let src = b"digraph { A -> B; }";
        let mut scene = Scene::with_capacity(16, 512);
        parse_into_scene(src, &mut scene).unwrap();
        assert_eq!(scene.glyphs.len(), 1);
        assert_eq!(scene.glyphs[0].flags, 0x80);
    }
}
