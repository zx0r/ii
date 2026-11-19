//! Non-blocking `poll(2)` reactor daemon with 32-slot per-client `Phase` FSM and socket hardening.

#![expect(
    clippy::cast_possible_truncation,
    reason = "32-byte IXPC header read counters fit in u8"
)]

use std::path::PathBuf;

use rustix::{
    event::{PollFd, PollFlags, poll},
    fd::{BorrowedFd, OwnedFd},
    fs::{
        AtFlags, CWD, FlockOperation, Mode, OFlags, chmod, flock, fstat, lstat, mkdirat, openat,
        unlinkat,
    },
    io::{Errno, fcntl_getfl, fcntl_setfl, read, write},
    net::{
        AddressFamily, SocketAddrUnix, SocketFlags, SocketType, accept_with, bind, listen,
        socket_with,
    },
    process::{getpid, getuid},
};

use crate::{
    cfg::Cfg,
    compose::{self, Session},
    err::Error,
    ipc::{self, Cmd, Header, MAX_CLIENTS},
    ir::Scratch,
};

#[derive(Clone, Copy, Debug)]
pub enum Phase {
    ReadHeader { got: u8 },
    ReadPayload { header: Header, got: u32 },
    WriteResp { sent: usize },
}

pub struct ClientSlot {
    pub fd: Option<OwnedFd>,
    pub phase: Phase,
    pub hdr: [u8; 32],
    pub buf: Vec<u8>,
}

impl ClientSlot {
    const fn empty() -> Self {
        Self {
            fd: None,
            phase: Phase::ReadHeader { got: 0 },
            hdr: [0; 32],
            buf: Vec::new(),
        }
    }

    fn reset(&mut self) {
        self.fd = None;
        self.phase = Phase::ReadHeader { got: 0 };
        self.hdr = [0; 32];
        self.buf.clear();
    }
}

#[must_use]
pub fn runtime_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        let mut p = PathBuf::from(xdg);
        p.push("ii");
        p
    } else {
        let uid = getuid().as_raw();
        PathBuf::from(format!("/tmp/ii-{uid}"))
    }
}

#[must_use]
pub fn socket_path() -> PathBuf {
    let mut p = runtime_dir();
    p.push("ii.sock");
    p
}

#[must_use]
pub fn lock_path() -> PathBuf {
    let mut p = runtime_dir();
    p.push("ii.lock");
    p
}

fn ensure_secure_runtime_dir() -> Result<PathBuf, Error> {
    let dir = runtime_dir();
    match mkdirat(CWD, dir.as_path(), Mode::RUSWX) {
        Ok(()) | Err(Errno::EXIST) => {}
        Err(e) => return Err(Error::Io(e)),
    }
    let st = lstat(dir.as_path())?;
    // S_IFMT = 0o170000, S_IFLNK = 0o120000
    if (st.st_mode & 0o17_0000) == 0o12_0000 {
        return Err(Error::SocketSymlink);
    }
    if st.st_uid != getuid().as_raw() {
        return Err(Error::SocketUidMismatch);
    }
    let _ = chmod(dir.as_path(), Mode::RUSWX);
    Ok(dir)
}

fn set_nonblocking(fd: BorrowedFd<'_>) -> Result<(), Error> {
    let flags = fcntl_getfl(fd)?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK)?;
    Ok(())
}

/// Acquire exclusive non-blocking `flock(LOCK_EX | LOCK_NB)` on `ii.lock`.
pub fn acquire_lock() -> Result<OwnedFd, Error> {
    let _ = ensure_secure_runtime_dir()?;
    let path = lock_path();
    let fd = openat(
        CWD,
        path.as_path(),
        OFlags::CREATE | OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )?;
    let st = fstat(&fd)?;
    if st.st_uid != getuid().as_raw() {
        return Err(Error::SocketUidMismatch);
    }
    match flock(&fd, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(fd),
        Err(Errno::WOULDBLOCK | Errno::AGAIN) => Err(Error::DaemonLocked),
        Err(e) => Err(Error::Io(e)),
    }
}

