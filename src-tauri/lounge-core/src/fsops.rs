//! File operations whose POSIX and Windows semantics differ.

use std::io;
use std::path::Path;

/// Rename `from` to `to`, replacing `to` when it already exists.
///
/// `std::fs::rename` replaces an existing destination on POSIX but fails on Windows. Node's
/// `fs.rename` — which `store.js`, `transfers.js` and `updater.js` all relied on — passes
/// `MOVEFILE_REPLACE_EXISTING`, so the Electron app replaced on every platform; this keeps
/// that behaviour.
#[cfg(windows)]
pub(crate) fn rename_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING};

    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(Some(0)).collect() };
    let (from_w, to_w) = (wide(from), wide(to));
    let ok = unsafe {
        MoveFileExW(from_w.as_ptr(), to_w.as_ptr(), MOVEFILE_REPLACE_EXISTING)
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub(crate) fn rename_replace(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_replaces_an_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        let to = dir.path().join("to");
        std::fs::write(&from, "new").unwrap();
        std::fs::write(&to, "old").unwrap();
        rename_replace(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "new");
        assert!(!from.exists());
    }
}
