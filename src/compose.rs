//! Ordered native prose and graphical presentation with source byte anchors.
use crate::{
    err::Error,
    fmt::{Kind, md},
    image,
    ipc::{self, Extmark, SpanCoord},
    kitty, style,
    theme::Palette,
};
use rustix::fd::BorrowedFd;

#[derive(Clone, Copy, Debug)]
pub struct ComposeCtx<'a> {
    pub cell_w: u16,
    pub cell_h: u16,
    pub cols: u16,
    pub rows: u16,
    pub max_w: u16,
    pub palette: &'a Palette,
    pub is_tmux: bool,
    pub is_remote: bool,
    pub skip_lines: u32,
    pub max_lines: u32,
    pub explicit_graphics: bool,
}

impl<'a> ComposeCtx<'a> {
    fn emit_graphic(self, id: u32, cols: u16, rows: u16, ansi: &mut Vec<u8>) {
        if self.explicit_graphics {
            kitty::emit_explicit_placeholder(id, cols, rows, ansi);
        } else {
            kitty::emit_placeholder(id, cols, rows, ansi);
        }
    }

    const fn image(self) -> image::Context<'a> {
        image::Context {
            cell_w: self.cell_w,
            cell_h: self.cell_h,
            cols: self.cols,
            rows: self.rows,
            max_w: self.max_w,
            palette: self.palette,
            is_tmux: self.is_tmux,
            is_remote: self.is_remote,
            fit_viewport: false,
        }
    }
}
#[derive(Debug, Default)]
pub struct ComposeScratch {
    spans: Vec<md::Span>,
    image_ids: std::collections::HashSet<u32>,
    coords: Vec<SpanCoord>,
    lines: Vec<usize>,
    pub image: image::Scratch,
}
impl ComposeScratch {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    pub fn reset(&mut self) {
        self.spans.clear();
        self.coords.clear();
        self.lines.clear();
        self.image.reset();
    }
}
#[derive(Debug, Default)]
pub struct ComposeOut {
    pub uploads: Vec<Upload>,
    pub blocks: Vec<(usize, usize, usize, usize)>,
    pub apc: Vec<u8>,
    pub ansi: Vec<u8>,
    pub extmarks: Vec<Extmark>,
}
#[derive(Debug)]
pub struct Upload {
    pub image_id: u32,
    pub start: usize,
    pub end: usize,
}

impl ComposeOut {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    pub fn clear(&mut self) {
        self.uploads.clear();
        self.blocks.clear();
        self.apc.clear();
        self.ansi.clear();
        self.extmarks.clear();
        if self.apc.capacity() > 32 * 1024 * 1024 {
            self.apc.shrink_to(256 * 1024);
        }
        if self.ansi.capacity() > 32 * 1024 * 1024 {
            self.ansi.shrink_to(256 * 1024);
        }
    }

    const fn check_budget(&self) -> Result<(), Error> {
        if self.apc.len().saturating_add(self.ansi.len()) > ipc::MAX_PAYLOAD as usize {
            return Err(Error::ResourceLimit);
        }
        Ok(())
    }
}

