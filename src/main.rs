//! `ii` entry point: SIGPIPE mask, zero-dep CLI, daemon client/server & streaming pipeline.

use std::path::Path;

use rustix::{
    fd::{AsFd, BorrowedFd},
    io::{Errno, read, write},
    termios::isatty,
};

use ii::{
    daemon,
    err::Error,
    fmt,
    ipc::{self, Cmd},
    raster,
    scene::RenderScratch,
    tty,
};

const USAGE: &[u8] = b"ii 0.0.0 - Terminal graphical coprocessor\n\
Usage: ii [OPTIONS] [FILE]\n\
Options:\n\
  --daemon       Run background IXPC v1 coprocessor daemon\n\
  --ping         Ping running daemon\n\
  --shutdown     Gracefully shut down running daemon\n\
  -h, --help     Show this help message\n\
  -V, --version  Show version\n";

fn write_all(fd: BorrowedFd<'_>, mut buf: &[u8]) -> Result<(), Error> {
    while !buf.is_empty() {
        match write(fd, buf) {
            Ok(n) => buf = &buf[n..],
            Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(())
}

fn read_stdin_all() -> Result<Vec<u8>, Error> {
    let stdin = std::io::stdin();
    let in_fd = stdin.as_fd();
    let mut buf = Vec::with_capacity(65536);
    let mut chunk = [0u8; 16384];
    loop {
        match read(in_fd, &mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(buf)
}

fn render_file_local(data: &[u8], ext: &str) -> Result<(), Error> {
    let mut scratch = RenderScratch::new();
    fmt::dispatch_into_scene(data, ext, &mut scratch.scene)?;

    let (metrics, _palette, tty_fd) = tty::probe();
    let cw = metrics.cell_w.max(1);
    let ch = metrics.cell_h.max(1);
    let max_w = (metrics.cols as u32) * (cw as u32);

    let (w, h, cols, rows) = raster::rasterize_scene(
        &scratch.scene,
        cw,
        ch,
        max_w as u16,
        &mut scratch.rgba_buf,
    )?;

    ii::kitty::stage_base64(
        &scratch.rgba_buf,
        ii::kitty::StageParams {
            w,
            h,
            image_id: 1,
            cols,
            rows,
            is_tmux: metrics.is_tmux != 0,
        },
        &mut scratch.out_buf,
        &mut Vec::new(),
    );

    let stdout = std::io::stdout();
    if let Some(ref tfd) = tty_fd {
        let _ = write_all(tfd.as_fd(), &scratch.out_buf);
    } else {
        let _ = write_all(stdout.as_fd(), &scratch.out_buf);
    }
    Ok(())
}

fn run_cli() -> Result<(), Error> {
    let mut args = std::env::args_os();
    let _ = args.next();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();

    let Some(first) = args.next() else {
        if isatty(&stdin) {
            write_all(stdout.as_fd(), USAGE)?;
            return Ok(());
        }
        let data = read_stdin_all()?;
        return render_file_local(&data, "");
    };

    let arg = first.to_string_lossy();
    match arg.as_ref() {
        "-h" | "--help" => write_all(stdout.as_fd(), USAGE),
        "-V" | "--version" => write_all(stdout.as_fd(), b"ii 0.0.0\n"),
        "--daemon" => daemon::run(),
        "--ping" => {
            let resp = ipc::client_send(Cmd::Ping, &[])?;
            write_all(stdout.as_fd(), &resp)
        }
        "--shutdown" => {
            let _ = ipc::client_send(Cmd::Shutdown, &[])?;
            Ok(())
        }
        path_str => {
            let p = Path::new(path_str);
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
            let data = std::fs::read(p).map_err(|_| Error::Io(Errno::NOENT))?;
            render_file_local(&data, ext)
        }
    }
}

fn main() {
    if let Err(e) = run_cli() {
        let msg = format!("ii: {e}\n");
        let stderr = std::io::stderr();
        let _ = write_all(stderr.as_fd(), msg.as_bytes());
        std::process::exit(1);
    }
}
