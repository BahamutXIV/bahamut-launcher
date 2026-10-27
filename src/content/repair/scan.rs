//! Hash-based scanning of managed game files against the catalog.

use std::fs;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::IO_BUFFER_BYTES;
use super::control::RepairShared;
use super::paths::{
    join_relative, open_regular_for_identity, set_no_follow, validate_regular_handle,
};
use super::types::{FileState, ManagedFileStatus};
use crate::atomic_fs::is_link_or_reparse;
use crate::content::manifest::InstallFile;

pub(super) fn is_repairable(state: FileState) -> bool {
    matches!(state, FileState::Missing | FileState::Corrupt)
}

pub(super) fn select_repair_files<'a>(
    files: &'a [InstallFile],
    statuses: &[ManagedFileStatus],
) -> Result<(Vec<&'a InstallFile>, Vec<ManagedFileStatus>), String> {
    if files.len() != statuses.len() {
        return Err("The verification result did not match the managed inventory.".into());
    }

    let mut selected = Vec::new();
    let mut selected_statuses = Vec::new();
    for (file, status) in files.iter().zip(statuses) {
        if !status.path.eq_ignore_ascii_case(&file.path) {
            return Err(format!("The verification result omitted {}.", file.path));
        }
        if is_repairable(status.state) {
            selected.push(file);
            selected_statuses.push(status.clone());
        }
    }
    Ok((selected, selected_statuses))
}

#[cfg(test)]
pub(super) fn scan_selected(root: &Path, files: &[&InstallFile]) -> Vec<ManagedFileStatus> {
    files.iter().map(|file| inspect_file(root, file)).collect()
}

pub(super) fn scan_files_controlled(
    root: &Path,
    files: &[InstallFile],
    shared: &RepairShared,
) -> Result<Vec<ManagedFileStatus>, String> {
    let mut statuses = Vec::with_capacity(files.len());
    for file in files {
        shared.checkpoint()?;
        shared.set_current_file(Some(&file.path));
        statuses.push(inspect_file_controlled(root, file, shared)?);
        shared.complete_file();
    }
    shared.set_current_file(None);
    Ok(statuses)
}

pub(super) fn scan_selected_controlled(
    root: &Path,
    files: &[&InstallFile],
    shared: &RepairShared,
) -> Result<Vec<ManagedFileStatus>, String> {
    let mut statuses = Vec::with_capacity(files.len());
    for file in files {
        shared.checkpoint()?;
        shared.set_current_file(Some(&file.path));
        statuses.push(inspect_file_controlled(root, file, shared)?);
        shared.complete_file();
    }
    shared.set_current_file(None);
    Ok(statuses)
}

pub(super) fn inspect_file(root: &Path, spec: &InstallFile) -> ManagedFileStatus {
    inspect_file_inner(root, spec, None).expect("uncontrolled repair scan cannot be cancelled")
}

pub(super) fn inspect_file_controlled(
    root: &Path,
    spec: &InstallFile,
    shared: &RepairShared,
) -> Result<ManagedFileStatus, String> {
    inspect_file_inner(root, spec, Some(shared))
}

