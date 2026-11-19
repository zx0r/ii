//! Non-blocking `poll(2)` server daemon. Zero wire duplicate code.
//! Reactor handles up to 32 concurrent Unix socket clients.

use std::path::PathBuf;

use rustix::{
    event::{PollFd, PollFlags, poll},
    fd::{AsFd, BorrowedFd, OwnedFd},
    fs::{
        AtFlags, CWD, FlockOperation, Mode, OFlags, chmod, fcntl_getfl, fcntl_setfl, flock, fstat,
        lstat, mkdirat, openat, unlinkat,
    },
    io::{Errno, FdFlags, fcntl_setfd, read, write},
    net::{AddressFamily, SocketAddrUnix, SocketType, accept, bind, listen, socket},
    process::{getpid, getuid},
};

use crate::{
    err::Error,
    ipc::{self, Cmd, Header, MAX_CLIENTS},
    scene::RenderScratch,
};

#[derive(Clone, Copy, Debug)]
pub enum Phase {
    ReadHeader { got: u8 },
    ReadPayload { header: Header, got: u32 },
    WriteResp { sent: usize },
}

#[derive(Debug)]
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

fn ensure_secure_runtime_dir() -> Result<PathBuf, Error> {
    let dir = ipc::socket_path().parent().map_or_else(
        || PathBuf::from(format!("/tmp/ii-{}", getuid().as_raw())),
        std::path::Path::to_path_buf,
    );
    match mkdirat(CWD, dir.as_path(), Mode::RWXU) {
        Ok(()) | Err(Errno::EXIST) => {}
        Err(e) => return Err(Error::Io(e)),
    }
    let st = lstat(dir.as_path())?;
    if (st.st_mode & 0o17_0000) == 0o12_0000 {
        return Err(Error::SocketSymlink);
    }
    if st.st_uid != getuid().as_raw() {
        return Err(Error::SocketUidMismatch);
    }
    let _ = chmod(dir.as_path(), Mode::RWXU);
    Ok(dir)
}

fn set_nonblocking(fd: BorrowedFd<'_>) -> Result<(), Error> {
    let flags = fcntl_getfl(fd)?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK)?;
    Ok(())
}

fn bind_listener() -> Result<OwnedFd, Error> {
    let dir = ensure_secure_runtime_dir()?;
    let lock_file = ipc::lock_path();
    let lock_fd = openat(
        CWD,
        lock_file.as_path(),
        OFlags::CREATE | OFlags::RDWR | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )?;
    if flock(&lock_fd, FlockOperation::NonBlockingLockExclusive).is_err() {
        return Err(Error::DaemonLocked);
    }

    let sock_path = ipc::socket_path();
    let _ = unlinkat(CWD, sock_path.as_path(), AtFlags::empty());

    let listener = socket(AddressFamily::UNIX, SocketType::STREAM, None)?;
    fcntl_setfd(&listener, FdFlags::CLOEXEC)?;
    set_nonblocking(listener.as_fd())?;

    let addr = SocketAddrUnix::new(sock_path.as_path()).map_err(|_| Error::SocketSymlink)?;
    bind(&listener, &addr)?;
    let _ = chmod(sock_path.as_path(), Mode::RUSR | Mode::WUSR);
    listen(&listener, 32)?;

    let _ = std::mem::ManuallyDrop::new(lock_fd); // Hold lockfile until daemon exits
    Ok(listener)
}

