//! Zero-copy Vega-Lite v5 / Plotly JSON chart synthesizer (bar, line, area, scatter, pie) to SVG.
//!
//! The spec is navigated in place as `&[u8]` (no DOM, no `serde`); series use bounded heap storage. Unsupported or missing inline data returns a typed error.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::suboptimal_flops,
    clippy::too_many_lines,
    reason = "chart geometry is bounded by MAX_POINTS and canvas size; plain arithmetic reads better than mul_add chains"
)]

use core::fmt::Write;

use crate::fmt::json;
use crate::{
    err::Error,
    scene::Scene,
    theme::{Palette, SemanticRole},
};

fn get<'a>(src: &'a [u8], key: &str) -> Option<&'a [u8]> {
    json::get_field(src, key.as_bytes())
}
fn items(raw: Option<&[u8]>) -> json::ArrayIter<'_> {
    json::iter_array(raw.unwrap_or(b"[]"))
}
fn num_of(raw: &[u8]) -> Option<f64> {
    json::parse_f64(raw).filter(|v| v.is_finite())
}
fn str_of(raw: Option<&[u8]>) -> &str {
    raw.and_then(|s| s.strip_prefix(b"\"")?.strip_suffix(b"\""))
        .and_then(|s| core::str::from_utf8(s).ok())
        .unwrap_or("")
}
fn push_xml_escaped(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
}

const MAX_POINTS: usize = 512;
const MAX_SERIES: usize = 4;
const W: f64 = 560.0;
const H: f64 = 320.0;
const ML: f64 = 64.0;
const MR: f64 = 24.0;
const MT: f64 = 44.0;
const MB: f64 = 52.0;

/// Text of a string-or-`{ "text": … }` value (Vega-Lite / Plotly titles).
fn title_of(v: Option<&[u8]>) -> &str {
    match v {
        Some(t) if t.first() == Some(&b'{') => str_of(get(t, "text")),
        t => str_of(t),
    }
}

// ── Chart model ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Bar,
    Line,
    Area,
    Point,
    Pie,
}

struct Chart<'a> {
    mark: Mark,
    title: &'a str,
    x_name: &'a str,
    y_name: &'a str,
    numeric_x: bool,
    labels: Vec<&'a str>,
    xs: [Vec<f64>; MAX_SERIES],
    marks: [Mark; MAX_SERIES],
    ys: [Vec<f64>; MAX_SERIES],
    n_series: usize,
    names: [&'a str; MAX_SERIES],
}

impl<'a> Chart<'a> {
    const fn new(mark: Mark) -> Self {
        Self {
            mark,
            title: "",
            x_name: "",
            y_name: "",
            numeric_x: false,
            labels: Vec::new(),
            xs: [const { Vec::new() }; MAX_SERIES],
            marks: [mark; MAX_SERIES],
            ys: [const { Vec::new() }; MAX_SERIES],
            n_series: 1,
            names: [""; MAX_SERIES],
        }
    }

    /// Appends an `(x token, y)` point; `x` is kept as a label and as a numeric hint.
    fn push(&mut self, series: usize, x: &'a [u8], y: f64) {
        let index = self.xs[series].len();
        if index == MAX_POINTS {
            return;
        }
        if series == 0 {
            let raw = core::str::from_utf8(x).unwrap_or("");
            self.labels.push(raw.trim_matches('"'));
        }
        self.xs[series].push(num_of(x).unwrap_or(index as f64));
        self.ys[series].push(y);
    }

    /// Numeric-x scale is valid only for continuous marks whose every x parses as a number.
    fn finish(&mut self, x_is_quantitative: bool) {
        self.numeric_x = x_is_quantitative
            && matches!(self.mark, Mark::Line | Mark::Area | Mark::Point)
            && self.labels.iter().all(|l| l.parse::<f64>().is_ok());
        if !self.numeric_x {
            for (i, x) in self.xs[0].iter_mut().enumerate() {
                *x = i as f64;
            }
        }
    }
}

fn vega_mark(m: Option<&[u8]>) -> Option<Mark> {
    let name = match m {
        Some(o) if o.first() == Some(&b'{') => str_of(get(o, "type")),
        m => str_of(m),
    };
    Some(match name {
        "bar" => Mark::Bar,
        "line" | "trail" => Mark::Line,
        "area" => Mark::Area,
        "point" | "circle" | "square" | "tick" => Mark::Point,
        "arc" => Mark::Pie,
        _ => return None,
    })
}

