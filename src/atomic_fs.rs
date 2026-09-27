//! Filesystem operations with explicit no-replace semantics.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// A process-scoped exclusive lock on a stable lock file.
///
/// Keep the lock file in place after releasing this value. Removing it can let
/// concurrent processes lock different inodes for the same logical lock.
#[derive(Debug)]
pub struct ExclusiveProcessLock {
    _file: File,
}

/// Try to acquire an exclusive lock without waiting for another process.
pub fn try_lock_exclusive(path: &Path) -> io::Result<ExclusiveProcessLock> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
        let file = options.open(path).map_err(map_windows_lock_error)?;
        Ok(ExclusiveProcessLock { _file: file })
    }

    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let file = options.open(path)?;
        // SAFETY: `file` owns a valid descriptor for the duration of this call.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            Ok(ExclusiveProcessLock { _file: file })
        } else {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EWOULDBLOCK)
                || error.raw_os_error() == Some(libc::EAGAIN)
            {
                Err(io::Error::new(io::ErrorKind::WouldBlock, error))
            } else {
                Err(error)
            }
        }
    }

    #[cfg(not(any(windows, unix)))]
    {
        let _ = (options, path);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "exclusive process locks are unsupported on this platform",
        ))
    }
}

/// Acquire an exclusive lock, waiting until the current owner releases it.
pub fn lock_exclusive(path: &Path) -> io::Result<ExclusiveProcessLock> {
    loop {
        match try_lock_exclusive(path) {
            Ok(lock) => return Ok(lock),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(windows)]
fn map_windows_lock_error(error: io::Error) -> io::Error {
    if matches!(error.raw_os_error(), Some(32) | Some(33)) {
        io::Error::new(io::ErrorKind::WouldBlock, error)
    } else {
        error
    }
}

/// Move a sibling staging file into place without replacing an existing path.
/// The operation is atomic on supported local filesystems and removes `source`
/// only when publication succeeds.
pub fn rename_noreplace(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::MoveFileW;
        use windows::core::PCWSTR;

        let source = source
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let destination = destination
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        unsafe { MoveFileW(PCWSTR(source.as_ptr()), PCWSTR(destination.as_ptr())) }.map_err(
            |error| {
                let code = (error.code().0 as u32 & 0xffff) as i32;
                if matches!(code, 80 | 183) {
                    io::Error::new(io::ErrorKind::AlreadyExists, error)
                } else {
                    io::Error::from_raw_os_error(code)
                }
            },
        )
    }

    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let source = CString::new(source.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
        let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "destination path contains NUL")
        })?;
        // SAFETY: both C strings are NUL-terminated and remain alive for the call.
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                1,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        unsafe extern "C" {
            fn renamex_np(
                source: *const libc::c_char,
                destination: *const libc::c_char,
                flags: u32,
            ) -> libc::c_int;
        }
        const RENAME_EXCL: u32 = 0x0000_0004;
        let source = CString::new(source.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
        let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "destination path contains NUL")
        })?;
        // SAFETY: both C strings are NUL-terminated and remain alive for the call.
        let result = unsafe { renamex_np(source.as_ptr(), destination.as_ptr(), RENAME_EXCL) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (source, destination);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic no-replace rename is unsupported on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn rename_noreplace_moves_without_replacing_an_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::write(&source, b"source").unwrap();
        fs::write(&destination, b"destination").unwrap();

        let error = rename_noreplace(&source, &destination).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(&destination).unwrap(), b"destination");
    }

    #[test]
    fn rename_noreplace_moves_when_the_destination_is_absent() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::write(&source, b"source").unwrap();

        rename_noreplace(&source, &destination).unwrap();

        assert!(!source.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"source");
    }

    #[test]
    fn exclusive_lock_blocks_another_process_lock_until_released() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("process.lock");
        let lock = try_lock_exclusive(&path).unwrap();

        assert_eq!(
            try_lock_exclusive(&path).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );

        drop(lock);
        assert!(try_lock_exclusive(&path).is_ok());
    }
}
