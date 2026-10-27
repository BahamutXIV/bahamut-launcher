//! Publishing staged replacements into the game directory under a journal.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use super::archive::create_owned_parents;
use super::control::RepairShared;
use super::journal::{RepairJournal, write_journal};
use super::paths::{join_relative, path_exists_safely, sync_parent};
use super::recover::{published_file_matches_stage_or_move, same_two_link_file};
use super::scan::{
    hash_allowing_transaction_link, hash_allowing_transaction_link_controlled,
    hash_regular_file_controlled, inspect_file, inspect_file_controlled,
};
use super::types::{FileState, ManagedFileStatus};
use super::{ROLLBACK_DIR, STAGED_DIR};
use crate::atomic_fs::is_link_or_reparse;
use crate::content::manifest::InstallFile;

pub(super) fn publish_selected(
    game: &Path,
    transaction: &Path,
    selected: &[&InstallFile],
    statuses: &[ManagedFileStatus],
    journal: &mut RepairJournal,
    shared: &RepairShared,
) -> Result<(), String> {
    let staged_root = transaction.join(STAGED_DIR);
    let rollback_root = transaction.join(ROLLBACK_DIR);
    create_managed_directories(game, journal, shared)?;
    for (index, (file, snapshot)) in selected.iter().zip(statuses).enumerate() {
        shared.checkpoint()?;
        shared.set_current_file(Some(&file.path));
        let current = inspect_file_controlled(game, file, shared)?;
        if current != *snapshot {
            return Err(format!(
                "{} changed while repair was being prepared; verify again.",
                file.path
            ));
        }
        let live = join_relative(game, &file.path);
        let staged = join_relative(&staged_root, &file.path);
        let rollback = join_relative(&rollback_root, &file.path);
        create_owned_parents(
            &rollback_root,
            rollback.parent().expect("managed file has a parent"),
        )?;
        if snapshot.state == FileState::Corrupt {
            move_original_aside(&live, &rollback, file, snapshot, shared)?;
        }
        link_staged_file(&staged, &live, file)?;
        sync_parent(&live);
        let journal_file = journal
            .files
            .get_mut(index)
            .filter(|entry| entry.path.eq_ignore_ascii_case(&file.path))
            .ok_or_else(|| format!("Repair journal omitted {}.", file.path))?;
        journal_file.published = true;
        write_journal(transaction, journal)?;
        if !published_file_matches_stage_or_move(&live, &staged, file)? {
            return Err(format!(
                "Published file lost its staging identity: {}.",
                file.path
            ));
        }
        let (length, digest) = hash_allowing_transaction_link_controlled(&live, shared)?;
        if length != file.length || !digest.eq_ignore_ascii_case(&file.sha256) {
            return Err(format!(
                "Published file failed verification: {}.",
                file.path
            ));
        }
        shared.complete_file();
    }
    shared.set_current_file(None);
    Ok(())
}

