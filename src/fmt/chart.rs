//! Vega-Lite and Plotly JSON → themed SVG chart renderer. Zero heap allocations beyond `out`.


use core::fmt::Write as _;

use crate::{
    ir::write_xml_escaped,
    json::Nav,
    theme::{Idx, Palette},
};

const MAX_POINTS: usize = 64;

#[derive(Clone, Copy)]
struct Datum<'a> {
    label: &'a [u8],
    y: f64,
}

fn render_svg_chart(
    mark: &[u8],
    title: Option<&[u8]>,
    pts: &[Datum<'_>],
    palette: &Palette,
    width: u16,
    out: &mut String,
) {
    let w = f64::from(width.max(240));
    let h = (w * 0.55).clamp(180.0, 420.0);
    let bg = palette.hex_str(Idx::BgSurface);
    let fg = palette.hex_str(Idx::Fg);
    let muted = palette.hex_str(Idx::FgMuted);
    let grid = palette.hex_str(Idx::GridLine);

    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}">"#
    );
    let _ = write!(
        out,
        r#"<rect width="{w:.0}" height="{h:.0}" rx="6" fill="{bg}"/>"#
    );

    let top_pad = if let Some(t) = title {
        let _ = write!(
            out,
            r#"<text x="{:.1}" y="24" fill="{fg}" font-family="sans-serif" font-size="14" font-weight="bold" text-anchor="middle">"#,
            w * 0.5
        );
        write_xml_escaped(out, t);
        out.push_str("</text>");
        40.0
    } else {
        20.0
    };

    if pts.is_empty() {
        out.push_str("</svg>");
        return;
    }

    let left = 48.0;
    let right = w - 20.0;
    let bottom = h - 32.0;
    let plot_w = (right - left).max(1.0);
    let plot_h = (bottom - top_pad).max(1.0);

    if mark.eq_ignore_ascii_case(b"arc") || mark.eq_ignore_ascii_case(b"pie") {
        let total: f64 = pts.iter().map(|d| d.y.max(0.0)).sum();
        let cx = w * 0.5;
        let cy = f64::mul_add(plot_h, 0.5, top_pad);
        let r = plot_h.min(plot_w) * 0.38;
        if total > 0.0 {
            let mut angle = -core::f64::consts::FRAC_PI_2;
            for (idx, d) in pts.iter().enumerate() {
                let frac = (d.y.max(0.0)) / total;
                let sweep = frac * core::f64::consts::TAU;
                let end = angle + sweep;
                let x0 = f64::mul_add(r, angle.cos(), cx);
                let y0 = f64::mul_add(r, angle.sin(), cy);
                let x1 = f64::mul_add(r, end.cos(), cx);
                let y1 = f64::mul_add(r, end.sin(), cy);
                let large = u8::from(sweep > core::f64::consts::PI);
                let col = palette.series_hex_str(idx);
                let _ = write!(
                    out,
                    r#"<path d="M {cx:.2} {cy:.2} L {x0:.2} {y0:.2} A {r:.2} {r:.2} 0 {large} 1 {x1:.2} {y1:.2} Z" fill="{col}" stroke="{bg}" stroke-width="1.5"/>"#
                );
                angle = end;
            }
        }
        out.push_str("</svg>");
        return;
    }

    let mut min_y = 0.0_f64;
    let mut max_y = f64::MIN;
    for d in pts {
        if d.y < min_y {
            min_y = d.y;
        }
        if d.y > max_y {
            max_y = d.y;
        }
    }
    if (max_y - min_y).abs() < 1e-9 {
        max_y = min_y + 1.0;
    }

    // Horizontal grid lines
    for step in 0..=4 {
        let ratio = f64::from(step) / 4.0;
        let gy = f64::mul_add(ratio, -plot_h, bottom);
        let val = f64::mul_add(ratio, max_y - min_y, min_y);
        let _ = write!(
            out,
            r#"<line x1="{left:.1}" y1="{gy:.1}" x2="{right:.1}" y2="{gy:.1}" stroke="{grid}" stroke-width="1" stroke-dasharray="3,3"/>"#
        );
        let _ = write!(
            out,
            r#"<text x="{:.1}" y="{:.1}" fill="{muted}" font-family="monospace" font-size="10" text-anchor="end">{val:.0}</text>"#,
            left - 6.0,
            gy + 3.0
        );
    }

    let n = pts.len();
    let is_line = mark.eq_ignore_ascii_case(b"line") || mark.eq_ignore_ascii_case(b"area");
    let is_point = mark.eq_ignore_ascii_case(b"point")
        || mark.eq_ignore_ascii_case(b"circle")
        || mark.eq_ignore_ascii_case(b"scatter");

    if is_line || is_point {
        let col = palette.series_hex_str(0);
        if is_line && n >= 2 {
            out.push_str(r#"<polyline fill="none" stroke=""#);
            out.push_str(col);
            out.push_str(r#"" stroke-width="2.2" points=""#);
            for (idx, d) in pts.iter().enumerate() {
                let px = ((idx as f64) / ((n - 1).max(1) as f64)).mul_add(plot_w, left);
                let py = ((d.y - min_y) / (max_y - min_y)).mul_add(-plot_h, bottom);
                let _ = write!(out, "{px:.2},{py:.2} ");
            }
            out.push_str(r#""/>"#);
        }
        for (idx, d) in pts.iter().enumerate() {
            let px = if n > 1 {
                ((idx as f64) / ((n - 1) as f64)).mul_add(plot_w, left)
            } else {
                f64::mul_add(plot_w, 0.5, left)
            };
            let py = ((d.y - min_y) / (max_y - min_y)).mul_add(-plot_h, bottom);
            let _ = write!(
                out,
                r#"<circle cx="{px:.2}" cy="{py:.2}" r="3.5" fill="{col}"/>"#
            );
        }
    } else {
        // Default bar chart
        let slot = plot_w / (n as f64);
        let bar_w = (slot * 0.68).max(2.0);
        let zero_y = ((0.0 - min_y) / (max_y - min_y))
            .clamp(0.0, 1.0)
            .mul_add(-plot_h, bottom);
        for (idx, d) in pts.iter().enumerate() {
            let x = (slot - bar_w).mul_add(0.5, (idx as f64).mul_add(slot, left));
            let y_val = ((d.y - min_y) / (max_y - min_y)).mul_add(-plot_h, bottom);
            let top = y_val.min(zero_y);
            let bh = (y_val - zero_y).abs().max(1.0);
            let col = palette.series_hex_str(idx);
            let _ = write!(
                out,
                r#"<rect x="{x:.2}" y="{top:.2}" width="{bar_w:.2}" height="{bh:.2}" rx="2" fill="{col}"/>"#
            );
            if !d.label.is_empty() {
                let lx = bar_w.mul_add(0.5, x);
                let _ = write!(
                    out,
                    r#"<text x="{lx:.2}" y="{:.1}" fill="{muted}" font-family="sans-serif" font-size="10" text-anchor="middle">"#,
                    bottom + 14.0
                );
                write_xml_escaped(out, d.label);
                out.push_str("</text>");
            }
        }
    }

    out.push_str("</svg>");
}

/// Render a Vega-Lite specification (`nav`) into `out` SVG.
pub fn vegalite(nav: Nav<'_>, palette: &Palette, width: u16, out: &mut String) {
    let mark = nav
        .get(b"mark")
        .and_then(|m| {
            m.str_raw()
                .or_else(|| m.get(b"type").and_then(|t| t.str_raw()))
        })
        .unwrap_or(b"bar");
    let title = nav.get(b"title").and_then(|t| t.str_raw());

    let enc = nav.get(b"encoding");
    let x_field = enc
        .and_then(|e| e.get(b"x").or_else(|| e.get(b"theta")))
        .and_then(|x| x.get(b"field"))
        .and_then(|f| f.str_raw())
        .unwrap_or(b"x");
    let y_field = enc
        .and_then(|e| e.get(b"y").or_else(|| e.get(b"theta")))
        .and_then(|y| y.get(b"field"))
        .and_then(|f| f.str_raw())
        .unwrap_or(b"y");

    let mut pts = [Datum { label: b"", y: 0.0 }; MAX_POINTS];
    let mut count = 0usize;

    if let Some(values) = nav.get(b"data").and_then(|d| d.get(b"values")) {
        for item in values.items() {
            if count >= MAX_POINTS {
                break;
            }
            let xv = item.get(x_field);
            let label = xv.and_then(|n| n.str_raw()).unwrap_or(b"");
            let y_num = item.get(y_field).and_then(|n| n.num_of()).unwrap_or(0.0);
            pts[count] = Datum { label, y: y_num };
            count += 1;
        }
    }

    render_svg_chart(mark, title, &pts[..count], palette, width, out);
}

/// Render a Plotly JSON specification (`nav`) into `out` SVG.
pub fn plotly(nav: Nav<'_>, palette: &Palette, width: u16, out: &mut String) {
    let title = nav
        .get(b"layout")
        .and_then(|l| l.get(b"title"))
        .and_then(|t| {
            t.str_raw()
                .or_else(|| t.get(b"text").and_then(|x| x.str_raw()))
        });

    let first_trace = nav
        .get(b"data")
        .and_then(|d| d.items().next())
        .unwrap_or(nav);
    let mark = first_trace
        .get(b"type")
        .and_then(|t| t.str_raw())
        .unwrap_or(b"bar");

    let mut pts = [Datum { label: b"", y: 0.0 }; MAX_POINTS];
    let mut count = 0usize;

    let xs = first_trace.get(b"x").or_else(|| first_trace.get(b"labels"));
    let ys = first_trace.get(b"y").or_else(|| first_trace.get(b"values"));

    if let Some(y_arr) = ys {
        let mut x_iter = xs.map(|x| x.items());
        for y_node in y_arr.items() {
            if count >= MAX_POINTS {
                break;
            }
            let x_node = x_iter.as_mut().and_then(Iterator::next);
            let label = x_node.and_then(|n| n.str_raw()).unwrap_or(b"");
            let y_num = y_node.num_of().unwrap_or(0.0);
            pts[count] = Datum { label, y: y_num };
            count += 1;
        }
    }

    render_svg_chart(mark, title, &pts[..count], palette, width, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::DARK;

    #[test]
    fn vegalite_bar_and_plotly_render_svg() {
        let vl = br#"{"mark":"bar","title":"Sales","encoding":{"x":{"field":"k"},"y":{"field":"v"}},"data":{"values":[{"k":"Q1","v":10},{"k":"Q2","v":25}]}}"#;
        let mut svg = String::new();
        vegalite(Nav::new(vl), &DARK, 400, &mut svg);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("<rect"));
        assert!(svg.ends_with("</svg>"));

        svg.clear();
        let pl = br#"{"data":[{"type":"scatter","x":[1,2,3],"y":[10,20,15]}],"layout":{"title":"Trend"}}"#;
        plotly(Nav::new(pl), &DARK, 400, &mut svg);
        assert!(svg.contains("<circle"));
        assert!(svg.ends_with("</svg>"));
    }
}
