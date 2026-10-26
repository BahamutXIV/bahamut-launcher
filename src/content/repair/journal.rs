//! Repair transaction journal: schema, validation, and atomic persistence.

use std::collections::HashSet;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::paths::{find_file, parent_paths, sync_parent};
use super::scan::hash_regular_file;
use super::types::{FileState, ManagedFileStatus};
use super::{JOURNAL_FILE, JOURNAL_NEXT_FILE, OWNER_FILE, ROLLBACK_DIR, STAGED_DIR};
use crate::atomic_fs::is_link_or_reparse;
use crate::content::manifest::{BasePackage, validate_hash, validate_relative_path};
use crate::digest::sha256_hex;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OwnerMarker {
    pub(super) schema_version: u32,
    pub(super) package_sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RepairJournal {
    pub(super) schema_version: u32,
    pub(super) package_sha256: String,
    pub(super) phase: JournalPhase,
    pub(super) files: Vec<JournalFile>,
    pub(super) created_directories: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum JournalPhase {
    Publishing,
    Committed,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JournalFile {
    pub(super) path: String,
    pub(super) original: OriginalState,
    pub(super) published: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OriginalState {
    Missing,
    Corrupt { length: u64, sha256: String },
}

pub(super) fn make_journal(
    package_sha256: &str,
    statuses: &[ManagedFileStatus],
    created_directories: Vec<String>,
) -> Result<RepairJournal, String> {
    let files = statuses
        .iter()
        .map(|status| {
            let original = match status.state {
                FileState::Missing => OriginalState::Missing,
                FileState::Corrupt => OriginalState::Corrupt {
                    length: status.actual_length.ok_or_else(|| {
                        format!("Corrupt file length was unavailable: {}.", status.path)
                    })?,
                    sha256: status.actual_sha256.clone().ok_or_else(|| {
                        format!("Corrupt file hash was unavailable: {}.", status.path)
                    })?,
                },
                FileState::Valid | FileState::Unresolved => {
                    return Err(format!("{} is not eligible for repair.", status.path));
                }
            };
            Ok(JournalFile {
                path: status.path.clone(),
                original,
                published: false,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(RepairJournal {
        schema_version: 1,
        package_sha256: package_sha256.to_owned(),
        phase: JournalPhase::Publishing,
        files,
        created_directories,
    })
}

pub(super) fn validate_journal(
    journal: &RepairJournal,
    package: &BasePackage,
    package_sha256: &str,
) -> Result<(), String> {
    if journal.schema_version != 1 || journal.package_sha256 != package_sha256 {
        return Err("The repair journal does not match this package.".into());
    }
    let mut seen = HashSet::new();
    for item in &journal.files {
        let file = find_file(package, &item.path)?;
        if matches!(journal.phase, JournalPhase::Committed) && !item.published {
            return Err(format!(
                "Committed repair journal has an unpublished file: {}.",
                item.path
            ));
        }
        if !seen.insert(item.path.to_ascii_lowercase()) {
            return Err(format!("Duplicate path in repair journal: {}.", item.path));
        }
        match &item.original {
            OriginalState::Missing => {}
            OriginalState::Corrupt { sha256, .. } => {
                if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(format!(
                        "Invalid original hash in repair journal: {}.",
                        item.path
                    ));
                }
                let _ = file;
            }
        }
    }
    let mut directories = HashSet::new();
    for directory in &journal.created_directories {
        validate_relative_path(directory)?;
        if !directories.insert(directory.to_ascii_lowercase()) {
            return Err(format!(
                "Duplicate directory in repair journal: {directory}."
            ));
        }
        if !journal.files.iter().any(|file| {
            file.path
                .to_ascii_lowercase()
                .starts_with(&format!("{}/", directory.to_ascii_lowercase()))
        }) {
            return Err(format!(
                "Unrelated directory in repair journal: {directory}."
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_transaction_tree(
    transaction: &Path,
    package: &BasePackage,
    journal: Option<&RepairJournal>,
) -> Result<(), String> {
    let allowed_files = journal
        .map(|journal| {
            journal
                .files
                .iter()
                .map(|file| file.path.to_ascii_lowercase())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_else(|| {
            package
                .final_files
                .iter()
                .map(|file| file.path.to_ascii_lowercase())
                .collect::<HashSet<_>>()
        });
    let allowed_dirs = package
        .final_files
        .iter()
        .flat_map(|file| parent_paths(&file.path))
        .map(|path| path.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut pending = vec![(transaction.to_path_buf(), String::new())];
    while let Some((directory, relative)) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("Could not inspect repair recovery data: {error}"))?
        {
            let entry = entry
                .map_err(|error| format!("Could not inspect repair recovery entry: {error}"))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
            if is_link_or_reparse(&metadata) {
                return Err(format!(
                    "Repair recovery contains a reparse point: {}",
                    path.display()
                ));
            }
            let leaf = entry.file_name().to_string_lossy().into_owned();
            let child_relative = if relative.is_empty() {
                leaf.clone()
            } else {
                format!("{relative}/{leaf}")
            };
            if metadata.is_dir() {
                check_recovery_directory(&relative, &leaf, &child_relative, &allowed_dirs)?;
                pending.push((path, child_relative));
            } else if metadata.is_file() {
                if !recovery_file_allowed(&relative, &leaf, &child_relative, &allowed_files) {
                    return Err(format!("Unexpected repair recovery file: {child_relative}"));
                }
            } else {
                return Err(format!(
                    "Unexpected special repair recovery entry: {}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

/// Directories may only be `staged`/`rollback` at the root and catalog parent
/// directories beneath them.
fn check_recovery_directory(
    relative: &str,
    leaf: &str,
    child_relative: &str,
    allowed_dirs: &HashSet<String>,
) -> Result<(), String> {
    if relative.is_empty() {
        if !matches!(leaf, STAGED_DIR | ROLLBACK_DIR) {
            return Err(format!("Unexpected repair recovery directory: {leaf}"));
        }
    } else if relative == STAGED_DIR || relative == ROLLBACK_DIR {
        let folded = child_relative[relative.len() + 1..].to_ascii_lowercase();
        if !allowed_dirs.contains(&folded) {
            return Err(format!(
                "Unexpected repair recovery directory: {child_relative}"
            ));
        }
    } else if relative.starts_with(&format!("{STAGED_DIR}/"))
        || relative.starts_with(&format!("{ROLLBACK_DIR}/"))
    {
        let prefix = if relative.starts_with(&format!("{STAGED_DIR}/")) {
            STAGED_DIR
        } else {
            ROLLBACK_DIR
        };
        let inner = child_relative[prefix.len() + 1..].to_ascii_lowercase();
        if !allowed_dirs.contains(&inner) {
            return Err(format!(
                "Unexpected repair recovery directory: {child_relative}"
            ));
        }
    } else {
        return Err(format!(
            "Unexpected repair recovery directory: {child_relative}"
        ));
    }
    Ok(())
}

/// Files may be the three control files at the root or journaled files under
/// `staged`/`rollback`.
fn recovery_file_allowed(
    relative: &str,
    leaf: &str,
    child_relative: &str,
    allowed_files: &HashSet<String>,
) -> bool {
    match relative {
        "" => matches!(leaf, OWNER_FILE | JOURNAL_FILE | JOURNAL_NEXT_FILE),
        STAGED_DIR | ROLLBACK_DIR => allowed_files.contains(&leaf.to_ascii_lowercase()),
        _ if relative.starts_with(&format!("{STAGED_DIR}/")) => {
            allowed_files.contains(&child_relative[STAGED_DIR.len() + 1..].to_ascii_lowercase())
        }
        _ if relative.starts_with(&format!("{ROLLBACK_DIR}/")) => {
            allowed_files.contains(&child_relative[ROLLBACK_DIR.len() + 1..].to_ascii_lowercase())
        }
        _ => false,
    }
}

pub(super) fn ensure_no_rollback_data(transaction: &Path) -> Result<(), String> {
    let rollback = transaction.join(ROLLBACK_DIR);
    match fs::read_dir(&rollback) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Could not inspect repair rollback directory: {error}"
        )),
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err("Repair rollback data has no readable transaction journal.".into());
            }
            Ok(())
        }
    }
}

pub(super) fn cleanup_unjournaled_transaction(
    transaction: &Path,
    package: &BasePackage,
    package_sha256: &str,
) -> Result<(), String> {
    let owner = read_json::<OwnerMarker>(&transaction.join(OWNER_FILE), 4096)?;
    if owner.schema_version != 1 || owner.package_sha256 != package_sha256 {
        return Err("Refusing to remove an unowned repair staging directory.".into());
    }
    validate_transaction_tree(transaction, package, None)?;
    ensure_no_rollback_data(transaction)?;
    cleanup_owned_transaction(transaction, package, None)
}

pub(super) fn cleanup_owned_transaction(
    transaction: &Path,
    package: &BasePackage,
    journal: Option<&RepairJournal>,
) -> Result<(), String> {
    validate_transaction_tree(transaction, package, journal)?;
    fs::remove_dir_all(transaction)
        .map_err(|error| format!("Could not remove completed repair recovery data: {error}"))
}

pub(super) fn create_transaction(
    transaction: &Path,
    package: &BasePackage,
    package_sha256: &str,
) -> Result<PathBuf, String> {
    fs::create_dir(transaction)
        .map_err(|error| format!("Could not create repair recovery directory: {error}"))?;
    let marker = OwnerMarker {
        schema_version: 1,
        package_sha256: package_sha256.to_owned(),
    };
    write_json_new(&transaction.join(OWNER_FILE), &marker)?;
    for child in [STAGED_DIR, ROLLBACK_DIR] {
        if let Err(error) = fs::create_dir(transaction.join(child)) {
            let _ = cleanup_unjournaled_transaction(transaction, package, package_sha256);
            return Err(format!(
                "Could not create repair {child} directory: {error}"
            ));
        }
    }
    Ok(transaction.to_path_buf())
}

pub(super) fn write_journal(transaction: &Path, journal: &RepairJournal) -> Result<(), String> {
    let next = transaction.join(JOURNAL_NEXT_FILE);
    match fs::symlink_metadata(&next) {
        Ok(metadata) if !is_link_or_reparse(&metadata) && metadata.is_file() => {
            fs::remove_file(&next)
                .map_err(|error| format!("Could not reset repair journal temp file: {error}"))?;
        }
        Ok(_) => return Err("Repair journal temp path is unsafe.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "Could not inspect repair journal temp path: {error}"
            ));
        }
    }
    write_json_new(&next, journal)?;
    fs::rename(&next, transaction.join(JOURNAL_FILE))
        .map_err(|error| format!("Could not persist repair transaction journal: {error}"))?;
    sync_parent(&transaction.join(JOURNAL_FILE));
    Ok(())
}

pub(super) fn write_json_new(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("Could not encode repair state: {error}"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("Could not create repair state: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Could not persist repair state: {error}"))
}

pub(super) fn read_json<T: for<'de> Deserialize<'de>>(
    path: &Path,
    max_bytes: u64,
) -> Result<T, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect repair state {}: {error}", path.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() || metadata.len() > max_bytes {
        return Err(format!("Invalid repair state file: {}", path.display()));
    }
    let (length, _) = hash_regular_file(path)?;
    let bytes = fs::read(path)
        .map_err(|error| format!("Could not read repair state {}: {error}", path.display()))?;
    if length != bytes.len() as u64 {
        return Err(format!(
            "Repair state changed while reading: {}",
            path.display()
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid repair state {}: {error}", path.display()))
}

pub(super) fn package_identity(package: &BasePackage) -> String {
    let encoded = serde_json::to_vec(package).expect("BasePackage is infallibly serializable");
    sha256_hex(&encoded)
}

pub(super) fn maximum_repair_journal(
    package: &BasePackage,
    package_sha256: &str,
) -> Result<RepairJournal, String> {
    validate_hash(package_sha256)?;
    let files = package
        .final_files
        .iter()
        .map(|file| JournalFile {
            path: file.path.clone(),
            original: OriginalState::Corrupt {
                length: u64::MAX,
                sha256: "f".repeat(64),
            },
            published: false,
        })
        .collect();
    let mut created_directories = package
        .final_files
        .iter()
        .flat_map(|file| parent_paths(&file.path))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    created_directories.sort();
    Ok(RepairJournal {
        schema_version: 1,
        package_sha256: package_sha256.to_owned(),
        phase: JournalPhase::Publishing,
        files,
        created_directories,
    })
}

pub(super) fn journal_read_limit(
    package: &BasePackage,
    package_sha256: &str,
) -> Result<u64, String> {
    let maximum = maximum_repair_journal(package, package_sha256)?;
    let bytes = serde_json::to_vec(&maximum)
        .map_err(|error| format!("Could not size repair journal: {error}"))?;
    u64::try_from(bytes.len()).map_err(|_| "Repair journal size overflow.".into())
}
