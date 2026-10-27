//! Roll an interrupted transaction back to the previous release.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::release::InventoryFile;

use super::fsutil::{ensure_safe_inventory_path, read_regular_bounded, reject_reparse, safe_join};
use super::inventory::{
    ensure_file_matches, inspect_file, managed_files, same_bytes, same_file_identity,
    validate_old_managed_files,
};
use super::overlay::{restore_legacy_overlay_identity, transition_old_files};
use super::payload::install_file_atomically;
use super::snapshot::repair_snapshot_identity_map;
use super::verify::load_transaction;
use super::{
    BACKUP_DIR, JOURNAL_SCHEMA, MAX_SNAPSHOT_BYTES, Phase, Result, SNAPSHOT_FILE, Snapshot,
    TransactionMode, invalid,
};

pub(super) fn recover_transaction(
    target: &Path,
    transaction: &Path,
    public_key: &[u8],
) -> Result<()> {
    let (journal, new_release, old_release) = load_transaction(target, transaction, public_key)?;
    match journal.phase {
        Phase::Prepared | Phase::Committed => return Ok(()),
        Phase::Applying | Phase::AwaitingValidation => {}
    }
    let snapshot = read_snapshot(transaction)?;
    let mut old_files = if journal.mode == TransactionMode::Update {
        transition_old_files(&old_release.metadata, &new_release.metadata)?
    } else {
        managed_files(&old_release.metadata)?
    };
    let new_files = managed_files(&new_release.metadata)?;
    if journal.mode == TransactionMode::Update {
        restore_legacy_overlay_identity(&snapshot, &mut old_files, &new_files)?;
    }
    let present = snapshot_present_paths(&snapshot, &old_files)?;
    if journal.mode == TransactionMode::Repair {
        return recover_repair_transaction(target, transaction, &old_files, &new_files, &snapshot);
    }
    verify_update_backups(transaction, &old_files, &present)?;
    roll_back_new_files(target, transaction, &old_files, &new_files, &present)?;
    restore_obsolete_files(target, transaction, &old_files, &new_files, &present)?;
    validate_old_managed_files(target, &old_files, &present)?;
    Ok(())
}

fn read_snapshot(transaction: &Path) -> Result<Snapshot> {
    let snapshot_path = transaction.join(SNAPSHOT_FILE);
    let snapshot_bytes =
        read_regular_bounded(&snapshot_path, MAX_SNAPSHOT_BYTES, "transaction snapshot")?;
    let snapshot: Snapshot = serde_json::from_slice(&snapshot_bytes)?;
    if snapshot.schema_version != JOURNAL_SCHEMA {
        return Err(invalid("Transaction snapshot schema is unsupported."));
    }
    Ok(snapshot)
}

fn snapshot_present_paths<'a>(
    snapshot: &'a Snapshot,
    old_files: &[InventoryFile],
) -> Result<BTreeSet<&'a str>> {
    let present = snapshot
        .present_old_managed
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected_old = old_files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    if present.iter().any(|path| !expected_old.contains(path)) {
        return Err(invalid(
            "Transaction snapshot names a file outside the old inventory.",
        ));
    }
    if present.len() != snapshot.present_old_managed.len() {
        return Err(invalid("Transaction snapshot repeats a managed path."));
    }
    Ok(present)
}

fn verify_update_backups(
    transaction: &Path,
    old_files: &[InventoryFile],
    present: &BTreeSet<&str>,
) -> Result<()> {
    let backup_root = transaction.join(BACKUP_DIR);
    reject_reparse(&backup_root)?;
    for old_file in old_files {
        ensure_safe_inventory_path(&backup_root, &old_file.path, false)?;
        let backup = safe_join(&backup_root, &old_file.path)?;
        if present.contains(old_file.path.as_str()) {
            ensure_file_matches(&backup, old_file)?;
        } else if fs::symlink_metadata(&backup).is_ok() {
            return Err(invalid("Transaction backup does not match its snapshot."));
        }
    }
    Ok(())
}

fn roll_back_new_files(
    target: &Path,
    transaction: &Path,
    old_files: &[InventoryFile],
    new_files: &[InventoryFile],
    present: &BTreeSet<&str>,
) -> Result<()> {
    let old_by_path = old_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    for new_file in new_files.iter().rev() {
        let destination = safe_join(target, &new_file.path)?;
        ensure_safe_inventory_path(target, &new_file.path, false)?;
        if let Some(old_file) = old_by_path.get(new_file.path.as_str()) {
            if present.contains(new_file.path.as_str()) {
                let backup = safe_join(&transaction.join(BACKUP_DIR), &old_file.path)?;
                ensure_safe_inventory_path(&transaction.join(BACKUP_DIR), &old_file.path, false)?;
                ensure_file_matches(&backup, old_file)?;
                if let Some(current) = inspect_file(&destination)?
                    && !same_bytes(&current, old_file)
                    && !same_bytes(&current, new_file)
                {
                    return Err(invalid(
                        "Cannot safely restore a changed managed file; preserving it.",
                    ));
                }
                if inspect_file(&destination)?.is_none_or(|current| !same_bytes(&current, old_file))
                {
                    install_file_atomically(transaction, target, &backup, &destination)?;
                }
            } else if let Some(current) = inspect_file(&destination)? {
                if same_bytes(&current, new_file) {
                    fs::remove_file(destination)?;
                } else {
                    return Err(invalid(
                        "Cannot safely roll back a changed managed file; preserving it.",
                    ));
                }
            }
        } else if let Some(current) = inspect_file(&destination)? {
            if same_bytes(&current, new_file) {
                fs::remove_file(destination)?;
            } else {
                return Err(invalid(
                    "Cannot safely remove an unexpected file during rollback; preserving it.",
                ));
            }
        }
    }
    Ok(())
}

