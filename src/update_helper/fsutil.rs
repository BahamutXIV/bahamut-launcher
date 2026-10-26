//! Path-safety, locking and atomic file-write helpers.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use super::{HELPER_LOCK_FILE, Result, UpdateError, invalid};

pub(super) static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty() || relative.contains('\\') || relative.contains(':') {
        return Err(invalid("Signed inventory path is unsafe."));
    }
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid("Signed inventory path is unsafe."));
    }
    Ok(root.join(path))
}

pub(super) fn ensure_safe_parent(root: &Path, destination: &Path) -> Result<()> {
    let relative = destination
        .strip_prefix(root)
        .map_err(|_| invalid("Managed destination escaped the installation directory."))?;
    let parent = relative
        .parent()
        .ok_or_else(|| invalid("Managed destination has no parent directory."))?;
    let mut current = root.to_path_buf();
    for component in parent.components() {
        let Component::Normal(part) = component else {
            return Err(invalid("Managed destination has an unsafe parent."));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(details) => {
                if !details.is_dir() || crate::atomic_fs::is_link_or_reparse(&details) {
                    return Err(invalid("Managed destination has a reparse parent."));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&current)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(super) fn ensure_safe_inventory_path(root: &Path, relative: &str, create: bool) -> Result<()> {
    let path = safe_join(root, relative)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Managed path has no parent."))?;
    let relative_parent = parent
        .strip_prefix(root)
        .map_err(|_| invalid("Managed path escaped its root."))?;
    let mut current = root.to_path_buf();
    for component in relative_parent.components() {
        let Component::Normal(part) = component else {
            return Err(invalid("Managed path has an unsafe parent."));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(details) => {
                if !details.is_dir() || crate::atomic_fs::is_link_or_reparse(&details) {
                    return Err(invalid("Managed path has a reparse parent."));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
                fs::create_dir(&current)?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(super) fn acquire_transaction_lock(
    target: &Path,
) -> Result<crate::atomic_fs::ExclusiveProcessLock> {
    ensure_safe_inventory_path(target, HELPER_LOCK_FILE, true)?;
    let path = target.join(HELPER_LOCK_FILE);
    let lock = crate::atomic_fs::lock_exclusive(&path)?;
    let details = fs::symlink_metadata(&path)?;
    if !details.is_file() || crate::atomic_fs::is_link_or_reparse(&details) {
        return Err(invalid(
            "Launcher update helper lock is not a regular file.",
        ));
    }
    Ok(lock)
}

pub(super) fn same_path(left: &Path, right: &Path) -> Result<bool> {
    let resolve = |path: &Path| -> Result<PathBuf> {
        match fs::canonicalize(path) {
            Ok(path) => Ok(path),
            Err(error) if error.kind() == io::ErrorKind::NotFound && path.is_absolute() => {
                Ok(path.to_path_buf())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Ok(std::env::current_dir()?.join(path))
            }
            Err(error) => Err(error.into()),
        }
    };
    let left = resolve(left)?;
    let right = resolve(right)?;
    #[cfg(windows)]
    {
        Ok(left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy()))
    }
    #[cfg(not(windows))]
    {
        Ok(left == right)
    }
}

pub(super) fn read_regular_bounded(path: &Path, max: usize, label: &str) -> Result<Vec<u8>> {
    let details = fs::symlink_metadata(path)?;
    if !details.is_file()
        || crate::atomic_fs::is_link_or_reparse(&details)
        || details.len() > max as u64
    {
        return Err(invalid(&format!("{label} is not a bounded regular file.")));
    }
    let mut bytes = Vec::with_capacity(details.len() as usize);
    File::open(path)?
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(invalid(&format!("{label} exceeds its size limit.")));
    }
    Ok(bytes)
}

pub(super) fn checked_existing_directory(path: &Path, label: &str) -> Result<PathBuf> {
    reject_reparse(path)?;
    let details = fs::metadata(path).map_err(|_| invalid(&format!("Could not read {label}.")))?;
    if !details.is_dir() {
        return Err(invalid(&format!("{label} is not a directory.")));
    }
    fs::canonicalize(path).map_err(UpdateError::from)
}

pub(super) fn canonicalize_target(path: &Path) -> Result<PathBuf> {
    checked_existing_directory(path, "installation directory")
}

pub(super) fn reject_reparse(path: &Path) -> Result<()> {
    let details =
        fs::symlink_metadata(path).map_err(|_| invalid("Could not inspect update path."))?;
    if crate::atomic_fs::is_link_or_reparse(&details) {
        return Err(invalid("Update path is a symbolic link or reparse point."));
    }
    Ok(())
}

pub(super) fn ensure_outside_target(stage: &Path, target: &Path) -> Result<()> {
    if stage == target || stage.starts_with(target) {
        return Err(invalid(
            "Update staging must be outside the installation tree.",
        ));
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn ensure_same_volume(stage: &Path, target: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };
    use windows::core::PCWSTR;

    fn volume(path: &Path) -> Result<String> {
        let wide = path
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>();
        let mut mount = vec![0_u16; 32768];
        unsafe { GetVolumePathNameW(PCWSTR(wide.as_ptr()), &mut mount) }
            .map_err(|error| invalid(&format!("Could not identify update volume: {error}")))?;
        let mut name = vec![0_u16; 128];
        unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(mount.as_ptr()), &mut name) }
            .map_err(|error| invalid(&format!("Could not identify update volume: {error}")))?;
        let end = name
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(name.len());
        Ok(String::from_utf16_lossy(&name[..end]))
    }
    if volume(stage)? != volume(target)? {
        return Err(invalid(
            "Update staging must be on the installation volume.",
        ));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn ensure_same_volume(stage: &Path, target: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if fs::metadata(stage)?.dev() != fs::metadata(target)?.dev() {
        return Err(invalid(
            "Update staging must be on the installation volume.",
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(super) fn ensure_same_volume(stage: &Path, target: &Path) -> Result<()> {
    let stage_prefix = stage.components().next();
    let target_prefix = target.components().next();
    if stage_prefix != target_prefix {
        return Err(invalid(
            "Update staging must be on the installation volume.",
        ));
    }
    Ok(())
}

pub(super) fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Transaction journal has no parent."))?;
    let temporary = unique_path(parent, ".bahamut-journal-");
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    crate::atomic_fs::rename_replace(&temporary, path)?;
    crate::atomic_fs::sync_directory(parent)?;
    Ok(())
}

pub(super) fn unique_path(parent: &Path, prefix: &str) -> PathBuf {
    let serial = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!("{prefix}{}-{serial}", std::process::id()))
}

pub(super) fn move_file_without_replace(temporary: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
        let temporary_wide = temporary
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>();
        let destination_wide = destination
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>();
        unsafe {
            MoveFileExW(
                windows::core::PCWSTR(temporary_wide.as_ptr()),
                windows::core::PCWSTR(destination_wide.as_ptr()),
                MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(io::Error::other)
    }
    #[cfg(unix)]
    {
        fs::hard_link(temporary, destination)?;
        fs::remove_file(temporary)
    }
    #[cfg(not(any(unix, windows)))]
    {
        if destination.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "destination exists",
            ));
        }
        fs::rename(temporary, destination)
    }
}