pub(super) fn inspect_file_inner(
    root: &Path,
    spec: &InstallFile,
    shared: Option<&RepairShared>,
) -> Result<ManagedFileStatus, String> {
    let path = join_relative(root, &spec.path);
    let mut current = root.to_path_buf();
    let parts = spec.path.split('/').collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(file_status(spec, FileState::Missing, None, None, None));
            }
            Err(error) => {
                return Ok(file_status(
                    spec,
                    FileState::Unresolved,
                    None,
                    None,
                    Some(format!("Could not inspect {}: {error}", current.display())),
                ));
            }
            Ok(metadata) => {
                if is_link_or_reparse(&metadata) {
                    return Ok(file_status(
                        spec,
                        FileState::Unresolved,
                        None,
                        None,
                        Some(format!(
                            "Refusing a link or reparse point: {}",
                            current.display()
                        )),
                    ));
                }
                if index + 1 < parts.len() && !metadata.is_dir() {
                    return Ok(file_status(
                        spec,
                        FileState::Unresolved,
                        None,
                        None,
                        Some(format!(
                            "A managed parent is not a directory: {}",
                            current.display()
                        )),
                    ));
                }
                if index + 1 == parts.len() && !metadata.is_file() {
                    return Ok(file_status(
                        spec,
                        FileState::Unresolved,
                        Some(metadata.len()),
                        None,
                        Some(format!(
                            "Managed path is not a regular file: {}",
                            current.display()
                        )),
                    ));
                }
            }
        }
    }
    let hash = match shared {
        Some(shared) => hash_regular_file_controlled(&path, shared),
        None => hash_regular_file(&path),
    };
    match hash {
        Ok((length, sha256)) => {
            let state = if length == spec.length && sha256.eq_ignore_ascii_case(&spec.sha256) {
                FileState::Valid
            } else {
                FileState::Corrupt
            };
            Ok(file_status(spec, state, Some(length), Some(sha256), None))
        }
        Err(error)
            if shared.is_some_and(|shared| {
                shared.is_cancel_requested() || error == "Repair control state is unavailable."
            }) =>
        {
            Err(error)
        }
        Err(error) => Ok(file_status(
            spec,
            FileState::Unresolved,
            None,
            None,
            Some(error),
        )),
    }
}

pub(super) fn file_status(
    spec: &InstallFile,
    state: FileState,
    actual_length: Option<u64>,
    actual_sha256: Option<String>,
    detail: Option<String>,
) -> ManagedFileStatus {
    ManagedFileStatus {
        path: spec.path.clone(),
        state,
        expected_length: spec.length,
        actual_length,
        actual_sha256,
        detail,
    }
}

pub(super) fn hash_regular_file(path: &Path) -> Result<(u64, String), String> {
    hash_regular_file_inner(path, None)
}

pub(super) fn hash_regular_file_controlled(
    path: &Path,
    shared: &RepairShared,
) -> Result<(u64, String), String> {
    hash_regular_file_inner(path, Some(shared))
}

pub(super) fn hash_regular_file_inner(
    path: &Path,
    shared: Option<&RepairShared>,
) -> Result<(u64, String), String> {
    let mut options = OpenOptions::new();
    options.read(true);
    set_no_follow(&mut options);
    let mut file = options
        .open(path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    validate_regular_handle(&file, path)?;
    let length = file
        .metadata()
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?
        .len();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; IO_BUFFER_BYTES];
    loop {
        if let Some(shared) = shared {
            shared.checkpoint()?;
        }
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Could not hash {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        if let Some(shared) = shared {
            shared.add_bytes(read as u64);
        }
    }
    let actual_length = file
        .metadata()
        .map_err(|error| {
            format!(
                "Could not inspect {} after hashing: {error}",
                path.display()
            )
        })?
        .len();
    if actual_length != length {
        return Err(format!(
            "File changed while it was being hashed: {}",
            path.display()
        ));
    }
    Ok((length, format!("{:x}", hasher.finalize())))
}

pub(super) fn hash_allowing_transaction_link(path: &Path) -> Result<(u64, String), String> {
    hash_allowing_transaction_link_inner(path, None)
}

pub(super) fn hash_allowing_transaction_link_controlled(
    path: &Path,
    shared: &RepairShared,
) -> Result<(u64, String), String> {
    hash_allowing_transaction_link_inner(path, Some(shared))
}

pub(super) fn hash_allowing_transaction_link_inner(
    path: &Path,
    shared: Option<&RepairShared>,
) -> Result<(u64, String), String> {
    let mut file = open_regular_for_identity(path)?;
    let length = file
        .metadata()
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?
        .len();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; IO_BUFFER_BYTES];
    loop {
        if let Some(shared) = shared {
            shared.checkpoint()?;
        }
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Could not hash {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        if let Some(shared) = shared {
            shared.add_bytes(read as u64);
        }
    }
    let after = file
        .metadata()
        .map_err(|error| {
            format!(
                "Could not inspect {} after hashing: {error}",
                path.display()
            )
        })?
        .len();
    if after != length {
        return Err(format!("File changed while hashing: {}", path.display()));
    }
    Ok((length, format!("{:x}", hasher.finalize())))
}
