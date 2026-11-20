//! Zero-copy Vega-Lite (`.vl`) and Plotly (`.plotly`) JSON chart compiler into `Scene`.

use crate::{
    err::Error,
    fmt::json,
    layout::LinearScale,
    scene::{Rect, Scene},
    theme::ColorToken,
};

const MAX_POINTS: usize = 512;
const PLOT_W: f32 = 560.0;
const PLOT_H: f32 = 280.0;
const PAD_L: f32 = 48.0;
const PAD_T: f32 = 24.0;

#[derive(Clone, Copy)]
struct Datum {
    x: f32,
    y: f32,
}

fn collect_plotly(src: &[u8], pts: &mut [Datum; MAX_POINTS]) -> (usize, bool) {
    let Some(data) = json::get_field(src, b"data") else {
        return (0, false);
    };
    let Some(first) = json::iter_array(data).next() else {
        return (0, false);
    };
    let is_bar = json::get_field(first, b"type").is_some_and(|t| t.windows(3).any(|w| w == b"bar"));
    let Some(ys) = json::get_field(first, b"y") else {
        return (0, is_bar);
    };
    let mut xs_iter = json::get_field(first, b"x").map(json::iter_array);

    let mut count = 0usize;
    for (idx, raw_y) in json::iter_array(ys).enumerate() {
        if count >= MAX_POINTS {
            break;
        }
        let Some(yv) = json::parse_f64(raw_y) else {
            continue;
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "chart index <= MAX_POINTS (512)"
        )]
        let fallback_x = idx as f32;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "screen-space chart coordinates use f32"
        )]
        let xv = xs_iter
            .as_mut()
            .and_then(Iterator::next)
            .and_then(json::parse_f64)
            .map_or(fallback_x, |v| v as f32);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "screen-space chart coordinates use f32"
        )]
        {
            pts[count] = Datum {
                x: xv,
                y: yv as f32,
            };
        }
        count += 1;
    }
    (count, is_bar)
}

fn collect_vega_lite(src: &[u8], pts: &mut [Datum; MAX_POINTS]) -> (usize, bool) {
    let is_bar = json::get_field(src, b"mark").is_some_and(|m| m.windows(3).any(|w| w == b"bar"));
    let values = json::get_field(src, b"data").and_then(|d| json::get_field(d, b"values"));
    let Some(arr) = values else {
        return (0, is_bar);
    };

    let enc = json::get_field(src, b"encoding");
    let enc_x = enc
        .and_then(|e| json::get_field(e, b"x"))
        .and_then(|x| json::get_field(x, b"field"))
        .and_then(|f| {
            let t = f.trim_ascii();
            t.strip_prefix(b"\"").and_then(|s| s.strip_suffix(b"\""))
        });
    let enc_y = enc
        .and_then(|e| json::get_field(e, b"y"))
        .and_then(|y| json::get_field(y, b"field"))
        .and_then(|f| {
            let t = f.trim_ascii();
            t.strip_prefix(b"\"").and_then(|s| s.strip_suffix(b"\""))
        });

    let mut count = 0usize;
    for (idx, row) in json::iter_array(arr).enumerate() {
        if count >= MAX_POINTS {
            break;
        }
        let y_val = enc_y
            .and_then(|k| json::get_field(row, k))
            .or_else(|| json::get_field(row, b"y"))
            .or_else(|| json::get_field(row, b"b"))
            .or_else(|| json::get_field(row, b"value"))
            .and_then(json::parse_f64);
        let Some(yv) = y_val else { continue };

        #[expect(
            clippy::cast_precision_loss,
            reason = "chart index <= MAX_POINTS (512)"
        )]
        let fallback_x = idx as f32;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "screen-space chart coordinates use f32"
        )]
        let xv = enc_x
            .and_then(|k| json::get_field(row, k))
            .or_else(|| json::get_field(row, b"x"))
            .or_else(|| json::get_field(row, b"a"))
            .and_then(json::parse_f64)
            .map_or(fallback_x, |v| v as f32);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "screen-space chart coordinates use f32"
        )]
        {
            pts[count] = Datum {
                x: xv,
                y: yv as f32,
            };
        }
        count += 1;
    }
    (count, is_bar)
}

/// Compile Vega-Lite or Plotly JSON spec into vector `Scene` primitives (`Rect` / `Path`).
///
/// # Errors
/// Returns `Error::JsonNav` if no numeric series can be extracted.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    let mut data = [Datum { x: 0.0, y: 0.0 }; MAX_POINTS];
    let (count, is_bar) =
        if json::get_field(src, b"data").is_some_and(|d| d.trim_ascii().starts_with(b"[")) {
            collect_plotly(src, &mut data)
        } else {
            collect_vega_lite(src, &mut data)
        };
    if count == 0 {
        return Err(Error::JsonNav);
    }

    let slice = &data[..count];
    let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut min_y, mut max_y) = (
        if is_bar { 0.0_f32 } else { f32::INFINITY },
        f32::NEG_INFINITY,
    );
    for d in slice {
        min_x = min_x.min(d.x);
        max_x = max_x.max(d.x);
        min_y = min_y.min(d.y);
        max_y = max_y.max(d.y);
    }

    let base_y = scene.height;
    let x_scale = LinearScale::new(min_x, max_x, PAD_L, PAD_L + PLOT_W);
    let y_scale = LinearScale::new(min_y, max_y, base_y + PAD_T + PLOT_H, base_y + PAD_T);

    // Plot background & axes.
    scene.push_rect(Rect {
        x: PAD_L,
        y: base_y + PAD_T,
        w: PLOT_W,
        h: PLOT_H,
        color: ColorToken::BG_SURFACE,
        radius: 2,
        stroke_width: 1,
        flags: Rect::FILL | Rect::STROKE,
    });

    if is_bar {
        #[expect(clippy::cast_precision_loss, reason = "count <= MAX_POINTS (512)")]
        let bar_w = (PLOT_W / (count as f32) * 0.72).clamp(2.0, 48.0);
        let y_zero = y_scale.map(0.0);
        for d in slice {
            let px = bar_w.mul_add(-0.5, x_scale.map(d.x));
            let py = y_scale.map(d.y);
            scene.push_rect(Rect {
                x: px,
                y: py.min(y_zero),
                w: bar_w,
                h: (y_zero - py).abs().max(1.0),
                color: ColorToken::series(0),
                radius: 1,
                stroke_width: 0,
                flags: Rect::FILL,
            });
        }
    } else {
        let mut mapped = [[0.0_f32; 2]; MAX_POINTS];
        for (dst, d) in mapped[..count].iter_mut().zip(slice.iter()) {
            *dst = [x_scale.map(d.x), y_scale.map(d.y)];
        }
        scene.push_series_line(&mapped[..count], ColorToken::series(0), 2);
    }

    scene.width = scene.width.max(PAD_L + PLOT_W + 24.0);
    scene.height = base_y + PAD_T + PLOT_H + 32.0;
    Ok(())
}