/// Render a complete source using the same entry point for local and IPC consumers.
///
/// # Errors
/// Returns parsing, rasterization or input-limit errors.
#[expect(
    clippy::too_many_arguments,
    reason = "shared render entry accepts caller-owned resources"
)]
pub fn render_file(
    src: &[u8],
    kind: Kind,
    ctx: ComposeCtx<'_>,
    dir: Option<BorrowedFd<'_>>,
    scratch: &mut ComposeScratch,
    seq: &mut u32,
    pid: u32,
    out: &mut ComposeOut,
) -> Result<(), Error> {
    render(src, kind, ctx, dir, scratch, seq, pid, out, Anchors::Both)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Anchors {
    None,
    Blocks,
    Extmarks,
    Both,
}

impl Anchors {
    const fn blocks(self) -> bool {
        matches!(self, Self::Blocks | Self::Both)
    }
    const fn extmarks(self) -> bool {
        matches!(self, Self::Extmarks | Self::Both)
    }
}

#[expect(clippy::too_many_arguments, reason = "caller owns render resources")]
pub(crate) fn render(
    src: &[u8],
    kind: Kind,
    ctx: ComposeCtx<'_>,
    dir: Option<BorrowedFd<'_>>,
    scratch: &mut ComposeScratch,
    seq: &mut u32,
    pid: u32,
    out: &mut ComposeOut,
    anchors: Anchors,
) -> Result<(), Error> {
    out.clear();
    if let Err(err) = render_full(src, kind, ctx, dir, scratch, seq, pid, out, anchors)
        .and_then(|()| out.check_budget())
    {
        if !ctx.is_remote {
            kitty::discard_shm(&out.apc);
        }
        *out = ComposeOut::new();
        return Err(err);
    }
    paginate(out, ctx);
    Ok(())
}

#[expect(clippy::too_many_arguments, reason = "caller owns render resources")]
fn render_full(
    src: &[u8],
    kind: Kind,
    ctx: ComposeCtx<'_>,
    dir: Option<BorrowedFd<'_>>,
    scratch: &mut ComposeScratch,
    seq: &mut u32,
    pid: u32,
    out: &mut ComposeOut,
    anchors: Anchors,
) -> Result<(), Error> {
    let mut normalized = Vec::new();
    if kind == Kind::Html {
        crate::fmt::html::presentation_source(src, &mut normalized)?;
        return document(&normalized, ctx, dir, scratch, seq, pid, out, anchors);
    }
    if kind == Kind::Mdx {
        crate::fmt::mdx::presentation_source(src, &mut normalized)?;
        return document(&normalized, ctx, dir, scratch, seq, pid, out, anchors);
    }
    if kind == Kind::Archive {
        crate::fmt::archive::to_tree_with_palette(src, ctx.palette, &mut out.ansi)?;
        return Ok(());
    }
    if matches!(kind, Kind::Csv | Kind::Tsv) {
        crate::fmt::csv::to_markdown(
            src,
            if kind == Kind::Tsv { b'\t' } else { b',' },
            &mut normalized,
        )?;
        let mut state = style::State::with_width(ctx.cols);
        state.full_width_tables = true;
        style::emit_blocks_bounded(
            &normalized,
            ctx.palette,
            &mut state,
            &mut out.ansi,
            ipc::MAX_PAYLOAD as usize,
            |_, _, _, _| {},
        )?;
        return Ok(());
    }
    if matches!(kind, Kind::Typst | Kind::Ipynb) {
        let mut ranges = Vec::new();
        match (kind, anchors.blocks()) {
            (Kind::Typst, true) => {
                crate::fmt::typst::to_markdown_mapped(src, &mut normalized, &mut ranges)?;
            }
            (Kind::Typst, false) => crate::fmt::typst::to_markdown(src, &mut normalized)?,
            (Kind::Ipynb, true) => {
                crate::fmt::ipynb::to_markdown_mapped(src, &mut normalized, |a, b, c, d| {
                    ranges.push((a, b, c, d));
                })?;
                let mut cursor = 0;
                for next in 1..=ranges.len() {
                    let end = ranges.get(next).map_or_else(
                        || src.len(),
                        |&(next, _, _, _)| {
                            memchr::memrchr(b'\n', &src[..next]).map_or(next, |i| i + 1)
                        },
                    );
                    (ranges[next - 1].0, ranges[next - 1].1) = (cursor, end);
                    cursor = end;
                }
            }
            _ => crate::fmt::ipynb::to_markdown(src, &mut normalized)?,
        }
        document(&normalized, ctx, dir, scratch, seq, pid, out, anchors)?;
        if anchors.blocks() {
            remap_blocks(out, &ranges);
        }
        return Ok(());
    }
    if kind == Kind::Tex && src.windows(14).any(|s| s == b"\\documentclass") {
        crate::fmt::tex::document_to_markdown(src, &mut normalized)?;
        return document(&normalized, ctx, dir, scratch, seq, pid, out, anchors);
    }
    if kind == Kind::Md {
        document(src, ctx, dir, scratch, seq, pid, out, anchors)?;
    } else {
        let mut image_ctx = ctx.image();
        image_ctx.fit_viewport = kind.fits_viewport_height();
        let (_, _, cols, rows, id) = image::render(
            kind,
            src,
            image_ctx,
            dir,
            &mut scratch.image,
            seq,
            pid,
            &mut out.apc,
        )?;
        out.uploads.push(Upload {
            image_id: id,
            start: 0,
            end: out.apc.len(),
        });
        ctx.emit_graphic(id, cols, rows, &mut out.ansi);
    }
    Ok(())
}

fn remap_blocks(out: &mut ComposeOut, ranges: &[(usize, usize, usize, usize)]) {
    let blocks = core::mem::take(&mut out.blocks);
    let mut last_end = 0;
    for &(src_start, src_end, md_start, md_end) in ranges {
        let first = blocks.partition_point(|&(_, end, _, _)| end <= md_start);
        let last = blocks.partition_point(|&(start, _, _, _)| start < md_end);
        let ansi_start = blocks.get(first).map_or(out.ansi.len(), |block| block.2);
        let ansi_end = if first < last {
            blocks[last - 1].3
        } else {
            ansi_start
        };
        if first < last_end
            && let Some(prev) = out.blocks.last_mut()
        {
            prev.1 = src_end;
            prev.3 = prev.3.max(ansi_end);
        } else {
            out.blocks.push((src_start, src_end, ansi_start, ansi_end));
        }
        last_end = last_end.max(last);
    }
}

/// Compose prose and graphic spans, keeping editor anchors in source coordinates.
///
/// # Errors
/// Returns input-limit errors. Individual graphical failures preserve their source.
pub fn compose_document(
    src: &[u8],
    ctx: ComposeCtx<'_>,
    dir: Option<BorrowedFd<'_>>,
    scratch: &mut ComposeScratch,
    seq: &mut u32,
    pid: u32,
    out: &mut ComposeOut,
) -> Result<(), Error> {
    out.clear();
    if let Err(err) = document(src, ctx, dir, scratch, seq, pid, out, Anchors::Both) {
        if !ctx.is_remote {
            kitty::discard_shm(&out.apc);
        }
        *out = ComposeOut::new();
        return Err(err);
    }
    paginate(out, ctx);
    Ok(())
}

struct Images<'a, 'b> {
    ctx: ComposeCtx<'a>,
    dir: Option<BorrowedFd<'a>>,
    scratch: &'b mut image::Scratch,
    ids: &'b mut std::collections::HashSet<u32>,
    seq: &'b mut u32,
    pid: u32,
    apc: &'b mut Vec<u8>,
    uploads: &'b mut Vec<Upload>,
    coords: Option<&'b mut Vec<SpanCoord>>,
    shm_room: usize,
}

impl Images<'_, '_> {
    const fn ansi_limit(&self) -> usize {
        (ipc::MAX_PAYLOAD as usize).saturating_sub(self.apc.len())
    }

    fn render(
        &mut self,
        kind: Kind,
        payload: &[u8],
        max_inline_cols: Option<usize>,
        ansi_bytes: usize,
    ) -> Result<Option<(u16, u16, u32)>, Error> {
        let start = self.apc.len();
        let mut fits = true;
        let mut ctx = self.ctx.image();
        if max_inline_cols.is_some() {
            ctx.fit_viewport = true;
            ctx.rows = 1;
        }
        let (_, _, cols, rows, id) = image::render_if(
            kind,
            payload,
            ctx,
            self.dir,
            self.scratch,
            self.seq,
            self.pid,
            self.apc,
            &mut self.shm_room,
            |id, cols, rows| {
                fits = max_inline_cols
                    .is_none_or(|max_cols| rows == 1 && usize::from(cols) <= max_cols);
                if !fits {
                    return Ok(None);
                }
                // Reserve room for the placeholder and the optional leading newline.
                let placeholder = 1 + usize::from(rows) * (26 + usize::from(cols) * 16);
                let limit = (ipc::MAX_PAYLOAD as usize)
                    .checked_sub(ansi_bytes.saturating_add(placeholder))
                    .filter(|&limit| start <= limit)
                    .ok_or(Error::ResourceLimit)?;
                Ok((!self.ids.contains(&id)).then_some(limit))
            },
        )?;
        if self.apc.len() > start {
            self.ids.insert(id);
            self.uploads.push(Upload {
                image_id: id,
                start,
                end: self.apc.len(),
            });
        }
        Ok(fits.then_some((cols, rows, id)))
    }

    fn emit(&mut self, span: md::Span, cols: u16, rows: u16, id: u32, ansi: &mut Vec<u8>) {
        if let Some(coords) = &mut self.coords {
            coords.push(SpanCoord {
                outer_start: span.outer_start,
                outer_end: span.outer_end,
                image_id: id,
                cols,
                rows,
            });
        }
        self.ctx.emit_graphic(id, cols, rows, ansi);
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "cell formatting uses borrowed source and frame offsets"
    )]
    fn cell(
        &mut self,
        raw: &[u8],
        width: usize,
        refs: &style::References,
        ansi: &mut Vec<u8>,
        src: &[u8],
        spans: &[md::Span],
        row_bytes: usize,
    ) -> Result<usize, Error> {
        if raw.is_empty() {
            return Ok(self.ansi_limit());
        }
        // Cells borrow the document. Only promote spans accepted by both the
        // lexical scanner and the table cell's inline parser.
        let start = raw.as_ptr().addr() - src.as_ptr().addr();
        let palette = self.ctx.palette;
        style::inline_with_math(
            raw,
            palette,
            ansi,
            refs,
            Some(&mut |range, ansi| {
                let span_start = start + range.start;
                let span_end = start + range.end;
                let index = spans.partition_point(|span| (span.outer_start as usize) < span_start);
                let Some(span) = spans.get(index).filter(|span| {
                    span.outer_start as usize == span_start
                        && span.outer_end as usize == span_end
                        && span.kind() == Kind::TexInline
                }) else {
                    return Ok(false);
                };
                let remaining = width.saturating_sub(style::width(ansi));
                if remaining == 0 {
                    return Ok(false);
                }
                let payload = &src[span.inner_start as usize..span.inner_end() as usize];
                match self.render(
                    span.kind(),
                    payload,
                    Some(remaining),
                    row_bytes.saturating_add(ansi.len()),
                ) {
                    Ok(Some((cols, rows, id))) => {
                        self.emit(*span, cols, rows, id, ansi);
                        ansi.pop();
                        Ok(true)
                    }
                    Err(Error::ResourceLimit) => Err(Error::ResourceLimit),
                    _ => Ok(false),
                }
            }),
        )?;
        Ok(self.ansi_limit())
    }
}