/// Run non-blocking poll reactor event loop.
pub fn run() -> Result<(), Error> {
    let listener = bind_listener()?;
    let mut slots: [ClientSlot; MAX_CLIENTS] = [const { ClientSlot::empty() }; MAX_CLIENTS];
    let mut poll_fds: [PollFd; MAX_CLIENTS + 1] = [PollFd::new(
        BorrowedFd::borrow_raw(-1),
        PollFlags::empty(),
    ); MAX_CLIENTS + 1];

    let mut scratch = RenderScratch::new();

    loop {
        poll_fds[0] = PollFd::new(listener.as_fd(), PollFlags::IN);
        let mut active_count = 1;

        for (i, slot) in slots.iter().enumerate() {
            if let Some(ref fd) = slot.fd {
                let flags = match slot.phase {
                    Phase::ReadHeader { .. } | Phase::ReadPayload { .. } => PollFlags::IN,
                    Phase::WriteResp { .. } => PollFlags::OUT,
                };
                poll_fds[i + 1] = PollFd::new(fd.as_fd(), flags);
                active_count = active_count.max(i + 2);
            } else {
                poll_fds[i + 1] = PollFd::new(BorrowedFd::borrow_raw(-1), PollFlags::empty());
            }
        }

        match poll(&mut poll_fds[..active_count], -1) {
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(e) => return Err(Error::Io(e)),
        }

        // Listener readable: accept new client
        if poll_fds[0].revents().contains(PollFlags::IN) {
            while let Ok(client_fd) = accept(&listener) {
                let _ = fcntl_setfd(&client_fd, FdFlags::CLOEXEC);
                let _ = set_nonblocking(client_fd.as_fd());
                let mut placed = false;
                for slot in &mut slots {
                    if slot.fd.is_none() {
                        slot.reset();
                        slot.fd = Some(client_fd);
                        placed = true;
                        break;
                    }
                }
                if !placed {
                    break; // Max clients reached, drop connection
                }
            }
        }

        // Process client slots
        for (i, slot) in slots.iter_mut().enumerate() {
            if slot.fd.is_none() {
                continue;
            }
            let rev = poll_fds[i + 1].revents();
            if rev.intersects(PollFlags::ERR | PollFlags::HUP) {
                slot.reset();
                continue;
            }

            if rev.contains(PollFlags::IN) {
                let Some(ref fd) = slot.fd else { continue; };
                match slot.phase {
                    Phase::ReadHeader { mut got } => {
                        match read(fd, &mut slot.hdr[got as usize..]) {
                            Ok(0) => slot.reset(),
                            Ok(n) => {
                                got += n as u8;
                                if got == 32 {
                                    let hdr = Header::from_le_bytes(&slot.hdr);
                                    if hdr.validate().is_err() {
                                        slot.reset();
                                    } else {
                                        slot.buf.clear();
                                        slot.buf.resize(hdr.payload_len as usize, 0);
                                        slot.phase = Phase::ReadPayload {
                                            header: hdr,
                                            got: 0,
                                        };
                                    }
                                } else {
                                    slot.phase = Phase::ReadHeader { got };
                                }
                            }
                            Err(Errno::AGAIN | Errno::INTR) => {}
                            Err(_) => slot.reset(),
                        }
                    }
                    Phase::ReadPayload { header, mut got } => {
                        let target = header.payload_len as usize;
                        if target == 0 {
                            dispatch_and_respond(slot, header, &mut scratch);
                        } else {
                            match read(fd, &mut slot.buf[got as usize..target]) {
                                Ok(0) => slot.reset(),
                                Ok(n) => {
                                    got += n as u32;
                                    if got == header.payload_len {
                                        dispatch_and_respond(slot, header, &mut scratch);
                                    } else {
                                        slot.phase = Phase::ReadPayload { header, got };
                                    }
                                }
                                Err(Errno::AGAIN | Errno::INTR) => {}
                                Err(_) => slot.reset(),
                            }
                        }
                    }
                    Phase::WriteResp { .. } => {}
                }
            }

            if rev.contains(PollFlags::OUT) {
                if let Phase::WriteResp { mut sent } = slot.phase {
                    let Some(ref fd) = slot.fd else { continue; };
                    match write(fd, &slot.buf[sent..]) {
                        Ok(n) => {
                            sent += n;
                            if sent >= slot.buf.len() {
                                slot.reset();
                            } else {
                                slot.phase = Phase::WriteResp { sent };
                            }
                        }
                        Err(Errno::AGAIN | Errno::INTR) => {}
                        Err(_) => slot.reset(),
                    }
                }
            }
        }
    }
}

fn dispatch_and_respond(slot: &mut ClientSlot, header: Header, scratch: &mut RenderScratch) {
    scratch.reset();
    let cmd = Cmd::from_u16(header.command).unwrap_or(Cmd::Ping);
    match cmd {
        Cmd::Ping => {
            ipc::encode_dual_response(Cmd::Ping, b"", b"PONG\n", &mut slot.buf);
            slot.phase = Phase::WriteResp { sent: 0 };
        }
        Cmd::Shutdown => {
            ipc::encode_dual_response(Cmd::Shutdown, b"", b"OK\n", &mut slot.buf);
            slot.phase = Phase::WriteResp { sent: 0 };
        }
        Cmd::RenderFile => {
            // Render file directly into Scene
            scratch.reset();
            let _ = crate::fmt::dispatch_into_scene(&slot.buf, "", &mut scratch.scene);
            ipc::encode_dual_response(Cmd::RenderFile, b"", b"RENDERED\n", &mut slot.buf);
            slot.phase = Phase::WriteResp { sent: 0 };
        }
        Cmd::ComposeAnsi | Cmd::ComposeExtmarks => {
            ipc::encode_dual_response(cmd, b"", b"", &mut slot.buf);
            slot.phase = Phase::WriteResp { sent: 0 };
        }
    }
}
