//! Non-blocking `poll(2)` Unix-socket reactor (`MAX_CLIENTS = 32`) with full L0/L1/L2 KGP pipeline.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fd::{AsFd, BorrowedFd, OwnedFd},
    fs::{
        AtFlags, CWD, FileType, FlockOperation, Mode, OFlags, chmodat, fchmod, fcntl_getfl,
        fcntl_setfl, flock, fstat, mkdirat, openat, statat, unlinkat,
    },
    io::{Errno, FdFlags, fcntl_setfd, read, write},
    net::{AddressFamily, SocketAddrUnix, SocketType, accept, bind, listen, socket, sockopt},
    process::{getpid, getuid},
};

pub use crate::ipc::{
    Cmd, ERR_FLAG, Extmark, Header, MAX_PAYLOAD, RESP_FLAG, encode_dual_response, encode_error,
    encode_extmarks,
};
use crate::{
    cache, compose,
    err::Error,
    fmt::{self, Kind},
    ipc::{self, MAX_CLIENTS},
    theme::{self, Palette},
    tty::Metrics,
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

#[derive(Clone, Copy)]
struct RenderCtx {
    cell_w: u16,
    cell_h: u16,
    cols: u16,
    rows: u16,
    max_w: u16,
    palette: &'static Palette,
    is_tmux: bool,
    is_remote: bool,
}

impl RenderCtx {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "max_w is clamped to u16::MAX"
    )]
    fn from_header(h: Header) -> Self {
        let cell_w = if h.cell_w > 0 {
            h.cell_w
        } else {
            Metrics::FALLBACK.cell_w
        };
        let cell_h = if h.cell_h > 0 {
            h.cell_h
        } else {
            Metrics::FALLBACK.cell_h
        };
        let cols = if h.cols > 0 {
            h.cols
        } else {
            Metrics::FALLBACK.cols
        };
        Self {
            cell_w,
            cell_h,
            cols,
            rows: h.rows.max(1),
            max_w: (u32::from(cols) * u32::from(cell_w)).min(u32::from(u16::MAX)) as u16,
            palette: if (h.flags & ipc::FLAG_THEME_LIGHT) != 0 {
                &theme::LIGHT
            } else {
                &theme::DARK
            },
            is_tmux: (h.flags & ipc::FLAG_TMUX) != 0,
            is_remote: (h.flags & ipc::FLAG_REMOTE) != 0,
        }
    }
}

struct DaemonState {
    preview_key: Option<blake3::Hash>,
    preview: compose::ComposeOut,
    page: Vec<u8>,
    row_index: crate::viewport::RowIndex,
    l2_dir: Option<OwnedFd>,
    compose_scratch: compose::ComposeScratch,
    compose_out: compose::ComposeOut,
    text_buf: Vec<u8>,
    shm_seq: u32,
    pid: u32,
}
impl DaemonState {
    fn new() -> Self {
        Self {
            preview_key: None,
            preview: compose::ComposeOut::new(),
            page: Vec::new(),
            row_index: crate::viewport::RowIndex::default(),
            l2_dir: cache::ensure_dirs().ok(),
            compose_scratch: compose::ComposeScratch::new(),
            compose_out: compose::ComposeOut::new(),
            text_buf: Vec::new(),
            shm_seq: 0,
            pid: getpid().as_raw_nonzero().get().unsigned_abs(),
        }
    }
    fn reset_request(&mut self) {
        self.compose_scratch.reset();
        self.compose_out.clear();
        // Keep the bounded response capacity between scroll requests. Shrinking
        // here forced a fresh allocation for every notebook page and amplified
        // diagram latency during rapid Yazi seeks.
        self.text_buf.clear();
    }
}

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

