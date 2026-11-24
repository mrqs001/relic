//! Free-space preflight for large reference imports. Relic does not cap how much
//! geography an operator selects; it refuses only work this disk cannot hold,
//! keeping a reserve so the live workspace is never starved.
use crate::error::{Error, Result};
use std::path::Path;

/// Space left untouched for the running workspace (database, objects, backups).
pub const RESERVE: u64 = 5 * 1024 * 1024 * 1024;

/// Bytes available to this process on the filesystem holding `path`.
#[cfg(unix)]
pub fn available(path: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let mut probe = path;
    while !probe.exists() {
        probe = probe.parent().ok_or_else(|| Error::invalid("No existing directory to measure free space"))?;
    }
    let c = std::ffi::CString::new(probe.as_os_str().as_bytes()).map_err(|_| Error::invalid("Invalid path"))?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `stat` is a writable struct.
    if unsafe { libc::statvfs(c.as_ptr(), &mut stat) } != 0 {
        return Err(Error::invalid("Cannot measure free disk space"));
    }
    Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}
#[cfg(not(unix))]
pub fn available(_path: &Path) -> Result<u64> {
    Ok(u64::MAX)
}
fn gb(n: u64) -> String {
    format!("{:.1} GB", n as f64 / 1e9)
}
/// Fail before starting work that needs about `needed` bytes on `path`'s disk.
pub fn ensure(path: &Path, needed: u64, what: &str) -> Result<()> {
    let free = available(path)?;
    if free < needed.saturating_add(RESERVE) {
        return Err(Error::invalid(format!(
            "{what} needs about {} free on this disk (plus a {} reserve for the workspace); {} is available",
            gb(needed),
            gb(RESERVE),
            gb(free)
        )));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn measures_the_nearest_existing_directory_and_refuses_impossible_work() {
        let dir = tempfile::tempdir().unwrap();
        let free = available(&dir.path().join("not/yet/created")).unwrap();
        assert!(free > 0);
        assert!(ensure(dir.path(), 0, "Test").is_ok() || free < RESERVE);
        let e = ensure(dir.path(), u64::MAX / 2, "Test import").unwrap_err();
        assert!(e.message.starts_with("Test import needs about"));
    }
}
