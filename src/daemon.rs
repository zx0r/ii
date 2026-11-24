//! Non-blocking Unix-socket reactor and bounded render worker.

use std::{
    ffi::OsString,
    io::Write as _,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    time::{Duration, Instant},
};

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fd::{AsFd, BorrowedFd, OwnedFd},
    fs::{
        AtFlags, CWD, FileType, FlockOperation, Mode, OFlags, chmodat, fchmod, fcntl_getfl,
        fcntl_setfl, flock, fstat, mkdirat, openat, unlinkat,
    },
    io::{Errno, FdFlags, fcntl_setfd, read},
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
    file,
    fmt::{self, Kind},
    ipc::{self, MAX_CLIENTS},
    theme,
    tty::Metrics,
};

const READ_CHUNK: usize = 64 * 1024;
const BUF_HIGH_WATER: usize = 64 * 1024;
const RECV_BUF_BYTES: usize = 512 * 1024;
const MAX_FRAME_BYTES: usize = MAX_PAYLOAD as usize;
const MAX_PATH_BYTES: usize = 4096;
const IDLE_TIMEOUT: Timespec = Timespec {
    tv_sec: 30,
    tv_nsec: 0,
};
const DRAIN_TIMEOUT: Timespec = Timespec {
    tv_sec: 1,
    tv_nsec: 0,
};
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_JOBS: usize = 4;
const JOB_BYTES: usize = 96 * 1024 * 1024;
const WIRE_BYTES: usize = 160 * 1024 * 1024;

fn render_context(h: Header) -> compose::ComposeCtx<'static> {
    let cell_w = if h.cell_w == 0 {
        Metrics::FALLBACK.cell_w
    } else {
        h.cell_w
    };
    let cell_h = if h.cell_h == 0 {
        Metrics::FALLBACK.cell_h
    } else {
        h.cell_h
    };
    let cols = if h.cols == 0 {
        Metrics::FALLBACK.cols
    } else {
        h.cols
    };
    compose::ComposeCtx {
        cell_w,
        cell_h,
        cols,
        rows: h.rows.max(1),
        max_w: cols.saturating_mul(cell_w),
        palette: if h.flags & ipc::FLAG_THEME_LIGHT != 0 {
            &theme::LIGHT
        } else {
            &theme::DARK
        },
        is_tmux: h.flags & ipc::FLAG_TMUX != 0,
        is_remote: h.flags & ipc::FLAG_REMOTE != 0,
        skip_lines: 0,
        max_lines: 0,
        explicit_graphics: true,
    }
}

