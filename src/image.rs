//! Graphical compilation, rasterization, caching and transport staging.
use crate::{
    cache,
    err::Error,
    fmt::{self, Kind},
    kitty::{self, StageParams},
    raster,
    scene::RenderScratch,
    theme::{ColorToken, Palette},
    vram,
};
use rustix::fd::BorrowedFd;

#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    pub cell_w: u16,
    pub cell_h: u16,
    pub cols: u16,
    pub rows: u16,
    pub max_w: u16,
    pub palette: &'a Palette,
    pub is_tmux: bool,
    pub is_remote: bool,
}

#[derive(Debug, Default)]
pub struct Scratch {
    pub render: RenderScratch,
    pub lz4: Vec<u8>,
    pub b64: Vec<u8>,
    cache: vram::Cache,
}
impl Scratch {
    pub fn reset(&mut self) {
        self.render.reset();
        self.lz4.clear();
        self.b64.clear();
        if self.lz4.capacity() > 8 * 1024 * 1024 {
            self.lz4.shrink_to(256 * 1024);
        }
        if self.b64.capacity() > 8 * 1024 * 1024 {
            self.b64.shrink_to(256 * 1024);
        }
    }
}

/// Render a single graphical span payload using L1/L2 caches and Hourglass Scene IR.
/// Returns `(w, h, cols, rows, image_id)`.
///
/// # Errors
/// Returns `Error` if scene parsing or rasterization fails.
#[expect(
    clippy::too_many_arguments,
    reason = "coordinates context, caching, scratch buffers, and staging parameters"
)]
pub fn render(
    kind: Kind,
    payload: &[u8],
    ctx: Context<'_>,
    l2_dir: Option<BorrowedFd<'_>>,
    scratch: &mut Scratch,
    shm_seq: &mut u32,
    pid: u32,
    apc_out: &mut Vec<u8>,
) -> Result<(u16, u16, u16, u16, u32), Error> {
    let fg = ctx.palette.color(ColorToken::FG_PRIMARY);
    let fg_rgb = [fg.r(), fg.g(), fg.b()];
    let mut key = cache::blake3_key(
        ctx.cell_w,
        ctx.cell_h,
        ctx.max_w,
        kind as u8,
        fg_rgb,
        ctx.palette.is_dark,
        payload,
    );

    // The scene/compiler revision and complete palette invalidate persisted pixels.
    let mut identity = blake3::Hasher::new();
    identity.update(&key);
    identity.update(&ctx.palette.theme_hash.to_le_bytes());
    identity.update(b"ii-image-8-transparent");
    key = *identity.finalize().as_bytes();

    let mut raw_tag = [0; 8];
    raw_tag.copy_from_slice(&key[..8]);
    let tag = vram::Tag::new(u64::from_le_bytes(raw_tag) | 1).ok_or(Error::CacheCorrupt)?;
    let image_id = tag.image_id().get();
    if let Some(hit) = scratch.cache.lookup(tag) {
        let params = StageParams {
            image_id,
            w: hit.w,
            h: hit.h,
            cols: hit.cols,
            rows: hit.rows,
            is_tmux: ctx.is_tmux,
        };
        stage(
            &hit.rgba,
            params,
            ctx.is_remote,
            shm_seq,
            pid,
            apc_out,
            &mut scratch.b64,
        );
        return Ok((hit.w, hit.h, hit.cols, hit.rows, image_id));
    }

    let cached = if let Some(dir) = l2_dir {
        if let Ok(Some((w, h))) =
            cache::read_blob(dir, &key, &mut scratch.render.rgba_buf, &mut scratch.lz4)
        {
            if let (Ok(w), Ok(h)) = (u16::try_from(w), u16::try_from(h)) {
                Some((w, h))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let (w, h) = if let Some(hit) = cached {
        hit
    } else {
        scratch.render.scene.clear();
        let (w, h) = if kind == Kind::Png {
            let (w, h) = raster::decode_png(payload, &mut scratch.render.rgba_buf)?;
            raster::fit_rgba(
                w,
                h,
                ctx.cell_w,
                ctx.cell_h,
                ctx.max_w,
                &mut scratch.render.rgba_buf,
            )?
        } else {
            fmt::dispatch_with_palette(kind, payload, ctx.palette, &mut scratch.render.scene)?;
            let (w, h, _, _) = raster::rasterize_scene(
                &scratch.render.scene,
                ctx.palette,
                ctx.cell_w,
                ctx.cell_h,
                ctx.max_w,
                &mut scratch.render.rgba_buf,
            )?;
            (w, h)
        };
        if let Some(dir) = l2_dir {
            let _ = cache::write_blob(
                dir,
                &key,
                u32::from(w),
                u32::from(h),
                &scratch.render.rgba_buf,
                &mut scratch.lz4,
            );
        }
        (w, h)
    };

    let cols = w.div_ceil(ctx.cell_w.max(1)).max(1);
    let rows = h.div_ceil(ctx.cell_h.max(1)).max(1);
    scratch
        .cache
        .insert(tag, cols, rows, w, h, &scratch.render.rgba_buf);

    let params = StageParams {
        image_id,
        w,
        h,
        cols,
        rows,
        is_tmux: ctx.is_tmux,
    };

    stage(
        &scratch.render.rgba_buf,
        params,
        ctx.is_remote,
        shm_seq,
        pid,
        apc_out,
        &mut scratch.b64,
    );

    Ok((w, h, cols, rows, image_id))
}

fn stage(
    rgba: &[u8],
    params: StageParams,
    remote: bool,
    seq: &mut u32,
    pid: u32,
    out: &mut Vec<u8>,
    b64: &mut Vec<u8>,
) {
    if (remote || kitty::stage_shm(rgba, params, seq, pid, out).is_err())
        && kitty::stage_png(rgba, params, out, b64).is_err()
    {
        kitty::stage_base64(rgba, params, out, b64);
    }
}
