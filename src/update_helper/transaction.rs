//! Transaction directory and journal lifecycle.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use super::fsutil::{TEMP_COUNTER, reject_reparse, write_json_atomic};
use super::{JOURNAL_FILE, Journal, Result, TransactionMode, invalid};

pub(super) fn create_transaction_directory(
    stage: &Path,
    metadata_sha256: &str,
    mode: TransactionMode,
    requested: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(transaction) = requested {
        let expected_name = format!("{}-{metadata_sha256}", mode.transaction_name());
        if !transaction.is_absolute()
            || transaction.parent() != Some(stage)
            || transaction.file_name().and_then(|name| name.to_str()) != Some(&expected_name)
        {
            return Err(invalid(
                "Requested transaction path does not match the external stage and signed release.",
            ));
        }
        fs::create_dir(transaction).map_err(|error| {
            invalid(&format!(
                "Could not create launcher transaction directory: {error}"
            ))
        })?;
        return Ok(transaction.to_path_buf());
    }
    for _ in 0..128 {
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let transaction = stage.join(format!(
            "{}-{metadata_sha256}-{}-{sequence}",
            mode.transaction_name(),
            std::process::id()
        ));
        match fs::create_dir(&transaction) {
            Ok(()) => return Ok(transaction),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(invalid(
        "Could not allocate a unique update transaction directory.",
    ))
}

pub(super) fn write_journal(transaction: &Path, journal: &Journal) -> Result<()> {
    write_json_atomic(&transaction.join(JOURNAL_FILE), journal)
}

pub(super) fn remove_transaction(transaction: &Path) -> Result<()> {
    reject_reparse(transaction)?;
    remove_entry_safely(transaction)?;
    Ok(())
}

fn remove_entry_safely(path: &Path) -> Result<()> {
    let mut pending = vec![(path.to_path_buf(), false)];
    while let Some((current, visited)) = pending.pop() {
        let details = fs::symlink_metadata(&current)?;
        if crate::atomic_fs::is_link_or_reparse(&details) {
            if details.is_dir() {
                fs::remove_dir(current)?;
            } else {
                fs::remove_file(current)?;
            }
        } else if details.is_dir() && !visited {
            pending.push((current.clone(), true));
            for entry in fs::read_dir(&current)? {
                pending.push((entry?.path(), false));
            }
        } else if details.is_dir() {
            fs::remove_dir(current)?;
        } else {
            fs::remove_file(current)?;
        }
    }
    Ok(())
}
