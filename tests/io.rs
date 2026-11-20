//! Kernel boundary tests: L2 `PX03` disk cache, real POSIX SHM, `resvg`, and PNG codecs.

use core::sync::atomic::{AtomicU32, Ordering};
use std::{ffi::CString, path::PathBuf};

use rustix::{
    fd::{AsFd, OwnedFd},
    fs::{CWD, Mode, OFlags, mkdirat, openat, symlinkat},
    mm::{MapFlags, ProtFlags, mmap, munmap},
    process::getpid,
    shm::{self, OFlags as ShmFlags},
};

use ii::{
    cache::{self, blake3_key, read_blob, write_blob},
    err::Error,
    kitty::{self, StageParams},
    raster::{MAX_DIM, decode_png, rasterize, rasterize_scene},
    scene::{Path, Rect, Scene, Verb},
    theme::{ColorToken, DARK, LIGHT},
};

static DIR_SEQ: AtomicU32 = AtomicU32::new(0);

struct TempDir {
    path: PathBuf,
    fd: OwnedFd,
}

impl TempDir {
    fn new(tag: &str) -> Result<Self, Error> {
        let pid = getpid().as_raw_nonzero().get().unsigned_abs();
        let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("ii-io-{tag}-{pid}-{seq}"));
        mkdirat(CWD, &path, Mode::RWXU)?;
        let fd = openat(
            CWD,
            &path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        Ok(Self { path, fd })
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct ShmGuard(CString);

impl Drop for ShmGuard {
    fn drop(&mut self) {
        let _ = shm::unlink(self.0.as_c_str());
    }
}

#[test]
fn l2_cache_roundtrip_key_axes_corruption_and_nofollow() -> Result<(), Error> {
    let dir = TempDir::new("l2")?;
    let mut rgba_out = Vec::new();
    let mut lz4 = Vec::new();

    let key = blake3_key(8, 16, 640, 0, [244, 244, 245], true, b"E=mc^2");
    assert_eq!(
        read_blob(dir.fd.as_fd(), &key, &mut rgba_out, &mut lz4)?,
        None
    );

    assert_ne!(
        key,
        blake3_key(9, 16, 640, 0, [244, 244, 245], true, b"E=mc^2")
    );
    assert_ne!(
        key,
        blake3_key(8, 17, 640, 0, [244, 244, 245], true, b"E=mc^2")
    );
    assert_ne!(
        key,
        blake3_key(8, 16, 800, 0, [244, 244, 245], true, b"E=mc^2")
    );
    assert_ne!(
        key,
        blake3_key(8, 16, 640, 1, [244, 244, 245], true, b"E=mc^2")
    );
    assert_ne!(
        key,
        blake3_key(8, 16, 640, 0, [200, 244, 245], true, b"E=mc^2")
    );
    assert_ne!(
        key,
        blake3_key(8, 16, 640, 0, [244, 244, 245], false, b"E=mc^2")
    );

    let src: Vec<u8> = (0..(16 * 8 * 4))
        .map(|idx| u8::try_from(idx & 255).unwrap_or(0))
        .collect();
    write_blob(dir.fd.as_fd(), &key, 16, 8, &src, &mut lz4)?;
    assert_eq!(
        read_blob(dir.fd.as_fd(), &key, &mut rgba_out, &mut lz4)?,
        Some((16, 8))
    );
    assert_eq!(rgba_out, src);
    assert_eq!(
        write_blob(dir.fd.as_fd(), &key, 16, 8, &src[..10], &mut lz4),
        Err(Error::CacheCorrupt)
    );

    let k_bad = blake3_key(8, 16, 640, 0, [0; 3], true, b"corrupt");
    let mut hex = [0u8; 65];
    cache::hex_encode_cstr(&k_bad, &mut hex);
    let name = core::str::from_utf8(&hex[..64]).unwrap_or("bad");
    let _ = std::fs::write(
        dir.path.join(name),
        b"NOPE\x01\x00\x00\x00\x01\x00\x00\x00\xff",
    );
    assert_eq!(
        read_blob(dir.fd.as_fd(), &k_bad, &mut rgba_out, &mut lz4),
        Err(Error::CacheCorrupt)
    );

    let mut bad_lz4 = cache::Header {
        magic: cache::MAGIC,
        width: 4,
        height: 4,
    }
    .to_le_bytes()
    .to_vec();
    bad_lz4.extend_from_slice(&[0xFF; 8]);
    let _ = std::fs::write(dir.path.join(name), &bad_lz4);
    assert_eq!(
        read_blob(dir.fd.as_fd(), &k_bad, &mut rgba_out, &mut lz4),
        Err(Error::Lz4Decompress)
    );

    let k_sym = blake3_key(8, 16, 640, 0, [0; 3], true, b"symlink");
    cache::hex_encode_cstr(&k_sym, &mut hex);
    let sym = core::str::from_utf8(&hex[..64]).unwrap_or("sym");
    symlinkat("/etc/hosts", dir.fd.as_fd(), sym)?;
    assert!(read_blob(dir.fd.as_fd(), &k_sym, &mut rgba_out, &mut lz4).is_err());
    Ok(())
}

#[test]
fn posix_shm_stage_roundtrip_and_unlink() -> Result<(), Box<dyn core::error::Error>> {
    let pid = getpid().as_raw_nonzero().get().unsigned_abs();
    let mut seq = 0x2000_0000_u32;
    let shm_name = CString::new(format!("/ii-{pid:08x}-{:08x}", seq + 1))?;
    let _cleanup = ShmGuard(shm_name.clone());

    let rgba: Vec<u8> = (0..256).map(|idx| u8::try_from(idx).unwrap_or(0)).collect();
    let mut apc = Vec::new();
    let params = StageParams {
        image_id: 77,
        w: 8,
        h: 8,
        cols: 1,
        rows: 1,
        is_tmux: false,
    };
    kitty::stage_shm(&rgba, params, &mut seq, pid, &mut apc)?;

    let text = core::str::from_utf8(&apc)?;
    assert!(text.starts_with("\x1b_Ga=t,t=s,f=32,q=2,i=77,s=8,v=8,S=256;"));
    assert!(text.ends_with("\x1b_Ga=p,U=1,q=2,i=77,c=1,r=1\x1b\\"));

    let fd = shm::open(shm_name.as_c_str(), ShmFlags::RDONLY, Mode::empty())?;
    // SAFETY: `fd` is a valid shm object truncated to `rgba.len()`.
    let map = unsafe {
        mmap(
            core::ptr::null_mut(),
            rgba.len(),
            ProtFlags::READ,
            MapFlags::SHARED,
            &fd,
            0,
        )?
    };
    // SAFETY: `map` points to `rgba.len()` readable bytes.
    let slice = unsafe { core::slice::from_raw_parts(map.cast::<u8>(), rgba.len()) };
    assert_eq!(slice, rgba.as_slice());
    // SAFETY: `map` was returned by `mmap`.
    unsafe { munmap(map, rgba.len())? };
    Ok(())
}

#[test]
fn raster_svg_and_scene_palettes() -> Result<(), Error> {
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="20"><rect width="30" height="20" fill="#ff0000"/></svg>"##;
    let mut rgba = Vec::new();
    assert_eq!(rasterize(svg, 8, 16, 640, &mut rgba)?, (32, 32, 4, 2));
    assert_eq!(&rgba[..4], &[255, 0, 0, 255]);
    assert_eq!(
        rasterize(b"invalid", 8, 16, 640, &mut rgba),
        Err(Error::SvgParse)
    );

    let huge = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="{}"/>"#,
        MAX_DIM + 64
    );
    assert!(matches!(
        rasterize(huge.as_bytes(), 8, 16, 640, &mut rgba),
        Err(Error::ImageTooLarge { .. })
    ));

    let mut scene = Scene::with_capacity(8, 32);
    scene.width = 16.0;
    scene.height = 16.0;
    scene.push_rect(Rect {
        x: 0.0,
        y: 0.0,
        w: 16.0,
        h: 16.0,
        color: ColorToken::BG_PRIMARY,
        radius: 0,
        stroke_width: 0,
        flags: Rect::FILL,
    });
    scene.points.extend_from_slice(&[
        [2.0, 2.0],
        [14.0, 2.0],
        [14.0, 8.0],
        [8.0, 14.0],
        [4.0, 14.0],
        [2.0, 10.0],
        [2.0, 6.0],
    ]);
    scene.verbs.extend_from_slice(&[
        Verb::MoveTo,
        Verb::LineTo,
        Verb::QuadTo,
        Verb::CubicTo,
        Verb::Close,
    ]);
    scene.paths.push(Path {
        pt_start: 0,
        pt_count: 7,
        verb_start: 0,
        verb_count: 5,
        color: ColorToken::ACCENT,
        stroke_width: 2,
        flags: Path::STROKE,
        _pad: 0,
    });
    // Out-of-bounds path span must be safely skipped.
    scene.paths.push(Path {
        pt_start: 999,
        pt_count: 5,
        verb_start: 999,
        verb_count: 2,
        color: ColorToken::ACCENT,
        stroke_width: 1,
        flags: Path::STROKE,
        _pad: 0,
    });

    let _ = rasterize_scene(&scene, &DARK, 8, 16, 640, &mut rgba)?;
    assert_eq!(&rgba[..4], &DARK.color(ColorToken::BG_PRIMARY).to_array());
    let _ = rasterize_scene(&scene, &LIGHT, 8, 16, 640, &mut rgba)?;
    assert_eq!(&rgba[..4], &LIGHT.color(ColorToken::BG_PRIMARY).to_array());
    Ok(())
}

fn make_png(
    color: png::ColorType,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<Vec<u8>, Box<dyn core::error::Error>> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, width, height);
        enc.set_color(color);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header()?;
        writer.write_image_data(pixels)?;
    }
    Ok(buf)
}

