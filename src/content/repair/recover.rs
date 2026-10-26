//! Rollback and recovery of interrupted or failed repair transactions.

use std::fs;
use std::path::Path;

use super::journal::{
    JournalPhase, OriginalState, OwnerMarker, RepairJournal, cleanup_owned_transaction,
    ensure_no_rollback_data, journal_read_limit, read_json, validate_journal,
    validate_transaction_tree,
};
use super::paths::{
    ensure_live_parent, find_file, join_relative, open_regular_for_identity, path_exists_safely,
    sync_parent,
};
use super::publish::validate_committed_publication;
use super::scan::{hash_allowing_transaction_link, hash_regular_file, inspect_file};
use super::types::FileState;
use super::{JOURNAL_FILE, OWNER_FILE, ROLLBACK_DIR, STAGED_DIR};
use crate::atomic_fs::is_link_or_reparse;
use crate::content::manifest::{BasePackage, InstallFile, validate_hash};

pub(super) fn rollback_after_final_verification_error(
    transaction: &Path,
    game: &Path,
    package: &BasePackage,
    package_sha256: &str,
    error: String,
) -> String {
    match recover_transaction(transaction, game, package, package_sha256) {
        Ok(()) => error,
        Err(recovery_error) => format!(
            "{error}. Rollback is incomplete and will be retried on the next Repair: {recovery_error}"
        ),
    }
}

pub(super) fn recover_transaction(
    transaction: &Path,
    game: &Path,
    package: &BasePackage,
    package_sha256: &str,
) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(transaction) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Could not inspect repair recovery directory: {error}"
            ));
        }
    };
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err("The repair recovery path is not a safe directory.".into());
    }
    validate_transaction_tree(transaction, package, None)?;
    let owner = read_json::<OwnerMarker>(&transaction.join(OWNER_FILE), 4096)?;
    if owner.schema_version != 1 || owner.package_sha256 != package_sha256 {
        return Err("The repair recovery directory belongs to another package.".into());
    }
    let journal_path = transaction.join(JOURNAL_FILE);
    match fs::symlink_metadata(&journal_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure_no_rollback_data(transaction)?;
            cleanup_owned_transaction(transaction, package, None)
        }
        Err(error) => Err(format!("Could not inspect repair journal: {error}")),
        Ok(_) => {
            let journal = read_json::<RepairJournal>(
                &journal_path,
                journal_read_limit(package, package_sha256)?,
            )?;
            validate_journal(&journal, package, package_sha256)?;
            validate_transaction_tree(transaction, package, Some(&journal))?;
            match journal.phase {
                JournalPhase::Publishing => rollback_journal(transaction, game, package, &journal)?,
                JournalPhase::Committed => {
                    let selected = journal
                        .files
                        .iter()
                        .map(|entry| find_file(package, &entry.path))
                        .collect::<Result<Vec<_>, _>>()?;
                    if let Err(error) = validate_committed_publication(transaction, game, &selected)
                    {
                        return Err(format!(
                            "Committed repair recovery data remains because a repaired file no longer validates: {error}"
                        ));
                    }
                }
            }
            cleanup_owned_transaction(transaction, package, Some(&journal))
        }
    }
}

pub(super) fn cleanup_committed_transaction(
    transaction: &Path,
    package: &BasePackage,
    package_sha256: &str,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(transaction)
        .map_err(|error| format!("Could not inspect repair recovery directory: {error}"))?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err("The repair recovery path is not a safe directory.".into());
    }
    validate_transaction_tree(transaction, package, None)?;
    let owner = read_json::<OwnerMarker>(&transaction.join(OWNER_FILE), 4096)?;
    if owner.schema_version != 1 || owner.package_sha256 != package_sha256 {
        return Err("The repair recovery directory belongs to another package.".into());
    }
    let journal = read_json::<RepairJournal>(
        &transaction.join(JOURNAL_FILE),
        journal_read_limit(package, package_sha256)?,
    )?;
    validate_journal(&journal, package, package_sha256)?;
    if !matches!(journal.phase, JournalPhase::Committed) {
        return Err("The repair transaction was not durably committed.".into());
    }
    validate_transaction_tree(transaction, package, Some(&journal))?;
    cleanup_owned_transaction(transaction, package, Some(&journal))
}

