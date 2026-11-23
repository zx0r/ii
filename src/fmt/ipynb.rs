//! Zero-copy Jupyter Notebook (`.ipynb`) JSON cell walker emitting into `Scene`.
//! Zero horizontal imports of sibling format parsers (routes via `fmt::dispatch_kind`).

use crate::theme;
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

fn emit_output(out_obj: &[u8], buf: &mut Vec<u8>, palette: &theme::Palette, scene: &mut Scene) {
    if let Some(data) = json::get_field(out_obj, b"data") {
        if let Some(raw_svg) = json::get_field(data, b"image/svg+xml") {
            collect_json_lines(raw_svg, buf);
            let _ = fmt::dispatch_with_palette(Kind::Svg, buf, palette, scene);
        } else if let Some(raw_tex) = json::get_field(data, b"text/latex") {
            collect_json_lines(raw_tex, buf);
            let _ = fmt::dispatch_with_palette(Kind::Tex, buf, palette, scene);
        } else if let Some(raw_md) = json::get_field(data, b"text/markdown") {
            collect_json_lines(raw_md, buf);
            let _ = fmt::dispatch_with_palette(Kind::Md, buf, palette, scene);
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
    parse_with_palette(src, &theme::DARK, scene)
}

/// Parse with an explicit receiving palette.
///
/// # Errors
/// Propagates format decoding errors.
pub fn parse_with_palette(
    src: &[u8],
    palette: &theme::Palette,
    scene: &mut Scene,
) -> Result<(), Error> {
    let cells = json::get_field(src, b"cells").ok_or(Error::JsonNav)?;
    let mut cell_buf = Vec::with_capacity(4096);

    for cell in json::iter_array(cells) {
        let is_md = json::get_field(cell, b"cell_type")
            .is_some_and(|cell_type| cell_type.trim_ascii() == b"\"markdown\"");

        if let Some(source) = json::get_field(cell, b"source") {
            collect_json_lines(source, &mut cell_buf);
            if is_md {
                let _ = fmt::dispatch_with_palette(Kind::Md, &cell_buf, palette, scene);
            } else {
                emit_mono_block(&cell_buf, ColorToken::ACCENT, 13, true, scene);
            }
        }

        if let Some(outputs) = json::get_field(cell, b"outputs") {
            for out_obj in json::iter_array(outputs) {
                emit_output(out_obj, &mut cell_buf, palette, scene);
            }
        }
    }
    Ok(())
}

/// Normalize notebook cells and MIME alternatives into the native document path.
///
/// # Errors
/// Returns `JsonNav` when the notebook has no cells array or invalid string payloads.
pub fn to_markdown(src: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
    let cells = json::get_field(src, b"cells").ok_or(Error::JsonNav)?;
    let mut text = Vec::new();
    for cell in json::iter_array(cells) {
        let markdown = json::get_field(cell, b"cell_type").is_some_and(|v| v == b"\"markdown\"");
        if let Some(source) = json::get_field(cell, b"source") {
            collect_json_lines(source, &mut text);
            if markdown {
                out.extend_from_slice(&text);
            } else {
                fenced(&text, b"text", out);
            }
            out.extend_from_slice(b"\n\n");
        }
        if let Some(outputs) = json::get_field(cell, b"outputs") {
            for output in json::iter_array(outputs) {
                output_markdown(output, &mut text, out);
            }
        }
    }
    Ok(())
}

fn output_markdown(output: &[u8], text: &mut Vec<u8>, out: &mut Vec<u8>) {
    if let Some(data) = json::get_field(output, b"data") {
        let alternatives: &[(&[u8], &[u8])] = &[
            (b"image/svg+xml", b"svg"),
            (b"image/png", b"png"),
            (b"text/latex", b"tex"),
            (b"application/vnd.vegalite.v6+json", b"vl"),
            (b"application/vnd.vegalite.v5+json", b"vl"),
            (b"application/vnd.plotly.v1+json", b"plotly"),
            (b"text/markdown", b""),
            (b"text/plain", b"text"),
        ];
        for &(mime, tag) in alternatives {
            if let Some(value) = json::get_field(data, mime) {
                if value.trim_ascii().starts_with(b"{") {
                    text.clear();
                    text.extend_from_slice(value);
                } else {
                    collect_json_lines(value, text);
                }
                if tag == b"png" {
                    let Some(bytes) = decode_base64(text) else {
                        continue;
                    };
                    fenced(&bytes, tag, out);
                } else if tag.is_empty() {
                    out.extend_from_slice(text);
                } else {
                    fenced(text, tag, out);
                }
                out.extend_from_slice(b"\n\n");
                break;
            }
        }
    } else if let Some(value) =
        json::get_field(output, b"text").or_else(|| json::get_field(output, b"evalue"))
    {
        collect_json_lines(value, text);
        fenced(text, b"text", out);
    }
}

fn fenced(text: &[u8], tag: &[u8], out: &mut Vec<u8>) {
    // Pick a fence longer than any run in the payload so code cannot become markup.
    let mut longest = 0usize;
    let mut run = 0;
    for &byte in text {
        run = if byte == b'`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let count = longest.saturating_add(1).max(3);
    out.resize(out.len() + count, b'`');
    out.extend_from_slice(tag);
    out.push(b'\n');
    out.extend_from_slice(text);
    if !text.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.resize(out.len() + count, b'`');
    out.push(b'\n');
}

// Notebook MIME data is Base64 text. Reject malformed padding and trailing bits;
// decoding is bounded by the already bounded notebook input.
fn decode_base64(src: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(src.len() / 4 * 3);
    let mut bits = 0u32;
    let mut count = 0;
    let mut padding = 0;
    let mut symbols = 0;
    for &b in src {
        if b.is_ascii_whitespace() {
            continue;
        }
        symbols += 1;
        if b == b'=' {
            padding += 1;
            if padding > 2 {
                return None;
            }
            continue;
        }
        if padding != 0 {
            return None;
        }
        let value = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count).to_le_bytes()[0]);
            bits &= (1 << count) - 1;
        }
    }
    if symbols % 4 != 0 || count != padding * 2 || bits != 0 {
        return None;
    }
    Some(out)
}
