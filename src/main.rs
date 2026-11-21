//! `ii` entry point: CLI, daemon control, and local L2-cached KGP pipeline.

use std::{ffi::OsString, path::Path};

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
    theme::{ColorToken, ThemeChoice},
    tty, vram,
};

const VERSION_LINE: &[u8] = concat!("ii ", env!("CARGO_PKG_VERSION"), "\n").as_bytes();
const USAGE: &[u8] = concat!(
    "ii ",
    env!("CARGO_PKG_VERSION"),
    " - Terminal graphical coprocessor\n\
Usage: ii [OPTIONS] [FILE]\n\
\n\
Geometry:\n\
  -s, --size <WxH>        Viewport bounds in cells (cols x rows)\n\
      --cell-size <WxH>   Physical cell size in pixels (cell_w x cell_h)\n\
      --skip <N>          Vertical line offset for paging\n\
\n\
Theme:\n\
  -t, --theme <THEME>     Palette polarity: [dark | light]\n\
\n\
Daemon & Socket:\n\
      --daemon            Run background IXPC v1 coprocessor daemon\n\
      --socket <PATH>     UNIX socket path (default: $XDG_RUNTIME_DIR/ii/ii.sock)\n\
      --ping              Ping running daemon\n\
      --shutdown          Gracefully shut down running daemon\n\
\n\
Info:\n\
  -h, --help              Show this help message\n\
  -V, --version           Show version\n"
)
.as_bytes();

#[derive(Debug)]
pub struct CliArgs<'a> {
    pub size: Option<(u16, u16)>,
    pub cell_size: Option<(u16, u16)>,
    pub skip: usize,
    pub theme: Option<ThemeChoice>,
    pub is_daemon: bool,
    pub is_ping: bool,
    pub is_shutdown: bool,
    pub socket_path: Option<&'a Path>,
    pub file_path: Option<&'a Path>,
}

fn parse_wxh(s: &str) -> Result<(u16, u16), Error> {
    let mut parts = s.splitn(2, 'x');
    let w = parts
        .next()
        .and_then(|v| v.parse::<u16>().ok())
        .ok_or(Error::InvalidCli)?;
    let h = parts
        .next()
        .and_then(|v| v.parse::<u16>().ok())
        .ok_or(Error::InvalidCli)?;
    if w == 0 || h == 0 {
        return Err(Error::InvalidCli);
    }
    Ok((w, h))
}

impl<'a> CliArgs<'a> {
    /// Parse command-line arguments into `CliArgs`.
    ///
    /// # Errors
    /// Returns `Error::InvalidCli` on malformed or unrecognized arguments.
    pub fn parse(args: &'a [OsString]) -> Result<Self, Error> {
        let mut parsed = Self {
            size: None,
            cell_size: None,
            skip: 0,
            theme: None,
            is_daemon: false,
            is_ping: false,
            is_shutdown: false,
            socket_path: None,
            file_path: None,
        };

        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let Some(s) = arg.to_str() else { continue };
            match s {
                "-s" | "--size" => {
                    let val = it
                        .next()
                        .and_then(|v| v.to_str())
                        .ok_or(Error::InvalidCli)?;
                    parsed.size = Some(parse_wxh(val)?);
                }
                "--cell-size" => {
                    let val = it
                        .next()
                        .and_then(|v| v.to_str())
                        .ok_or(Error::InvalidCli)?;
                    parsed.cell_size = Some(parse_wxh(val)?);
                }
                "--skip" => {
                    let val = it
                        .next()
                        .and_then(|v| v.to_str())
                        .ok_or(Error::InvalidCli)?;
                    parsed.skip = val.parse::<usize>().map_err(|_| Error::InvalidCli)?;
                }
                "-t" | "--theme" => {
                    let val = it
                        .next()
                        .and_then(|v| v.to_str())
                        .ok_or(Error::InvalidCli)?;
                    parsed.theme = match val {
                        "dark" => Some(ThemeChoice::Dark),
                        "light" => Some(ThemeChoice::Light),
                        _ => return Err(Error::InvalidCli),
                    };
                }
                "--daemon" => parsed.is_daemon = true,
                "--ping" => parsed.is_ping = true,
                "--shutdown" => parsed.is_shutdown = true,
                "--socket" => {
                    let val = it.next().ok_or(Error::InvalidCli)?;
                    parsed.socket_path = Some(Path::new(val));
                }
                "-h" | "--help" => {
                    let _ = write_all(std::io::stdout().as_fd(), USAGE);
                    std::process::exit(0);
                }
                "-V" | "--version" => {
                    let _ = write_all(std::io::stdout().as_fd(), VERSION_LINE);
                    std::process::exit(0);
                }
                pos if !pos.starts_with('-') => {
                    parsed.file_path = Some(Path::new(arg));
                }
                _ => return Err(Error::InvalidCli),
            }
        }
        Ok(parsed)
    }
}

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

