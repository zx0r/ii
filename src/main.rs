//! `ii` entry point: CLI, daemon control, and local L2-cached KGP pipeline.

use std::{ffi::OsString, os::unix::ffi::OsStrExt, path::Path};

use rustix::{
    fd::{AsFd, BorrowedFd},
    io::{Errno, read, write},
    process::getpid,
    termios::isatty,
};

use ii::{
    cache, compose, daemon,
    err::Error,
    file, fmt,
    ipc::{self, Cmd, MAX_PAYLOAD},
    style,
    theme::ThemeChoice,
    tty,
};

const VERSION_LINE: &[u8] = concat!("ii ", env!("CARGO_PKG_VERSION"), "\n").as_bytes();
const USAGE: &[u8] = concat!(
    "ii ",
    env!("CARGO_PKG_VERSION"),
    " - Terminal graphical coprocessor\n\
Usage: ii [OPTIONS] [FILE]\n\
       ii doctor\n\
\n\
Geometry:\n\
  -s, --size <WxH>        Viewport bounds in cells (cols x rows)\n\
      --cell-size <WxH>   Physical cell size in pixels (cell_w x cell_h)\n\
      --skip <N>          Vertical line offset for paging\n\
      --machine           Framed graphics and text output for adapters\n\
      --ansi              Styled text on piped stdout\n\
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Control {
    #[default]
    Render,
    Daemon,
    Ping,
    Shutdown,
}

