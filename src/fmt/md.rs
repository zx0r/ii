//! Single-pass SIMD (`memchr`) scanner & `Scene` compiler for `.md`, `.qmd`, `.mdx`, `.rmd`, `.typ`.
//! Zero horizontal imports of sibling format parsers (routes via `fmt::dispatch_kind`).

use crate::{
    err::Error,
    fmt::{self, Kind, font, trim_ascii},
    scene::{GlyphRun, Scene},
    theme::ColorToken,
};

const MATH_FENCE: &[u8; 2] = b"\x24\x24";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Span {
    pub outer_start: u32,
    pub outer_end: u32,
    pub inner_start: u32,
    pub inner_end: u32,
    pub kind: Kind,
    _pad: [u8; 3],
}
const _: () = assert!(size_of::<Span>() == 20);

#[inline]
fn fence_kind(info: &[u8]) -> Option<Kind> {
    let clean = trim_ascii(info);
    let tag = clean
        .strip_prefix(b"{")
        .unwrap_or(clean)
        .split(|&byte| byte.is_ascii_whitespace() || byte == b',' || byte == b'}')
        .next()
        .and_then(|bytes| core::str::from_utf8(bytes).ok())?;
    if tag.eq_ignore_ascii_case("math") {
        return Some(Kind::Tex);
    }
    if tag.eq_ignore_ascii_case("graphviz") {
        return Some(Kind::Dot);
    }
    fmt::kind_from_ext(tag).filter(|kind| !matches!(kind, Kind::Md | Kind::Ipynb | Kind::Archive))
}

#[inline]
fn push_span(out: &mut Vec<Span>, outer: (usize, usize), inner: (usize, usize), kind: Kind) {
    let (Ok(outer_start), Ok(outer_end), Ok(inner_start), Ok(inner_end)) = (
        u32::try_from(outer.0),
        u32::try_from(outer.1),
        u32::try_from(inner.0),
        u32::try_from(inner.1),
    ) else {
        return;
    };
    out.push(Span {
        outer_start,
        outer_end,
        inner_start,
        inner_end,
        kind,
        _pad: [0; 3],
    });
}

fn scan_inline_math(src: &[u8], line_start: usize, line_end: usize, out: &mut Vec<Span>) {
    let mut pos = line_start;
    while pos < line_end {
        let Some(rel) = memchr::memchr2(b'$', b'`', &src[pos..line_end]) else {
            break;
        };
        let hit = pos + rel;

        // Skip inline code spans (`...`).
        if src[hit] == b'`' {
            pos = memchr::memchr(b'`', &src[hit + 1..line_end])
                .map_or(line_end, |close| hit + 2 + close);
            continue;
        }
        // Skip escaped `\$`.
        if hit > line_start && src[hit - 1] == b'\\' {
            pos = hit + 1;
            continue;
        }

        // Single-line display math (`MATH_FENCE` ... `MATH_FENCE`).
        if src.get(hit + 1) == Some(&b'$') {
            let inner_s = hit + 2;
            if let Some(close_rel) = src[inner_s..line_end]
                .windows(2)
                .position(|win| win == MATH_FENCE)
            {
                let inner_e = inner_s + close_rel;
                if inner_e > inner_s {
                    push_span(out, (hit, inner_e + 2), (inner_s, inner_e), Kind::Tex);
                }
                pos = inner_e + 2;
                continue;
            }
            pos = inner_s;
            continue;
        }

        // Inline math (`$...$`): no space after opening `$` or before closing `$`.
        let inner_s = hit + 1;
        if inner_s >= line_end || src[inner_s].is_ascii_whitespace() {
            pos = inner_s;
            continue;
        }
        let mut search = inner_s;
        let mut matched = false;
        while search < line_end {
            let Some(close_rel) = memchr::memchr(b'$', &src[search..line_end]) else {
                break;
            };
            let close = search + close_rel;
            if src[close - 1] == b'\\' {
                search = close + 1;
                continue;
            }
            let valid_close = !src[close - 1].is_ascii_whitespace()
                && !src.get(close + 1).is_some_and(u8::is_ascii_digit);
            if valid_close && close > inner_s {
                push_span(out, (hit, close + 1), (inner_s, close), Kind::TexInline);
                pos = close + 1;
                matched = true;
            }
            break;
        }
        if !matched {
            pos = inner_s;
        }
    }
}