struct DaemonState {
    preview_key: Option<blake3::Hash>,
    preview: compose::ComposeOut,
    page: Vec<u8>,
    visible: Vec<u32>,
    row_index: crate::viewport::RowIndex,
    l2_dir: Option<OwnedFd>,
    compose_scratch: compose::ComposeScratch,
    compose_out: compose::ComposeOut,
    shm_seq: u32,
    pid: u32,
}
impl DaemonState {
    fn new(l2_dir: Option<OwnedFd>) -> Self {
        Self {
            preview_key: None,
            preview: compose::ComposeOut::new(),
            page: Vec::new(),
            visible: Vec::new(),
            row_index: crate::viewport::RowIndex::default(),
            l2_dir,
            compose_scratch: compose::ComposeScratch::new(),
            compose_out: compose::ComposeOut::new(),
            shm_seq: 0,
            pid: getpid().as_raw_nonzero().get().unsigned_abs(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    ReadHeader { got: u8 },
    ReadPayload { header: Header, got: u32 },
    Render { id: u64 },
    WriteResp { sent: usize },
}

#[derive(Debug)]
struct ClientSlot {
    fd: Option<OwnedFd>,
    phase: Phase,
    hdr: [u8; 32],
    buf: Vec<u8>,
    deadline: Option<Instant>,
}

impl ClientSlot {
    const fn empty() -> Self {
        Self {
            fd: None,
            phase: Phase::ReadHeader { got: 0 },
            hdr: [0; 32],
            buf: Vec::new(),
            deadline: None,
        }
    }

    fn accept(&mut self, fd: OwnedFd) {
        self.reset();
        self.fd = Some(fd);
        self.progress();
    }

    fn progress(&mut self) {
        self.deadline = Some(Instant::now() + CLIENT_TIMEOUT);
    }

    fn reset(&mut self) {
        if matches!(self.phase, Phase::WriteResp { .. }) {
            discard_response(&self.buf);
        }
        self.fd = None;
        self.deadline = None;
        self.phase = Phase::ReadHeader { got: 0 };
        self.hdr = [0; 32];
        self.buf.clear();
        if self.buf.capacity() > BUF_HIGH_WATER {
            self.buf.shrink_to(BUF_HIGH_WATER);
        }
    }
}

struct Job {
    index: usize,
    id: u64,
    header: Header,
    charge: usize,
    buf: Vec<u8>,
    response: bool,
}

impl Drop for Job {
    fn drop(&mut self) {
        if self.response
            && self.header.flags & ipc::FLAG_REMOTE == 0
            && self.header.command != Cmd::PreviewFile as u16
        {
            discard_response(&self.buf);
        }
    }
}

fn discard_response(buf: &[u8]) {
    if let Some(header) = buf.first_chunk::<32>() {
        let len = Header::from_le_bytes(header).flags as usize;
        if let Some(apc) = buf.get(32..32 + len) {
            crate::kitty::discard_shm(apc);
        }
    }
}

struct Worker {
    wake: UnixStream,
    jobs: SyncSender<Job>,
    replies: Receiver<Job>,
    current: Arc<[AtomicU64; MAX_CLIENTS]>,
    pending: usize,
    bytes: usize,
    next: u64,
}

impl Worker {
    fn new(use_disk_cache: bool) -> Result<Self, Error> {
        let (wake, mut notify) = UnixStream::pair().map_err(|err| os_error(&err))?;
        wake.set_nonblocking(true).map_err(|err| os_error(&err))?;
        let (jobs, queue) = sync_channel::<Job>(MAX_JOBS);
        let (done, replies) = sync_channel(1);
        let current = Arc::new([const { AtomicU64::new(0) }; MAX_CLIENTS]);
        let active = Arc::clone(&current);
        std::thread::Builder::new()
            .name("ii-render".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let mut state =
                    DaemonState::new(use_disk_cache.then(|| cache::ensure_dirs().ok()).flatten());
                let mut src = Vec::new();
                while let Ok(mut job) = queue.recv() {
                    if active[job.index].load(Ordering::Acquire) == job.id {
                        render_request(&mut job.buf, job.header, &mut state, &mut src);
                        correlate(&mut job.buf, job.header);
                        job.response = true;
                    } else {
                        job.buf.clear();
                    }
                    if src.capacity() > 4 * 1024 * 1024 {
                        src = Vec::new();
                    }
                    if done.send(job).is_err() || notify.write_all(&[1]).is_err() {
                        break;
                    }
                }
            })
            .map_err(|err| os_error(&err))?;
        Ok(Self {
            wake,
            jobs,
            replies,
            current,
            pending: 0,
            bytes: 0,
            next: 0,
        })
    }

    fn submit(&mut self, slot: &mut ClientSlot, index: usize, header: Header) -> bool {
        let charge = slot.buf.capacity();
        if self.pending >= MAX_JOBS || self.bytes.saturating_add(charge) > JOB_BYTES {
            return false;
        }
        self.next = self.next.wrapping_add(1).max(1);
        let id = self.next;
        self.current[index].store(id, Ordering::Release);
        let job = Job {
            index,
            id,
            header,
            charge,
            buf: core::mem::take(&mut slot.buf),
            response: false,
        };
        if self.jobs.try_send(job).is_err() {
            return false;
        }
        self.pending += 1;
        self.bytes += charge;
        slot.phase = Phase::Render { id };
        slot.progress();
        true
    }

    fn cancel(&self, index: usize) {
        self.current[index].store(0, Ordering::Release);
    }

    fn receive(&mut self, slots: &mut [ClientSlot; MAX_CLIENTS]) {
        let _ = read(&self.wake, &mut [0; MAX_JOBS]);
        while let Ok(mut reply) = self.replies.try_recv() {
            self.pending -= 1;
            self.bytes -= reply.charge;
            let slot = &slots[reply.index];
            if slot.fd.is_none() || !matches!(slot.phase, Phase::Render { id } if id == reply.id) {
                continue;
            }
            let room = wire_room(slots, self.bytes);
            let slot = &mut slots[reply.index];
            if reply.buf.capacity() > room {
                encode_error(
                    reply.header.command,
                    b"response budget exhausted",
                    &mut slot.buf,
                );
                correlate(&mut slot.buf, reply.header);
            } else {
                slot.buf = core::mem::take(&mut reply.buf);
            }
            slot.progress();
            slot.phase = Phase::WriteResp { sent: 0 };
            step_write_resp(slot, 0);
        }
    }
}

fn os_error(err: &std::io::Error) -> Error {
    Error::Io(Errno::from_io_error(err).unwrap_or(Errno::IO))
}

fn wire_room(slots: &[ClientSlot; MAX_CLIENTS], queued: usize) -> usize {
    let used = slots
        .iter()
        .map(|slot| match slot.phase {
            Phase::ReadPayload { header, .. } => {
                (header.payload_len as usize).max(slot.buf.capacity())
            }
            Phase::WriteResp { .. } => slot.buf.capacity(),
            _ => 0,
        })
        .sum::<usize>();
    WIRE_BYTES.saturating_sub(used.saturating_add(queued))
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
    #[cfg(target_os = "macos")]
    sockopt::set_socket_nosigpipe(fd, true)?;
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

fn handle_render_file(
    buf: &mut Vec<u8>,
    ctx: compose::ComposeCtx<'_>,
    state: &mut DaemonState,
    file_buf: &mut Vec<u8>,
) {
    if buf.is_empty() || buf.len() > MAX_PATH_BYTES {
        encode_error(Cmd::RenderFile as u16, b"invalid path length", buf);
        return;
    }
    let Ok(path_str) = core::str::from_utf8(buf) else {
        encode_error(Cmd::RenderFile as u16, b"non-utf8 path", buf);
        return;
    };
    let ext = Path::new(path_str)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if file::read_bounded(Path::new(path_str), MAX_PAYLOAD as usize, file_buf).is_err() {
        encode_error(Cmd::RenderFile as u16, b"file read failed", buf);
        return;
    }
    let kind = fmt::detect_kind(file_buf, ext);
    render_response(buf, Cmd::RenderFile, ctx, state, file_buf, kind);
}

// One bounded prepared preview, independent of scroll position. File content is
// re-read and hashed on every request, so replacement and same-mtime edits invalidate it.
fn handle_preview_file(
    buf: &mut Vec<u8>,
    header: Header,
    state: &mut DaemonState,
    file_buf: &mut Vec<u8>,
) {
    if buf.len() < 5 || buf.len() > MAX_PATH_BYTES + 4 {
        encode_error(Cmd::PreviewFile as u16, b"invalid preview path", buf);
        return;
    }
    let skip = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let path = Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(&buf[4..]));
    let kind = fmt::kind_from_ext(path.extension().and_then(|s| s.to_str()).unwrap_or(""));
    if file::read_bounded(path, MAX_PAYLOAD as usize, file_buf).is_err() {
        encode_error(Cmd::PreviewFile as u16, b"file read failed", buf);
        return;
    }
    let kind = kind.unwrap_or_else(|| fmt::detect_kind(file_buf, ""));
    let mut hash = blake3::Hasher::new();
    hash.update(file_buf);
    hash.update(&[kind as u8]);
    hash.update(&(header.flags & (ipc::FLAG_THEME_LIGHT | ipc::FLAG_TMUX)).to_le_bytes());
    let geometry = header.to_le_bytes();
    hash.update(&geometry[24..26]);
    hash.update(&geometry[28..]);
    if kind.fits_viewport_height() {
        hash.update(&geometry[26..28]);
    }
    let key = hash.finalize();
    let ctx = render_context(header);
    if state.preview_key != Some(key) {
        state.preview_key = None;
        state.row_index.clear();
        let context = compose::ComposeCtx {
            is_remote: true,
            ..ctx
        };
        state.compose_scratch.reset();
        if let Err(error) = compose::render(
            file_buf,
            kind,
            context,
            state.l2_dir.as_ref().map(AsFd::as_fd),
            &mut state.compose_scratch,
            &mut state.shm_seq,
            state.pid,
            &mut state.preview,
            compose::Anchors::None,
        ) {
            encode_error(Cmd::PreviewFile as u16, error.to_string().as_bytes(), buf);
            return;
        }
        state
            .preview
            .uploads
            .sort_unstable_by_key(|upload| upload.image_id);
        if state.preview.apc.len() + state.preview.ansi.len() <= 32 * 1024 * 1024 {
            state.preview_key = Some(key);
        }
    }
    let result = (|| {
        state.page.clear();
        state.row_index.page(
            &state.preview.ansi,
            ctx.cols,
            skip,
            u32::from(ctx.rows),
            &mut state.page,
            MAX_FRAME_BYTES,
        )?;
        buf.clear();
        buf.resize(32, 0);
        let upload_limit = 32 + MAX_FRAME_BYTES - state.page.len();
        if !state.preview.uploads.is_empty() {
            compose::visible_ids(&state.page, &mut state.visible);
            for id in &state.visible {
                if let Ok(index) = state
                    .preview
                    .uploads
                    .binary_search_by_key(id, |upload| upload.image_id)
                {
                    let upload = &state.preview.uploads[index];
                    if buf.len() + (upload.end - upload.start) > upload_limit {
                        return Err(Error::ResourceLimit);
                    }
                    buf.extend_from_slice(&state.preview.apc[upload.start..upload.end]);
                }
            }
        }
        let apc_len = buf.len() - 32;
        buf.extend_from_slice(&state.page);
        ipc::finish_dual_response(Cmd::PreviewFile, apc_len, buf);
        Ok(())
    })();
    if state.preview_key.is_none() {
        state.preview = compose::ComposeOut::new();
        state.row_index.clear();
    }
    if let Err(error) = result {
        encode_error(Cmd::PreviewFile as u16, error.to_string().as_bytes(), buf);
    }
}

fn render_response(
    buf: &mut Vec<u8>,
    cmd: Cmd,
    ctx: compose::ComposeCtx<'_>,
    state: &mut DaemonState,
    src: &[u8],
    kind: Kind,
) {
    let context = compose::ComposeCtx {
        explicit_graphics: matches!(cmd, Cmd::DecorateBuffer | Cmd::RenderFile),
        ..ctx
    };
    let dir = state.l2_dir.as_ref().map(AsFd::as_fd);
    state.compose_scratch.reset();
    if let Err(error) = compose::render(
        src,
        kind,
        context,
        dir,
        &mut state.compose_scratch,
        &mut state.shm_seq,
        state.pid,
        &mut state.compose_out,
        match cmd {
            Cmd::DecorateBuffer => compose::Anchors::Blocks,
            Cmd::ComposeExtmarks => compose::Anchors::Extmarks,
            _ => compose::Anchors::None,
        },
    ) {
        encode_error(cmd as u16, error.to_string().as_bytes(), buf);
        return;
    }
    let out = &state.compose_out;
    match cmd {
        Cmd::DecorateBuffer => {
            buf.clear();
            buf.resize(32, 0);
            buf.extend_from_slice(&out.apc);
            let limit = 32 + MAX_FRAME_BYTES;
            if pack_blocks(src, out, kind, ctx.cols, limit, buf).is_err() {
                if !ctx.is_remote {
                    crate::kitty::discard_shm(&out.apc);
                }
                encode_error(cmd as u16, b"render exceeds frame limit", buf);
            } else {
                ipc::finish_dual_response(cmd, out.apc.len(), buf);
            }
        }
        Cmd::ComposeExtmarks => {
            let total = out
                .apc
                .len()
                .saturating_add(out.extmarks.len().saturating_mul(16));
            if total > MAX_FRAME_BYTES {
                if !ctx.is_remote {
                    crate::kitty::discard_shm(&out.apc);
                }
                encode_error(cmd as u16, b"render exceeds frame limit", buf);
            } else {
                buf.clear();
                buf.resize(32, 0);
                buf.extend_from_slice(&out.apc);
                encode_extmarks(&out.extmarks, buf);
                ipc::finish_dual_response(cmd, out.apc.len(), buf);
            }
        }
        _ => encode_dual_response(cmd, &out.apc, &out.ansi, buf),
    }
}

fn pack_blocks(
    src: &[u8],
    out: &compose::ComposeOut,
    kind: Kind,
    cols: u16,
    limit: usize,
    dst: &mut Vec<u8>,
) -> Result<(), Error> {
    let mut append = |first: usize, end: usize, ansi: &[u8]| {
        let header = dst.len();
        if header.saturating_add(12) > limit {
            return Err(Error::ResourceLimit);
        }
        dst.extend_from_slice(&[0; 12]);
        crate::viewport::append_bounded(ansi, cols, limit, dst)?;
        let len = dst.len() - header - 12;
        for (bytes, n) in dst[header..header + 12]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip([first, end, len])
        {
            *bytes = u32::try_from(n)
                .map_err(|_| Error::ResourceLimit)?
                .to_le_bytes();
        }
        Ok(())
    };
    if !matches!(kind, Kind::Md | Kind::Mdx | Kind::Ipynb | Kind::Typst) {
        return append(0, memchr::memchr_iter(b'\n', src).count() + 1, &out.ansi);
    }

    let mut pending: Option<(usize, usize, usize, usize)> = None;
    let mut cursor = 0;
    let mut row = 0;
    for &(a, b, c, d) in &out.blocks {
        row += memchr::memchr_iter(b'\n', &src[cursor..a]).count();
        let lines = memchr::memchr_iter(b'\n', &src[a..b]).count();
        let end = row + lines + usize::from(b > a && src[b - 1] != b'\n');
        if let Some(block) = pending.as_mut().filter(|block| row < block.1) {
            block.1 = block.1.max(end);
            block.3 = d;
        } else {
            if let Some((first, last, start, end)) = pending {
                append(first, last, &out.ansi[start..end])?;
            }
            pending = Some((row, end.max(row + 1), c, d));
        }
        row += lines;
        cursor = b;
    }
    if let Some((first, last, start, end)) = pending {
        append(first, last, &out.ansi[start..end])?;
    }
    Ok(())
}

fn render_request(
    buf: &mut Vec<u8>,
    header: Header,
    state: &mut DaemonState,
    file_buf: &mut Vec<u8>,
) {
    let Ok(cmd) = Cmd::from_u16(header.command) else {
        encode_error(header.command, b"unknown command", buf);
        return;
    };
    let ctx = render_context(header);
    match cmd {
        Cmd::RenderFile => handle_render_file(buf, ctx, state, file_buf),
        Cmd::PreviewFile => handle_preview_file(buf, header, state, file_buf),
        Cmd::ComposeAnsi | Cmd::ComposeExtmarks | Cmd::RenderBuffer | Cmd::DecorateBuffer => {
            core::mem::swap(buf, file_buf);
            let src = file_buf.as_slice();
            if matches!(cmd, Cmd::ComposeAnsi | Cmd::ComposeExtmarks) {
                render_response(buf, cmd, ctx, state, src, Kind::Md);
            } else if let Some(end) = memchr::memchr(0, src).filter(|&end| end <= 32) {
                let ext = core::str::from_utf8(&src[..end]).unwrap_or("");
                let bytes = &src[end + 1..];
                render_response(buf, cmd, ctx, state, bytes, fmt::detect_kind(bytes, ext));
            } else {
                encode_error(cmd as u16, b"invalid format prefix", buf);
            }
        }
        Cmd::Ping | Cmd::Shutdown => {
            encode_error(header.command, b"unexpected render command", buf);
        }
    }
}

fn dispatch(
    slot: &mut ClientSlot,
    index: usize,
    header: Header,
    worker: &mut Worker,
    shutting_down: &mut bool,
) {
    match Cmd::from_u16(header.command) {
        Ok(cmd @ (Cmd::Ping | Cmd::Shutdown)) => {
            *shutting_down |= cmd == Cmd::Shutdown;
            encode_dual_response(cmd, b"", b"", &mut slot.buf);
        }
        _ if *shutting_down => {
            encode_error(header.command, b"daemon is shutting down", &mut slot.buf);
        }
        _ if worker.submit(slot, index, header) => return,
        _ => encode_error(header.command, b"render queue is full", &mut slot.buf),
    }
    start_reply(slot, header);
}

fn correlate(buf: &mut [u8], header: Header) {
    buf[8..12].copy_from_slice(&header.req_id_lo.to_le_bytes());
    buf[12..16].copy_from_slice(&header.req_id_hi.to_le_bytes());
}

fn start_reply(slot: &mut ClientSlot, header: Header) {
    correlate(&mut slot.buf, header);
    slot.phase = Phase::WriteResp { sent: 0 };
    step_write_resp(slot, 0);
}

fn step_read_header(
    slot: &mut ClientSlot,
    mut got: u8,
    room: usize,
    index: usize,
    worker: &mut Worker,
    shutting_down: &mut bool,
) {
    let Some(fd) = slot.fd.as_ref() else { return };
    match read(fd.as_fd(), &mut slot.hdr[usize::from(got)..]) {
        Ok(0) => slot.reset(),
        Ok(n) => {
            slot.progress();
            got += u8::try_from(n).unwrap_or(32);
            if got < 32 {
                slot.phase = Phase::ReadHeader { got };
                return;
            }
            let hdr = Header::from_le_bytes(&slot.hdr);
            if hdr.validate().is_err() {
                encode_error(hdr.command, b"invalid header", &mut slot.buf);
                start_reply(slot, hdr);
                return;
            }
            slot.buf.clear();
            if hdr.payload_len == 0 {
                dispatch(slot, index, hdr, worker, shutting_down);
            } else if hdr.payload_len as usize > room
                || slot
                    .buf
                    .try_reserve_exact(hdr.payload_len as usize)
                    .is_err()
            {
                encode_error(hdr.command, b"request budget exhausted", &mut slot.buf);
                start_reply(slot, hdr);
            } else {
                slot.phase = Phase::ReadPayload {
                    header: hdr,
                    got: 0,
                };
                step_read_payload(slot, hdr, 0, index, worker, shutting_down);
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
    index: usize,
    worker: &mut Worker,
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
            slot.progress();
            got += u32::try_from(n).unwrap_or(0);
            if got == header.payload_len {
                dispatch(slot, index, header, worker, shutting_down);
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
        let end = sent.saturating_add(READ_CHUNK).min(slot.buf.len());
        #[cfg(target_os = "linux")]
        let progress =
            rustix::net::send(fd, &slot.buf[sent..end], rustix::net::SendFlags::NOSIGNAL);
        #[cfg(not(target_os = "linux"))]
        let progress = rustix::io::write(fd, &slot.buf[sent..end]);
        match progress {
            Ok(0) => slot.reset(),
            Ok(n) => {
                sent += n;
                if sent == slot.buf.len() {
                    slot.buf.clear();
                    slot.reset();
                } else {
                    slot.progress();
                    slot.phase = Phase::WriteResp { sent };
                }
            }
            Err(Errno::INTR) => continue,
            Err(Errno::AGAIN) => slot.phase = Phase::WriteResp { sent },
            Err(_) => slot.reset(),
        }
        break;
    }
}

fn step_client(
    slot: &mut ClientSlot,
    index: usize,
    rev: PollFlags,
    room: usize,
    worker: &mut Worker,
    shutting_down: &mut bool,
) {
    if rev.intersects(PollFlags::ERR | PollFlags::NVAL) {
        worker.cancel(index);
        slot.reset();
        return;
    }
    match slot.phase {
        Phase::ReadHeader { got } if rev.contains(PollFlags::IN) => {
            step_read_header(slot, got, room, index, worker, shutting_down);
        }
        Phase::ReadPayload { header, got } if rev.contains(PollFlags::IN) => {
            step_read_payload(slot, header, got, index, worker, shutting_down);
        }
        Phase::WriteResp { sent } if rev.contains(PollFlags::OUT) => step_write_resp(slot, sent),
        _ => {}
    }
    if slot.fd.is_some() && rev.contains(PollFlags::HUP) && !rev.contains(PollFlags::IN) {
        worker.cancel(index);
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
    let mut worker = Worker::new(true)?;
    let mut shutting_down = false;

    loop {
        let mut slot_map = [0u8; MAX_CLIENTS];
        let mut ready = [PollFlags::empty(); MAX_CLIENTS + 2];
        let mut n_active = 0usize;

        let now = Instant::now();
        for (index, slot) in slots.iter_mut().enumerate() {
            if slot.deadline.is_some_and(|deadline| deadline <= now) {
                worker.cancel(index);
                slot.reset();
            }
        }
        let client_timeout = slots
            .iter()
            .filter_map(|slot| slot.deadline)
            .min()
            .and_then(|deadline| Timespec::try_from(deadline.saturating_duration_since(now)).ok());

        let active_clients = slots.iter().filter(|s| s.fd.is_some()).count();
        if shutting_down && active_clients == 0 {
            break;
        }
        let listen_enabled = !shutting_down && active_clients < MAX_CLIENTS;
        let offset = 1 + usize::from(listen_enabled);

        let poll_res = {
            let mut pfds = core::array::from_fn::<_, { MAX_CLIENTS + 2 }, _>(|_| {
                PollFd::new(&listener, PollFlags::empty())
            });
            pfds[0] = PollFd::new(&worker.wake, PollFlags::IN);
            if listen_enabled {
                pfds[1] = PollFd::new(&listener, PollFlags::IN);
            }
            for (idx, slot) in slots.iter().enumerate() {
                if let Some(ref fd) = slot.fd {
                    let ev = match slot.phase {
                        Phase::ReadHeader { .. } | Phase::ReadPayload { .. } => PollFlags::IN,
                        Phase::Render { .. } => PollFlags::empty(),
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
                client_timeout.as_ref()
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
        for (i, &idx) in slot_map.iter().take(n_active).enumerate() {
            let rev = ready[offset + i];
            if !rev.is_empty() {
                let room = wire_room(&slots, worker.bytes);
                let index = usize::from(idx);
                step_client(
                    &mut slots[index],
                    index,
                    rev,
                    room,
                    &mut worker,
                    &mut shutting_down,
                );
            }
        }
        if ready[0].contains(PollFlags::IN) {
            worker.receive(&mut slots);
        }
        if ready[0].intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL) {
            return Err(Error::InvalidState);
        }
        if listen_enabled && ready[1].contains(PollFlags::IN) {
            accept_pending(listener.as_fd(), &mut slots);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        os::unix::{ffi::OsStrExt, net::UnixStream},
    };

    use super::*;

    #[test]
    fn decoration_reports_svg_error_and_recovers() -> Result<(), core::array::TryFromSliceError> {
        let mut state = DaemonState::new(None);
        let mut reply = Vec::new();
        let mut src = Vec::new();
        for (payload, flag) in [
            (b"<svg><g></svg>".as_slice(), RESP_FLAG | ERR_FLAG),
            (br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16"/></svg>"#.as_slice(), RESP_FLAG),
        ] {
            reply.clear();
            reply.extend_from_slice(b"svg\0");
            reply.extend_from_slice(payload);
            let mut hdr = Header::new(Cmd::DecorateBuffer, 0);
            hdr.flags = ipc::FLAG_REMOTE;
            render_request(&mut reply, hdr, &mut state, &mut src);
            let response = Header::from_le_bytes(reply[..32].try_into()?);
            assert_eq!(response.command, Cmd::DecorateBuffer as u16 | flag);
            if flag & ERR_FLAG != 0 {
                assert_eq!(&reply[32..], b"SVG parse failed");
            } else {
                assert!(response.flags > 0);
            }
        }
        Ok(())
    }

    #[test]
    #[cfg_attr(miri, ignore = "test exercises the Unix socket transport")]
    fn notebook_pages_are_stable_through_the_socket_state_machine()
    -> Result<(), Box<dyn core::error::Error>> {
        let mut worker = Worker::new(false)?;
        let mut page = |skip: u32| -> Result<Vec<u8>, Box<dyn core::error::Error>> {
            let (mut client, server) = UnixStream::pair()?;
            server.set_nonblocking(true)?;
            let mut slots = [const { ClientSlot::empty() }; MAX_CLIENTS];
            slots[0].accept(server.into());
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("showcase/03.ipynb");
            let mut payload = skip.to_le_bytes().to_vec();
            payload.extend_from_slice(path.as_os_str().as_bytes());
            let mut hdr = Header::new(Cmd::PreviewFile, u32::try_from(payload.len())?);
            hdr.req_id_lo = skip;
            hdr.rows = 12;
            client.write_all(&hdr.to_le_bytes())?;
            client.write_all(&payload)?;
            client.set_nonblocking(true)?;
            let mut shutdown = false;
            step_client(
                &mut slots[0],
                0,
                PollFlags::IN,
                WIRE_BYTES,
                &mut worker,
                &mut shutdown,
            );
            assert!(matches!(slots[0].phase, Phase::Render { .. }));

            // A second socket can complete PING before the reactor accepts a
            // single byte of the cold render response.
            let mut samples = [Duration::ZERO; 120];
            for sample in &mut samples {
                let start = Instant::now();
                let (mut ping, peer) = UnixStream::pair()?;
                peer.set_nonblocking(true)?;
                ping.set_read_timeout(Some(Duration::from_secs(1)))?;
                slots[1].accept(peer.into());
                ping.write_all(&Header::new(Cmd::Ping, 0).to_le_bytes())?;
                step_client(
                    &mut slots[1],
                    1,
                    PollFlags::IN,
                    WIRE_BYTES,
                    &mut worker,
                    &mut shutdown,
                );
                let mut response = [0; 32];
                ping.read_exact(&mut response)?;
                assert_eq!(
                    Header::from_le_bytes(&response).command,
                    Cmd::Ping as u16 | RESP_FLAG
                );
                *sample = start.elapsed();
            }
            if skip == 0 {
                samples.sort_unstable();
                writeln!(
                    std::io::stdout(),
                    "IXPC socketpair PING (queued notebook) p50={:?} p95={:?} p99={:?}",
                    samples[59],
                    samples[113],
                    samples[118],
                )?;
            }

            let mut ready = [PollFd::new(&worker.wake, PollFlags::IN)];
            assert_eq!(
                poll(
                    &mut ready,
                    Some(&Timespec {
                        tv_sec: 5,
                        tv_nsec: 0
                    })
                )?,
                1
            );
            worker.receive(&mut slots);

            let mut reply = Vec::new();
            loop {
                match client.read_to_end(&mut reply) {
                    Ok(_) => break,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && matches!(slots[0].phase, Phase::WriteResp { .. }) =>
                    {
                        step_client(
                            &mut slots[0],
                            0,
                            PollFlags::OUT,
                            WIRE_BYTES,
                            &mut worker,
                            &mut shutdown,
                        );
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            let header = Header::from_le_bytes(reply[..32].try_into()?);
            assert_eq!(header.command, Cmd::PreviewFile as u16 | RESP_FLAG);
            assert_eq!(header.req_id_lo, skip);
            assert_eq!(header.payload_len as usize, reply.len() - 32);
            if skip != u32::MAX {
                assert!(header.flags > 0);
                assert!(
                    memchr::memmem::find(
                        &reply[32 + header.flags as usize..],
                        &crate::kitty::PLACEHOLDER_UTF8
                    )
                    .is_some()
                );
            }
            Ok(reply)
        };
        let first = page(0)?;
        assert_ne!(page(5)?[32..], first[32..]);
        assert_eq!(page(0)?, first);
        assert_eq!(page(u32::MAX)?.len(), 32);
        Ok(())
    }
}
