//! Rollback-capable replacement of live data by staged units.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use crate::atomic_fs::is_link_or_reparse;

use super::{BackupError, TRANSACTION_COUNTER, io_error};

pub(super) fn unique_transaction_root(parent: &Path) -> PathBuf {
    loop {
        let counter = TRANSACTION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".bahamut-restore-{}-{counter}", std::process::id()));
        if !candidate.exists() {
            return candidate;
        }
    }
}

pub(super) struct AppliedTransaction {
    live_root: PathBuf,
    transaction_root: PathBuf,
    rollback_root: PathBuf,
    moved_live: Vec<PathBuf>,
    installed: Vec<PathBuf>,
}

impl AppliedTransaction {
    pub(super) fn rollback(&self) -> Result<(), BackupError> {
        for relative in self.installed.iter().rev() {
            remove_path(&self.live_root.join(relative))?;
        }
        for relative in self.moved_live.iter().rev() {
            let source = self.rollback_root.join(relative);
            let destination = self.live_root.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    io_error("recreating a rollback destination", parent, error)
                })?;
            }
            fs::rename(&source, &destination).map_err(|error| {
                io_error(
                    "restoring original files after restore failure",
                    &destination,
                    error,
                )
            })?;
        }
        if let Err(error) = remove_path(&self.transaction_root) {
            tracing::warn!(
                path = %self.transaction_root.display(),
                error = %error,
                "restore rollback completed but transaction cleanup failed"
            );
        }
        Ok(())
    }

    pub(super) fn commit(self) {
        if let Err(error) = remove_path(&self.transaction_root) {
            tracing::warn!(
                path = %self.transaction_root.display(),
                error = %error,
                "restore completed but transaction cleanup failed"
            );
        }
    }
}

pub(super) fn apply_staged_units(
    live_root: &Path,
    staging_root: &Path,
    rollback_root: &Path,
    units: &[PathBuf],
) -> Result<AppliedTransaction, BackupError> {
    fs::create_dir_all(live_root)
        .map_err(|error| io_error("creating the restore destination", live_root, error))?;
    fs::create_dir_all(rollback_root)
        .map_err(|error| io_error("creating restore rollback", rollback_root, error))?;
    let transaction_root = rollback_root
        .parent()
        .expect("rollback always has a transaction parent")
        .to_path_buf();
    let mut transaction = AppliedTransaction {
        live_root: live_root.to_path_buf(),
        transaction_root,
        rollback_root: rollback_root.to_path_buf(),
        moved_live: Vec::new(),
        installed: Vec::new(),
    };

    let result = (|| {
        for relative in units {
            let live = live_root.join(relative);
            if !live.exists() {
                continue;
            }
            let rollback = rollback_root.join(relative);
            if let Some(parent) = rollback.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| io_error("creating a rollback parent", parent, error))?;
            }
            fs::rename(&live, &rollback)
                .map_err(|error| io_error("moving live data into rollback", &live, error))?;
            transaction.moved_live.push(relative.clone());
        }
        for relative in units {
            let staged = staging_root.join(relative);
            if !staged.exists() {
                continue;
            }
            let live = live_root.join(relative);
            if let Some(parent) = live.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    io_error("creating a restore destination parent", parent, error)
                })?;
            }
            fs::rename(&staged, &live)
                .map_err(|error| io_error("installing staged backup data", &live, error))?;
            transaction.installed.push(relative.clone());
        }
        Ok(())
    })();

    if let Err(primary) = result {
        return match transaction.rollback() {
            Ok(()) => Err(primary),
            Err(rollback_error) => Err(BackupError::RollbackFailed {
                primary: primary.to_string(),
                rollback: rollback_root.to_path_buf(),
                rollback_error: rollback_error.to_string(),
            }),
        };
    }
    Ok(transaction)
}

pub(super) fn remove_path(path: &Path) -> Result<(), BackupError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error("reading restore cleanup metadata", path, error)),
    };
    if metadata.is_dir() && !is_link_or_reparse(&metadata) {
        fs::remove_dir_all(path)
            .map_err(|error| io_error("removing restore work data", path, error))
    } else {
        fs::remove_file(path).map_err(|error| io_error("removing restore work data", path, error))
    }
}