fn bind_listener(sock_override: Option<&Path>) -> Result<(OwnedFd, DaemonGuard), Error> {
    let sock_path = sock_override.map_or_else(ipc::socket_path, Path::to_path_buf);
    let mut lock_path = sock_path.clone();
    lock_path.set_file_name("ii.lock");
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

fn read_bounded_file(path: &Path, out: &mut Vec<u8>) -> Result<(), Errno> {
    let metadata = statat(CWD, path, AtFlags::SYMLINK_NOFOLLOW)?;
    if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile {
        return Err(Errno::INVAL);
    }
    let fd = openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?;
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

fn handle_render_file(
    slot: &mut ClientSlot,
    ctx: RenderCtx,
    state: &mut DaemonState,
    file_buf: &mut Vec<u8>,
) {
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
    if read_bounded_file(Path::new(path_str), file_buf).is_err() {
        encode_error(Cmd::RenderFile as u16, b"file read failed", &mut slot.buf);
        return;
    }
    let kind = fmt::detect_kind(file_buf, ext);
    render_response(slot, Cmd::RenderFile, ctx, state, file_buf, kind);
}

// One bounded prepared preview, independent of scroll position. File content is
// re-read and hashed on every request, so replacement and same-mtime edits invalidate it.
fn handle_preview_file(
    slot: &mut ClientSlot,
    header: Header,
    state: &mut DaemonState,
    file_buf: &mut Vec<u8>,
) {
    if slot.buf.len() < 5 || slot.buf.len() > MAX_PATH_BYTES + 4 {
        encode_error(
            Cmd::PreviewFile as u16,
            b"invalid preview path",
            &mut slot.buf,
        );
        return;
    }
    let skip = u32::from_le_bytes([slot.buf[0], slot.buf[1], slot.buf[2], slot.buf[3]]);
    let path =
        Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(&slot.buf[4..]));
    let kind = fmt::kind_from_ext(path.extension().and_then(|s| s.to_str()).unwrap_or(""));
    if read_bounded_file(path, file_buf).is_err() {
        encode_error(Cmd::PreviewFile as u16, b"file read failed", &mut slot.buf);
        return;
    }
    let kind = kind.unwrap_or_else(|| fmt::detect_kind(file_buf, ""));
    let mut hash = blake3::Hasher::new();
    hash.update(file_buf);
    hash.update(&[kind as u8]);
    hash.update(&header.to_le_bytes()[16..20]);
    hash.update(&header.to_le_bytes()[24..]);
    let key = hash.finalize();
    if state.preview_key != Some(key) {
        state.preview_key = None;
        state.row_index.clear();
        let ctx = RenderCtx::from_header(header);
        let context = compose::ComposeCtx {
            cell_w: ctx.cell_w,
            cell_h: ctx.cell_h,
            cols: ctx.cols,
            rows: ctx.rows,
            max_w: ctx.max_w,
            palette: ctx.palette,
            is_tmux: ctx.is_tmux,
            is_remote: true,
            skip_lines: 0,
            max_lines: 0,
            explicit_graphics: false,
        };
        if compose::render_file(
            file_buf,
            kind,
            context,
            state.l2_dir.as_ref().map(AsFd::as_fd),
            &mut state.compose_scratch,
            &mut state.shm_seq,
            state.pid,
            &mut state.preview,
        )
        .is_err()
        {
            encode_error(Cmd::PreviewFile as u16, b"render failed", &mut slot.buf);
            return;
        }
        if state.preview.apc.len() + state.preview.ansi.len() <= 32 * 1024 * 1024 {
            state.preview_key = Some(key);
        }
    }
    state.page.clear();
    state.row_index.page(
        &state.preview.ansi,
        header.cols,
        skip,
        u32::from(header.rows),
        &mut state.page,
    );
    compose::append_visible_uploads(&state.preview, &state.page, &mut state.compose_out.apc);
    encode_dual_response(
        Cmd::PreviewFile,
        &state.compose_out.apc,
        &state.page,
        &mut slot.buf,
    );
    if state.preview_key.is_none() {
        state.preview = compose::ComposeOut::new();
        state.row_index.clear();
    }
}

fn render_response(
    slot: &mut ClientSlot,
    cmd: Cmd,
    ctx: RenderCtx,
    state: &mut DaemonState,
    src: &[u8],
    kind: Kind,
) {
    let context = compose::ComposeCtx {
        cell_w: ctx.cell_w,
        cell_h: ctx.cell_h,
        cols: ctx.cols,
        rows: ctx.rows,
        max_w: ctx.max_w,
        palette: ctx.palette,
        is_tmux: ctx.is_tmux,
        is_remote: ctx.is_remote,
        skip_lines: 0,
        max_lines: 0,
        explicit_graphics: matches!(cmd, Cmd::DecorateBuffer | Cmd::RenderFile),
    };
    let dir = state.l2_dir.as_ref().map(AsFd::as_fd);
    if compose::render_file(
        src,
        kind,
        context,
        dir,
        &mut state.compose_scratch,
        &mut state.shm_seq,
        state.pid,
        &mut state.compose_out,
    )
    .is_err()
    {
        encode_error(cmd as u16, b"render failed", &mut slot.buf);
        return;
    }
    if cmd == Cmd::DecorateBuffer {
        let out = &state.compose_out;
        let mut blocks = Vec::<(usize, usize, Vec<u8>)>::new();
        if matches!(kind, Kind::Md | Kind::Mdx | Kind::Ipynb) {
            let mut cursor = 0;
            let mut row = 0;
            for &(a, b, c, d) in &out.blocks {
                row += memchr::memchr_iter(b'\n', &src[cursor..a]).count();
                let start = row;
                let end = start
                    + memchr::memchr_iter(b'\n', &src[a..b]).count()
                    + usize::from(b > a && src.get(b - 1) != Some(&b'\n'));
                if let Some(last) = blocks.last_mut().filter(|last| start < last.1) {
                    last.1 = last.1.max(end);
                    last.2.extend_from_slice(&out.ansi[c..d]);
                } else {
                    blocks.push((start, end.max(start + 1), out.ansi[c..d].to_vec()));
                }
                row += memchr::memchr_iter(b'\n', &src[a..b]).count();
                cursor = b;
            }
        }
        state.text_buf.clear();
        if matches!(kind, Kind::Md | Kind::Mdx | Kind::Ipynb) {
            for (a, b, text) in blocks {
                let wrapped_start = state.text_buf.len();
                state.text_buf.extend_from_slice(&[0; 12]);
                let content_start = state.text_buf.len();
                crate::viewport::append(&text, ctx.cols, 0, 0, &mut state.text_buf);
                let content_len = state.text_buf.len() - content_start;
                let values = [a, b, content_len];
                for (index, value) in values.into_iter().enumerate() {
                    state.text_buf[wrapped_start + index * 4..wrapped_start + index * 4 + 4]
                        .copy_from_slice(&u32::try_from(value).unwrap_or(u32::MAX).to_le_bytes());
                }
            }
        } else {
            let first = 0u32.to_le_bytes();
            let last = u32::try_from(memchr::memchr_iter(b'\n', src).count() + 1)
                .unwrap_or(u32::MAX)
                .to_le_bytes();
            state.text_buf.extend_from_slice(&first);
            state.text_buf.extend_from_slice(&last);
            let size_at = state.text_buf.len();
            state.text_buf.extend_from_slice(&[0; 4]);
            let content_start = state.text_buf.len();
            crate::viewport::append(&out.ansi, ctx.cols, 0, 0, &mut state.text_buf);
            let content_len =
                u32::try_from(state.text_buf.len() - content_start).unwrap_or(u32::MAX);
            state.text_buf[size_at..size_at + 4].copy_from_slice(&content_len.to_le_bytes());
        }
        encode_dual_response(cmd, &out.apc, &state.text_buf, &mut slot.buf);
    } else if cmd == Cmd::ComposeExtmarks {
        encode_extmarks(&state.compose_out.extmarks, &mut state.text_buf);
        encode_dual_response(cmd, &state.compose_out.apc, &state.text_buf, &mut slot.buf);
    } else {
        encode_dual_response(
            cmd,
            &state.compose_out.apc,
            &state.compose_out.ansi,
            &mut slot.buf,
        );
    }
}

fn handle_compose(slot: &mut ClientSlot, cmd: Cmd, ctx: RenderCtx, state: &mut DaemonState) {
    let src = core::mem::take(&mut slot.buf);
    render_response(slot, cmd, ctx, state, &src, Kind::Md);
}

fn dispatch_and_respond(
    slot: &mut ClientSlot,
    header: Header,
    state: &mut DaemonState,
    file_buf: &mut Vec<u8>,
    shutting_down: &mut bool,
) {
    state.reset_request();
    let Ok(cmd) = Cmd::from_u16(header.command) else {
        encode_error(header.command, b"unknown command", &mut slot.buf);
        if slot.buf.len() >= 16 {
            slot.buf[8..12].copy_from_slice(&header.req_id_lo.to_le_bytes());
            slot.buf[12..16].copy_from_slice(&header.req_id_hi.to_le_bytes());
        }
        step_write_resp(slot, 0);
        return;
    };
    let ctx = RenderCtx::from_header(header);
    match cmd {
        Cmd::Ping => {
            encode_dual_response(Cmd::Ping, b"", b"", &mut slot.buf);
        }
        Cmd::Shutdown => {
            *shutting_down = true;
            encode_dual_response(Cmd::Shutdown, b"", b"", &mut slot.buf);
        }
        Cmd::RenderFile => handle_render_file(slot, ctx, state, file_buf),
        Cmd::PreviewFile => handle_preview_file(slot, header, state, file_buf),
        Cmd::ComposeAnsi | Cmd::ComposeExtmarks => handle_compose(slot, cmd, ctx, state),
        Cmd::RenderBuffer | Cmd::DecorateBuffer => {
            let src = core::mem::take(&mut slot.buf);
            if let Some(end) = memchr::memchr(0, &src).filter(|&end| end <= 32) {
                let ext = core::str::from_utf8(&src[..end]).unwrap_or("");
                let bytes = &src[end + 1..];
                render_response(slot, cmd, ctx, state, bytes, fmt::detect_kind(bytes, ext));
            } else {
                encode_error(cmd as u16, b"invalid format prefix", &mut slot.buf);
            }
        }
    }
    if slot.buf.len() >= 16 {
        slot.buf[8..12].copy_from_slice(&header.req_id_lo.to_le_bytes());
        slot.buf[12..16].copy_from_slice(&header.req_id_hi.to_le_bytes());
    }
    step_write_resp(slot, 0);
}

fn step_read_header(
    slot: &mut ClientSlot,
    mut got: u8,
    state: &mut DaemonState,
    file_buf: &mut Vec<u8>,
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
                if slot.buf.len() >= 16 {
                    slot.buf[8..12].copy_from_slice(&hdr.req_id_lo.to_le_bytes());
                    slot.buf[12..16].copy_from_slice(&hdr.req_id_hi.to_le_bytes());
                }
                step_write_resp(slot, 0);
                return;
            }
            slot.buf.clear();
            if hdr.payload_len == 0 {
                dispatch_and_respond(slot, hdr, state, file_buf, shutting_down);
            } else {
                slot.phase = Phase::ReadPayload {
                    header: hdr,
                    got: 0,
                };
                step_read_payload(slot, hdr, 0, state, file_buf, shutting_down);
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
    state: &mut DaemonState,
    file_buf: &mut Vec<u8>,
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
                dispatch_and_respond(slot, header, state, file_buf, shutting_down);
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
    state: &mut DaemonState,
    file_buf: &mut Vec<u8>,
    shutting_down: &mut bool,
) {
    if rev.intersects(PollFlags::ERR | PollFlags::NVAL) {
        slot.reset();
        return;
    }
    if rev.contains(PollFlags::IN) {
        match slot.phase {
            Phase::ReadHeader { got } => {
                step_read_header(slot, got, state, file_buf, shutting_down);
            }
            Phase::ReadPayload { header, got } => {
                step_read_payload(slot, header, got, state, file_buf, shutting_down);
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
pub fn run(sock_override: Option<&Path>) -> Result<(), Error> {
    let (listener, _guard) = bind_listener(sock_override)?;
    let mut slots: [ClientSlot; MAX_CLIENTS] = [const { ClientSlot::empty() }; MAX_CLIENTS];
    let mut state = DaemonState::new();
    let mut file_buf = Vec::with_capacity(64 * 1024);
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
                    &mut state,
                    &mut file_buf,
                    &mut shutting_down,
                );
            }
        }
    }
    Ok(())
}
