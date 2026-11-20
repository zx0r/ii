//! Non-blocking `poll(2)` Unix-socket reactor (`MAX_CLIENTS = 32`). Zero `unsafe`, zero hot-loop allocs.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fd::{AsFd, BorrowedFd, OwnedFd},
    fs::{
        AtFlags, CWD, FileType, FlockOperation, Mode, OFlags, chmodat, fchmod, fcntl_getfl,
        fcntl_setfl, flock, fstat, mkdirat, openat, unlinkat,
    },
    io::{Errno, FdFlags, fcntl_setfd, read, write},
    net::{AddressFamily, SocketAddrUnix, SocketType, accept, bind, listen, socket, sockopt},
    process::getuid,
};

pub use crate::ipc::{
    Cmd, ERR_FLAG, Extmark, Header, MAX_PAYLOAD, RESP_FLAG, encode_dual_response, encode_error,
    encode_extmarks,
};
use crate::{
    err::Error,
    ipc::{self, MAX_CLIENTS},
    scene::RenderScratch,
};

const READ_CHUNK: usize = 64 * 1024;
const BUF_HIGH_WATER: usize = 64 * 1024;
const RECV_BUF_BYTES: usize = 512 * 1024;
const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 4096;
const IDLE_TIMEOUT: Timespec = Timespec {
    tv_sec: 30,
    tv_nsec: 0,
};
const DRAIN_TIMEOUT: Timespec = Timespec {
    tv_sec: 1,
    tv_nsec: 0,
};

#[derive(Clone, Copy, Debug)]
enum Phase {
    ReadHeader { got: u8 },
    ReadPayload { header: Header, got: u32 },
    WriteResp { sent: usize },
}

#[derive(Debug)]
struct ClientSlot {
    fd: Option<OwnedFd>,
    phase: Phase,
    hdr: [u8; 32],
    buf: Vec<u8>,
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

    fn accept(&mut self, fd: OwnedFd) {
        self.reset();
        self.fd = Some(fd);
    }

    fn reset(&mut self) {
        self.fd = None;
        self.phase = Phase::ReadHeader { got: 0 };
        self.hdr = [0; 32];
        self.buf.clear();
        if self.buf.capacity() > BUF_HIGH_WATER {
            self.buf.shrink_to(BUF_HIGH_WATER);
        }
    }
}

struct DaemonGuard {
    dir_fd: OwnedFd,
    _lock_fd: OwnedFd,
    sock_name: OsString,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = unlinkat(&self.dir_fd, &self.sock_name, AtFlags::empty());
    }
}

fn configure_fd(fd: BorrowedFd<'_>) -> Result<(), Error> {
    fcntl_setfd(fd, FdFlags::CLOEXEC)?;
    let flags = fcntl_getfl(fd)?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK)?;
    Ok(())
}

fn open_secure_runtime_dir(dir: &Path) -> Result<OwnedFd, Error> {
    match mkdirat(CWD, dir, Mode::RWXU) {
        Ok(()) | Err(Errno::EXIST) => {}
        Err(e) => return Err(Error::Io(e)),
    }
    let dir_fd = openat(
        CWD,
        dir,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| {
        if matches!(e, Errno::LOOP | Errno::NOTDIR) {
            Error::SocketSymlink
        } else {
            Error::Io(e)
        }
    })?;

    let st = fstat(&dir_fd)?;
    if FileType::from_raw_mode(st.st_mode) != FileType::Directory {
        return Err(Error::SocketSymlink);
    }
    if st.st_uid != getuid().as_raw() {
        return Err(Error::SocketUidMismatch);
    }
    fchmod(&dir_fd, Mode::RWXU)?;
    Ok(dir_fd)
}

fn bind_listener() -> Result<(OwnedFd, DaemonGuard), Error> {
    let (sock_path, lock_path) = (ipc::socket_path(), ipc::lock_path());
    let dir = sock_path.parent().map_or_else(
        || PathBuf::from(format!("/tmp/ii-{}", getuid().as_raw())),
        Path::to_path_buf,
    );
    let sock_name = sock_path
        .file_name()
        .ok_or(Error::Io(Errno::INVAL))?
        .to_os_string();
    let lock_name = lock_path.file_name().ok_or(Error::Io(Errno::INVAL))?;

    let dir_fd = open_secure_runtime_dir(&dir)?;
    let lock_fd = openat(
        &dir_fd,
        lock_name,
        OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )?;
    if flock(&lock_fd, FlockOperation::NonBlockingLockExclusive).is_err() {
        return Err(Error::DaemonLocked);
    }
    let _ = unlinkat(&dir_fd, &sock_name, AtFlags::empty());

    let listener = socket(AddressFamily::UNIX, SocketType::STREAM, None)?;
    configure_fd(listener.as_fd())?;
    let _ = sockopt::set_socket_recv_buffer_size(&listener, RECV_BUF_BYTES);
    bind(
        &listener,
        &SocketAddrUnix::new(&sock_path).map_err(|_| Error::Io(Errno::NAMETOOLONG))?,
    )?;
    let _ = chmodat(
        &dir_fd,
        &sock_name,
        Mode::RUSR | Mode::WUSR,
        AtFlags::empty(),
    );
    listen(&listener, i32::try_from(MAX_CLIENTS).unwrap_or(32))?;

    Ok((
        listener,
        DaemonGuard {
            dir_fd,
            _lock_fd: lock_fd,
            sock_name,
        },
    ))
}

