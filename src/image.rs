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

pub(crate) const SHM_FRAME_BYTES: usize = 32 * 1024 * 1024;

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
    pub fit_viewport: bool,
}

impl Context<'_> {
    fn cache_key(self, kind: Kind, payload: &[u8]) -> [u8; 32] {
        let fg = self.palette.color(ColorToken::FG_PRIMARY);
        let fg_rgb = [fg.r(), fg.g(), fg.b()];
        let key = cache::blake3_key(
            self.cell_w,
            self.cell_h,
            self.max_w,
            kind as u8,
            fg_rgb,
            self.palette.is_dark,
            payload,
        );

        // The scene/compiler revision and complete palette invalidate persisted pixels.
        let mut identity = blake3::Hasher::new();
        identity.update(&key);
        identity.update(&self.palette.theme_hash.to_le_bytes());
        identity.update(b"ii-image-19-grid-fit");
        identity.update(&[u8::from(self.fit_viewport)]);
        if self.fit_viewport {
            identity.update(&self.rows.to_le_bytes());
        }
        *identity.finalize().as_bytes()
    }

    fn vector_scale(self, kind: Kind) -> f32 {
        if matches!(kind, Kind::Tex | Kind::TexInline | Kind::Chem) {
            (f32::from(self.cell_h.max(1)) / 18.0).min(4.0)
        } else {
            (f32::from(self.cell_h.max(1)) / 20.0).clamp(1.0, 4.0)
        }
    }
}

#[derive(Debug, Default)]
pub struct Scratch {
    pub render: RenderScratch,
    pub lz4: Vec<u8>,
    pub b64: Vec<u8>,
    png: Vec<u8>,
    cache: vram::Cache,
}
impl Scratch {
    pub fn reset(&mut self) {
        self.render.reset();
        self.lz4.clear();
        self.b64.clear();
        self.png.clear();
        if self.lz4.capacity() > 8 * 1024 * 1024 {
            self.lz4.shrink_to(256 * 1024);
        }
        if self.png.capacity() > 8 * 1024 * 1024 {
            self.png.shrink_to(256 * 1024);
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
    let mut shm_room = SHM_FRAME_BYTES;
    render_if(
        kind,
        payload,
        ctx,
        l2_dir,
        scratch,
        shm_seq,
        pid,
        apc_out,
        &mut shm_room,
        |_, _, _| Ok(Some(crate::ipc::MAX_PAYLOAD as usize)),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "shares caller-owned staging buffers with render"
)]
pub(crate) fn render_if(
    kind: Kind,
    payload: &[u8],
    ctx: Context<'_>,
    l2_dir: Option<BorrowedFd<'_>>,
    scratch: &mut Scratch,
    shm_seq: &mut u32,
    pid: u32,
    apc_out: &mut Vec<u8>,
    shm_room: &mut usize,
    upload_limit: impl FnOnce(u32, u16, u16) -> Result<Option<usize>, Error>,
) -> Result<(u16, u16, u16, u16, u32), Error> {
    let key = ctx.cache_key(kind, payload);

    let mut raw_tag = [0; 8];
    raw_tag.copy_from_slice(&key[..8]);
    let tag = vram::Tag::new(u64::from_le_bytes(raw_tag) | 1).ok_or(Error::CacheCorrupt)?;
    let image_id = tag.image_id().get();
    let upload = |w, h, cols, rows, rgba: &[u8], b64: &mut Vec<u8>, png: &mut Vec<u8>| {
        if let Some(limit) = upload_limit(image_id, cols, rows)? {
            let params = StageParams {
                image_id,
                w,
                h,
                cols,
                rows,
                is_tmux: ctx.is_tmux,
            };
            if !ctx.is_remote
                && rgba.len() <= *shm_room
                && kitty::stage_shm_bounded(rgba, params, shm_seq, pid, apc_out, limit).is_ok()
            {
                *shm_room -= rgba.len();
            } else {
                match kitty::stage_png_bounded(rgba, params, apc_out, b64, png, limit) {
                    Err(Error::PngDecode) => kitty::stage_raw(rgba, params, apc_out, b64, limit)?,
                    result => result?,
                }
            }
        }
        Ok((w, h, cols, rows, image_id))
    };
    if let Some(hit) = scratch.cache.lookup(tag) {
        return upload(
            hit.w,
            hit.h,
            hit.cols,
            hit.rows,
            &hit.rgba,
            &mut scratch.b64,
            &mut scratch.png,
        );
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
            let preferred_scale = if ctx.fit_viewport && kind != Kind::TexInline {
                4.0
            } else {
                ctx.vector_scale(kind)
            };
            let max_h = if ctx.fit_viewport {
                ctx.rows.max(1).saturating_mul(ctx.cell_h.max(1))
            } else {
                u16::MAX
            };
            let (w, h, _, _) = raster::rasterize_scene_fitted(
                &scratch.render.scene,
                ctx.palette,
                (ctx.cell_w, ctx.cell_h),
                (ctx.max_w, max_h),
                preferred_scale,
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

    upload(
        w,
        h,
        cols,
        rows,
        &scratch.render.rgba_buf,
        &mut scratch.b64,
        &mut scratch.png,
    )
}
