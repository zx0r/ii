//! `ii` entry point: `SIGPIPE` mask, hand-rolled zero-dep CLI, daemon client/server & streaming pipeline.

use rustix::{
    fd::BorrowedFd,
    io::{Errno, read, write},
    net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with},
    process::getpid,
    stdio::{stderr, stdin, stdout},
    termios::isatty,
};

use ii::{
    cache,
    cfg::Cfg,
    compose::{self, Session},
    daemon,
    err::Error,
    ipc::{Cmd, Header, ERR_FLAG},
    ir::Scratch,
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

fn write_all_fd(fd: BorrowedFd<'_>, mut buf: &[u8]) -> Result<(), Error> {
    while !buf.is_empty() {
        match write(fd, buf) {
            Ok(n) => buf = &buf[n..],
            Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(())
}

fn read_exact_fd(fd: BorrowedFd<'_>, mut buf: &mut [u8]) -> Result<(), Error> {
    while !buf.is_empty() {
        match read(fd, buf) {
            Ok(0) => return Err(Error::BadMagic),
            Ok(n) => buf = &mut buf[n..],
            Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(())
}

fn send_ipc_cmd(cmd: Cmd, payload: &[u8]) -> Result<Vec<u8>, Error> {
    let sock_path = daemon::socket_path();
    let fd = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )?;
    let addr = SocketAddrUnix::new(sock_path.as_path()).map_err(|_| Error::SocketSymlink)?;
    connect(&fd, &addr)?;

    #[expect(
        clippy::cast_possible_truncation,
        reason = "CLI IPC payloads fit in u32"
    )]
    let hdr = Header::new(cmd, payload.len() as u32);
    write_all_fd(fd.as_fd(), &hdr.to_le_bytes())?;
    if !payload.is_empty() {
        write_all_fd(fd.as_fd(), payload)?;
    }

    let mut resp_hdr_bytes = [0u8; 32];
    read_exact_fd(fd.as_fd(), &mut resp_hdr_bytes)?;
    let resp_hdr = Header::from_le_bytes(&resp_hdr_bytes);
    let _ = resp_hdr.validate()?;

    let mut resp_buf = vec![0u8; resp_hdr.payload_len as usize];
    if !resp_buf.is_empty() {
        read_exact_fd(fd.as_fd(), &mut resp_buf)?;
    }
    if (resp_hdr.command & ERR_FLAG) != 0 {
        return Err(Error::UnknownCommand(resp_hdr.command));
    }
    Ok(resp_buf)
}

fn run_render_local(input: &[u8]) -> Result<(), Error> {
    let (metrics, palette, tty_fd) = tty::probe();
    let cache_dir = cache::ensure_dirs().ok();
    // Safety: `stdout()` is valid for the lifetime of the process.
    let is_stdout_tty = unsafe { isatty(stdout()) };
    let cfg = Cfg {
        tty: metrics,
        viewport: ii::cfg::Viewport::DEFAULT,
        palette,
        cache_dir,
        is_stdout_tty,
    };
    let pid = getpid().as_raw_nonzero().get().unsigned_abs();
    let mut session = Session::new(pid);
    let mut scratch = Scratch::new();

    compose::compose_ansi(input, &cfg, &mut session, &mut scratch)?;

    if !scratch.apc_buf.is_empty() {
        if let Some(ref tfd) = tty_fd {
            let _ = write_all_fd(tfd.as_fd(), &scratch.apc_buf);
        } else {
            // Safety: `stdout()` is valid for the lifetime of the process.
            let out_fd = unsafe { stdout() };
            let _ = write_all_fd(out_fd, &scratch.apc_buf);
        }
    }
    // Safety: `stdout()` is valid for the lifetime of the process.
    let out_fd = unsafe { stdout() };
    write_all_fd(out_fd, &scratch.text_buf)?;
    Ok(())
}

fn read_stdin_all() -> Result<Vec<u8>, Error> {
    // Safety: `stdin()` is valid for the lifetime of the process.
    let in_fd = unsafe { stdin() };
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

fn run_cli() -> Result<(), Error> {
    let mut args = std::env::args_os();
    let _ = args.next();

    let Some(first) = args.next() else {
        // Safety: `stdin()` is valid for the lifetime of the process.
        if unsafe { isatty(stdin()) } {
            // Safety: `stdout()` is valid for the lifetime of the process.
            write_all_fd(unsafe { stdout() }, USAGE)?;
            return Ok(());
        }
        let data = read_stdin_all()?;
        return run_render_local(&data);
    };

    let arg = first.to_string_lossy();
    match arg.as_ref() {
        "-h" | "--help" => {
            // Safety: `stdout()` is valid for the lifetime of the process.
            write_all_fd(unsafe { stdout() }, USAGE)
        }
        "-V" | "--version" => {
            // Safety: `stdout()` is valid for the lifetime of the process.
            write_all_fd(unsafe { stdout() }, b"ii 0.0.0\n")
        }
        "--daemon" => {
            let _lock_fd = daemon::acquire_lock()?;
            let listen_fd = daemon::bind_socket()?;
            let (metrics, palette, _tty_fd) = tty::probe();
            let cache_dir = cache::ensure_dirs().ok();
            let cfg = Cfg {
                tty: metrics,
                viewport: ii::cfg::Viewport::DEFAULT,
                palette,
                cache_dir,
                is_stdout_tty: false,
            };
            daemon::run(cfg, listen_fd)
        }
        "--ping" => {
            let resp = send_ipc_cmd(Cmd::Ping, &[])?;
            let text = if resp.len() >= 4 { &resp[4..] } else { &resp[..] };
            // Safety: `stdout()` is valid for the lifetime of the process.
            let out = unsafe { stdout() };
            write_all_fd(out, text)?;
            write_all_fd(out, b"\n")
        }
        "--shutdown" => {
            let _ = send_ipc_cmd(Cmd::Shutdown, &[])?;
            Ok(())
        }
        path => {
            let data = std::fs::read(path).map_err(|_| Error::Io(Errno::NOENT))?;
            run_render_local(&data)
        }
    }
}

fn main() {
    if let Err(e) = run_cli() {
        let msg = format!("ii: {e}\n");
        // Safety: `stderr()` is valid for the lifetime of the process.
        let _ = write_all_fd(unsafe { stderr() }, msg.as_bytes());
        std::process::exit(1);
    }
}
