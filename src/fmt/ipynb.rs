//! Zero-copy Jupyter Notebook (`.ipynb`) JSON cell walker emitting into `Scene`.
//! Zero horizontal imports of sibling format parsers (routes via `fmt::dispatch_kind`).

use crate::{
    err::Error,
    fmt::{self, Kind, font, json},
    scene::{GlyphRun, Rect, Scene},
    theme::ColorToken,
};

fn collect_json_lines(field: &[u8], out: &mut Vec<u8>) {
    out.clear();
    let trimmed = field.trim_ascii();
    if trimmed.starts_with(b"[") {
        for elem in json::iter_array(trimmed) {
            let _ = json::unescape_str(elem, out);
        }
    } else {
        let _ = json::unescape_str(trimmed, out);
    }
}

fn emit_mono_block(buf: &[u8], color: ColorToken, font_px: u16, bordered: bool, scene: &mut Scene) {
    if buf.is_empty() {
        return;
    }
    let line_h = f32::from(font_px) + 5.0;
    let top_y = scene.height + 6.0;
    let mut line_y = top_y + line_h;
    let mut block_w = 120.0_f32;

    for raw in buf.split(|&byte| byte == b'\n') {
        let text = core::str::from_utf8(raw.strip_suffix(b"\r").unwrap_or(raw)).unwrap_or("");
        scene.push_text_styled(12.0, line_y, text, color, font_px, GlyphRun::MONO);
        block_w = block_w.max(font::measure_str(text, f32::from(font_px)) + 24.0);
        line_y += line_h;
    }

    if bordered {
        scene.push_rect(Rect {
            x: 4.0,
            y: top_y,
            w: block_w,
            h: (line_y - top_y).max(line_h),
            color: ColorToken::BG_SURFACE,
            radius: 4,
            stroke_width: 1,
            flags: Rect::FILL | Rect::STROKE,
        });
    }
    scene.width = scene.width.max(block_w + 8.0);
    scene.height = line_y + 6.0;
}

fn emit_output(out_obj: &[u8], buf: &mut Vec<u8>, scene: &mut Scene) {
    if let Some(data) = json::get_field(out_obj, b"data") {
        if let Some(raw_svg) = json::get_field(data, b"image/svg+xml") {
            collect_json_lines(raw_svg, buf);
            let _ = fmt::dispatch_kind(Kind::Svg, buf, scene);
        } else if let Some(raw_tex) = json::get_field(data, b"text/latex") {
            collect_json_lines(raw_tex, buf);
            let _ = fmt::dispatch_kind(Kind::Tex, buf, scene);
        } else if let Some(raw_md) = json::get_field(data, b"text/markdown") {
            collect_json_lines(raw_md, buf);
            let _ = fmt::dispatch_kind(Kind::Md, buf, scene);
        } else if let Some(raw_txt) = json::get_field(data, b"text/plain") {
            collect_json_lines(raw_txt, buf);
            emit_mono_block(buf, ColorToken::FG_MUTED, 12, false, scene);
        }
    } else if let Some(stream) = json::get_field(out_obj, b"text") {
        collect_json_lines(stream, buf);
        emit_mono_block(buf, ColorToken::FG_MUTED, 12, false, scene);
    } else if let Some(evalue) = json::get_field(out_obj, b"evalue") {
        collect_json_lines(evalue, buf);
        emit_mono_block(buf, ColorToken::ACCENT_RED, 12, false, scene);
    }
}

/// Parse Jupyter Notebook JSON into `Scene` (markdown, code cells, stream/error, and MIME outputs).
///
/// # Errors
/// Returns `Error::JsonNav` if the notebook has no `"cells"` array.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    let cells = json::get_field(src, b"cells").ok_or(Error::JsonNav)?;
    let mut cell_buf = Vec::with_capacity(4096);

    for cell in json::iter_array(cells) {
        let is_md = json::get_field(cell, b"cell_type")
            .is_some_and(|cell_type| cell_type.trim_ascii() == b"\"markdown\"");

        if let Some(source) = json::get_field(cell, b"source") {
            collect_json_lines(source, &mut cell_buf);
            if is_md {
                let _ = fmt::dispatch_kind(Kind::Md, &cell_buf, scene);
            } else {
                emit_mono_block(&cell_buf, ColorToken::ACCENT, 13, true, scene);
            }
        }

        if let Some(outputs) = json::get_field(cell, b"outputs") {
            for out_obj in json::iter_array(outputs) {
                emit_output(out_obj, &mut cell_buf, scene);
            }
        }
    }
    Ok(())
}
