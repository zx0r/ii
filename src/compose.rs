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
}

impl<'a> ComposeCtx<'a> {
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
    out.clear();
    let mut normalized = Vec::new();
    if matches!(kind, Kind::Csv | Kind::Tsv) {
        crate::fmt::csv::to_markdown(
            src,
            if kind == Kind::Tsv { b'\t' } else { b',' },
            &mut normalized,
        )?;
        return compose_document(&normalized, ctx, dir, scratch, seq, pid, out);
    }
    if kind == Kind::Ipynb {
        crate::fmt::ipynb::to_markdown(src, &mut normalized)?;
        return compose_document(&normalized, ctx, dir, scratch, seq, pid, out);
    }
    if kind == Kind::Tex && src.windows(14).any(|s| s == b"\\documentclass") {
        let text = String::from_utf8_lossy(src)
            .replace("\\[", "$$")
            .replace("\\]", "$$")
            .replace("\\(", "$")
            .replace("\\)", "$");
        return compose_document(text.as_bytes(), ctx, dir, scratch, seq, pid, out);
    }
    if kind == Kind::Md {
        compose_document(src, ctx, dir, scratch, seq, pid, out)?;
    } else {
        let (_, _, cols, rows, id) = image::render(
            kind,
            src,
            ctx.image(),
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
        kitty::emit_placeholder(id, cols, rows, &mut out.ansi);
        paginate(out, ctx);
    }
    Ok(())
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
    md::scan(src, &mut scratch.spans);
    scratch.coords.clear();
    scratch.image_ids.clear();
    let mut cursor = 0;
    let mut state = style::State::default();
    for index in 0..scratch.spans.len() {
        let span = scratch.spans[index];
        let start = span.outer_start as usize;
        style::emit_prose(&src[cursor..start], ctx.palette, &mut state, &mut out.ansi);
        let payload = &src[span.inner_start as usize..span.inner_end() as usize];
        let apc_start = out.apc.len();
        match image::render(
            span.kind(),
            payload,
            ctx.image(),
            dir,
            &mut scratch.image,
            seq,
            pid,
            &mut out.apc,
        ) {
            Ok((_, _, cols, rows, id)) => {
                if scratch.image_ids.insert(id) {
                    out.uploads.push(Upload {
                        image_id: id,
                        start: apc_start,
                        end: out.apc.len(),
                    });
                } else {
                    out.apc.truncate(apc_start);
                }
                scratch.coords.push(SpanCoord {
                    outer_start: span.outer_start,
                    outer_end: span.outer_end,
                    image_id: id,
                    cols,
                    rows,
                });
                let inline = span.kind() == Kind::TexInline && rows == 1;
                if !inline && !out.ansi.is_empty() && !out.ansi.ends_with(b"\n") {
                    out.ansi.push(b'\n');
                }
                kitty::emit_placeholder(id, cols, rows, &mut out.ansi);
                if inline {
                    out.ansi.pop();
                }
            }
            Err(_) => style::emit_prose(
                &src[start..span.outer_end as usize],
                ctx.palette,
                &mut state,
                &mut out.ansi,
            ),
        }
        cursor = span.outer_end as usize;
        if span.kind() != Kind::TexInline
            && src.get(cursor.wrapping_sub(1)) != Some(&b'\n')
            && src.get(cursor) == Some(&b'\n')
            && out.ansi.ends_with(b"\n")
        {
            cursor += 1;
        }
    }
    style::emit_prose(&src[cursor..], ctx.palette, &mut state, &mut out.ansi);
    ipc::compose_extmarks(src, &scratch.coords, &mut scratch.lines, &mut out.extmarks);
    paginate(out, ctx);
    Ok(())
}

/// Select complete logical lines without overflow or inventing a trailing newline.
pub fn crop_lines(bytes: &mut Vec<u8>, skip: u32, max: u32) {
    let mut start = 0;
    for _ in 0..skip {
        let Some(nl) = memchr::memchr(b'\n', &bytes[start..]) else {
            bytes.clear();
            return;
        };
        start += nl + 1;
    }
    let end = if max != 0 {
        let mut cursor = start;
        for _ in 0..max {
            let Some(nl) = memchr::memchr(b'\n', &bytes[cursor..]) else {
                cursor = bytes.len();
                break;
            };
            cursor += nl + 1;
        }
        cursor
    } else {
        bytes.len()
    };
    bytes.copy_within(start..end, 0);
    bytes.truncate(end - start);
}

fn paginate(out: &mut ComposeOut, ctx: ComposeCtx<'_>) {
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
    for upload in &frame.uploads {
        let [_, r, g, b] = upload.image_id.to_be_bytes();
        let marker = format!("\x1b[38;2;{r};{g};{b}m\u{10eeee}");
        if memchr::memmem::find(page, marker.as_bytes()).is_some() {
            out.extend_from_slice(&frame.apc[upload.start..upload.end]);
        }
    }
}
