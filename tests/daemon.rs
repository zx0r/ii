//! Live Unix-socket IXPC v1 daemon & CLI end-to-end tests (isolated `XDG_RUNTIME_DIR`, zero `unsafe`).

use core::sync::atomic::{AtomicU32, Ordering};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use rustix::{
    fd::AsFd,
    io::{Errno, read, write},
    net::{AddressFamily, SocketAddrUnix, SocketType, connect, socket},
    process::getpid,
};

use ii::ipc::{Cmd, ERR_FLAG, Header, RESP_FLAG};

static RUN_SEQ: AtomicU32 = AtomicU32::new(0);

struct RuntimeDir(PathBuf);

impl RuntimeDir {
    fn new() -> Result<Self, Box<dyn core::error::Error>> {
        let pid = getpid().as_raw_nonzero().get().unsigned_abs();
        let seq = RUN_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ii-e2e-{pid}-{seq}"));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn sock_path(&self) -> PathBuf {
        self.0.join("ii").join("ii.sock")
    }
}

impl Drop for RuntimeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn send_raw(
    sock: &Path,
    hdr: &Header,
    payload: &[u8],
) -> Result<(Header, Vec<u8>), Box<dyn core::error::Error>> {
    let fd = socket(AddressFamily::UNIX, SocketType::STREAM, None)?;
    connect(&fd, &SocketAddrUnix::new(sock)?)?;

    let mut req = Vec::with_capacity(32 + payload.len());
    req.extend_from_slice(&hdr.to_le_bytes());
    req.extend_from_slice(payload);
    let mut off = 0usize;
    while off < req.len() {
        match write(fd.as_fd(), &req[off..]) {
            Ok(0) => return Err("short write".into()),
            Ok(n) => off += n,
            Err(Errno::INTR) => {}
            Err(err) => return Err(err.into()),
        }
    }

    let mut raw_hdr = [0u8; 32];
    let mut got = 0usize;
    while got < 32 {
        match read(fd.as_fd(), &mut raw_hdr[got..]) {
            Ok(0) => return Err("eof reading header".into()),
            Ok(n) => got += n,
            Err(Errno::INTR) => {}
            Err(err) => return Err(err.into()),
        }
    }
    let resp_hdr = Header::from_le_bytes(&raw_hdr);
    let mut body = vec![0u8; resp_hdr.payload_len as usize];
    got = 0;
    while got < body.len() {
        match read(fd.as_fd(), &mut body[got..]) {
            Ok(0) => return Err("eof reading body".into()),
            Ok(n) => got += n,
            Err(Errno::INTR) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok((resp_hdr, body))
}

#[test]
fn cli_help_and_version_smoke() -> Result<(), Box<dyn core::error::Error>> {
    let bin = env!("CARGO_BIN_EXE_ii");

    let ver = Command::new(bin).arg("--version").output()?;
    assert!(ver.status.success());
    assert!(ver.stdout.starts_with(b"ii "));

    let help = Command::new(bin).arg("--help").output()?;
    assert!(help.status.success());
    assert!(help.stdout.windows(8).any(|w| w == b"--daemon"));
    Ok(())
}

#[test]
fn daemon_lifecycle_lock_ping_render_bad_magic_and_shutdown()
-> Result<(), Box<dyn core::error::Error>> {
    let bin = env!("CARGO_BIN_EXE_ii");
    let rt = RuntimeDir::new()?;
    let sock = rt.sock_path();

    let mut daemon = ChildGuard(
        Command::new(bin)
            .arg("--daemon")
            .env("XDG_RUNTIME_DIR", &rt.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );

    // Wait for listener socket creation (bounded 2s).
    let deadline = Instant::now() + Duration::from_secs(2);
    while !sock.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(sock.exists(), "daemon socket never appeared");

    // Second daemon must fail immediately via exclusive flock (`DaemonLocked`).
    let dup = Command::new(bin)
        .arg("--daemon")
        .env("XDG_RUNTIME_DIR", &rt.0)
        .output()?;
    assert!(!dup.status.success());

    // CLI `--ping` roundtrip.
    let ping = Command::new(bin)
        .arg("--ping")
        .env("XDG_RUNTIME_DIR", &rt.0)
        .output()?;
    assert!(ping.status.success());

    // Raw IXPC v1 `Cmd::Ping`.
    let (resp_hdr, _) = send_raw(&sock, &Header::new(Cmd::Ping, 0), &[])?;
    let ping_cmd = resp_hdr.command;
    assert_eq!(ping_cmd, (Cmd::Ping as u16) | RESP_FLAG);

    // Raw IXPC v1 `Cmd::RenderFile` against a real CSV file.
    let csv_path = rt.0.join("sample.csv");
    std::fs::write(&csv_path, b"a,b\n1,2\n")?;
    let path_bytes = csv_path.to_string_lossy().as_bytes().to_vec();
    let render_hdr = Header::new(Cmd::RenderFile, u32::try_from(path_bytes.len())?);
    let (resp_hdr, _) = send_raw(&sock, &render_hdr, &path_bytes)?;
    let render_cmd = resp_hdr.command;
    assert_eq!(render_cmd, (Cmd::RenderFile as u16) | RESP_FLAG);

    // Prepared previews crop in visual coordinates and never upload offscreen graphics.
    let preview_path = rt.0.join("scroll.md");
    let source = format!("top\n{}\n$$x^2$$\nbottom\n", "plain\n".repeat(40));
    std::fs::write(&preview_path, &source)?;
    let preview =
        |skip: u32, cols: u16| -> Result<(Header, Vec<u8>), Box<dyn core::error::Error>> {
            let mut payload = skip.to_le_bytes().to_vec();
            payload.extend_from_slice(preview_path.to_str().ok_or("path")?.as_bytes());
            let mut h = Header::new(Cmd::PreviewFile, u32::try_from(payload.len())?);
            h.cols = cols;
            h.rows = 4;
            h.req_id_lo = skip;
            h.req_id_hi = 123;
            let (reply, body) = ii::ipc::exchange(h, &payload, Some(&sock))?;
            assert_eq!(reply.req_id_lo, skip);
            assert_eq!(reply.req_id_hi, 123);
            Ok((reply, body))
        };
    let (top, text) = preview(0, 80)?;
    assert_eq!(top.flags, 0, "invisible formula was uploaded");
    assert!(text.windows(3).any(|w| w == b"top"));
    let (formula, _) = preview(42, 80)?;
    assert!(formula.flags > 0, "visible formula was not uploaded");
    let (_, again) = preview(0, 80)?;
    assert_eq!(again, text, "scroll back changed prepared document");
    let (end, empty) = preview(10000, 80)?;
    assert_eq!(end.flags, 0);
    assert_eq!(empty, b"");
    std::fs::write(&preview_path, b"changed\nabcdefghij\n")?;
    let (_, edited) = preview(0, 4)?;
    assert!(edited.windows(4).any(|w| w == b"chan"));
    assert!(memchr::memchr_iter(b'\n', &edited).count() <= 4);

    // Unsaved data-science buffers must route by declared format, not Markdown.
    let plotly = b"plotly\0{\"data\":[{\"x\":[0,1],\"y\":[2,3]}]}";
    let mut h = Header::new(Cmd::RenderBuffer, u32::try_from(plotly.len())?);
    h.flags = ii::ipc::FLAG_REMOTE;
    let (reply, body) = ii::ipc::exchange(h, plotly, Some(&sock))?;
    assert!(reply.flags > 0);
    assert!(memchr::memmem::find(&body[reply.flags as usize..], "\u{10eeee}".as_bytes()).is_some());
    assert!(ii::ipc::exchange(Header::new(Cmd::RenderBuffer, 3), b"bad", Some(&sock)).is_err());

    // Source rows remain stable when one table expands into several visual rows.
    let source = b"md\0intro\n| A | B |\n|---|---|\n| 1 | 2 |\ntail";
    let h = Header::new(Cmd::DecorateBuffer, u32::try_from(source.len())?);
    let (reply, body) = ii::ipc::exchange(h, source, Some(&sock))?;
    let mut pos = reply.flags as usize;
    let mut ranges = Vec::new();
    while pos < body.len() {
        let first = u32::from_le_bytes(body[pos..pos + 4].try_into()?);
        let last = u32::from_le_bytes(body[pos + 4..pos + 8].try_into()?);
        let size = u32::from_le_bytes(body[pos + 8..pos + 12].try_into()?) as usize;
        ranges.push((first, last));
        pos += 12 + size;
        assert!(pos <= body.len());
    }
    assert_eq!(ranges, [(0, 1), (1, 4), (4, 5)]);

    assert_notebook_cell_anchors(&sock)?;

    // Corrupt magic frame must yield `ERR_FLAG` without crashing the reactor.
    let mut bad_hdr = Header::new(Cmd::Ping, 0);
    bad_hdr.magic = 0xDEAD_BEEF;
    let (err_hdr, err_body) = send_raw(&sock, &bad_hdr, &[])?;
    let err_cmd = err_hdr.command;
    assert_ne!(err_cmd & ERR_FLAG, 0);
    assert_eq!(err_body, b"invalid header");

    // Graceful `--shutdown` exits 0 and RAII `DaemonGuard` unlinks socket.
    let shut = Command::new(bin)
        .arg("--shutdown")
        .env("XDG_RUNTIME_DIR", &rt.0)
        .output()?;
    assert!(shut.status.success());

    let status = daemon.0.wait()?;
    assert!(status.success());
    assert!(!sock.exists(), "socket leaked after graceful shutdown");
    Ok(())
}

fn assert_notebook_cell_anchors(sock: &Path) -> Result<(), Box<dyn core::error::Error>> {
    // Notebook cells keep independent source anchors, rather than one giant
    // virtual block attached to the first JSON row.
    let notebook = include_bytes!("../showcase/03.ipynb");
    let mut payload = b"ipynb\0".to_vec();
    payload.extend_from_slice(notebook);
    let mut header = Header::new(Cmd::DecorateBuffer, u32::try_from(payload.len())?);
    header.flags = ii::ipc::FLAG_REMOTE;
    let (reply, body) = ii::ipc::exchange(header, &payload, Some(sock))?;
    let mut offset = reply.flags as usize;
    let mut count = 0;
    let mut previous_end = 0;
    while offset < body.len() {
        let first = u32::from_le_bytes(body[offset..offset + 4].try_into()?);
        let last = u32::from_le_bytes(body[offset + 4..offset + 8].try_into()?);
        let len = u32::from_le_bytes(body[offset + 8..offset + 12].try_into()?) as usize;
        assert!(last > first && first >= previous_end);
        previous_end = last;
        count += 1;
        offset += 12 + len;
    }
    assert_eq!(offset, body.len());
    assert_eq!(count, 19, "notebook cells collapsed into one cursor anchor");

    Ok(())
}