fn create_managed_directories(
    game: &Path,
    journal: &RepairJournal,
    shared: &RepairShared,
) -> Result<(), String> {
    for directory in &journal.created_directories {
        shared.checkpoint()?;
        let path = join_relative(game, directory);
        match fs::create_dir(&path) {
            Ok(()) => sync_parent(&path),
            Err(error) => {
                return Err(format!(
                    "Could not create managed parent directory {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

/// Move the corrupt original into the rollback tree and confirm it is the file
/// that was scanned; otherwise restore it.
fn move_original_aside(
    live: &Path,
    rollback: &Path,
    file: &InstallFile,
    snapshot: &ManagedFileStatus,
    shared: &RepairShared,
) -> Result<(), String> {
    fs::rename(live, rollback).map_err(|error| {
        format!(
            "Could not move {} into repair recovery; it may be in use. Repair is deferred: {error}",
            file.path
        )
    })?;
    sync_parent(live);
    let (length, digest) = match hash_regular_file_controlled(rollback, shared) {
        Ok(identity) => identity,
        Err(error) => {
            restore_original(live, rollback);
            return Err(error);
        }
    };
    if Some(length) != snapshot.actual_length
        || snapshot
            .actual_sha256
            .as_deref()
            .is_none_or(|expected| !digest.eq_ignore_ascii_case(expected))
    {
        restore_original(live, rollback);
        return Err(format!(
            "{} changed during repair publication; its latest original bytes were restored.",
            file.path
        ));
    }
    Ok(())
}

fn restore_original(live: &Path, rollback: &Path) {
    if !live.exists() {
        let _ = fs::rename(rollback, live);
        sync_parent(live);
    }
}

/// Hard-link the staged file into place without replacing an existing path.
fn link_staged_file(staged: &Path, live: &Path, file: &InstallFile) -> Result<(), String> {
    match fs::hard_link(staged, live) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(format!(
            "Could not publish repaired file {}; its live path appeared during repair.",
            file.path
        )),
        Err(_) => crate::atomic_fs::rename_noreplace(staged, live).map_err(|error| {
            format!(
                "Could not publish repaired file {}; it may be in use. Repair is deferred: {error}",
                file.path
            )
        }),
    }
}

pub(super) fn validate_staged_publication(
    transaction: &Path,
    game: &Path,
    files: &[&InstallFile],
    shared: &RepairShared,
) -> Result<(), String> {
    for file in files {
        shared.checkpoint()?;
        shared.set_current_file(Some(&file.path));
        let live = join_relative(game, &file.path);
        let staged = join_relative(&transaction.join(STAGED_DIR), &file.path);
        if !published_file_matches_stage_or_move(&live, &staged, file)? {
            return Err(format!("Published file identity changed: {}.", file.path));
        }
        let (length, digest) = hash_allowing_transaction_link_controlled(&live, shared)?;
        if length != file.length || !digest.eq_ignore_ascii_case(&file.sha256) {
            return Err(format!(
                "Published file failed verification: {}.",
                file.path
            ));
        }
        shared.complete_file();
    }
    shared.set_current_file(None);
    Ok(())
}

pub(super) fn detach_published_stage_links(
    transaction: &Path,
    game: &Path,
    files: &[&InstallFile],
    shared: &RepairShared,
) -> Result<(), String> {
    let staged_root = transaction.join(STAGED_DIR);
    for file in files {
        shared.checkpoint()?;
        shared.set_current_file(Some(&file.path));
        let live = join_relative(game, &file.path);
        let staged = join_relative(&staged_root, &file.path);
        match fs::symlink_metadata(&staged) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "Could not inspect staged repair file {}: {error}",
                    staged.display()
                ));
            }
            Ok(metadata) if is_link_or_reparse(&metadata) || !metadata.is_file() => {
                return Err(format!(
                    "Refusing an unsafe staged repair file: {}",
                    staged.display()
                ));
            }
            Ok(_) => {}
        }
        if !same_two_link_file(&live, &staged)? {
            return Err(format!(
                "Published file lost its staging identity: {}.",
                file.path
            ));
        }
        fs::remove_file(&staged).map_err(|error| {
            format!(
                "Could not detach staged repair file {}: {error}",
                staged.display()
            )
        })?;
        sync_parent(&staged);
    }
    shared.set_current_file(None);
    Ok(())
}

pub(super) fn validate_committed_publication(
    transaction: &Path,
    game: &Path,
    files: &[&InstallFile],
) -> Result<(), String> {
    for file in files {
        let live = join_relative(game, &file.path);
        let staged = join_relative(&transaction.join(STAGED_DIR), &file.path);
        if path_exists_safely(&staged)? {
            if !published_file_matches_stage_or_move(&live, &staged, file)? {
                return Err(format!("Published file identity changed: {}.", file.path));
            }
            let (length, digest) = hash_allowing_transaction_link(&live)?;
            if length != file.length || !digest.eq_ignore_ascii_case(&file.sha256) {
                return Err(format!(
                    "Published file failed verification: {}.",
                    file.path
                ));
            }
        } else {
            let status = inspect_file(game, file);
            if status.state != FileState::Valid {
                return Err(format!(
                    "Published file failed verification: {}.",
                    file.path
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn ensure_snapshots_unchanged(
    expected: &[ManagedFileStatus],
    current: &[ManagedFileStatus],
) -> Result<(), String> {
    if expected.len() != current.len() {
        return Err("The selected file set changed while repair was being prepared.".into());
    }
    for (expected, current) in expected.iter().zip(current) {
        if expected != current {
            return Err(format!(
                "{} changed while repair was being prepared; verify again.",
                expected.path
            ));
        }
    }
    Ok(())
}

pub(super) fn missing_parent_directories(
    root: &Path,
    files: &[&InstallFile],
) -> Result<Vec<String>, String> {
    let mut missing = HashSet::new();
    for file in files {
        let mut current = root.to_path_buf();
        let components = file.path.split('/').collect::<Vec<_>>();
        for part in components.iter().take(components.len().saturating_sub(1)) {
            current.push(part);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if !is_link_or_reparse(&metadata) && metadata.is_dir() => {}
                Ok(_) => return Err(format!("Unsafe parent directory for {}.", file.path)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let relative = current
                        .strip_prefix(root)
                        .map_err(|_| "A managed parent escaped the game directory.".to_string())?
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    missing.insert(relative);
                }
                Err(error) => {
                    return Err(format!(
                        "Could not inspect parent directory {}: {error}",
                        current.display()
                    ));
                }
            }
        }
    }
    let mut result = missing.into_iter().collect::<Vec<_>>();
    result.sort_by_key(|path| path.matches('/').count());
    Ok(result)
}