fn emit_dual_channel(
    stdout_fd: BorrowedFd<'_>,
    apc_bytes: &[u8],
    text_bytes: &[u8],
) -> Result<(), Error> {
    let apc_len = u32::try_from(apc_bytes.len()).map_err(|_| Error::PayloadTooLarge(0))?;
    let prefix = apc_len.to_le_bytes();
    write_all(stdout_fd, &prefix)?;
    write_all(stdout_fd, apc_bytes)?;
    write_all(stdout_fd, text_bytes)?;
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

fn render_file_local(
    data: &[u8],
    ext: &str,
    args: &CliArgs<'_>,
    is_interactive: bool,
) -> Result<(), Error> {
    let metrics = tty::resolve_metrics(args.size, args.cell_size, is_interactive);
    let palette = tty::resolve_theme(args.theme);
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
    let mut apc_buf = Vec::new();
    if metrics.is_remote != 0
        || kitty::stage_shm(&scratch.rgba_buf, params, &mut seq, pid, &mut apc_buf).is_err()
    {
        apc_buf.clear();
        let mut b64 = Vec::new();
        kitty::stage_base64(&scratch.rgba_buf, params, &mut apc_buf, &mut b64);
    }

    let mut text_buf = Vec::new();
    kitty::emit_placeholder(image_id, cols, rows, &mut text_buf);

    let stdout = std::io::stdout();
    let stdout_fd = stdout.as_fd();

    if is_interactive {
        write_all(stdout_fd, &apc_buf)?;
        write_all(stdout_fd, &text_buf)?;
    } else {
        emit_dual_channel(stdout_fd, &apc_buf, &text_buf)?;
    }
    Ok(())
}

fn run_cli() -> Result<(), Error> {
    let raw_args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (stdin, stdout) = (std::io::stdin(), std::io::stdout());
    let is_interactive = isatty(&stdout);

    if raw_args.is_empty() {
        return if isatty(&stdin) {
            write_all(stdout.as_fd(), USAGE)
        } else {
            let empty_args = CliArgs {
                size: None,
                cell_size: None,
                skip: 0,
                theme: None,
                is_daemon: false,
                is_ping: false,
                is_shutdown: false,
                socket_path: None,
                file_path: None,
            };
            render_file_local(&read_stdin_bounded()?, "", &empty_args, is_interactive)
        };
    }

    let args = CliArgs::parse(&raw_args)?;

    if args.is_daemon {
        return daemon::run();
    }
    if args.is_ping {
        return write_all(stdout.as_fd(), &ipc::client_send(Cmd::Ping, &[])?);
    }
    if args.is_shutdown {
        return ipc::client_send(Cmd::Shutdown, &[]).map(|_| ());
    }

    if let Some(file_path) = args.file_path {
        let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let data = read_file_bounded(file_path)?;
        render_file_local(&data, ext, &args, is_interactive)
    } else if !isatty(&stdin) {
        let data = read_stdin_bounded()?;
        render_file_local(&data, "", &args, is_interactive)
    } else {
        write_all(stdout.as_fd(), USAGE)
    }
}

fn main() {
    if let Err(e) = run_cli() {
        let msg = format!("ii: {e}\n");
        let _ = write_all(std::io::stderr().as_fd(), msg.as_bytes());
        std::process::exit(1);
    }
}