fn restore_obsolete_files(
    target: &Path,
    transaction: &Path,
    old_files: &[InventoryFile],
    new_files: &[InventoryFile],
    present: &BTreeSet<&str>,
) -> Result<()> {
    let new_by_path = new_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    for old_file in old_files
        .iter()
        .filter(|file| !new_by_path.contains_key(file.path.as_str()))
    {
        if !present.contains(old_file.path.as_str()) {
            continue;
        }
        let backup = safe_join(&transaction.join(BACKUP_DIR), &old_file.path)?;
        ensure_safe_inventory_path(&transaction.join(BACKUP_DIR), &old_file.path, false)?;
        ensure_file_matches(&backup, old_file)?;
        let destination = safe_join(target, &old_file.path)?;
        ensure_safe_inventory_path(target, &old_file.path, false)?;
        if let Some(current) = inspect_file(&destination)? {
            if !same_bytes(&current, old_file) {
                return Err(invalid(
                    "Cannot safely restore a changed obsolete file; preserving it.",
                ));
            }
        } else {
            install_file_atomically(transaction, target, &backup, &destination)?;
        }
    }
    Ok(())
}

fn recover_repair_transaction(
    target: &Path,
    transaction: &Path,
    old_files: &[InventoryFile],
    new_files: &[InventoryFile],
    snapshot: &Snapshot,
) -> Result<()> {
    let previous = repair_snapshot_identity_map(snapshot, old_files)?;
    let backup_root = transaction.join(BACKUP_DIR);
    reject_reparse(&backup_root)?;
    let new_by_path = new_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();

    for old_file in old_files {
        ensure_safe_inventory_path(&backup_root, &old_file.path, false)?;
        let backup = safe_join(&backup_root, &old_file.path)?;
        match previous.get(&old_file.path) {
            Some(expected) => {
                let actual = inspect_file(&backup)?
                    .ok_or_else(|| invalid("Transaction repair backup is missing."))?;
                if !same_file_identity(&actual, expected) {
                    return Err(invalid("Transaction repair backup changed."));
                }
            }
            None if inspect_file(&backup)?.is_some() => {
                return Err(invalid("Transaction backup does not match its snapshot."));
            }
            None => {}
        }
    }

    for old_file in old_files.iter().rev() {
        let destination = safe_join(target, &old_file.path)?;
        ensure_safe_inventory_path(target, &old_file.path, false)?;
        let current = inspect_file(&destination)?;
        match previous.get(&old_file.path) {
            Some(expected) => {
                if current
                    .as_ref()
                    .is_some_and(|actual| same_file_identity(actual, expected))
                {
                    continue;
                }
                if current.as_ref().is_some_and(|actual| {
                    !new_by_path
                        .get(old_file.path.as_str())
                        .is_some_and(|new_file| same_bytes(actual, new_file))
                }) {
                    return Err(invalid(
                        "Cannot safely restore a changed repair file; preserving it.",
                    ));
                }
                let backup = safe_join(&backup_root, &old_file.path)?;
                install_file_atomically(transaction, target, &backup, &destination)?;
            }
            None => {
                if let Some(current) = current {
                    if new_by_path
                        .get(old_file.path.as_str())
                        .is_some_and(|new_file| same_bytes(&current, new_file))
                    {
                        fs::remove_file(destination)?;
                    } else {
                        return Err(invalid(
                            "Cannot safely remove an unexpected repair file; preserving it.",
                        ));
                    }
                }
            }
        }
    }

    for old_file in old_files {
        ensure_safe_inventory_path(target, &old_file.path, false)?;
        let path = safe_join(target, &old_file.path)?;
        match previous.get(&old_file.path) {
            Some(expected) => {
                let actual = inspect_file(&path)?
                    .ok_or_else(|| invalid("Repair recovery did not restore a managed file."))?;
                if !same_file_identity(&actual, expected) {
                    return Err(invalid("Repair recovery did not restore prior file bytes."));
                }
            }
            None if inspect_file(&path)?.is_some() => {
                return Err(invalid("Repair recovery found an unexpected prior file."));
            }
            None => {}
        }
    }
    Ok(())
}
