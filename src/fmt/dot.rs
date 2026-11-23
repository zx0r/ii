//! Zero-copy Graphviz DOT / GV tokenizer, layered DAG layout, and SVG + Scene emitters.

use crate::{
    err::Error,
    fmt::{push_f64_1, push_u32, write_xml_escaped},
    scene::{Rect, Scene},
    theme::{ColorToken, Palette, SemanticRole},
};

const MAX_NODES: usize = 128;
const MAX_EDGES: usize = 256;

#[derive(Clone, Copy)]
struct Node {
    id_start: u32,
    id_end: u32,
    label_start: u32,
    label_end: u32,
    x: f32,
    y: f32,
    layer: u8,
}
const _: () = assert!(size_of::<[Node; MAX_NODES]>() <= 4096);

impl Node {
    const EMPTY: Self = Self {
        id_start: 0,
        id_end: 0,
        label_start: 0,
        label_end: 0,
        x: 0.0,
        y: 0.0,
        layer: 0,
    };

    #[inline]
    fn label(self, src: &[u8]) -> &[u8] {
        src.get(self.label_start as usize..self.label_end as usize)
            .unwrap_or(b"")
    }
}

#[derive(Clone, Copy)]
struct Edge {
    from: u8,
    to: u8,
}

struct Graph {
    nodes: [Node; MAX_NODES],
    node_len: usize,
    edges: [Edge; MAX_EDGES],
    edge_len: usize,
}

impl Graph {
    const fn new() -> Self {
        Self {
            nodes: [Node::EMPTY; MAX_NODES],
            node_len: 0,
            edges: [Edge { from: 0, to: 0 }; MAX_EDGES],
            edge_len: 0,
        }
    }

    fn intern(&mut self, src: &[u8], span: (usize, usize)) -> Option<u8> {
        let id = src.get(span.0..span.1)?;
        for (idx, cur) in self.nodes.iter().enumerate().take(self.node_len) {
            if src.get(cur.id_start as usize..cur.id_end as usize) == Some(id) {
                return u8::try_from(idx).ok();
            }
        }
        if self.node_len >= MAX_NODES {
            return None;
        }
        let (Ok(start), Ok(end)) = (u32::try_from(span.0), u32::try_from(span.1)) else {
            return None;
        };
        let idx = self.node_len;
        self.nodes[idx] = Node {
            id_start: start,
            id_end: end,
            label_start: start,
            label_end: end,
            x: 0.0,
            y: 0.0,
            layer: 0,
        };
        self.node_len += 1;
        u8::try_from(idx).ok()
    }

    const fn add_edge(&mut self, from: u8, to: u8) {
        if self.edge_len < MAX_EDGES {
            self.edges[self.edge_len] = Edge { from, to };
            self.edge_len += 1;
        }
    }
}

#[inline]
fn next_ident(src: &[u8], pos: &mut usize) -> Option<(usize, usize)> {
    while let Some(&b) = src.get(*pos) {
        if b.is_ascii_whitespace() || matches!(b, b';' | b',') {
            *pos += 1;
        } else {
            break;
        }
    }
    let start = *pos;
    if src.get(start) == Some(&b'"') {
        *pos += 1;
        while *pos < src.len() && src[*pos] != b'"' {
            *pos += 1;
        }
        let inner = (start + 1, *pos);
        if *pos < src.len() {
            *pos += 1;
        }
        return Some(inner);
    }
    while let Some(&b) = src.get(*pos) {
        if b.is_ascii_alphanumeric() || b == b'_' {
            *pos += 1;
        } else {
            break;
        }
    }
    (*pos > start).then_some((start, *pos))
}

