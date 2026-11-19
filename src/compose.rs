//! Frame orchestrator: disjoint borrow `Session`, `CompileCtx`, 14-entry `FORMAT_TABLE` O(1) dispatch,
//! and L0 VRAM → L1 RAM → L2 Disk cache cascade.

#![expect(
    clippy::cast_possible_truncation,
    reason = "bounded terminal cell dimensions and span offsets fit in u16/u32"
)]

use core::fmt::Write as _;

use crate::{
    cache,
    cfg::Cfg,
    chart, chem, csv, dot,
    err::Error,
    heal,
    ipc::{self, Extmark},
    ir::{Kind, Scratch, Span},
    json::Nav,
    kgp, raster, scan, style, svg,
    theme::{Idx, Palette},
    vram::{self, Payload},
};

pub struct Session {
    pub vram: vram::Table,
    pub l1: vram::Cache,
    pub heal: heal::State,
    pub shm_seq: u32,
    pub pid: u32,
    pub request_id: u64,
}

impl Session {
    #[must_use]
    pub fn new(pid: u32) -> Self {
        Self {
            vram: vram::Table::new(),
            l1: vram::Cache::new(),
            heal: heal::State::new(),
            shm_seq: 0,
            pid,
            request_id: 0,
        }
    }
}

/// Disjoint context passed through `FORMAT_TABLE` to every frontend.
pub struct CompileCtx<'a> {
    pub palette: &'a Palette,
    pub width: u16,
    pub heal: &'a mut heal::State,
    pub svg: &'a mut String,
    pub aux: &'a mut Vec<u8>,
}