#[expect(clippy::too_many_arguments, reason = "caller owns render resources")]
fn document(
    src: &[u8],
    ctx: ComposeCtx<'_>,
    dir: Option<BorrowedFd<'_>>,
    scratch: &mut ComposeScratch,
    seq: &mut u32,
    pid: u32,
    out: &mut ComposeOut,
    anchors: Anchors,
) -> Result<(), Error> {
    md::scan(src, &mut scratch.spans);
    scratch.coords.clear();
    scratch.image_ids.clear();
    let mut images = Images {
        ctx,
        dir,
        scratch: &mut scratch.image,
        ids: &mut scratch.image_ids,
        seq,
        pid,
        apc: &mut out.apc,
        uploads: &mut out.uploads,
        coords: anchors.extmarks().then_some(&mut scratch.coords),
        shm_room: image::SHM_FRAME_BYTES,
    };
    let mut cursor = 0;
    let mut state = style::State::with_width(ctx.cols);
    state.prepare(src);
    let prose = |images: &mut Images<'_, '_>,
                 state: &mut style::State,
                 ansi: &mut Vec<u8>,
                 blocks: &mut Vec<_>,
                 start: usize,
                 end: usize| {
        style::emit_blocks_with_cells(
            &src[start..end],
            ctx.palette,
            state,
            ansi,
            images.ansi_limit(),
            |a, b, c, d| {
                if anchors.blocks() {
                    blocks.push((start + a, start + b, c, d));
                }
            },
            &mut |raw, width, refs, rendered, row_bytes| {
                images.cell(raw, width, refs, rendered, src, &scratch.spans, row_bytes)
            },
        )
    };
    for span in &scratch.spans {
        let start = span.outer_start as usize;
        let end = span.outer_end as usize;
        // Native tables own a complete source range. Their cells resolve inline
        // spans through the same image cache without splitting the table block.
        if state.overlaps_table(start, end) {
            continue;
        }
        prose(
            &mut images,
            &mut state,
            &mut out.ansi,
            &mut out.blocks,
            cursor,
            start,
        )?;
        let payload = &src[span.inner_start as usize..span.inner_end() as usize];
        let ansi_start = out.ansi.len();
        match images.render(span.kind(), payload, None, out.ansi.len()) {
            Ok(Some((cols, rows, id))) => {
                let inline = span.kind() == Kind::TexInline && rows == 1;
                if !inline && !out.ansi.is_empty() && !out.ansi.ends_with(b"\n") {
                    out.ansi.push(b'\n');
                }
                images.emit(*span, cols, rows, id, &mut out.ansi);
                if inline {
                    out.ansi.pop();
                }
            }
            Err(Error::ResourceLimit) => return Err(Error::ResourceLimit),
            _ => style::emit_blocks_bounded(
                &src[start..end],
                ctx.palette,
                &mut state,
                &mut out.ansi,
                images.ansi_limit(),
                |_, _, _, _| {},
            )?,
        }
        if anchors.blocks() {
            out.blocks.push((start, end, ansi_start, out.ansi.len()));
        }
        cursor = end;
        if span.kind() != Kind::TexInline
            && src.get(cursor.wrapping_sub(1)) != Some(&b'\n')
            && src.get(cursor) == Some(&b'\n')
            && out.ansi.ends_with(b"\n")
        {
            cursor += 1;
        }
    }
    prose(
        &mut images,
        &mut state,
        &mut out.ansi,
        &mut out.blocks,
        cursor,
        src.len(),
    )?;
    if anchors.extmarks() {
        ipc::compose_extmarks(src, &scratch.coords, &mut scratch.lines, &mut out.extmarks);
    }
    out.check_budget()
}