pub(super) fn inspect_recovery_state(
    transaction: &Path,
    package: &BasePackage,
) -> Result<(bool, Option<String>), String> {
    match fs::symlink_metadata(transaction) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((false, None)),
        Err(error) => Err(format!(
            "Could not inspect repair recovery directory: {error}"
        )),
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_dir() {
                return Err("The repair recovery path is not a safe directory.".into());
            }
            validate_transaction_tree(transaction, package, None)?;
            let owner = read_json::<OwnerMarker>(&transaction.join(OWNER_FILE), 4096)?;
            if owner.schema_version != 1 {
                return Err("The repair recovery marker has an unsupported version.".into());
            }
            let journal_path = transaction.join(JOURNAL_FILE);
            match fs::symlink_metadata(&journal_path) {
                Ok(_) => {
                    validate_hash(&owner.package_sha256)?;
                    let journal = read_json::<RepairJournal>(
                        &journal_path,
                        journal_read_limit(package, &owner.package_sha256)?,
                    )?;
                    validate_journal(&journal, package, &owner.package_sha256)?;
                    validate_transaction_tree(transaction, package, Some(&journal))?;
                    Ok((true, Some("A previous repair was interrupted. Recover it before repairing more files.".into())))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    ensure_no_rollback_data(transaction)?;
                    Ok((
                        true,
                        Some("An unfinished repair staging directory needs recovery.".into()),
                    ))
                }
                Err(error) => Err(format!("Could not inspect repair journal: {error}")),
            }
        }
    }
}

pub(super) fn rollback_journal(
    transaction: &Path,
    game: &Path,
    package: &BasePackage,
    journal: &RepairJournal,
) -> Result<(), String> {
    let rollback_root = transaction.join(ROLLBACK_DIR);
    for item in journal.files.iter().rev() {
        let file = find_file(package, &item.path)?;
        let live = join_relative(game, &item.path);
        let backup = join_relative(&rollback_root, &item.path);
        let backup_status = path_exists_safely(&backup)?;
        match (&item.original, backup_status) {
            (OriginalState::Corrupt { length, sha256 }, true) => {
                let (backup_length, backup_hash) = hash_regular_file(&backup)?;
                if backup_length != *length || !backup_hash.eq_ignore_ascii_case(sha256) {
                    return Err(format!(
                        "Repair recovery copy failed validation: {}.",
                        item.path
                    ));
                }
                let staged = join_relative(&transaction.join(STAGED_DIR), &item.path);
                match inspect_file(game, file).state {
                    FileState::Missing => {}
                    FileState::Unresolved | FileState::Valid
                        if remove_in_progress_publication(&live, &staged, file)? => {}
                    _ => {
                        return Err(format!(
                            "Cannot safely roll back {} because its live path changed.",
                            item.path
                        ));
                    }
                }
                ensure_live_parent(game, &live)?;
                fs::rename(&backup, &live).map_err(|error| {
                    format!("Could not restore original {}: {error}", item.path)
                })?;
                sync_parent(&live);
            }
            (OriginalState::Corrupt { length, sha256 }, false) => {
                let current = inspect_file(game, file);
                if current.state != FileState::Corrupt
                    || current.actual_length != Some(*length)
                    || current
                        .actual_sha256
                        .as_deref()
                        .is_none_or(|actual| !actual.eq_ignore_ascii_case(sha256))
                {
                    return Err(format!(
                        "Cannot safely recover {} because its original copy is missing.",
                        item.path
                    ));
                }
            }
            (OriginalState::Missing, true) => {
                return Err(format!(
                    "Unexpected recovery copy exists for originally missing file {}.",
                    item.path
                ));
            }
            (OriginalState::Missing, false) => {
                let staged = join_relative(&transaction.join(STAGED_DIR), &item.path);
                match inspect_file(game, file).state {
                    FileState::Missing => {}
                    FileState::Unresolved | FileState::Valid | FileState::Corrupt => {
                        let _ = remove_in_progress_publication(&live, &staged, file)?;
                    }
                }
            }
        }
    }
    for relative in journal.created_directories.iter().rev() {
        let path = join_relative(game, relative);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Could not inspect created directory: {error}")),
            Ok(metadata) if is_link_or_reparse(&metadata) || !metadata.is_dir() => {
                return Err(format!(
                    "Created repair directory changed: {}",
                    path.display()
                ));
            }
            Ok(_) => match fs::remove_dir(&path) {
                Ok(()) => sync_parent(&path),
                Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!("Could not remove empty repair directory: {error}"));
                }
            },
        }
    }
    Ok(())
}