fn parse_vega(spec: &[u8]) -> (&str, Option<Chart<'_>>) {
    let title = match title_of(get(spec, "title")) {
        "" => str_of(get(spec, "description")),
        t => t,
    };
    let layer0 = items(get(spec, "layer")).next();
    let node = layer0.unwrap_or(spec);
    let Some(mark) = vega_mark(get(node, "mark").or_else(|| get(spec, "mark"))) else {
        return (title, None);
    };
    let enc = get(node, "encoding").or_else(|| get(spec, "encoding"));
    let channel = |name: &str| enc.and_then(|e| get(e, name));
    let field = |name: &str| str_of(channel(name).and_then(|c| get(c, "field")));
    let (mut xf, mut yf) = if mark == Mark::Pie {
        (field("color"), field("theta"))
    } else {
        (field("x"), field("y"))
    };
    if xf.is_empty() {
        xf = "x";
    }
    if yf.is_empty() {
        yf = "y";
    }
    let mut chart = Chart::new(mark);
    chart.title = title;
    chart.x_name = xf;
    chart.y_name = yf;
    let values = get(spec, "data")
        .and_then(|d| get(d, "values"))
        .or_else(|| get(node, "data").and_then(|d| get(d, "values")));
    let group_field = if mark == Mark::Pie {
        ""
    } else {
        field("color")
    };
    if group_field.is_empty() {
        for row in items(values) {
            if let (Some(x), Some(y)) = (get(row, xf), get(row, yf).and_then(num_of)) {
                chart.push(0, x, y);
            }
        }
    } else {
        chart.n_series = 0;
        for row in items(values) {
            let (Some(x), Some(y)) = (get(row, xf), get(row, yf).and_then(num_of)) else {
                continue;
            };
            let name = str_of(get(row, group_field));
            let series = if let Some(index) = chart.names[..chart.n_series]
                .iter()
                .position(|&n| n == name)
            {
                index
            } else {
                if chart.n_series == MAX_SERIES {
                    return (title, None);
                }
                let index = chart.n_series;
                chart.names[index] = name;
                chart.n_series += 1;
                index
            };
            chart.push(series, x, y);
        }
    }
    let quantitative = str_of(channel("x").and_then(|c| get(c, "type"))) == "quantitative";
    chart.finish(quantitative);
    (title, (!chart.labels.is_empty()).then_some(chart))
}

fn parse_plotly(spec: &[u8]) -> (&str, Option<Chart<'_>>) {
    let layout = get(spec, "layout");
    let title = title_of(layout.and_then(|l| get(l, "title")));
    let axis = |name: &str| {
        title_of(
            layout
                .and_then(|l| get(l, name))
                .and_then(|a| get(a, "title")),
        )
    };
    let mut chart: Option<Chart<'_>> = None;
    for trace in items(get(spec, "data")).take(MAX_SERIES) {
        let kind = str_of(get(trace, "type"));
        let name = str_of(get(trace, "name"));
        if kind == "pie" {
            let mut c = Chart::new(Mark::Pie);
            for (l, v) in items(get(trace, "labels")).zip(items(get(trace, "values"))) {
                if let Some(v) = num_of(v) {
                    c.push(0, l, v);
                }
            }
            c.title = title;
            c.finish(false);
            return (title, (!c.labels.is_empty()).then_some(c));
        }
        // Keep statistical and hierarchical traces unsupported rather than mislabeling them.
        let mark = match (kind, str_of(get(trace, "mode"))) {
            ("bar", _) => Mark::Bar,
            ("scatter" | "scattergl" | "", "markers") => Mark::Point,
            // Default Plotly trace type is scatter/line; empty type string means scatter.
            ("scatter" | "scattergl" | "", _) => Mark::Line,
            // Unknown trace types: skip only this trace, do not abort the whole spec.
            _ => continue,
        };
        let mark = if mark == Mark::Line && str_of(get(trace, "fill")) == "tozeroy" {
            Mark::Area
        } else {
            mark
        };
        let ys = items(get(trace, "y"));
        if let Some(c) = chart.as_mut() {
            // Additional series appended to the first plottable chart.
            if c.n_series < MAX_SERIES {
                let s = c.n_series;
                let mut xs = items(get(trace, "x"));
                for raw in ys.take(MAX_POINTS) {
                    let x = xs.next().unwrap_or_default();
                    if let Some(y) = num_of(raw) {
                        c.push(s, x, y);
                    }
                }
                if c.ys[s].is_empty() {
                    continue;
                }
                c.marks[s] = mark;
                c.names[s] = name;
                c.n_series += 1;
            }
            continue;
        }
        // First plottable trace: build the primary chart.
        let mut c = Chart::new(mark);
        let mut xs = items(get(trace, "x"));
        for raw in ys.take(MAX_POINTS) {
            let x = xs.next().unwrap_or_default();
            if let Some(y) = num_of(raw) {
                c.push(0, x, y);
            }
        }
        c.title = title;
        c.x_name = axis("xaxis");
        c.y_name = axis("yaxis");
        c.names[0] = name;
        c.finish(true);
        if !c.labels.is_empty() {
            chart = Some(c);
        }
    }
    (title, chart.filter(|c| !c.labels.is_empty()))
}