#[derive(Debug)]
pub struct CliArgs<'a> {
    pub size: Option<(u16, u16)>,
    pub cell_size: Option<(u16, u16)>,
    pub skip: u32,
    pub machine: bool,
    pub ansi: bool,
    pub theme: Option<ThemeChoice>,
    control: Control,
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
            machine: false,
            ansi: false,
            theme: None,
            control: Control::Render,
            socket_path: None,
            file_path: None,
        };

        let mut it = args.iter();
        while let Some(arg) = it.next() {
            if arg == "--" {
                let path = it.next().ok_or(Error::InvalidCli)?;
                if parsed.file_path.is_some() || it.next().is_some() {
                    return Err(Error::InvalidCli);
                }
                parsed.file_path = Some(Path::new(path));
                break;
            }
            let Some(s) = arg.to_str() else {
                if parsed.file_path.replace(Path::new(arg)).is_some() {
                    return Err(Error::InvalidCli);
                }
                continue;
            };
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
                    parsed.skip = val.parse::<u32>().map_err(|_| Error::InvalidCli)?;
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
                "--machine" => parsed.machine = true,
                "--ansi" => parsed.ansi = true,
                "--daemon" => parsed.control = Control::Daemon,
                "--ping" => parsed.control = Control::Ping,
                "--shutdown" => parsed.control = Control::Shutdown,
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
                    if parsed.file_path.replace(Path::new(arg)).is_some() {
                        return Err(Error::InvalidCli);
                    }
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

fn run_doctor() -> Result<(), Error> {
    let (metrics, palette, _) = tty::probe();
    let cache_state = if cache::ensure_dirs().is_ok() {
        b"OK".as_slice()
    } else {
        b"WARN".as_slice()
    };
    let daemon_state = if ipc::client_send(Cmd::Ping, &[]).is_ok() {
        b"OK".as_slice()
    } else {
        b"OFFLINE".as_slice()
    };
    let mut out = Vec::with_capacity(160);
    out.extend_from_slice(b"ii doctor\nterminal grid : ");
    fmt::push_u32(&mut out, u32::from(metrics.cols));
    out.extend_from_slice(b" x ");
    fmt::push_u32(&mut out, u32::from(metrics.rows));
    out.extend_from_slice(b"\ncell pixels   : ");
    fmt::push_u32(&mut out, u32::from(metrics.cell_w));
    out.extend_from_slice(b" x ");
    fmt::push_u32(&mut out, u32::from(metrics.cell_h));
    out.extend_from_slice(b"\ntheme         : ");
    if palette.is_dark {
        out.extend_from_slice(b"dark");
    } else {
        out.extend_from_slice(b"light");
    }
    out.extend_from_slice(b"\ncache runtime : ");
    out.extend_from_slice(cache_state);
    out.extend_from_slice(b"\ndaemon        : ");
    out.extend_from_slice(daemon_state);
    out.push(b'\n');
    write_all(std::io::stdout().as_fd(), &out)
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

fn render_file_local(
    data: &[u8],
    ext: &str,
    args: &CliArgs<'_>,
    interactive: bool,
) -> Result<(), Error> {
    let metrics = tty::resolve_metrics(args.size, args.cell_size, interactive);
    let palette = tty::resolve_theme(args.theme);
    let stdout = std::io::stdout();
    if !interactive && !args.machine {
        let mut text = Vec::new();
        if args.ansi {
            style::emit_prose(data, palette, &mut style::State::default(), &mut text);
        } else {
            style::emit_literal(data, &mut text);
        }
        let mut page = Vec::new();
        ii::viewport::append(
            &text,
            metrics.cols,
            args.skip,
            args.size.map_or(0, |(_, h)| u32::from(h)),
            &mut page,
        );
        return write_all(stdout.as_fd(), &page);
    }
    let mut scratch = compose::ComposeScratch::new();
    let mut out = compose::ComposeOut::new();
    let dir = cache::ensure_dirs().ok();
    let context = compose::ComposeCtx {
        cell_w: metrics.cell_w,
        cell_h: metrics.cell_h,
        cols: metrics.cols,
        rows: metrics.rows,
        max_w: metrics.max_width_px(),
        palette,
        is_tmux: metrics.is_tmux != 0,
        is_remote: metrics.is_remote != 0 || args.machine,
        skip_lines: args.skip,
        max_lines: args.size.map_or(0, |(_, h)| u32::from(h)),
        explicit_graphics: args.machine,
    };
    let mut seq = 0;
    compose::render_file(
        data,
        fmt::detect_kind(data, ext),
        context,
        dir.as_ref().map(AsFd::as_fd),
        &mut scratch,
        &mut seq,
        getpid().as_raw_nonzero().get().unsigned_abs(),
        &mut out,
    )?;
    if args.machine {
        let mut apc = Vec::new();
        compose::append_visible_uploads(&out, &out.ansi, &mut apc);
        emit_dual_channel(stdout.as_fd(), &apc, &out.ansi)
    } else {
        write_all(stdout.as_fd(), &out.apc)?;
        write_all(stdout.as_fd(), &out.ansi)
    }
}

fn render_preview(path: &Path, args: &CliArgs<'_>, interactive: bool) -> Result<(), Error> {
    let metrics = tty::resolve_metrics(args.size, args.cell_size, interactive);
    let path = std::fs::canonicalize(path).map_err(|_| Error::Io(Errno::NOENT))?;
    let mut payload = args.skip.to_le_bytes().to_vec();
    payload.extend_from_slice(path.as_os_str().as_bytes());
    let mut header = ipc::Header::new(
        Cmd::PreviewFile,
        u32::try_from(payload.len()).map_err(|_| Error::InvalidCli)?,
    );
    header.cols = metrics.cols;
    header.rows = metrics.rows;
    header.cell_w = metrics.cell_w;
    header.cell_h = metrics.cell_h;
    header.flags = ipc::FLAG_REMOTE;
    if metrics.is_tmux != 0 {
        header.flags |= ipc::FLAG_TMUX;
    }
    if !tty::resolve_theme(args.theme).is_dark {
        header.flags |= ipc::FLAG_THEME_LIGHT;
    }
    let mut result = ipc::exchange(header, &payload, args.socket_path);
    if matches!(result, Err(Error::Io(Errno::NOENT | Errno::CONNREFUSED))) {
        let exe = std::env::current_exe().map_err(|_| Error::InvalidState)?;
        let mut child = std::process::Command::new(exe);
        child.arg("--daemon");
        if let Some(socket) = args.socket_path {
            child.arg("--socket").arg(socket);
        }
        let _ = child
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            result = ipc::exchange(header, &payload, args.socket_path);
            if result.is_ok() {
                break;
            }
        }
    }
    let (reply, body) = result?;
    let (apc, text) = body.split_at(reply.flags as usize);
    let stdout = std::io::stdout();
    if args.machine {
        emit_dual_channel(stdout.as_fd(), apc, text)
    } else {
        write_all(stdout.as_fd(), apc)?;
        write_all(stdout.as_fd(), text)
    }
}

fn run_cli() -> Result<(), Error> {
    let raw_args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (stdin, stdout) = (std::io::stdin(), std::io::stdout());
    let is_interactive = isatty(&stdout);

    if let Some(command) = raw_args.first().and_then(|arg| arg.to_str())
        && command == "doctor"
        && raw_args.len() == 1
    {
        return run_doctor();
    }

    if raw_args.is_empty() {
        return if isatty(&stdin) {
            write_all(stdout.as_fd(), USAGE)
        } else {
            let empty_args = CliArgs {
                size: None,
                cell_size: None,
                skip: 0,
                machine: false,
                ansi: false,
                theme: None,
                control: Control::Render,
                socket_path: None,
                file_path: None,
            };
            render_file_local(&read_stdin_bounded()?, "", &empty_args, is_interactive)
        };
    }

    let args = CliArgs::parse(&raw_args)?;

    if args.control == Control::Daemon {
        return daemon::run(args.socket_path);
    }
    if args.control == Control::Ping {
        return write_all(
            stdout.as_fd(),
            &ipc::client_send_to(Cmd::Ping, &[], args.socket_path)?,
        );
    }
    if args.control == Control::Shutdown {
        return ipc::client_send_to(Cmd::Shutdown, &[], args.socket_path).map(|_| ());
    }

    if let Some(file_path) = args.file_path {
        if args.machine || is_interactive {
            return render_preview(file_path, &args, is_interactive);
        }
        let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let mut data = Vec::new();
        file::read_bounded(file_path, MAX_PAYLOAD as usize, &mut data).map_err(|err| {
            if err == Errno::FBIG {
                Error::PayloadTooLarge(MAX_PAYLOAD)
            } else {
                Error::Io(err)
            }
        })?;
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
