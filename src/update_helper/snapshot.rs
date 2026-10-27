//! Pre-update backup capture and repair snapshot identities.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::release::InventoryFile;

use super::fsutil::{ensure_safe_inventory_path, safe_join};
use super::inventory::{ensure_file_matches, inspect_file, same_bytes};
use super::payload::copy_new_file;
use super::{FileIdentity, JOURNAL_SCHEMA, Result, Snapshot, TransactionMode, invalid};

pub(super) fn repair_snapshot_identity_map(
    snapshot: &Snapshot,
    expected_files: &[InventoryFile],
) -> Result<BTreeMap<String, FileIdentity>> {
    let present = snapshot
        .present_old_managed
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected_paths = expected_files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    if present.len() != snapshot.present_old_managed.len()
        || present.iter().any(|path| !expected_paths.contains(path))
    {
        return Err(invalid("Repair snapshot names an unexpected managed path."));
    }

    let mut identities = BTreeMap::new();
    for identity in &snapshot.previous_managed_files {
        if !present.contains(identity.path.as_str())
            || !crate::digest::is_lower_sha256_hex(&identity.sha256)
            || identities
                .insert(identity.path.clone(), identity.clone())
                .is_some()
        {
            return Err(invalid("Repair snapshot file identities are invalid."));
        }
    }
    if identities.len() != present.len() {
        return Err(invalid(
            "Repair snapshot does not identify every previous managed file.",
        ));
    }
    Ok(identities)
}

pub(super) fn capture_previous_files(
    target: &Path,
    backup_root: &Path,
    old_files: &[InventoryFile],
    mode: TransactionMode,
) -> Result<Snapshot> {
    let mut present = Vec::new();
    let mut previous_managed_files = Vec::new();
    for file in old_files {
        ensure_safe_inventory_path(target, &file.path, false)?;
        let path = safe_join(target, &file.path)?;
        if inspect_file(&path)?.is_some() {
            if mode == TransactionMode::Update {
                ensure_file_matches(&path, file)?;
            }
            let backup = safe_join(backup_root, &file.path)?;
            ensure_safe_inventory_path(backup_root, &file.path, true)?;
            copy_new_file(&path, &backup)?;
            present.push(file.path.clone());
            if mode == TransactionMode::Repair {
                let backup_identity = inspect_file(&backup)?
                    .ok_or_else(|| invalid("Repair backup disappeared while being captured."))?;
                let current_identity = inspect_file(&path)?.ok_or_else(|| {
                    invalid("Managed file changed while repair backup was captured.")
                })?;
                if !same_bytes(&backup_identity, &current_identity) {
                    return Err(invalid(
                        "Managed file changed while repair backup was captured.",
                    ));
                }
                previous_managed_files.push(FileIdentity {
                    path: file.path.clone(),
                    length: backup_identity.length,
                    sha256: backup_identity.sha256,
                });
            }
        }
    }
    Ok(Snapshot {
        schema_version: JOURNAL_SCHEMA,
        present_old_managed: present,
        previous_managed_files,
    })
}