fn step_read_header(
    slot: &mut ClientSlot,
    mut got: u8,
    scratch: &mut RenderScratch,
    shutting_down: &mut bool,
) {
    let Some(fd) = slot.fd.as_ref() else { return };
    match read(fd.as_fd(), &mut slot.hdr[usize::from(got)..]) {
        Ok(0) => slot.reset(),
        Ok(n) => {
            got += u8::try_from(n).unwrap_or(32);
            if got < 32 {
                slot.phase = Phase::ReadHeader { got };
                return;
            }
            let hdr = Header::from_le_bytes(&slot.hdr);
            if hdr.validate().is_err() {
                encode_error(hdr.command, b"invalid header", &mut slot.buf);
                step_write_resp(slot, 0);
                return;
            }
            slot.buf.clear();
            if hdr.payload_len == 0 {
                dispatch_and_respond(slot, hdr, scratch, shutting_down);
            } else {
                slot.phase = Phase::ReadPayload {
                    header: hdr,
                    got: 0,
                };
                step_read_payload(slot, hdr, 0, scratch, shutting_down);
            }
        }
        Err(Errno::AGAIN | Errno::INTR) => {}
        Err(_) => slot.reset(),
    }
}

fn step_read_payload(
    slot: &mut ClientSlot,
    header: Header,
    mut got: u32,
    scratch: &mut RenderScratch,
    shutting_down: &mut bool,
) {
    let next_end = (got as usize)
        .saturating_add(READ_CHUNK)
        .min(header.payload_len as usize);
    if slot.buf.len() < next_end {
        slot.buf.resize(next_end, 0);
    }
    let Some(fd) = slot.fd.as_ref() else { return };
    match read(fd.as_fd(), &mut slot.buf[got as usize..next_end]) {
        Ok(0) => slot.reset(),
        Ok(n) => {
            got += u32::try_from(n).unwrap_or(0);
            if got == header.payload_len {
                dispatch_and_respond(slot, header, scratch, shutting_down);
            } else {
                slot.phase = Phase::ReadPayload { header, got };
            }
        }
        Err(Errno::AGAIN | Errno::INTR) => {}
        Err(_) => slot.reset(),
    }
}

fn step_write_resp(slot: &mut ClientSlot, mut sent: usize) {
    while let Some(fd) = slot.fd.as_ref() {
        match write(fd.as_fd(), &slot.buf[sent..]) {
            Ok(0) => {
                slot.reset();
                return;
            }
            Ok(n) => {
                sent += n;
                if sent >= slot.buf.len() {
                    slot.reset();
                    return;
                }
            }
            Err(Errno::INTR) => {}
            Err(Errno::AGAIN) => {
                slot.phase = Phase::WriteResp { sent };
                return;
            }
            Err(_) => {
                slot.reset();
                return;
            }
        }
    }
}

fn step_client(
    slot: &mut ClientSlot,
    rev: PollFlags,
    scratch: &mut RenderScratch,
    shutting_down: &mut bool,
) {
    if rev.intersects(PollFlags::ERR | PollFlags::NVAL) {
        slot.reset();
        return;
    }
    if rev.contains(PollFlags::IN) {
        match slot.phase {
            Phase::ReadHeader { got } => step_read_header(slot, got, scratch, shutting_down),
            Phase::ReadPayload { header, got } => {
                step_read_payload(slot, header, got, scratch, shutting_down);
            }
            Phase::WriteResp { .. } => {}
        }
    } else if rev.contains(PollFlags::OUT)
        && let Phase::WriteResp { sent } = slot.phase
    {
        step_write_resp(slot, sent);
    }
    if slot.fd.is_some() && rev.contains(PollFlags::HUP) && !rev.contains(PollFlags::IN) {
        slot.reset();
    }
}

fn accept_pending(listener: BorrowedFd<'_>, slots: &mut [ClientSlot; MAX_CLIENTS]) {
    while let Some(slot) = slots.iter_mut().find(|s| s.fd.is_none()) {
        match accept(listener) {
            Ok(client_fd) => {
                if configure_fd(client_fd.as_fd()).is_ok() {
                    slot.accept(client_fd);
                }
            }
            Err(Errno::INTR) => {}
            Err(_) => break,
        }
    }
}

