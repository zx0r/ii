//! Bounded input from ordinary files.

use std::path::Path;

use rustix::{
    fs::{CWD, FileType, Mode, OFlags, fstat, openat},
    io::{Errno, read},
};

/// Replace `out` with at most `limit` bytes of a regular file.
///
/// # Errors
/// Returns a filesystem error, `INVAL` for a special file, or `FBIG` above the limit.
pub fn read_bounded(path: &Path, limit: usize, out: &mut Vec<u8>) -> Result<(), Errno> {
    out.clear();
    let fd = openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    let st = fstat(&fd)?;
    if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
        return Err(Errno::INVAL);
    }
    let size = usize::try_from(st.st_size).map_err(|_| Errno::FBIG)?;
    if size > limit {
        return Err(Errno::FBIG);
    }
    out.resize(size, 0);
    let mut pos = 0;
    while pos < size {
        match read(&fd, &mut out[pos..]) {
            Ok(0) => break,
            Ok(n) => pos += n,
            Err(Errno::INTR) => {}
            Err(e) => {
                out.clear();
                return Err(e);
            }
        }
    }
    out.truncate(pos);
    Ok(())
}
