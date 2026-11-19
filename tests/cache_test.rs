use ii::cache::{blake3_key, read_blob, write_blob};
use rustix::{
    fd::AsFd,
    fs::{AtFlags, CWD, Mode, OFlags, mkdirat, openat, unlinkat},
};

#[test]
fn cache_px02_atomic_write_and_read_roundtrip() {
    let dir_name = format!(
        "/tmp/ii-test-cache-{}",
        rustix::process::getpid().as_raw_nonzero().get()
    );
    let _ = mkdirat(CWD, dir_name.as_str(), Mode::RWXU);
    let dir_fd = openat(
        CWD,
        dir_name.as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    );
    assert!(dir_fd.is_ok());
    let Ok(dir_fd) = dir_fd else { return };

    let key = blake3_key(8, 16, 640, true, b"test-svg-payload");
    let src_rgba = vec![0x5Au8; 16 * 16 * 4];
    let mut lz4 = Vec::new();
    let mut dst_rgba = Vec::new();

    assert!(write_blob(dir_fd.as_fd(), &key, 16, 16, &src_rgba, &mut lz4).is_ok());
    let loaded = read_blob(dir_fd.as_fd(), &key, &mut dst_rgba, &mut lz4);
    assert_eq!(loaded, Ok(Some((16, 16))));
    assert_eq!(dst_rgba, src_rgba);

    let mut hex_name = [0u8; 64];
    ii::cache::hex_encode(&key, &mut hex_name);
    if let Ok(name) = core::str::from_utf8(&hex_name) {
        let _ = unlinkat(dir_fd.as_fd(), name, AtFlags::empty());
    }
    drop(dir_fd);
    let _ = unlinkat(CWD, dir_name.as_str(), AtFlags::REMOVEDIR);
}
