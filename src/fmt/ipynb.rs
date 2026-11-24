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

const MIME_KEYS: [&[u8]; 8] = [
    b"image/svg+xml",
    b"image/png",
    b"text/latex",
    b"application/vnd.vegalite.v6+json",
    b"application/vnd.vegalite.v5+json",
    b"application/vnd.plotly.v1+json",
    b"text/markdown",
    b"text/plain",
];

const MIME_TAGS: [&[u8]; MIME_KEYS.len()] = [
    b"svg", b"png", b"tex", b"vl", b"vl", b"plotly", b"", b"text",
];

fn mime(data: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    MIME_TAGS
        .into_iter()
        .zip(json::get_fields(data, MIME_KEYS))
        .filter_map(|(tag, value)| Some((tag, value?)))
}

fn cell_fields(cell: &[u8]) -> [Option<&[u8]>; 3] {
    json::get_fields(cell, [b"cell_type", b"source", b"outputs"])
}

fn output_fields(output: &[u8]) -> [Option<&[u8]>; 3] {
    json::get_fields(output, [b"data", b"text", b"evalue"])
}

fn collect_mime(value: &[u8], buf: &mut Vec<u8>) {
    if value.trim_ascii().starts_with(b"{") {
        buf.clear();
        buf.extend_from_slice(value);
    } else {
        collect_json_lines(value, buf);
    }
}

fn emit_output(out_obj: &[u8], buf: &mut Vec<u8>, palette: &theme::Palette, scene: &mut Scene) {
    let [data, stream, error] = output_fields(out_obj);
    if let Some(data) = data {
        for (tag, value) in mime(data) {
            let kind = match tag {
                b"svg" => Kind::Svg,
                b"tex" => Kind::Tex,
                b"vl" | b"plotly" => Kind::Chart,
                b"" => Kind::Md,
                b"text" => {
                    collect_mime(value, buf);
                    emit_mono_block(buf, ColorToken::FG_MUTED, 12, false, scene);
                    break;
                }
                // PNG is transported as pixels by the document compositor.
                _ => continue,
            };
            collect_mime(value, buf);
            if fmt::dispatch_with_palette(kind, buf, palette, scene).is_ok() {
                break;
            }
        }
    } else if let Some(value) = stream.or(error) {
        collect_json_lines(value, buf);
        let color = if stream.is_some() {
            ColorToken::FG_MUTED
        } else {
            ColorToken::ACCENT_RED
        };
        emit_mono_block(buf, color, 12, false, scene);
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
        let [kind, source, outputs] = cell_fields(cell);
        let is_md = kind == Some(b"\"markdown\"");

        if let Some(source) = source {
            collect_json_lines(source, &mut cell_buf);
            if is_md {
                let _ = fmt::dispatch_with_palette(Kind::Md, &cell_buf, palette, scene);
            } else {
                emit_mono_block(&cell_buf, ColorToken::ACCENT, 13, true, scene);
            }
        }

        if let Some(outputs) = outputs {
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
/// Returns `JsonNav` when the notebook has no cells array or `ResourceLimit` if output exceeds the document budget.
pub fn to_markdown(src: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
    to_markdown_mapped(src, out, |_, _, _, _| {})
}

/// Normalize with original JSON cell ranges and normalized byte ranges.
///
/// # Errors
/// Returns `JsonNav` when the notebook has no cells array or `ResourceLimit` if output exceeds the document budget.
pub fn to_markdown_mapped(
    src: &[u8],
    out: &mut Vec<u8>,
    mut cell_range: impl FnMut(usize, usize, usize, usize),
) -> Result<(), Error> {
    let cells = json::get_field(src, b"cells").ok_or(Error::JsonNav)?;
    let language = json::get_field(src, b"metadata")
        .and_then(|metadata| {
            json::get_field(metadata, b"language_info")
                .and_then(|info| json::get_field(info, b"name"))
                .or_else(|| {
                    json::get_field(metadata, b"kernelspec")
                        .and_then(|kernel| json::get_field(kernel, b"language"))
                })
        })
        .and_then(|name| name.strip_prefix(b"\"")?.strip_suffix(b"\""))
        .filter(|name| {
            matches!(
                *name,
                b"python" | b"rust" | b"javascript" | b"typescript" | b"bash" | b"sql" | b"json"
            )
        })
        .unwrap_or(b"text");
    let mut text = Vec::new();
    for cell in json::iter_array(cells) {
        let start = out.len();
        let offset = cell.as_ptr().addr() - src.as_ptr().addr();
        let [kind, source, outputs] = cell_fields(cell);
        let markdown = kind == Some(b"\"markdown\"");
        if let Some(source) = source {
            collect_json_lines(source, &mut text);
            if markdown {
                check_output(out, text.len() + 2)?;
                out.extend_from_slice(&text);
            } else {
                fenced(&text, language, out, 2)?;
            }
            out.extend_from_slice(b"\n\n");
        }
        if let Some(outputs) = outputs {
            for output in json::iter_array(outputs) {
                output_markdown(output, &mut text, out)?;
            }
        }
        cell_range(offset, offset + cell.len(), start, out.len());
    }
    Ok(())
}

fn output_markdown(output: &[u8], text: &mut Vec<u8>, out: &mut Vec<u8>) -> Result<(), Error> {
    let [data, stream, error] = output_fields(output);
    if let Some(data) = data {
        for (tag, value) in mime(data) {
            collect_mime(value, text);
            if tag == b"png" {
                let Some(bytes) = decode_base64(text) else {
                    continue;
                };
                fenced(&bytes, tag, out, 2)?;
            } else if tag.is_empty() {
                check_output(out, text.len() + 2)?;
                out.extend_from_slice(text);
            } else {
                fenced(text, tag, out, 2)?;
            }
            out.extend_from_slice(b"\n\n");
            break;
        }
    } else if let Some(value) = stream.or(error) {
        collect_json_lines(value, text);
        fenced(text, b"text", out, 0)?;
    }
    Ok(())
}

const fn check_output(out: &[u8], append: usize) -> Result<(), Error> {
    if out.len().saturating_add(append) > super::MAX_DOCUMENT_BYTES {
        return Err(Error::ResourceLimit);
    }
    Ok(())
}

fn fenced(text: &[u8], tag: &[u8], out: &mut Vec<u8>, trailing: usize) -> Result<(), Error> {
    // Pick a fence longer than any run in the payload so code cannot become markup.
    let mut longest = 0usize;
    let mut tail = text;
    while let Some(start) = memchr::memchr(b'`', tail) {
        let len = 1 + tail[start + 1..]
            .iter()
            .take_while(|&&byte| byte == b'`')
            .count();
        longest = longest.max(len);
        tail = &tail[start + len..];
    }
    let count = longest.saturating_add(1).max(3);
    check_output(out, 2 * count + tag.len() + text.len() + 3 + trailing)?;
    out.resize(out.len() + count, b'`');
    out.extend_from_slice(tag);
    out.push(b'\n');
    out.extend_from_slice(text);
    if !text.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.resize(out.len() + count, b'`');
    out.push(b'\n');
    Ok(())
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