// ── SVG emission ────────────────────────────────────────────────────────────

fn push_num(out: &mut String, v: f64) {
    let start = out.len();
    let _ = write!(out, "{v:.3}");
    while out.len() > start && out.ends_with('0') {
        out.pop();
    }
    if out.ends_with('.') {
        out.pop();
    }
}

fn nice_ceil(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let mag = 10f64.powf(v.log10().floor());
    let f = v / mag;
    mag * if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    }
}

fn open_svg(out: &mut String, theme: &Palette, title: &str, fg: &str) {
    out.clear();
    let bg = theme.svg_surface_fill();
    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">
  <rect width="100%" height="100%" fill="{bg}" rx="6"/>
  <text x="{ML}" y="26" font-family="monospace" font-size="13" font-weight="bold" fill="{fg}">"#
    );
    push_xml_escaped(out, title);
    out.push_str("</text>\n");
}

fn text(out: &mut String, x: f64, y: f64, anchor: &str, fg: &str, s: &str) {
    let _ = write!(
        out,
        r#"  <text x="{x:.1}" y="{y:.1}" font-family="monospace" font-size="10" fill="{fg}" text-anchor="{anchor}">"#
    );
    push_xml_escaped(out, s);
    out.push_str("</text>\n");
}

fn render_pie(c: &Chart<'_>, theme: &Palette, fg: &str, out: &mut String) {
    let total: f64 = c.ys[0].iter().filter(|v| **v > 0.0).sum();
    let (cx, cy, r) = (W * 0.36, H * 0.54, (H - MT - 24.0) * 0.5);
    let mut angle = -core::f64::consts::FRAC_PI_2;
    for i in 0..c.labels.len() {
        let v = c.ys[0][i];
        if v <= 0.0 || total <= 0.0 {
            continue;
        }
        let color = theme.series_hex_str(i);
        let frac = v / total;
        let next = frac.mul_add(core::f64::consts::TAU, angle);
        if frac >= 0.9999 {
            let _ = writeln!(
                out,
                r#"  <circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" fill="{color}"/>"#
            );
        } else {
            let large = u8::from(frac > 0.5);
            let _ = writeln!(
                out,
                r#"  <path d="M{cx:.1} {cy:.1} L{:.1} {:.1} A{r:.1} {r:.1} 0 {large} 1 {:.1} {:.1} Z" fill="{color}" stroke="none" stroke-width="1"/>"#,
                r.mul_add(angle.cos(), cx),
                r.mul_add(angle.sin(), cy),
                r.mul_add(next.cos(), cx),
                r.mul_add(next.sin(), cy),
            );
        }
        angle = next;
        let ly = (i as f64).mul_add(16.0, MT + 8.0);
        let _ = writeln!(
            out,
            r#"  <rect x="{lx:.1}" y="{ly:.1}" width="10" height="10" fill="{color}" rx="2"/>"#,
            lx = W * 0.68
        );
        let mut label = String::new();
        label.push_str(c.labels[i]);
        label.push_str(" (");
        push_num(&mut label, frac * 100.0);
        label.push_str("%)");
        text(out, W.mul_add(0.68, 16.0), ly + 9.0, "start", fg, &label);
    }
}

