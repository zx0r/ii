//! Zero-copy Graphviz DOT / GV tokenizer, layered DAG layout, and SVG + Scene emitters.

use crate::{
    err::Error,
    fmt::write_xml_escaped,
    scene::Scene,
    theme::{Palette, SemanticRole},
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
    w: f32,
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
        w: 60.0,
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
    horizontal: bool,
}

impl Graph {
    const fn new() -> Self {
        Self {
            nodes: [Node::EMPTY; MAX_NODES],
            node_len: 0,
            edges: [Edge { from: 0, to: 0 }; MAX_EDGES],
            edge_len: 0,
            horizontal: false,
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
            w: 60.0,
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
        while *pos < src.len()
            && (src[*pos] != b'"' || (*pos > start + 1 && src[*pos - 1] == b'\\'))
        {
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
            let mut from = lhs;
            while src[pos..].starts_with(b"->") || src[pos..].starts_with(b"--") {
                pos += 2;
                let Some(to) = next_ident(src, &mut pos) else {
                    break;
                };
                if let (Some(a), Some(b)) = (graph.intern(src, from), graph.intern(src, to)) {
                    graph.add_edge(a, b);
                }
                from = to;
                while src.get(pos).is_some_and(u8::is_ascii_whitespace) {
                    pos += 1;
                }
            }
            if src.get(pos) == Some(&b'[')
                && let Some(close) = memchr::memchr(b']', &src[pos..])
            {
                pos += close + 1;
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

    layout_graph(&mut graph, src);
    graph
}

fn layout_graph(graph: &mut Graph, src: &[u8]) {
    // Kahn ordering with a deterministic cycle break. Back edges are drawn but
    // never repeatedly increase every rank in a cycle.
    let mut done = [false; MAX_NODES];
    for _ in 0..graph.node_len {
        let candidate = (0..graph.node_len)
            .filter(|&i| !done[i])
            .find(|&i| {
                !graph.edges[..graph.edge_len]
                    .iter()
                    .any(|e| usize::from(e.to) == i && !done[usize::from(e.from)])
            })
            .or_else(|| (0..graph.node_len).find(|&i| !done[i]));
        let Some(i) = candidate else { break };
        done[i] = true;
        for edge in &graph.edges[..graph.edge_len] {
            let to = usize::from(edge.to);
            if usize::from(edge.from) == i && !done[to] {
                graph.nodes[to].layer = graph.nodes[to]
                    .layer
                    .max(graph.nodes[i].layer.saturating_add(1));
            }
        }
    }
    let horizontal = src.windows(7).any(|w| w == b"rankdir")
        && core::str::from_utf8(src).is_ok_and(|s| {
            s.split("rankdir").skip(1).any(|s| {
                s.trim_start()
                    .strip_prefix('=')
                    .is_some_and(|s| s.trim_start().starts_with("LR"))
            })
        });
    graph.horizontal = horizontal;
    let mut rank_widths = [0.0_f32; MAX_NODES];
    for node in &mut graph.nodes[..graph.node_len] {
        let chars = core::str::from_utf8(node.label(src)).map_or(1, |s| s.chars().count());
        node.w = f32::from(u16::try_from(chars).unwrap_or(u16::MAX))
            .mul_add(8.5, 32.0)
            .clamp(60.0, 300.0);
        rank_widths[usize::from(node.layer)] += if horizontal { 60.0 } else { node.w + 24.0 };
    }
    let widest = rank_widths.iter().copied().fold(0.0_f32, f32::max);
    let mut offsets = [0.0_f32; MAX_NODES];
    let mut rank_x = [20.0_f32; MAX_NODES];
    if horizontal {
        for layer in 1..MAX_NODES {
            let prev = graph.nodes[..graph.node_len]
                .iter()
                .filter(|n| usize::from(n.layer) == layer - 1)
                .map(|n| n.w)
                .fold(0.0_f32, f32::max);
            rank_x[layer] = rank_x[layer - 1] + prev + 48.0;
        }
    }
    for node in &mut graph.nodes[..graph.node_len] {
        let rank = usize::from(node.layer);
        let across = (widest - rank_widths[rank]).mul_add(0.5, offsets[rank]) + 20.0;
        node.x = if horizontal { rank_x[rank] } else { across };
        node.y = if horizontal {
            across
        } else {
            f32::from(node.layer).mul_add(84.0, 20.0)
        };
        offsets[rank] += if horizontal { 60.0 } else { node.w + 24.0 };
    }
}

/// Render DOT as one SVG, sharing geometry and labels with the Scene adapter.
pub fn render(src: &[u8], palette: &Palette, _width: u16, out: &mut Vec<u8>) {
    use std::fmt::Write;
    let graph = parse_graph(src);
    let body_w = graph.nodes[..graph.node_len]
        .iter()
        .map(|n| n.x + n.w + 20.0)
        .fold(1.0_f32, f32::max);
    let body_h = graph.nodes[..graph.node_len]
        .iter()
        .map(|n| n.y + 56.0)
        .fold(1.0_f32, f32::max);
    let has_return = graph.edges[..graph.edge_len]
        .iter()
        .any(|e| graph.nodes[usize::from(e.to)].layer <= graph.nodes[usize::from(e.from)].layer);
    let w = body_w
        + if has_return && !graph.horizontal {
            64.0
        } else {
            0.0
        };
    let h = body_h
        + if has_return && graph.horizontal {
            64.0
        } else {
            0.0
        };
    let accent = palette.series_hex_str(0);
    let line = palette.series_hex_str(1);
    let fg = palette.hex_str(SemanticRole::TextPrimary);
    let bg = palette.svg_surface_fill();
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\"><defs><marker id=\"arrow\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" markerWidth=\"6\" markerHeight=\"6\" orient=\"auto-start-reverse\"><path d=\"M0 1L10 5L0 9z\" fill=\"{line}\"/></marker></defs>"
    );
    let horizontal = graph.horizontal;
    for edge in &graph.edges[..graph.edge_len] {
        let a = graph.nodes[usize::from(edge.from)];
        let b = graph.nodes[usize::from(edge.to)];
        if b.layer <= a.layer {
            let (x1, y1, x2, y2, c1x, c1y, c2x, c2y) = if horizontal {
                let detour = body_h + 36.0;
                (
                    a.w.mul_add(0.5, a.x),
                    a.y + 36.0,
                    b.w.mul_add(0.5, b.x),
                    b.y + 36.0,
                    a.w.mul_add(0.5, a.x),
                    detour,
                    b.w.mul_add(0.5, b.x),
                    detour,
                )
            } else {
                let detour = body_w + 36.0;
                (
                    a.x + a.w,
                    a.y + 18.0,
                    b.x + b.w,
                    b.y + 18.0,
                    detour,
                    a.y + 18.0,
                    detour,
                    b.y + 18.0,
                )
            };
            let _ = write!(
                svg,
                "<path d=\"M{x1} {y1} C{c1x} {c1y} {c2x} {c2y} {x2} {y2}\" fill=\"none\" stroke=\"{line}\" stroke-width=\"2\" marker-end=\"url(#arrow)\"/>"
            );
            continue;
        }
        let (x1, y1, x2, y2) = if horizontal {
            (a.x + a.w, a.y + 18.0, b.x, b.y + 18.0)
        } else if a.y <= b.y {
            (
                a.w.mul_add(0.5, a.x),
                a.y + 36.0,
                b.w.mul_add(0.5, b.x),
                b.y,
            )
        } else {
            (
                a.w.mul_add(0.5, a.x),
                a.y,
                b.w.mul_add(0.5, b.x),
                b.y + 36.0,
            )
        };
        let (cx, cy) = (f32::midpoint(x1, x2), f32::midpoint(y1, y2));
        let (c1x, c1y, c2x, c2y) = if horizontal {
            (cx, y1, cx, y2)
        } else {
            (x1, cy, x2, cy)
        };
        let _ = write!(
            svg,
            "<path d=\"M{x1} {y1} C{c1x} {c1y} {c2x} {c2y} {x2} {y2}\" fill=\"none\" stroke=\"{line}\" stroke-width=\"2\" marker-end=\"url(#arrow)\"/>"
        );
    }
    out.extend_from_slice(svg.as_bytes());
    for n in &graph.nodes[..graph.node_len] {
        let (x, y, w) = (n.x, n.y, n.w);
        let cx = w.mul_add(0.5, x);
        let baseline = y + 22.0;
        out.extend_from_slice(format!("<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"36\" rx=\"5\" fill=\"{bg}\" stroke=\"{accent}\" stroke-width=\"2\"/><text x=\"{cx}\" y=\"{baseline}\" text-anchor=\"middle\" font-family=\"monospace\" font-size=\"12\" fill=\"{fg}\">").as_bytes());
        write_xml_escaped(n.label(src), out);
        out.extend_from_slice(b"</text>");
    }
    out.extend_from_slice(b"</svg>");
}

/// Compile the same DOT presentation through the shared SVG ingestion boundary.
///
/// # Errors
/// Returns SVG validation errors.
pub fn parse_with_palette(src: &[u8], palette: &Palette, scene: &mut Scene) -> Result<(), Error> {
    let mut svg = Vec::new();
    render(src, palette, 0, &mut svg);
    crate::fmt::dispatch_with_palette(crate::fmt::Kind::Svg, &svg, palette, scene)
}

/// Compile DOT with the default dark palette.
///
/// # Errors
/// Returns SVG validation errors.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    parse_with_palette(src, &crate::theme::DARK, scene)
}