type CompileFn = fn(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error>;

static FORMAT_TABLE: [CompileFn; 14] = [
    compile_math_display,
    compile_math_inline,
    compile_mermaid,
    compile_raw_svg,
    compile_vegalite,
    compile_plotly,
    compile_csv,
    compile_dot,
    compile_quarto,
    compile_callout,
    compile_notebook_out,
    compile_pipe_table,
    compile_chem,
    compile_typst_math,
];

fn compile_latex_core(latex_bytes: &[u8], display: bool, ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    ctx.aux.clear();
    ctx.heal.heal(latex_bytes, ctx.aux);
    let healed = core::str::from_utf8(ctx.aux).map_err(|_| Error::LatexCompile)?;

    let ast = ratex_parser::parse_latex(healed).map_err(|_| Error::LatexCompile)?;
    let layout_opts = ratex_layout::LayoutOptions {
        display_mode: display,
        ..ratex_layout::LayoutOptions::default()
    };
    let dlist = ratex_layout::layout(&ast, &layout_opts);
    let svg_opts = ratex_svg::SvgOptions::default();
    let raw_svg = ratex_svg::render_to_svg(&dlist, &svg_opts);

    ctx.aux.clear();
    svg::rewrite_colors(raw_svg.as_bytes(), ctx.palette, ctx.aux);
    let themed = core::str::from_utf8(ctx.aux).map_err(|_| Error::SvgParse)?;
    ctx.svg.clear();
    ctx.svg.push_str(themed);
    Ok(())
}

fn compile_math_display(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    compile_latex_core(src, true, ctx)
}

fn compile_math_inline(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    compile_latex_core(src, false, ctx)
}

fn compile_mermaid(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    let text = core::str::from_utf8(src).map_err(|_| Error::MermaidCompile)?;
    let raw_svg = mermaid_svg::render(text).map_err(|_| Error::MermaidCompile)?;
    ctx.aux.clear();
    svg::rewrite_colors(raw_svg.as_bytes(), ctx.palette, ctx.aux);
    let themed = core::str::from_utf8(ctx.aux).map_err(|_| Error::SvgParse)?;
    ctx.svg.clear();
    ctx.svg.push_str(themed);
    Ok(())
}

fn compile_raw_svg(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    ctx.aux.clear();
    svg::rewrite_colors(src, ctx.palette, ctx.aux);
    let themed = core::str::from_utf8(ctx.aux).map_err(|_| Error::SvgParse)?;
    ctx.svg.clear();
    ctx.svg.push_str(themed);
    Ok(())
}

fn compile_vegalite(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    ctx.svg.clear();
    chart::vegalite(Nav::new(src), ctx.palette, ctx.width, ctx.svg);
    Ok(())
}

fn compile_plotly(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    ctx.svg.clear();
    chart::plotly(Nav::new(src), ctx.palette, ctx.width, ctx.svg);
    Ok(())
}

fn compile_csv(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    ctx.svg.clear();
    csv::render_csv(src, ctx.palette, ctx.width, ctx.svg);
    Ok(())
}

fn compile_dot(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    ctx.svg.clear();
    dot::render(src, ctx.palette, ctx.width, ctx.svg);
    Ok(())
}

fn compile_quarto(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    ctx.svg.clear();
    csv::render_csv(src, ctx.palette, ctx.width, ctx.svg);
    Ok(())
}

fn compile_callout(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    ctx.svg.clear();
    let w = ctx.width.max(240);
    let bg = core::str::from_utf8(ctx.palette.get(Idx::BgSurface).hex()).unwrap_or("#313244");
    let fg = core::str::from_utf8(ctx.palette.get(Idx::Fg).hex()).unwrap_or("#cdd6f4");
    let accent = core::str::from_utf8(ctx.palette.get(Idx::HeadingSub).hex()).unwrap_or("#89b4fa");
    let _ = write!(
        ctx.svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="64" viewBox="0 0 {w} 64"><rect width="{w}" height="64" rx="6" fill="{bg}"/><rect width="4" height="64" rx="2" fill="{accent}"/><text x="16" y="36" fill="{fg}" font-family="sans-serif" font-size="13">"#
    );
    let line = src
        .split(|&b| b == b'\n')
        .nth(1)
        .unwrap_or(src);
    for ch in core::str::from_utf8(line).unwrap_or("").chars() {
        match ch {
            '&' => ctx.svg.push_str("&amp;"),
            '<' => ctx.svg.push_str("&lt;"),
            '>' => ctx.svg.push_str("&gt;"),
            '"' => ctx.svg.push_str("&quot;"),
            _ => ctx.svg.push(ch),
        }
    }
    ctx.svg.push_str("</text></svg>");
    Ok(())
}

fn compile_notebook_out(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    compile_raw_svg(src, ctx)
}

fn compile_pipe_table(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    ctx.svg.clear();
    csv::render_pipe_table(src, ctx.palette, ctx.width, ctx.svg);
    Ok(())
}

fn compile_chem(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    let mut chem_buf = [0u8; 512];
    ctx.aux.clear();
    chem::transpile(src, ctx.aux);
    let len = ctx.aux.len().min(chem_buf.len());
    chem_buf[..len].copy_from_slice(&ctx.aux[..len]);
    compile_latex_core(&chem_buf[..len], false, ctx)
}

fn compile_typst_math(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    compile_latex_core(src, true, ctx)
}

/// O(1) dispatch via `Kind` discriminant as array index.
#[inline]
pub fn dispatch(span: Span, src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    FORMAT_TABLE[span.kind() as usize](span.payload(src), ctx)
}

fn stage_transport(
    rgba: &[u8],
    w: u16,
    h: u16,
    image_id: u32,
    cols: u16,
    rows: u16,
    cfg: &Cfg,
    session: &mut Session,
    apc: &mut Vec<u8>,
    b64: &mut Vec<u8>,
) {
    let is_tmux = cfg.tty.is_tmux != 0;
    if cfg.tty.is_remote == 0 {
        if kgp::stage_shm(
            rgba,
            w,
            h,
            image_id,
            cols,
            rows,
            is_tmux,
            &mut session.shm_seq,
            session.pid,
            apc,
        )
        .is_ok()
        {
            return;
        }
    }
    kgp::stage_base64(rgba, w, h, image_id, cols, rows, is_tmux, apc, b64);
}

fn resolve_span_image(
    sp: Span,
    src: &[u8],
    cfg: &Cfg,
    session: &mut Session,
    scratch: &mut Scratch,
) -> Result<Payload, Error> {
    let payload_bytes = sp.payload(src);
    let fg_rgb = cfg.palette.get(Idx::Fg).rgb;
    let tag = vram::compute_tag(
        cfg.tty.cell_w,
        cfg.tty.cell_h,
        cfg.tty.cols,
        sp.kind() as u8,
        fg_rgb,
        payload_bytes,
    );
    let image_id = vram::image_id_from_tag(tag);

    // Tier 0: L0 VRAM Tag Table hit -> 0 bytes transmitted, emit placeholder only
    if let Some(hit) = session.vram.lookup(tag) {
        return Ok(hit);
    }

    // Tier 1: L1 RAM RGBA Cache hit -> transmit cached RGBA without SVG compile or rasterize
    if let Some(rep) = session.l1.lookup(tag) {
        let cols = rep.cols;
        let rows = rep.rows;
        let w = rep.w;
        let h = rep.h;
        scratch.rgba_buf.clear();
        scratch.rgba_buf.extend_from_slice(&rep.rgba);
        stage_transport(
            &scratch.rgba_buf,
            w,
            h,
            image_id,
            cols,
            rows,
            cfg,
            session,
            &mut scratch.apc_buf,
            &mut scratch.b64_buf,
        );
        let entry = Payload {
            image_id,
            cols,
            rows,
        };
        session.vram.insert(tag, entry);
        return Ok(entry);
    }

    let max_w = cfg.max_width_px();
    let cw = cfg.tty.cell_w.max(1);
    let ch = cfg.tty.cell_h.max(1);

    // Tier 2: L2 Disk Cache check (BLAKE3 computed only on L1 miss)
    let b3_key = cache::blake3_key(cw, ch, max_w, cfg.palette.is_dark, payload_bytes);
    if let Some(ref dir_fd) = cfg.cache_dir {
        if let Ok(Some((w32, h32))) = cache::read_blob(
            dir_fd.as_fd(),
            &b3_key,
            &mut scratch.rgba_buf,
            &mut scratch.lz4_buf,
        ) {
            let w = w32 as u16;
            let h = h32 as u16;
            let cols = w.div_ceil(cw).max(1);
            let rows = h.div_ceil(ch).max(1);
            session
                .l1
                .insert(tag, cols, rows, w, h, &scratch.rgba_buf);
            stage_transport(
                &scratch.rgba_buf,
                w,
                h,
                image_id,
                cols,
                rows,
                cfg,
                session,
                &mut scratch.apc_buf,
                &mut scratch.b64_buf,
            );
            let entry = Payload {
                image_id,
                cols,
                rows,
            };
            session.vram.insert(tag, entry);
            return Ok(entry);
        }
    }

    // Full miss: compile frontend -> SVG -> rasterize -> populate L2, L1, L0 -> stage KGP
    {
        let mut ctx = CompileCtx {
            palette: cfg.palette,
            width: max_w,
            heal: &mut session.heal,
            svg: &mut scratch.svg_buf,
            aux: &mut scratch.aux_buf,
        };
        dispatch(sp, src, &mut ctx)?;
    }

    let (w, h, cols, rows) = raster::rasterize(
        scratch.svg_buf.as_bytes(),
        cw,
        ch,
        max_w,
        &mut scratch.rgba_buf,
    )?;

    if let Some(ref dir_fd) = cfg.cache_dir {
        let _ = cache::write_blob(
            dir_fd.as_fd(),
            &b3_key,
            u32::from(w),
            u32::from(h),
            &scratch.rgba_buf,
            &mut scratch.lz4_buf,
        );
    }

    session
        .l1
        .insert(tag, cols, rows, w, h, &scratch.rgba_buf);
    stage_transport(
        &scratch.rgba_buf,
        w,
        h,
        image_id,
        cols,
        rows,
        cfg,
        session,
        &mut scratch.apc_buf,
        &mut scratch.b64_buf,
    );
    let entry = Payload {
        image_id,
        cols,
        rows,
    };
    session.vram.insert(tag, entry);
    Ok(entry)
}

/// Orchestrate full Markdown/LaTeX/SVG/Chart document `src` into `scratch.apc_buf` + `scratch.text_buf`.
pub fn compose_ansi(
    src: &[u8],
    cfg: &Cfg,
    session: &mut Session,
    scratch: &mut Scratch,
) -> Result<(), Error> {
    scratch.reset_frame();
    session.request_id = session.request_id.wrapping_add(1);
    scan::scan(src, &mut scratch.spans);

    let n_spans = scratch.spans.len();
    let mut cursor = 0usize;

    for idx in 0..n_spans {
        let sp = scratch.spans[idx];
        let start = (sp.outer_start as usize).min(src.len());
        let end = (sp.outer_end as usize).min(src.len());
        if start > cursor {
            style::render(
                &src[cursor..start],
                &[],
                cfg.tty.cols,
                cfg.palette,
                &mut scratch.text_buf,
            );
        }

        if let Ok(entry) = resolve_span_image(sp, src, cfg, session, scratch) {
            if !scratch.text_buf.is_empty() && scratch.text_buf.last() != Some(&b'\n') {
                scratch.text_buf.push(b'\n');
            }
            kgp::emit_placeholder(entry.image_id, entry.cols, entry.rows, &mut scratch.text_buf);
        } else {
            // Honest degradation: emit raw source slice styled as error/fallback text
            scratch
                .text_buf
                .extend_from_slice(cfg.palette.get(Idx::Error).sgr());
            scratch.text_buf.extend_from_slice(&src[start..end]);
            scratch.text_buf.extend_from_slice(b"\x1b[0m");
        }
        cursor = end.max(cursor);
    }

    if cursor < src.len() {
        style::render(
            &src[cursor..],
            &[],
            cfg.tty.cols,
            cfg.palette,
            &mut scratch.text_buf,
        );
    }

    scratch.shrink_if_bloated();
    Ok(())
}

/// Orchestrate `src` for Neovim/editor extmark overlays: KGP APC in `scratch.apc_buf`
/// and binary `Extmark` array in `scratch.text_buf`.
pub fn compose_extmarks(
    src: &[u8],
    cfg: &Cfg,
    session: &mut Session,
    scratch: &mut Scratch,
) -> Result<(), Error> {
    scratch.reset_frame();
    session.request_id = session.request_id.wrapping_add(1);
    scan::scan(src, &mut scratch.spans);

    let n_spans = scratch.spans.len();
    let mut byte_pos = 0usize;
    let mut cur_row = 0u16;

    for idx in 0..n_spans {
        let sp = scratch.spans[idx];
        let start = (sp.outer_start as usize).min(src.len());
        let end = (sp.outer_end as usize).min(src.len());
        if start > byte_pos {
            let newlines = src[byte_pos..start].iter().filter(|&&b| b == b'\n').count() as u16;
            cur_row = cur_row.saturating_add(newlines);
        }
        let start_row = cur_row;
        let span_newlines = src[start..end].iter().filter(|&&b| b == b'\n').count() as u16;
        let end_row = start_row.saturating_add(span_newlines);
        cur_row = end_row;
        byte_pos = end;

        if let Ok(entry) = resolve_span_image(sp, src, cfg, session, scratch) {
            let rec = Extmark {
                image_id: entry.image_id,
                start_row,
                start_col: 0,
                end_row,
                end_col: 0,
                cols: entry.cols,
                rows: entry.rows,
            };
            ipc::encode_extmarks(core::slice::from_ref(&rec), &mut scratch.text_buf);
        }
    }

    scratch.shrink_if_bloated();
    Ok(())
}
