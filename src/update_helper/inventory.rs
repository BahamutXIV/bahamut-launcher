//! Managed-file inventory reads and comparisons.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io;
use std::path::Path;

use crate::release::{FileOwnership, InventoryFile, ReleaseInventory, ReleaseMetadata};

use super::fsutil::{ensure_safe_inventory_path, safe_join};
use super::{FileIdentity, Result, invalid};

pub(super) fn same_file_identity(actual: &InventoryFile, expected: &FileIdentity) -> bool {
    actual.length == expected.length && actual.sha256 == expected.sha256
}

pub(super) fn managed_files(metadata: &ReleaseMetadata) -> Result<Vec<InventoryFile>> {
    let ReleaseInventory::Launcher { files } = &metadata.inventory else {
        return Err(invalid("Launcher inventory is required."));
    };
    Ok(files
        .iter()
        .filter(|file| file.ownership == FileOwnership::Managed)
        .cloned()
        .collect())
}

pub(super) fn validate_transition_destinations(
    target: &Path,
    new_metadata: &ReleaseMetadata,
    old_files: &[InventoryFile],
) -> Result<()> {
    let old_managed_paths = old_files
        .iter()
        .map(|file| file.path.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    for file in managed_files(new_metadata)? {
        let folded = file.path.to_ascii_lowercase();
        ensure_safe_inventory_path(target, &file.path, false)?;
        let path = safe_join(target, &file.path)?;
        if !old_managed_paths.contains(&folded) && inspect_file(&path)?.is_some() {
            return Err(invalid(
                "A new managed path is already occupied; preserving the existing file.",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_installed_inventory(
    target: &Path,
    metadata: &ReleaseMetadata,
) -> Result<()> {
    validate_tree_inventory(target, &managed_files(metadata)?)
}

pub(super) fn validate_tree_inventory(root: &Path, files: &[InventoryFile]) -> Result<()> {
    for file in files {
        ensure_safe_inventory_path(root, &file.path, false)?;
        let path = safe_join(root, &file.path)?;
        ensure_file_matches(&path, file)?;
    }
    Ok(())
}

pub(super) fn validate_old_managed_files(
    target: &Path,
    old_files: &[InventoryFile],
    present: &BTreeSet<&str>,
) -> Result<()> {
    for file in old_files {
        ensure_safe_inventory_path(target, &file.path, false)?;
        let path = safe_join(target, &file.path)?;
        if present.contains(file.path.as_str()) {
            ensure_file_matches(&path, file)?;
        } else if inspect_file(&path)?.is_some() {
            return Err(invalid("Rollback found an unexpected prior managed file."));
        }
    }
    Ok(())
}

pub(super) fn inspect_file(path: &Path) -> Result<Option<InventoryFile>> {
    match fs::symlink_metadata(path) {
        Ok(details) => {
            if !details.is_file() || crate::atomic_fs::is_link_or_reparse(&details) {
                return Err(invalid("Managed path is not a regular file."));
            }
            let (length, sha256) = crate::digest::sha256_reader(&mut File::open(path)?)?;
            Ok(Some(InventoryFile {
                path: String::new(),
                length,
                sha256,
                ownership: FileOwnership::Managed,
            }))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn ensure_file_matches(path: &Path, expected: &InventoryFile) -> Result<()> {
    let actual = inspect_file(path)?
        .ok_or_else(|| invalid(&format!("Managed file is missing: {}", expected.path)))?;
    if actual.length != expected.length || actual.sha256 != expected.sha256 {
        return Err(invalid(&format!(
            "Managed file differs from its signed inventory: {}",
            expected.path
        )));
    }
    Ok(())
}

pub(super) fn same_bytes(actual: &InventoryFile, expected: &InventoryFile) -> bool {
    actual.length == expected.length && actual.sha256 == expected.sha256
}