/// Bind and listen on hardened Unix domain socket (`0700` dir, symlink/UID checks, non-blocking).
pub fn bind_socket() -> Result<OwnedFd, Error> {
    let _ = ensure_secure_runtime_dir()?;
    let sock_path = socket_path();

    if let Ok(st) = lstat(sock_path.as_path()) {
        if (st.st_mode & 0o17_0000) == 0o12_0000 {
            return Err(Error::SocketSymlink);
        }
        if st.st_uid != getuid().as_raw() {
            return Err(Error::SocketUidMismatch);
        }
        let _ = unlinkat(CWD, sock_path.as_path(), AtFlags::empty());
    }

    let listen_fd = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )?;
    set_nonblocking(listen_fd.as_fd())?;

    let addr = SocketAddrUnix::new(sock_path.as_path()).map_err(|_| Error::SocketSymlink)?;
    bind(&listen_fd, &addr)?;
    listen(&listen_fd, 32)?;
    Ok(listen_fd)
}

fn process_request(
    hdr: &Header,
    payload: &[u8],
    cfg: &mut Cfg,
    session: &mut Session,
    scratch: &mut Scratch,
    resp: &mut Vec<u8>,
) -> Result<bool, Error> {
    let cmd = hdr.validate()?;
    if hdr.cols > 0 {
        cfg.tty.cols = hdr.cols;
    }
    if hdr.rows > 0 {
        cfg.tty.rows = hdr.rows;
    }
    if hdr.cell_w > 0 {
        cfg.tty.cell_w = hdr.cell_w;
    }
    if hdr.cell_h > 0 {
        cfg.tty.cell_h = hdr.cell_h;
    }

    match cmd {
        Cmd::Ping => {
            ipc::encode_dual_response(Cmd::Ping, b"", b"PONG", resp);
            Ok(false)
        }
        Cmd::Shutdown => {
            ipc::encode_dual_response(Cmd::Shutdown, b"", b"BYE", resp);
            Ok(true)
        }
        Cmd::ComposeAnsi => {
            match compose::compose_ansi(payload, cfg, session, scratch) {
                Ok(()) => ipc::encode_dual_response(
                    Cmd::ComposeAnsi,
                    &scratch.apc_buf,
                    &scratch.text_buf,
                    resp,
                ),
                Err(e) => ipc::encode_error(Cmd::ComposeAnsi, &e.to_string(), resp),
            }
            Ok(false)
        }
        Cmd::ComposeExtmarks => {
            match compose::compose_extmarks(payload, cfg, session, scratch) {
                Ok(()) => ipc::encode_dual_response(
                    Cmd::ComposeExtmarks,
                    &scratch.apc_buf,
                    &scratch.text_buf,
                    resp,
                ),
                Err(e) => ipc::encode_error(Cmd::ComposeExtmarks, &e.to_string(), resp),
            }
            Ok(false)
        }
        Cmd::RenderFile => {
            let path = core::str::from_utf8(payload).unwrap_or("");
            match std::fs::read(path) {
                Ok(data) => match compose::compose_ansi(&data, cfg, session, scratch) {
                    Ok(()) => ipc::encode_dual_response(
                        Cmd::RenderFile,
                        &scratch.apc_buf,
                        &scratch.text_buf,
                        resp,
                    ),
                    Err(e) => ipc::encode_error(Cmd::RenderFile, &e.to_string(), resp),
                },
                Err(_) => ipc::encode_error(Cmd::RenderFile, "io: file read failed", resp),
            }
            Ok(false)
        }
    }
}

