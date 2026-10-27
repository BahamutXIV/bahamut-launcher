//! The install-relative transaction pointer and launcher handoff receipt.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::fsutil::{
    checked_existing_directory, ensure_outside_target, ensure_safe_inventory_path,
    ensure_same_volume, move_file_without_replace, read_regular_bounded, same_path, unique_path,
};
use super::{
    JOURNAL_SCHEMA, Journal, PENDING_FILE, POINTER_FILE, Result, TransactionMode, invalid,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TransactionPointer {
    schema_version: u32,
    transaction: PathBuf,
    new_metadata_sha256: String,
    old_metadata_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LauncherHandoffReceipt {
    schema_version: u32,
    target: PathBuf,
    stage: PathBuf,
    transaction: PathBuf,
    operation: TransactionMode,
    helper_pid: Option<u32>,
    previous_metadata_sha256: String,
    new_metadata_sha256: String,
}

pub(super) fn validate_launcher_handoff(
    target: &Path,
    stage: &Path,
    transaction: &Path,
    mode: TransactionMode,
    new_metadata_sha256: &str,
    old_metadata_sha256: &str,
) -> Result<()> {
    ensure_safe_inventory_path(target, PENDING_FILE, false)?;
    let bytes = read_regular_bounded(
        &target.join(PENDING_FILE),
        32 * 1024,
        "launcher update handoff receipt",
    )?;
    let receipt: LauncherHandoffReceipt = serde_json::from_slice(&bytes)?;
    let expected_transaction =
        stage.join(format!("{}-{new_metadata_sha256}", mode.transaction_name()));
    if receipt.schema_version != 1
        || receipt.helper_pid != Some(std::process::id())
        || receipt.operation != mode
        || !crate::digest::is_lower_sha256_hex(&receipt.previous_metadata_sha256)
        || !crate::digest::is_lower_sha256_hex(&receipt.new_metadata_sha256)
        || receipt.previous_metadata_sha256 != old_metadata_sha256
        || receipt.new_metadata_sha256 != new_metadata_sha256
        || !same_path(&receipt.target, target)?
        || !same_path(&receipt.stage, stage)?
        || !same_path(&receipt.transaction, &expected_transaction)?
        || !same_path(transaction, &expected_transaction)?
    {
        return Err(invalid(
            "Launcher helper handoff does not match its persisted signed receipt.",
        ));
    }
    Ok(())
}

fn read_pointer(target: &Path) -> Result<Option<TransactionPointer>> {
    let pointer = target.join(POINTER_FILE);
    let config = pointer
        .parent()
        .ok_or_else(|| invalid("Update pointer path has no parent."))?;
    match fs::symlink_metadata(config) {
        Ok(details) => {
            if !details.is_dir() || crate::atomic_fs::is_link_or_reparse(&details) {
                return Err(invalid(
                    "Update pointer directory is not a regular directory.",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let details = match fs::symlink_metadata(&pointer) {
        Ok(details) => details,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !details.is_file()
        || crate::atomic_fs::is_link_or_reparse(&details)
        || details.len() > 16 * 1024
    {
        return Err(invalid("Update pointer is not a bounded regular file."));
    }
    let mut bytes = Vec::with_capacity(details.len() as usize);
    File::open(pointer)?
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 {
        return Err(invalid("Update pointer exceeds its size limit."));
    }
    let pointer: TransactionPointer = serde_json::from_slice(&bytes)?;
    if pointer.schema_version != JOURNAL_SCHEMA
        || !pointer.transaction.is_absolute()
        || !crate::digest::is_lower_sha256_hex(&pointer.new_metadata_sha256)
        || !crate::digest::is_lower_sha256_hex(&pointer.old_metadata_sha256)
    {
        return Err(invalid("Update pointer fields are invalid."));
    }
    Ok(Some(pointer))
}

pub(super) fn publish_pointer(
    target: &Path,
    transaction: &Path,
    new_metadata_sha256: &str,
    old_metadata_sha256: &str,
) -> Result<()> {
    ensure_safe_inventory_path(target, POINTER_FILE, true)?;
    let path = target.join(POINTER_FILE);
    let transaction = fs::canonicalize(transaction)?;
    let pointer = TransactionPointer {
        schema_version: JOURNAL_SCHEMA,
        transaction: transaction.clone(),
        new_metadata_sha256: new_metadata_sha256.to_owned(),
        old_metadata_sha256: old_metadata_sha256.to_owned(),
    };
    if let Some(existing) = read_pointer(target)? {
        if existing.transaction == transaction
            && existing.new_metadata_sha256 == pointer.new_metadata_sha256
            && existing.old_metadata_sha256 == pointer.old_metadata_sha256
        {
            return Ok(());
        }
        return Err(invalid("Update pointer is already owned by another state."));
    }
    let bytes = serde_json::to_vec(&pointer)?;
    let temporary = unique_path(transaction.parent().unwrap_or(target), ".bahamut-pointer-");
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    let result = move_file_without_replace(&temporary, &path);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    crate::atomic_fs::sync_directory(path.parent().expect("pointer has parent"))?;
    Ok(())
}

pub(super) fn resolve_transaction(
    target: &Path,
    explicit: Option<PathBuf>,
) -> Result<Option<PathBuf>> {
    let path = if let Some(path) = explicit {
        path
    } else {
        let Some(pointer) = read_pointer(target)? else {
            return Ok(None);
        };
        pointer.transaction
    };
    if !path.exists() {
        return Err(invalid(
            "Update transaction is missing; preserving the portable pointer for manual recovery.",
        ));
    }
    let transaction = checked_existing_directory(&path, "transaction directory")?;
    ensure_outside_target(&transaction, target)?;
    ensure_same_volume(&transaction, target)?;
    Ok(Some(transaction))
}

pub(super) fn validate_pointer_identity(
    target: &Path,
    transaction: &Path,
    journal: &Journal,
) -> Result<()> {
    let Some(pointer) = read_pointer(target)? else {
        return Ok(());
    };
    if !same_path(&pointer.transaction, transaction)?
        || pointer.new_metadata_sha256 != journal.new_metadata_sha256
        || pointer.old_metadata_sha256 != journal.old_metadata_sha256
    {
        return Err(invalid(
            "Update pointer does not match its signed transaction.",
        ));
    }
    Ok(())
}

pub(super) fn ensure_no_pending_pointer(target: &Path) -> Result<()> {
    if read_pointer(target)?.is_some() {
        return Err(invalid(
            "An update transaction is already pending; confirm or recover it first.",
        ));
    }
    Ok(())
}

pub(super) fn clear_pointer(
    target: &Path,
    transaction: &Path,
    new_metadata_sha256: &str,
    old_metadata_sha256: &str,
) -> Result<()> {
    let Some(pointer) = read_pointer(target)? else {
        return Ok(());
    };
    if pointer.transaction != transaction {
        return Ok(());
    }
    if pointer.new_metadata_sha256 != new_metadata_sha256
        || pointer.old_metadata_sha256 != old_metadata_sha256
    {
        return Err(invalid("Refusing to remove a changed update pointer."));
    }
    fs::remove_file(target.join(POINTER_FILE))?;
    crate::atomic_fs::sync_directory(
        target
            .join(POINTER_FILE)
            .parent()
            .expect("pointer has parent"),
    )?;
    Ok(())
}