pub(super) fn remove_in_progress_publication(
    live: &Path,
    staged: &Path,
    spec: &InstallFile,
) -> Result<bool, String> {
    match fs::symlink_metadata(staged) {
        Ok(_) if !same_two_link_file(live, staged)? => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // On filesystems without hard-link support, publication atomically moves
            // the verified staged file. Its absence records that the move completed.
        }
        Err(error) => {
            return Err(format!(
                "Could not inspect repair staging identity {}: {error}",
                staged.display()
            ));
        }
    }
    let (length, digest) = hash_allowing_transaction_link(live)?;
    if length != spec.length || !digest.eq_ignore_ascii_case(&spec.sha256) {
        return Err(format!(
            "The interrupted repair link failed catalog verification: {}.",
            spec.path
        ));
    }
    if staged.exists() && !same_two_link_file(live, staged)? {
        return Err(format!(
            "The interrupted repair publication changed during recovery: {}.",
            spec.path
        ));
    }
    fs::remove_file(live).map_err(|error| {
        format!(
            "Could not remove interrupted replacement {}: {error}",
            spec.path
        )
    })?;
    sync_parent(live);
    Ok(true)
}

pub(super) fn published_file_matches_stage_or_move(
    live: &Path,
    staged: &Path,
    spec: &InstallFile,
) -> Result<bool, String> {
    match fs::symlink_metadata(staged) {
        Ok(_) => same_two_link_file(live, staged),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let (length, digest) = hash_allowing_transaction_link(live)?;
            Ok(length == spec.length && digest.eq_ignore_ascii_case(&spec.sha256))
        }
        Err(error) => Err(format!(
            "Could not inspect staged repair file {}: {error}",
            staged.display()
        )),
    }
}

pub(super) fn same_two_link_file(left: &Path, right: &Path) -> Result<bool, String> {
    let left_file = open_regular_for_identity(left)?;
    let right_file = open_regular_for_identity(right)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let left_metadata = left_file
            .metadata()
            .map_err(|error| format!("Could not inspect repair file identity: {error}"))?;
        let right_metadata = right_file
            .metadata()
            .map_err(|error| format!("Could not inspect repair file identity: {error}"))?;
        Ok(left_metadata.nlink() == 2
            && right_metadata.nlink() == 2
            && left_metadata.dev() == right_metadata.dev()
            && left_metadata.ino() == right_metadata.ino())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };

        let mut left_info = BY_HANDLE_FILE_INFORMATION::default();
        let mut right_info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe {
            GetFileInformationByHandle(HANDLE(left_file.as_raw_handle()), &mut left_info).and_then(
                |()| {
                    GetFileInformationByHandle(HANDLE(right_file.as_raw_handle()), &mut right_info)
                },
            )
        }
        .map_err(|error| format!("Could not inspect repair file identity: {error}"))?;
        Ok(left_info.nNumberOfLinks == 2
            && right_info.nNumberOfLinks == 2
            && left_info.dwVolumeSerialNumber == right_info.dwVolumeSerialNumber
            && left_info.nFileIndexHigh == right_info.nFileIndexHigh
            && left_info.nFileIndexLow == right_info.nFileIndexLow)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (left_file, right_file);
        Ok(false)
    }
}