fn parse_graph(src: &[u8]) -> Graph {
    let mut graph = Graph::new();
    let mut pos = memchr::memchr(b'{', src).map_or(0, |off| off + 1);

    while pos < src.len() {
        while let Some(&b) = src.get(pos) {
            if b.is_ascii_whitespace() || matches!(b, b';' | b',') {
                pos += 1;
            } else if b == b'/' && src.get(pos + 1) == Some(&b'/') {
                pos = memchr::memchr(b'\n', &src[pos..]).map_or(src.len(), |nl| pos + nl + 1);
            } else {
                break;
            }
        }
        if pos >= src.len() || src[pos] == b'}' {
            break;
        }
        let Some(lhs) = next_ident(src, &mut pos) else {
            pos += 1;
            continue;
        };
        if matches!(
            &src[lhs.0..lhs.1],
            b"graph" | b"node" | b"edge" | b"subgraph" | b"rankdir"
        ) {
            pos = memchr::memchr2(b';', b'\n', &src[pos..]).map_or(src.len(), |off| pos + off + 1);
            continue;
        }

        while pos < src.len() && src[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if src[pos..].starts_with(b"->") || src[pos..].starts_with(b"--") {
            pos += 2;
            if let Some(rhs) = next_ident(src, &mut pos)
                && let (Some(a), Some(b)) = (graph.intern(src, lhs), graph.intern(src, rhs))
            {
                graph.add_edge(a, b);
            }
        } else if let Some(idx) = graph.intern(src, lhs)
            && src.get(pos) == Some(&b'[')
            && let Some(close) = memchr::memchr(b']', &src[pos..])
        {
            let attrs = &src[pos + 1..pos + close];
            if let Some(lpos) = attrs.windows(6).position(|w| w == b"label=") {
                let mut v_start = pos + 1 + lpos + 6;
                let mut v_end = pos + close;
                while v_start < v_end && src[v_start].is_ascii_whitespace() {
                    v_start += 1;
                }
                while v_end > v_start && src[v_end - 1].is_ascii_whitespace() {
                    v_end -= 1;
                }
                if src.get(v_start) == Some(&b'"') {
                    v_start += 1;
                    v_end =
                        memchr::memchr(b'"', &src[v_start..v_end]).map_or(v_end, |q| v_start + q);
                }
                if let (Ok(ls), Ok(le)) = (u32::try_from(v_start), u32::try_from(v_end)) {
                    graph.nodes[usize::from(idx)].label_start = ls;
                    graph.nodes[usize::from(idx)].label_end = le;
                }
            }
            pos += close + 1;
        }
    }

    for _ in 0..graph.node_len {
        let mut changed = false;
        for edge in &graph.edges[..graph.edge_len] {
            let next_layer = graph.nodes[usize::from(edge.from)].layer.saturating_add(1);
            let dst = &mut graph.nodes[usize::from(edge.to)];
            if next_layer > dst.layer && usize::from(next_layer) < graph.node_len {
                dst.layer = next_layer;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut per_layer = [0u16; MAX_NODES];
    for idx in 0..graph.node_len {
        let layer = usize::from(graph.nodes[idx].layer);
        let slot = per_layer[layer];
        per_layer[layer] += 1;
        graph.nodes[idx].x = f32::from(slot).mul_add(140.0, 24.0);
        graph.nodes[idx].y = f32::from(graph.nodes[idx].layer).mul_add(72.0, 24.0);
    }
    graph
}

/// Render DOT graph directly into standalone SVG bytes inside `out`.
pub fn render(src: &[u8], palette: &Palette, width: u16, out: &mut Vec<u8>) {
    let graph = parse_graph(src);
    let max_x = graph.nodes[..graph.node_len]
        .iter()
        .map(|n| n.x + 120.0)
        .fold(f32::from(width.min(320)), f32::max);
    let max_y = graph.nodes[..graph.node_len]
        .iter()
        .map(|n| n.y + 52.0)
        .fold(120.0_f32, f32::max);

    out.extend_from_slice(b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"");
    push_f64_1(out, f64::from(max_x));
    out.extend_from_slice(b"\" height=\"");
    push_f64_1(out, f64::from(max_y));
    out.extend_from_slice(b"\">");

    let stroke = palette.series_hex_str(0).as_bytes();
    let fill = b"none";
    let text_col = palette.hex_str(SemanticRole::TextPrimary).as_bytes();

    for edge in &graph.edges[..graph.edge_len] {
        let (a, b) = (
            graph.nodes[usize::from(edge.from)],
            graph.nodes[usize::from(edge.to)],
        );
        out.extend_from_slice(b"<line x1=\"");
        push_f64_1(out, f64::from(a.x + 56.0));
        out.extend_from_slice(b"\" y1=\"");
        push_f64_1(out, f64::from(a.y + 32.0));
        out.extend_from_slice(b"\" x2=\"");
        push_f64_1(out, f64::from(b.x + 56.0));
        out.extend_from_slice(b"\" y2=\"");
        push_f64_1(out, f64::from(b.y));
        out.extend_from_slice(b"\" stroke=\"");
        out.extend_from_slice(stroke);
        out.extend_from_slice(b"\" stroke-width=\"1.5\"/>");
    }

    for node in &graph.nodes[..graph.node_len] {
        out.extend_from_slice(b"<rect x=\"");
        push_f64_1(out, f64::from(node.x));
        out.extend_from_slice(b"\" y=\"");
        push_f64_1(out, f64::from(node.y));
        out.extend_from_slice(b"\" width=\"112\" height=\"32\" rx=\"4\" fill=\"");
        out.extend_from_slice(fill);
        out.extend_from_slice(b"\" stroke=\"");
        out.extend_from_slice(stroke);
        out.extend_from_slice(b"\"/><text x=\"");
        push_f64_1(out, f64::from(node.x + 8.0));
        out.extend_from_slice(b"\" y=\"");
        push_f64_1(out, f64::from(node.y + 20.0));
        out.extend_from_slice(b"\" fill=\"");
        out.extend_from_slice(text_col);
        out.extend_from_slice(b"\" font-size=\"");
        push_u32(out, 12);
        out.extend_from_slice(b"\">");
        write_xml_escaped(node.label(src), out);
        out.extend_from_slice(b"</text>");
    }
    out.extend_from_slice(b"</svg>");
}

/// Emit DOT/GV graph natively as `Rect` + `Path` + `GlyphRun` primitives into `Scene`.
///
/// # Errors
/// Returns `Ok(())` on valid or empty DOT graphs.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    let graph = parse_graph(src);
    let base_y = scene.height;
    let mut max_w = 0.0_f32;
    let mut max_h = 0.0_f32;

    for edge in &graph.edges[..graph.edge_len] {
        let (a, b) = (
            graph.nodes[usize::from(edge.from)],
            graph.nodes[usize::from(edge.to)],
        );
        scene.push_series_line(
            &[
                [a.x + 56.0, base_y + a.y + 32.0],
                [b.x + 56.0, base_y + b.y],
            ],
            ColorToken::BORDER,
            2,
        );
    }
    for node in &graph.nodes[..graph.node_len] {
        scene.push_rect(Rect {
            x: node.x,
            y: base_y + node.y,
            w: 112.0,
            h: 32.0,
            color: ColorToken::BG_SURFACE,
            radius: 4,
            stroke_width: 1,
            flags: Rect::FILL | Rect::STROKE,
        });
        let label = core::str::from_utf8(node.label(src)).unwrap_or("");
        scene.push_text(
            node.x + 8.0,
            base_y + node.y + 20.0,
            label,
            ColorToken::FG_PRIMARY,
            12,
        );
        max_w = max_w.max(node.x + 128.0);
        max_h = max_h.max(node.y + 48.0);
    }
    scene.width = scene.width.max(max_w);
    scene.height = base_y + max_h;
    Ok(())
}