/// Run non-blocking `poll(2)` reactor until `SHUTDOWN` or idle timeout.
///
/// # Errors
/// Returns `Error::DaemonLocked`, `Error::SocketSymlink`, `Error::SocketUidMismatch`, or `Error::Io`.
pub fn run() -> Result<(), Error> {
    let (listener, _guard) = bind_listener()?;
    let mut slots: [ClientSlot; MAX_CLIENTS] = [const { ClientSlot::empty() }; MAX_CLIENTS];
    let mut scratch = RenderScratch::new();
    let mut shutting_down = false;

    loop {
        let mut slot_map = [0u8; MAX_CLIENTS];
        let mut ready = [PollFlags::empty(); MAX_CLIENTS + 1];
        let mut n_active = 0usize;

        let active_clients = slots.iter().filter(|s| s.fd.is_some()).count();
        if shutting_down && active_clients == 0 {
            break;
        }
        let listen_enabled = !shutting_down && active_clients < MAX_CLIENTS;
        let offset = usize::from(listen_enabled);

        let poll_res = {
            let mut pfds = core::array::from_fn::<_, { MAX_CLIENTS + 1 }, _>(|_| {
                PollFd::new(&listener, PollFlags::empty())
            });
            if listen_enabled {
                pfds[0] = PollFd::new(&listener, PollFlags::IN);
            }
            for (idx, slot) in slots.iter().enumerate() {
                if let Some(ref fd) = slot.fd {
                    let ev = match slot.phase {
                        Phase::ReadHeader { .. } | Phase::ReadPayload { .. } => PollFlags::IN,
                        Phase::WriteResp { .. } => PollFlags::OUT,
                    };
                    pfds[offset + n_active] = PollFd::new(fd, ev);
                    slot_map[n_active] = u8::try_from(idx).unwrap_or(0);
                    n_active += 1;
                }
            }
            let active = &mut pfds[..offset + n_active];
            let timeout = if shutting_down {
                Some(&DRAIN_TIMEOUT)
            } else if n_active == 0 {
                Some(&IDLE_TIMEOUT)
            } else {
                None
            };
            let res = poll(active, timeout);
            if res.is_ok() {
                for (i, pfd) in active.iter().enumerate() {
                    ready[i] = pfd.revents();
                }
            }
            res
        };

        match poll_res {
            Ok(0) if n_active == 0 || shutting_down => break,
            Ok(_) | Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
        if listen_enabled && ready[0].contains(PollFlags::IN) {
            accept_pending(listener.as_fd(), &mut slots);
        }
        for (i, &idx) in slot_map.iter().take(n_active).enumerate() {
            let rev = ready[offset + i];
            if !rev.is_empty() {
                step_client(
                    &mut slots[usize::from(idx)],
                    rev,
                    &mut scratch,
                    &mut shutting_down,
                );
            }
        }
    }
    Ok(())
}

fn read_bounded_file(path: &str, out: &mut Vec<u8>) -> Result<(), Errno> {
    let fd = openat(CWD, path, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?;
    let st = fstat(&fd)?;
    if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
        return Err(Errno::INVAL);
    }
    let size = usize::try_from(st.st_size).map_err(|_| Errno::FBIG)?;
    if size > MAX_FILE_BYTES {
        return Err(Errno::FBIG);
    }
    out.clear();
    out.resize(size, 0);
    let mut got = 0usize;
    while got < size {
        match read(&fd, &mut out[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(Errno::INTR) => {}
            Err(e) => return Err(e),
        }
    }
    out.truncate(got);
    Ok(())
}

fn handle_render_file(slot: &mut ClientSlot, scratch: &mut RenderScratch) {
    if slot.buf.is_empty() || slot.buf.len() > MAX_PATH_BYTES {
        encode_error(
            Cmd::RenderFile as u16,
            b"invalid path length",
            &mut slot.buf,
        );
        return;
    }
    let Ok(path_str) = core::str::from_utf8(&slot.buf) else {
        encode_error(Cmd::RenderFile as u16, b"non-utf8 path", &mut slot.buf);
        return;
    };
    let ext = Path::new(path_str)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    scratch.reset();
    if read_bounded_file(path_str, &mut scratch.out_buf).is_err() {
        encode_error(Cmd::RenderFile as u16, b"file read failed", &mut slot.buf);
        return;
    }
    if crate::fmt::dispatch_into_scene(&scratch.out_buf, ext, &mut scratch.scene).is_err() {
        encode_error(Cmd::RenderFile as u16, b"parse failed", &mut slot.buf);
        return;
    }
    encode_dual_response(Cmd::RenderFile, b"", b"", &mut slot.buf);
}

fn dispatch_and_respond(
    slot: &mut ClientSlot,
    header: Header,
    scratch: &mut RenderScratch,
    shutting_down: &mut bool,
) {
    scratch.reset();
    let Ok(cmd) = Cmd::from_u16(header.command) else {
        encode_error(header.command, b"unknown command", &mut slot.buf);
        step_write_resp(slot, 0);
        return;
    };
    match cmd {
        Cmd::Ping => encode_dual_response(Cmd::Ping, b"", b"", &mut slot.buf),
        Cmd::Shutdown => {
            *shutting_down = true;
            encode_dual_response(Cmd::Shutdown, b"", b"", &mut slot.buf);
        }
        Cmd::RenderFile => handle_render_file(slot, scratch),
        Cmd::ComposeAnsi | Cmd::ComposeExtmarks => {
            encode_dual_response(cmd, b"", b"", &mut slot.buf);
        }
    }
    step_write_resp(slot, 0);
}