/// Scan document bytes for renderable graphical/math/table spans (zero alloc after warmup).
pub fn scan(src: &[u8], out: &mut Vec<Span>) {
    out.clear();
    // Skip YAML frontmatter (`--- ... ---`).
    let mut pos = if (src.starts_with(b"---\n") || src.starts_with(b"---\r\n"))
        && let Some(close) = src[4..].windows(4).position(|win| win == b"\n---")
    {
        let after = 4 + close + 4;
        memchr::memchr(b'\n', &src[after..]).map_or(src.len(), |nl| after + nl + 1)
    } else {
        0usize
    };

    while pos < src.len() {
        let nl = memchr::memchr(b'\n', &src[pos..]).map_or(src.len(), |off| pos + off);
        let trimmed = trim_ascii(&src[pos..nl]);

        // Fenced code blocks (` ``` ` or `~~~`).
        let fence = trimmed
            .strip_prefix(b"```")
            .map(|info| (b"```".as_slice(), info))
            .or_else(|| {
                trimmed
                    .strip_prefix(b"~~~")
                    .map(|info| (b"~~~".as_slice(), info))
            });

        if let Some((delim, info)) = fence {
            let inner_start = (nl + 1).min(src.len());
            let mut cur = inner_start;
            let mut inner_end = src.len();
            let mut outer_end = src.len();
            while cur < src.len() {
                let end_nl = memchr::memchr(b'\n', &src[cur..]).map_or(src.len(), |off| cur + off);
                if trim_ascii(&src[cur..end_nl]).starts_with(delim) {
                    inner_end = cur;
                    outer_end = (end_nl + 1).min(src.len());
                    break;
                }
                cur = (end_nl + 1).min(src.len());
            }
            if let Some(kind) = fence_kind(info) {
                push_span(out, (pos, outer_end), (inner_start, inner_end), kind);
            }
            pos = outer_end;
            continue;
        }

        // Multi-line display math (`MATH_FENCE` on its own line).
        if trimmed == MATH_FENCE {
            let inner_start = (nl + 1).min(src.len());
            let mut cur = inner_start;
            let mut closed = false;
            while cur < src.len() {
                let end_nl = memchr::memchr(b'\n', &src[cur..]).map_or(src.len(), |off| cur + off);
                if trim_ascii(&src[cur..end_nl]) == MATH_FENCE {
                    let outer_end = (end_nl + 1).min(src.len());
                    push_span(out, (pos, outer_end), (inner_start, cur), Kind::Tex);
                    pos = outer_end;
                    closed = true;
                    break;
                }
                cur = (end_nl + 1).min(src.len());
            }
            if !closed {
                pos = (nl + 1).min(src.len());
            }
            continue;
        }

        // Markdown pipe tables (`| ... |`).
        if trimmed.starts_with(b"|") && trimmed.ends_with(b"|") {
            let start = pos;
            let mut cur = pos;
            while cur < src.len() {
                let end_nl = memchr::memchr(b'\n', &src[cur..]).map_or(src.len(), |off| cur + off);
                let row = trim_ascii(&src[cur..end_nl]);
                if !(row.starts_with(b"|") && row.ends_with(b"|")) {
                    break;
                }
                cur = (end_nl + 1).min(src.len());
            }
            push_span(out, (start, cur), (start, cur), Kind::Csv);
            pos = cur;
            continue;
        }

        scan_inline_math(src, pos, nl, out);
        pos = (nl + 1).min(src.len());
    }
}

/// Compile Markdown / Quarto / MDX / RMD / Typst document into `Scene`.
///
/// # Errors
/// Degrades gracefully on individual sub-block errors; returns `Ok(())`.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    let mut spans = Vec::with_capacity(16);
    scan(src, &mut spans);

    let mut cursor = 0usize;
    for sp in &spans {
        let (outer_start, outer_end) = (sp.outer_start as usize, sp.outer_end as usize);
        if outer_start > cursor {
            emit_text_block(&src[cursor..outer_start], scene);
        }
        let payload = &src[sp.inner_start as usize..sp.inner_end as usize];
        let _ = fmt::dispatch_kind(sp.kind, payload, scene);
        cursor = outer_end.max(cursor);
    }
    if cursor < src.len() {
        emit_text_block(&src[cursor..], scene);
    }
    Ok(())
}