fn paginate(out: &mut ComposeOut, ctx: ComposeCtx<'_>) {
    if ctx.skip_lines == 0 && ctx.max_lines == 0 {
        return;
    }
    let full = core::mem::take(&mut out.ansi);
    crate::viewport::append(
        &full,
        ctx.cols,
        ctx.skip_lines,
        ctx.max_lines,
        &mut out.ansi,
    );
}

/// Append uploads referenced by visible placeholder colours. Encoded uploads must
/// use Base64 transport so replay does not depend on one-shot SHM names.
pub fn append_visible_uploads(frame: &ComposeOut, page: &[u8], out: &mut Vec<u8>) {
    if frame.uploads.is_empty() {
        return;
    }
    let mut visible = Vec::new();
    visible_ids(page, &mut visible);
    for upload in &frame.uploads {
        if visible.binary_search(&upload.image_id).is_ok() {
            out.extend_from_slice(&frame.apc[upload.start..upload.end]);
        }
    }
}

pub(crate) fn visible_ids(page: &[u8], visible: &mut Vec<u32>) {
    const FG: &[u8] = b"\x1b[38;2;";
    visible.clear();
    for start in memchr::memmem::find_iter(page, FG) {
        let tail = &page[start + FG.len()..];
        let Some(end) = memchr::memchr(b'm', &tail[..tail.len().min(12)]) else {
            continue;
        };
        let row = &tail[end + 1..];
        let row = row.strip_prefix(b"\n").unwrap_or(row);
        if !row.starts_with(&kitty::PLACEHOLDER_UTF8) {
            continue;
        }
        let mut channels = tail[..end].split(|&byte| byte == b';');
        let mut next = || {
            core::str::from_utf8(channels.next()?)
                .ok()?
                .parse::<u8>()
                .ok()
        };
        if let (Some(r), Some(g), Some(b)) = (next(), next(), next())
            && let Some(id) = kitty::placeholder_id([r, g, b], row)
        {
            visible.push(id);
        }
    }
    visible.sort_unstable();
    visible.dedup();
}
