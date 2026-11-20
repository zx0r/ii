//! `ii` entry point: CLI, daemon control, and local L2-cached KGP pipeline.

use std::path::Path;

use rustix::{
    fd::{AsFd, BorrowedFd},
    fs::{CWD, FileType, Mode, OFlags, fstat, openat},
    io::{Errno, read, write},
    process::getpid,
    termios::isatty,
};

use ii::{
    cache, daemon,
    err::Error,
    fmt,
    ipc::{self, Cmd, MAX_PAYLOAD},
    kitty::{self, StageParams},
    raster,
    scene::RenderScratch,
    theme::ColorToken,
    tty, vram,
};

const VERSION_LINE: &[u8] = concat!("ii ", env!("CARGO_PKG_VERSION"), "\n").as_bytes();
const USAGE: &[u8] = concat!(
    "ii ",
    env!("CARGO_PKG_VERSION"),
    " - Terminal graphical coprocessor\n\
Usage: ii [OPTIONS] [FILE]\n\
Options:\n\
  --daemon       Run background IXPC v1 coprocessor daemon\n\
  --ping         Ping running daemon\n\
  --shutdown     Gracefully shut down running daemon\n\
  -h, --help     Show this help message\n\
  -V, --version  Show version\n"
)
.as_bytes();

fn write_all(fd: BorrowedFd<'_>, mut buf: &[u8]) -> Result<(), Error> {
    while !buf.is_empty() {
        match write(fd, buf) {
            Ok(0) => return Err(Error::Io(Errno::PIPE)),
            Ok(n) => buf = &buf[n..],
            Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(())
}

fn read_stdin_bounded() -> Result<Vec<u8>, Error> {
    const STEP: usize = 64 * 1024;
    let stdin = std::io::stdin();
    let in_fd = stdin.as_fd();
    let limit = MAX_PAYLOAD as usize;
    let mut buf = Vec::with_capacity(STEP);
    let mut len = 0usize;

    loop {
        if len == buf.len() {
            buf.resize(len.saturating_add(STEP).min(limit + 1), 0);
        }
        match read(in_fd, &mut buf[len..]) {
            Ok(0) => {
                buf.truncate(len);
                return Ok(buf);
            }
            Ok(n) => {
                len += n;
                if len > limit {
                    return Err(Error::PayloadTooLarge(MAX_PAYLOAD));
                }
            }
            Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
}

fn read_file_bounded(path: &Path) -> Result<Vec<u8>, Error> {
    let fd = openat(CWD, path, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?;
    let st = fstat(&fd)?;
    if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
        return Err(Error::Io(Errno::INVAL));
    }
    let size = usize::try_from(st.st_size).map_err(|_| Error::PayloadTooLarge(MAX_PAYLOAD))?;
    if size > MAX_PAYLOAD as usize {
        return Err(Error::PayloadTooLarge(MAX_PAYLOAD));
    }
    let mut buf = vec![0u8; size];
    let mut pos = 0usize;
    while pos < size {
        match read(&fd, &mut buf[pos..]) {
            Ok(0) => break,
            Ok(n) => pos += n,
            Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    buf.truncate(pos);
    Ok(buf)
}

fn render_file_local(data: &[u8], ext: &str) -> Result<(), Error> {
    let (metrics, palette, tty_fd) = tty::probe();
    let kind = fmt::detect_kind(data, ext);
    let fg = palette.color(ColorToken::FG_PRIMARY);
    let fg_rgb = [fg.r(), fg.g(), fg.b()];
    let image_id = vram::compute_tag(
        metrics.cell_w,
        metrics.cell_h,
        metrics.cols,
        kind as u8,
        fg_rgb,
        data,
    )
    .image_id()
    .get();

    let mut scratch = RenderScratch::new();
    let mut lz4 = Vec::new();
    let l2_dir = cache::ensure_dirs().ok();
    let b3 = cache::blake3_key(
        metrics.cell_w,
        metrics.cell_h,
        metrics.max_width_px(),
        kind as u8,
        fg_rgb,
        palette.is_dark,
        data,
    );

    let cached = if let Some(ref dir_fd) = l2_dir
        && let Ok(Some((w32, h32))) =
            cache::read_blob(dir_fd.as_fd(), &b3, &mut scratch.rgba_buf, &mut lz4)
        && let (Ok(w), Ok(h)) = (u16::try_from(w32), u16::try_from(h32))
    {
        let cols = w.div_ceil(metrics.cell_w.max(1)).max(1);
        let rows = h.div_ceil(metrics.cell_h.max(1)).max(1);
        Some((w, h, cols, rows))
    } else {
        None
    };

    let (w, h, cols, rows) = if let Some(hit) = cached {
        hit
    } else {
        fmt::dispatch_kind(kind, data, &mut scratch.scene)?;
        let res = raster::rasterize_scene(
            &scratch.scene,
            palette,
            metrics.cell_w,
            metrics.cell_h,
            metrics.max_width_px(),
            &mut scratch.rgba_buf,
        )?;
        if let Some(ref dir_fd) = l2_dir {
            let _ = cache::write_blob(
                dir_fd.as_fd(),
                &b3,
                u32::from(res.0),
                u32::from(res.1),
                &scratch.rgba_buf,
                &mut lz4,
            );
        }
        res
    };

    let params = StageParams {
        image_id,
        w,
        h,
        cols,
        rows,
        is_tmux: metrics.is_tmux != 0,
    };
    let mut seq = 0u32;
    let pid = getpid().as_raw_nonzero().get().unsigned_abs();
    if metrics.is_remote != 0
        || kitty::stage_shm(
            &scratch.rgba_buf,
            params,
            &mut seq,
            pid,
            &mut scratch.out_buf,
        )
        .is_err()
    {
        scratch.out_buf.clear();
        let mut b64 = Vec::new();
        kitty::stage_base64(&scratch.rgba_buf, params, &mut scratch.out_buf, &mut b64);
    }

    let stdout = std::io::stdout();
    if let Some(ref tfd) = tty_fd {
        write_all(tfd.as_fd(), &scratch.out_buf)?;
    } else {
        write_all(stdout.as_fd(), &scratch.out_buf)?;
    }

    scratch.out_buf.clear();
    kitty::emit_placeholder(image_id, cols, rows, &mut scratch.out_buf);
    write_all(stdout.as_fd(), &scratch.out_buf)
}

fn run_cli() -> Result<(), Error> {
    let mut args = std::env::args_os();
    let _ = args.next();
    let (stdin, stdout) = (std::io::stdin(), std::io::stdout());

    let Some(first) = args.next() else {
        return if isatty(&stdin) {
            write_all(stdout.as_fd(), USAGE)
        } else {
            render_file_local(&read_stdin_bounded()?, "")
        };
    };

    match first.to_str() {
        Some("-h" | "--help") => write_all(stdout.as_fd(), USAGE),
        Some("-V" | "--version") => write_all(stdout.as_fd(), VERSION_LINE),
        Some("--daemon") => daemon::run(),
        Some("--ping") => write_all(stdout.as_fd(), &ipc::client_send(Cmd::Ping, &[])?),
        Some("--shutdown") => ipc::client_send(Cmd::Shutdown, &[]).map(|_| ()),
        _ => {
            let p = Path::new(&first);
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
            render_file_local(&read_file_bounded(p)?, ext)
        }
    }
}

fn main() {
    if let Err(e) = run_cli() {
        let msg = format!("ii: {e}\n");
        let _ = write_all(std::io::stderr().as_fd(), msg.as_bytes());
        std::process::exit(1);
    }
}