fn render_heatmap(spec: &[u8], trace: &[u8], palette: &Palette) -> Result<String, Error> {
    // Bound both dimensions before retaining matrix values; never silently crop data.
    const LIMIT: usize = 64;
    let mut matrix = Vec::new();
    for row in items(get(trace, "z")).take(LIMIT + 1) {
        let values: Vec<_> = items(Some(row)).take(LIMIT + 1).map(num_of).collect();
        if values.is_empty() || values.len() > LIMIT {
            return Err(Error::JsonNav);
        }
        matrix.push(values);
    }
    let cols = matrix.first().ok_or(Error::JsonNav)?.len();
    if matrix.len() > LIMIT || matrix.iter().any(|row| row.len() != cols) {
        return Err(Error::JsonNav);
    }
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for value in matrix.iter().flatten().flatten() {
        min = min.min(*value);
        max = max.max(*value);
    }
    if !min.is_finite() {
        return Err(Error::JsonNav);
    }
    min = get(trace, "zmin").and_then(num_of).unwrap_or(min);
    max = get(trace, "zmax").and_then(num_of).unwrap_or(max);
    if max < min {
        return Err(Error::JsonNav);
    }
    let fg = palette.hex_str(SemanticRole::TextPrimary);
    let layout = get(spec, "layout");
    let title = title_of(layout.and_then(|l| get(l, "title")));
    let mut svg = String::new();
    open_svg(&mut svg, palette, title, fg);
    let x_labels: Vec<_> = items(get(trace, "x")).take(cols).collect();
    let y_labels: Vec<_> = items(get(trace, "y")).take(matrix.len()).collect();
    let reversed = str_of(
        layout
            .and_then(|l| get(l, "yaxis"))
            .and_then(|a| get(a, "autorange")),
    ) == "reversed";
    let top = str_of(
        layout
            .and_then(|l| get(l, "xaxis"))
            .and_then(|a| get(a, "side")),
    ) == "top";
    let cell_w = (W - ML - MR - 24.0) / cols as f64;
    let cell_h = (H - MT - MB) / matrix.len() as f64;
    let low = palette.role_color(SemanticRole::TextMuted).to_array();
    let high = palette.role_color(SemanticRole::Accent).to_array();
    for (row, values) in matrix.iter().enumerate() {
        let y = MT
            + (if reversed {
                row
            } else {
                matrix.len() - 1 - row
            }) as f64
                * cell_h;
        for (col, value) in values.iter().enumerate() {
            let Some(value) = value else { continue };
            let t = if max > min {
                ((value - min) / (max - min)).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let rgb: [u8; 3] = core::array::from_fn(|i| {
                (f64::from(low[i]) * (1.0 - t) + f64::from(high[i]) * t).round() as u8
            });
            let x = ML + col as f64 * cell_w;
            let _ = write!(
                svg,
                "<rect class=\"heatmap-cell\" x=\"{x:.2}\" y=\"{y:.2}\" width=\"{cell_w:.2}\" height=\"{cell_h:.2}\" fill=\"#{:02x}{:02x}{:02x}\"><title>",
                rgb[0], rgb[1], rgb[2]
            );
            push_num(&mut svg, *value);
            svg.push_str("</title></rect>");
        }
        let label = y_labels.get(row).map_or_else(
            || row.to_string(),
            |raw| {
                core::str::from_utf8(raw)
                    .unwrap_or("")
                    .trim_matches('"')
                    .to_owned()
            },
        );
        text(
            &mut svg,
            ML - 8.0,
            y + cell_h * 0.5 + 3.0,
            "end",
            fg,
            &label,
        );
    }
    for col in 0..cols {
        let label = x_labels.get(col).map_or_else(
            || col.to_string(),
            |raw| {
                core::str::from_utf8(raw)
                    .unwrap_or("")
                    .trim_matches('"')
                    .to_owned()
            },
        );
        text(
            &mut svg,
            ML + (col as f64 + 0.5) * cell_w,
            if top { MT - 8.0 } else { H - MB + 18.0 },
            "middle",
            fg,
            &label,
        );
    }
    // Color key uses the same theme gradient as the cells, never a plot background.
    if get(trace, "showscale") != Some(b"false") {
        for step in 0..32 {
            let t = f64::from(step) / 31.0;
            let rgb: [u8; 3] = core::array::from_fn(|i| {
                (f64::from(low[i]) * (1.0 - t) + f64::from(high[i]) * t).round() as u8
            });
            let y = MT + (1.0 - t) * (H - MT - MB - 4.0);
            let _ = write!(
                svg,
                "<rect x=\"{}\" y=\"{y:.2}\" width=\"8\" height=\"8\" fill=\"#{:02x}{:02x}{:02x}\"/>",
                W - MR - 12.0,
                rgb[0],
                rgb[1],
                rgb[2]
            );
        }
        text(
            &mut svg,
            W - MR - 8.0,
            MT - 8.0,
            "middle",
            fg,
            &format!("{max:.2}"),
        );
        text(
            &mut svg,
            W - MR - 8.0,
            H - MB + 18.0,
            "middle",
            fg,
            &format!("{min:.2}"),
        );
    }
    svg.push_str("</svg>");
    Ok(svg)
}

fn render_cartesian(c: &Chart<'_>, theme: &Palette, fg: &str, out: &mut String) {
    let (pw, ph) = (W - ML - MR, H - MT - MB);
    let max_y = c.ys[..c.n_series]
        .iter()
        .flatten()
        .copied()
        .fold(0.0f64, f64::max);
    let min_y = c.ys[..c.n_series]
        .iter()
        .flatten()
        .copied()
        .fold(0.0f64, f64::min);
    let top = nice_ceil(max_y);
    let bottom = if min_y < 0.0 { -nice_ceil(-min_y) } else { 0.0 };
    let span = (top - bottom).max(f64::EPSILON);
    let y_of = |v: f64| (1.0 - (v - bottom) / span).mul_add(ph, MT);

    let (x_min, x_max) = c.xs[..c.n_series]
        .iter()
        .flatten()
        .fold((f64::MAX, f64::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    let band = pw / c.labels.len() as f64;
    let x_of = |x: f64| {
        if c.mark == Mark::Bar || !c.numeric_x && c.labels.len() == 1 {
            ML + band * (x + 0.5)
        } else if c.numeric_x {
            ML + (x - x_min) / (x_max - x_min).max(f64::EPSILON) * pw
        } else {
            ML + x / (c.labels.len() - 1) as f64 * pw
        }
    };

    for t in 0..=4 {
        let v = (f64::from(t) / 4.0).mul_add(span, bottom);
        let y = y_of(v);
        let _ = writeln!(
            out,
            r#"  <line x1="{ML}" y1="{y:.1}" x2="{x2}" y2="{y:.1}" stroke="{fg}" stroke-opacity="0.18"/>"#,
            x2 = ML + pw
        );
        let mut label = String::new();
        push_num(&mut label, v);
        text(out, ML - 6.0, y + 3.0, "end", fg, &label);
    }
    let _ = writeln!(
        out,
        r#"  <line x1="{ML}" y1="{MT}" x2="{ML}" y2="{y2}" stroke="{fg}" stroke-opacity="0.6"/>
  <line x1="{ML}" y1="{y0:.1}" x2="{x2}" y2="{y0:.1}" stroke="{fg}" stroke-opacity="0.6"/>"#,
        y2 = MT + ph,
        y0 = y_of(0.0f64.max(bottom)),
        x2 = ML + pw
    );

    let step = c.labels.len().div_ceil(12);
    for i in (0..c.labels.len()).step_by(step) {
        let mut label = String::new();
        if c.numeric_x {
            push_num(&mut label, c.xs[0][i]);
        } else {
            label.push_str(c.labels[i]);
        }
        text(
            out,
            x_of(if c.numeric_x { c.xs[0][i] } else { i as f64 }),
            MT + ph + 16.0,
            "middle",
            fg,
            &label,
        );
    }
    text(out, ML + pw * 0.5, H - 10.0, "middle", fg, c.x_name);
    text(out, ML, MT - 8.0, "start", fg, c.y_name);

    for s in 0..c.n_series {
        let color = theme.series_hex_str(s);
        let ys = &c.ys[s];
        match c.marks[s] {
            Mark::Bar => {
                let bw = band * 0.8 / c.n_series as f64;
                for (i, &v) in ys.iter().enumerate() {
                    let x = (s as f64).mul_add(bw, band.mul_add(i as f64, ML) + band * 0.1);
                    let (y, y0) = (y_of(v.max(0.0)), y_of(v.min(0.0).max(bottom)));
                    let _ = writeln!(
                        out,
                        r#"  <rect x="{x:.1}" y="{y:.1}" width="{bw:.1}" height="{h:.1}" fill="{color}" rx="2"/>"#,
                        h = (y0 - y).max(1.0)
                    );
                }
            }
            Mark::Line | Mark::Area => {
                let mut pts = String::new();
                for (i, &v) in ys.iter().enumerate() {
                    let _ = write!(
                        pts,
                        "{:.1},{:.1} ",
                        x_of(if c.numeric_x { c.xs[s][i] } else { i as f64 }),
                        y_of(v)
                    );
                }
                if c.marks[s] == Mark::Area {
                    let _ = writeln!(
                        out,
                        r#"  <polygon points="{:.1},{y0:.1} {pts}{:.1},{y0:.1}" fill="{color}" fill-opacity="0.25"/>"#,
                        x_of(if c.numeric_x { c.xs[s][0] } else { 0.0 }),
                        x_of(if c.numeric_x {
                            c.xs[s][ys.len() - 1]
                        } else {
                            (ys.len() - 1) as f64
                        }),
                        y0 = y_of(0.0f64.max(bottom))
                    );
                }
                let _ = writeln!(
                    out,
                    r#"  <polyline points="{pts}" fill="none" stroke="{color}" stroke-width="2" stroke-linejoin="round"/>"#
                );
                for (i, &v) in ys.iter().enumerate().take(24) {
                    let _ = writeln!(
                        out,
                        r#"  <circle cx="{:.1}" cy="{:.1}" r="2.5" fill="{color}"/>"#,
                        x_of(if c.numeric_x { c.xs[s][i] } else { i as f64 }),
                        y_of(v)
                    );
                }
            }
            Mark::Point | Mark::Pie => {
                for (i, &v) in ys.iter().enumerate() {
                    let _ = writeln!(
                        out,
                        r#"  <circle cx="{:.1}" cy="{:.1}" r="4" fill="{color}" fill-opacity="0.85"/>"#,
                        x_of(if c.numeric_x { c.xs[s][i] } else { i as f64 }),
                        y_of(v)
                    );
                }
            }
        }
        if c.n_series > 1 {
            let lx = W - MR - 90.0;
            let ly = (s as f64).mul_add(14.0, MT - 2.0);
            let _ = writeln!(
                out,
                r#"  <rect x="{lx:.1}" y="{ly:.1}" width="9" height="9" fill="{color}" rx="2"/>"#
            );
            text(out, lx + 14.0, ly + 8.0, "start", fg, c.names[s]);
        }
    }
}

/// Compile supported inline chart data with the receiving palette.
///
/// # Errors
/// Returns `JsonNav` for unsupported or nonnumeric chart data.
pub fn parse_with_palette(src: &[u8], palette: &Palette, scene: &mut Scene) -> Result<(), Error> {
    let plotly = get(src, "data").is_some_and(|d| d.trim_ascii().starts_with(b"["));
    if plotly
        && let Some(trace) = items(get(src, "data")).next()
        && str_of(get(trace, "type")) == "heatmap"
    {
        if items(get(src, "data")).count() != 1 {
            return Err(Error::JsonNav);
        }
        let svg = render_heatmap(src, trace, palette)?;
        return crate::fmt::dispatch_with_palette(
            crate::fmt::Kind::Svg,
            svg.as_bytes(),
            palette,
            scene,
        );
    }
    let (title, chart) = if plotly {
        parse_plotly(src)
    } else {
        parse_vega(src)
    };
    let chart = chart.ok_or(Error::JsonNav)?;
    let mut svg = String::new();
    let fg = palette.hex_str(SemanticRole::TextPrimary);
    open_svg(
        &mut svg,
        palette,
        if title.is_empty() {
            if plotly { "Plotly" } else { "Vega-Lite" }
        } else {
            title
        },
        fg,
    );
    if chart.mark == Mark::Pie {
        render_pie(&chart, palette, fg, &mut svg);
    } else {
        render_cartesian(&chart, palette, fg, &mut svg);
    }
    svg.push_str("</svg>");
    crate::fmt::dispatch_with_palette(crate::fmt::Kind::Svg, svg.as_bytes(), palette, scene)
}

/// Compile using the default palette.
///
/// # Errors
/// Returns `JsonNav` for unsupported or missing inline data.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    parse_with_palette(src, &crate::theme::DARK, scene)
}