fn emit_line(
    scene: &mut Scene,
    prefix: &str,
    prefix_color: ColorToken,
    content: &[u8],
    content_color: ColorToken,
    font_size: u16,
    flags: u8,
) {
    let text = core::str::from_utf8(content).unwrap_or("");
    let cursor_y = scene.height + f32::from(font_size) + 4.0;
    let mut total_w = 8.0_f32;

    if !prefix.is_empty() {
        scene.push_text_styled(8.0, cursor_y, prefix, prefix_color, font_size, flags);
        total_w += font::measure_str(prefix, f32::from(font_size));
    }
    if !text.is_empty() {
        scene.push_text_styled(total_w, cursor_y, text, content_color, font_size, flags);
        total_w += font::measure_str(text, f32::from(font_size));
    }
    scene.width = scene.width.max(total_w + 8.0);
    scene.height = cursor_y + 4.0;
}

fn emit_text_block(block: &[u8], scene: &mut Scene) {
    for raw in block.split(|&byte| byte == b'\n') {
        let trimmed = trim_ascii(raw);
        if trimmed.is_empty() {
            continue;
        }

        // Headings: Zero-Tofu blocks
        if let Some(content) = trimmed.strip_prefix(b"# ") {
            emit_line(scene, "▌ ", ColorToken::ACCENT_ORANGE, content, ColorToken::FG_PRIMARY, 18, GlyphRun::BOLD);
            continue;
        }
        if let Some(content) = trimmed.strip_prefix(b"## ") {
            emit_line(scene, "▎ ", ColorToken::ACCENT_GREEN, content, ColorToken::FG_PRIMARY, 16, GlyphRun::BOLD);
            continue;
        }
        if let Some(content) = trimmed.strip_prefix(b"### ") {
            emit_line(scene, "▪ ", ColorToken::FG_PRIMARY, content, ColorToken::FG_PRIMARY, 14, GlyphRun::BOLD);
            continue;
        }

        // Horizontal rules
        if trimmed == b"---" || trimmed == b"***" || trimmed == b"___" {
            emit_line(scene, "────────────────────────────────────────", ColorToken::BORDER, b"", ColorToken::BORDER, 12, 0);
            continue;
        }

        // GFM Callouts / Alerts & Blockquotes
        if let Some(inner) = trimmed.strip_prefix(b">") {
            let inner_trim = trim_ascii(inner);
            if inner_trim.eq_ignore_ascii_case(b"[!NOTE]") {
                emit_line(scene, "┃ ℹ NOTE", ColorToken::INFO, b"", ColorToken::INFO, 14, GlyphRun::BOLD);
                continue;
            }
            if inner_trim.eq_ignore_ascii_case(b"[!TIP]") {
                emit_line(scene, "┃ ◆ TIP", ColorToken::SUCCESS, b"", ColorToken::SUCCESS, 14, GlyphRun::BOLD);
                continue;
            }
            if inner_trim.eq_ignore_ascii_case(b"[!WARNING]") {
                emit_line(scene, "┃ ▲ WARNING", ColorToken::WARN, b"", ColorToken::WARN, 14, GlyphRun::BOLD);
                continue;
            }
            if inner_trim.eq_ignore_ascii_case(b"[!CAUTION]") {
                emit_line(scene, "┃ ✕ CAUTION", ColorToken::DANGER, b"", ColorToken::DANGER, 14, GlyphRun::BOLD);
                continue;
            }
            emit_line(scene, "┃ ", ColorToken::BORDER, inner_trim, ColorToken::FG_MUTED, 14, 0);
            continue;
        }

        // Checkboxes & Task lists
        if let Some(task) = trimmed.strip_prefix(b"- [x] ").or_else(|| trimmed.strip_prefix(b"- [X] ")) {
            emit_line(scene, "✓ ", ColorToken::SUCCESS, task, ColorToken::FG_PRIMARY, 14, 0);
            continue;
        }
        if let Some(task) = trimmed.strip_prefix(b"- [ ] ") {
            emit_line(scene, "○ ", ColorToken::FG_MUTED, task, ColorToken::FG_MUTED, 14, 0);
            continue;
        }

        // Bullet lists
        if let Some(item) = trimmed.strip_prefix(b"- ").or_else(|| trimmed.strip_prefix(b"* ")) {
            emit_line(scene, "• ", ColorToken::FG_MUTED, item, ColorToken::FG_PRIMARY, 14, 0);
            continue;
        }

        // Standard text line
        emit_line(scene, "", ColorToken::FG_PRIMARY, trimmed, ColorToken::FG_PRIMARY, 14, 0);
    }
}
