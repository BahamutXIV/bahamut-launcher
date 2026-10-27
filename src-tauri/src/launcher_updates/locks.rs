//! Update and helper process locks.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;
use std::time::Duration;

use bahamut_launcher::atomic_fs::is_link_or_reparse;
use bahamut_launcher::update_helper::HELPER_LOCK_FILE;

use super::UPDATE_HELPER_FILE;
use super::fs_io::ensure_plain_relative_directory;
use super::helper::process_matches_executable;
use super::pending::read_pending;

pub(super) struct UpdateLock(File);

pub(super) fn acquire_helper_lock(
    root: &Path,
) -> Result<bahamut_launcher::atomic_fs::ExclusiveProcessLock, String> {
    ensure_plain_relative_directory(root, Path::new("config"))?;
    let path = root.join(HELPER_LOCK_FILE);
    let lock = bahamut_launcher::atomic_fs::try_lock_exclusive(&path).map_err(|error| {
        if error.kind() == io::ErrorKind::WouldBlock {
            "Another launcher update helper is active. Close this window and retry after the update completes.".to_owned()
        } else {
            format!("Could not acquire launcher update helper lock: {error}")
        }
    })?;
    let details = fs::symlink_metadata(&path)
        .map_err(|error| format!("Could not inspect launcher update helper lock: {error}"))?;
    if !details.is_file() || is_link_or_reparse(&details) {
        return Err("Launcher update helper lock is not a regular file.".into());
    }
    Ok(lock)
}

pub(super) fn acquire_idle_helper_lock(
    root: &Path,
) -> Result<bahamut_launcher::atomic_fs::ExclusiveProcessLock, String> {
    loop {
        let lock = acquire_helper_lock_wait(root)?;
        let pending = read_pending(root)?;
        if let Some(pending) = pending
            && let Some(pid) = pending.helper_pid
        {
            let helper_path = pending.stage.join(UPDATE_HELPER_FILE);
            if process_matches_executable(pid, &helper_path)? {
                drop(lock);
                while process_matches_executable(pid, &helper_path)? {
                    std::thread::sleep(Duration::from_millis(50));
                }
                continue;
            }
        }
        return Ok(lock);
    }
}

fn acquire_helper_lock_wait(
    root: &Path,
) -> Result<bahamut_launcher::atomic_fs::ExclusiveProcessLock, String> {
    ensure_plain_relative_directory(root, Path::new("config"))?;
    let path = root.join(HELPER_LOCK_FILE);
    let lock = bahamut_launcher::atomic_fs::lock_exclusive(&path)
        .map_err(|error| format!("Could not wait for launcher update helper lock: {error}"))?;
    let details = fs::symlink_metadata(&path)
        .map_err(|error| format!("Could not inspect launcher update helper lock: {error}"))?;
    if !details.is_file() || is_link_or_reparse(&details) {
        return Err("Launcher update helper lock is not a regular file.".into());
    }
    Ok(lock)
}

impl UpdateLock {
    pub(super) fn acquire(root: &Path) -> Result<Self, String> {
        ensure_plain_relative_directory(root, Path::new("config"))?;
        let path = root.join("config/launcher-update.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("Could not open launcher update lock: {error}"))?;
        let details = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if !details.is_file() || is_link_or_reparse(&details) {
            return Err("Launcher update lock is not a regular file.".into());
        }
        file.lock()
            .map_err(|error| format!("Could not acquire launcher update lock: {error}"))?;
        Ok(Self(file))
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