#[test]
fn decode_png_all_color_types_and_corrupt() -> Result<(), Box<dyn core::error::Error>> {
    let mut rgba = Vec::new();

    let p_rgba = make_png(png::ColorType::Rgba, 2, 1, &[1, 2, 3, 4, 5, 6, 7, 8])?;
    assert_eq!(decode_png(&p_rgba, &mut rgba)?, (2, 1));
    assert_eq!(rgba, [1, 2, 3, 4, 5, 6, 7, 8]);

    let p_rgb = make_png(png::ColorType::Rgb, 2, 1, &[10, 20, 30, 40, 50, 60])?;
    assert_eq!(decode_png(&p_rgb, &mut rgba)?, (2, 1));
    assert_eq!(rgba, [10, 20, 30, 255, 40, 50, 60, 255]);

    let p_ga = make_png(png::ColorType::GrayscaleAlpha, 2, 1, &[12, 34, 56, 78])?;
    assert_eq!(decode_png(&p_ga, &mut rgba)?, (2, 1));
    assert_eq!(rgba, [12, 12, 12, 34, 56, 56, 56, 78]);

    let p_gray = make_png(png::ColorType::Grayscale, 2, 1, &[99, 200])?;
    assert_eq!(decode_png(&p_gray, &mut rgba)?, (2, 1));
    assert_eq!(rgba, [99, 99, 99, 255, 200, 200, 200, 255]);

    assert_eq!(decode_png(b"garbage", &mut rgba), Err(Error::PngDecode));
    Ok(())
}