/// Run the single-threaded non-blocking `poll(2)` reactor over `listen_fd` and up to 32 `ClientSlot`s.
pub fn run(mut cfg: Cfg, listen_fd: OwnedFd) -> Result<(), Error> {
    let pid = getpid().as_raw_nonzero().get().unsigned_abs();
    let mut session = Session::new(pid);
    let mut scratch = Scratch::new();
    let mut slots: [ClientSlot; MAX_CLIENTS] = [const { ClientSlot::empty() }; MAX_CLIENTS];
    let mut resp_staging = Vec::with_capacity(65536);
    let mut shutdown_requested = false;

    loop {
        let mut pfds = Vec::with_capacity(1 + MAX_CLIENTS);
        let mut slot_map = [0usize; MAX_CLIENTS];
        let mut n_active = 0usize;

        pfds.push(PollFd::new(&listen_fd, PollFlags::IN));
        for (idx, slot) in slots.iter().enumerate() {
            if let Some(ref fd) = slot.fd {
                let events = match slot.phase {
                    Phase::ReadHeader { .. } | Phase::ReadPayload { .. } => PollFlags::IN,
                    Phase::WriteResp { .. } => PollFlags::OUT,
                };
                pfds.push(PollFd::new(fd, events));
                slot_map[n_active] = idx;
                n_active += 1;
            }
        }

        match poll(&mut pfds, None) {
            Ok(_) | Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }

        let listen_ready = pfds[0].revents().contains(PollFlags::IN);
        let mut ready_events = [(0usize, PollFlags::empty()); MAX_CLIENTS];
        for i in 0..n_active {
            ready_events[i] = (slot_map[i], pfds[1 + i].revents());
        }
        drop(pfds);

        if listen_ready {
            while let Ok(client_fd) = accept_with(&listen_fd, SocketFlags::CLOEXEC) {
                let _ = set_nonblocking(client_fd.as_fd());
                if let Some(free) = slots.iter_mut().find(|s| s.fd.is_none()) {
                    free.reset();
                    free.fd = Some(client_fd);
                }
            }
        }

        for &(idx, revents) in ready_events.iter().take(n_active) {
            if revents.is_empty() {
                continue;
            }
            if revents.intersects(PollFlags::ERR | PollFlags::NVAL) {
                slots[idx].reset();
                continue;
            }

            let slot = &mut slots[idx];
            let Some(ref fd) = slot.fd else {
                continue;
            };

            match slot.phase {
                Phase::ReadHeader { mut got } => {
                    match read(fd, &mut slot.hdr[got as usize..]) {
                        Ok(0) => {
                            slot.reset();
                        }
                        Ok(n) => {
                            got += n as u8;
                            if got == 32 {
                                let hdr = Header::from_le_bytes(&slot.hdr);
                                match hdr.validate() {
                                    Ok(_) if hdr.payload_len == 0 => {
                                        resp_staging.clear();
                                        let quit = process_request(
                                            &hdr,
                                            &[],
                                            &mut cfg,
                                            &mut session,
                                            &mut scratch,
                                            &mut resp_staging,
                                        )
                                        .unwrap_or(false);
                                        if quit {
                                            shutdown_requested = true;
                                        }
                                        slot.buf.clear();
                                        slot.buf.extend_from_slice(&resp_staging);
                                        slot.phase = Phase::WriteResp { sent: 0 };
                                    }
                                    Ok(_) => {
                                        slot.buf.clear();
                                        slot.buf.resize(hdr.payload_len as usize, 0);
                                        slot.phase = Phase::ReadPayload { header: hdr, got: 0 };
                                    }
                                    Err(e) => {
                                        resp_staging.clear();
                                        ipc::encode_error(Cmd::Ping, &e.to_string(), &mut resp_staging);
                                        slot.buf.clear();
                                        slot.buf.extend_from_slice(&resp_staging);
                                        slot.phase = Phase::WriteResp { sent: 0 };
                                    }
                                }
                            } else {
                                slot.phase = Phase::ReadHeader { got };
                            }
                        }
                        Err(Errno::AGAIN | Errno::WOULDBLOCK | Errno::INTR) => {}
                        Err(_) => slot.reset(),
                    }
                }
                Phase::ReadPayload { header, mut got } => {
                    let rem = &mut slot.buf[got as usize..];
                    match read(fd, rem) {
                        Ok(0) => slot.reset(),
                        Ok(n) => {
                            got += n as u32;
                            if got == header.payload_len {
                                resp_staging.clear();
                                let quit = process_request(
                                    &header,
                                    &slot.buf,
                                    &mut cfg,
                                    &mut session,
                                    &mut scratch,
                                    &mut resp_staging,
                                )
                                .unwrap_or(false);
                                if quit {
                                    shutdown_requested = true;
                                }
                                slot.buf.clear();
                                slot.buf.extend_from_slice(&resp_staging);
                                slot.phase = Phase::WriteResp { sent: 0 };
                            } else {
                                slot.phase = Phase::ReadPayload { header, got };
                            }
                        }
                        Err(Errno::AGAIN | Errno::WOULDBLOCK | Errno::INTR) => {}
                        Err(_) => slot.reset(),
                    }
                }
                Phase::WriteResp { mut sent } => {
                    match write(fd, &slot.buf[sent..]) {
                        Ok(n) => {
                            sent += n;
                            if sent >= slot.buf.len() {
                                slot.buf.clear();
                                slot.phase = Phase::ReadHeader { got: 0 };
                            } else {
                                slot.phase = Phase::WriteResp { sent };
                            }
                        }
                        Err(Errno::AGAIN | Errno::WOULDBLOCK | Errno::INTR) => {}
                        Err(_) => slot.reset(),
                    }
                }
            }
        }

        if shutdown_requested
            && slots
                .iter()
                .all(|s| !matches!(s.phase, Phase::WriteResp { .. }))
        {
            let _ = unlinkat(CWD, socket_path().as_path(), AtFlags::empty());
            return Ok(());
        }
    }
}
