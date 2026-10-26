//! Backup archive naming, ordering, lookup, and retention.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::paths::read_sorted_dir;
use super::{BackupError, BackupTarget, io_error};

pub(super) const BACKUP_KEEP: usize = 5;

pub(super) fn unique_archive_path(
    target: BackupTarget,
    root: &Path,
    timestamp: &str,
) -> Result<PathBuf, BackupError> {
    let base = format!("{}_{}", target.file_prefix(), timestamp);
    let mut highest = 0_u32;
    for entry in read_sorted_dir(root)? {
        let name = entry.file_name();
        let Some(tail) = name
            .to_str()
            .and_then(|name| name.strip_prefix(&base))
            .and_then(|name| name.strip_suffix(".zip"))
        else {
            continue;
        };
        let sequence = if tail.is_empty() {
            Some(1)
        } else {
            tail.strip_prefix('-')
                .and_then(|value| value.parse::<u32>().ok())
        };
        highest = highest.max(sequence.unwrap_or(0));
    }
    // Do not reuse a lower sequence removed by retention at the same timestamp.
    let mut sequence = highest.checked_add(1).ok_or_else(|| {
        io_error(
            "allocating a backup name",
            root,
            io::Error::other("backup sequence exhausted"),
        )
    })?;
    loop {
        let name = if sequence == 1 {
            format!("{base}.zip")
        } else {
            format!("{base}-{sequence}.zip")
        };
        let candidate = root.join(name);
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(candidate),
            Err(error) => return Err(io_error("checking a backup name", &candidate, error)),
            Ok(_) => {
                // Include dangling links and case aliases on case-insensitive filesystems.
                sequence = sequence.checked_add(1).ok_or_else(|| {
                    io_error(
                        "allocating a backup name",
                        root,
                        io::Error::other("backup sequence exhausted"),
                    )
                })?;
            }
        }
    }
}

fn backup_order(path: &Path) -> (String, u32) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let stem = name.strip_suffix(".zip").unwrap_or(name);
    if let Some((base, suffix)) = stem.rsplit_once('-')
        && let Some((_, timestamp)) = base.split_once('_')
        && timestamp.len() == 19
        && timestamp.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 15) {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
        && let Ok(sequence) = suffix.parse::<u32>()
        && sequence >= 2
    {
        return (base.to_owned(), sequence);
    }
    (stem.to_owned(), 1)
}

pub(super) fn latest_backup(
    target: BackupTarget,
    backups_root: &Path,
) -> Result<Option<PathBuf>, BackupError> {
    let listing = match fs::read_dir(backups_root) {
        Ok(listing) => listing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(io_error("reading the backup folder", backups_root, error));
        }
    };
    let prefix = format!("{}_", target.file_prefix());
    let mut paths = listing
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".zip"))
        })
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| std::cmp::Reverse(backup_order(path)));
    Ok(paths.into_iter().next())
}

pub(super) fn prune_backups(target: BackupTarget, backups_root: &Path) -> Result<(), BackupError> {
    let prefix = format!("{}_", target.file_prefix());
    let mut backups = read_sorted_dir(backups_root)?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".zip"))
        })
        .collect::<Vec<_>>();
    backups.sort_by_key(|path| std::cmp::Reverse(backup_order(path)));
    for stale in backups.into_iter().skip(BACKUP_KEEP) {
        fs::remove_file(&stale)
            .map_err(|error| io_error("pruning an old backup", &stale, error))?;
    }
    Ok(())
}
