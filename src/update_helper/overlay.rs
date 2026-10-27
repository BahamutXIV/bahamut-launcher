//! Legacy official overlay manifest transition.

use std::fs;
use std::io;
use std::path::Path;

use crate::release::{FileOwnership, InventoryFile, ReleaseInventory, ReleaseMetadata};

use super::fsutil::ensure_safe_inventory_path;
use super::inventory::{inspect_file, managed_files};
use super::{FileIdentity, OFFICIAL_OVERLAY_MANIFEST, Result, Snapshot, invalid};

pub(super) fn transition_old_files(
    old_metadata: &ReleaseMetadata,
    new_metadata: &ReleaseMetadata,
) -> Result<Vec<InventoryFile>> {
    let mut files = managed_files(old_metadata)?;
    let new_managed = managed_files(new_metadata)?;
    let ReleaseInventory::Launcher {
        files: old_inventory,
    } = &old_metadata.inventory
    else {
        return Err(invalid("Launcher inventory is required."));
    };
    if let Some(seed) = old_inventory.iter().find(|file| {
        file.path.eq_ignore_ascii_case(OFFICIAL_OVERLAY_MANIFEST)
            && file.ownership == FileOwnership::Seed
            && new_managed
                .iter()
                .any(|new| new.path.eq_ignore_ascii_case(&file.path))
    }) {
        files.push(seed.clone());
    }
    Ok(files)
}

pub(super) fn adopt_legacy_overlay_manifest(
    target: &Path,
    old_files: &mut [InventoryFile],
) -> Result<Option<FileIdentity>> {
    let Some(seed) = old_files.iter_mut().find(|file| {
        file.path == OFFICIAL_OVERLAY_MANIFEST && file.ownership == FileOwnership::Seed
    }) else {
        return Ok(None);
    };
    let Some(actual) = inspect_file(&target.join(OFFICIAL_OVERLAY_MANIFEST))? else {
        return Ok(None);
    };
    seed.length = actual.length;
    seed.sha256 = actual.sha256.clone();
    Ok(Some(FileIdentity {
        path: OFFICIAL_OVERLAY_MANIFEST.into(),
        length: actual.length,
        sha256: actual.sha256,
    }))
}

pub(super) fn restore_legacy_overlay_identity(
    snapshot: &Snapshot,
    old_files: &mut [InventoryFile],
    new_files: &[InventoryFile],
) -> Result<()> {
    let Some(identity) = snapshot.previous_managed_files.first() else {
        return Ok(());
    };
    if snapshot.previous_managed_files.len() != 1
        || identity.path != OFFICIAL_OVERLAY_MANIFEST
        || !crate::digest::is_lower_sha256_hex(&identity.sha256)
        || !snapshot
            .present_old_managed
            .iter()
            .any(|path| path == OFFICIAL_OVERLAY_MANIFEST)
        || !new_files
            .iter()
            .any(|file| file.path == OFFICIAL_OVERLAY_MANIFEST)
    {
        return Err(invalid("Legacy overlay snapshot identity is invalid."));
    }
    let seed = old_files
        .iter_mut()
        .find(|file| {
            file.path == OFFICIAL_OVERLAY_MANIFEST && file.ownership == FileOwnership::Seed
        })
        .ok_or_else(|| invalid("Legacy overlay snapshot has no signed seed."))?;
    seed.length = identity.length;
    seed.sha256 = identity.sha256.clone();
    Ok(())
}

pub(super) fn reject_legacy_overlay_extras(
    target: &Path,
    old_metadata: &ReleaseMetadata,
    new_metadata: &ReleaseMetadata,
) -> Result<()> {
    let ReleaseInventory::Launcher { files } = &old_metadata.inventory else {
        return Err(invalid("Launcher inventory is required."));
    };
    let has_legacy_seed = files.iter().any(|file| {
        file.path == OFFICIAL_OVERLAY_MANIFEST && file.ownership == FileOwnership::Seed
    });
    let adopts_overlay = managed_files(new_metadata)?
        .iter()
        .any(|file| file.path == OFFICIAL_OVERLAY_MANIFEST);
    if !has_legacy_seed || !adopts_overlay {
        return Ok(());
    }
    ensure_safe_inventory_path(target, OFFICIAL_OVERLAY_MANIFEST, false)?;
    let package = target.join("plugins/dats/bahamut-dats-overlay");
    let entries = match fs::read_dir(&package) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let details = fs::symlink_metadata(entry.path())?;
        if entry.file_name() != "overlay.toml"
            || !details.is_file()
            || crate::atomic_fs::is_link_or_reparse(&details)
        {
            return Err(invalid(
                "The legacy official overlay has extra files. Remove those files from plugins/dats/bahamut-dats-overlay (leave overlay.toml), or extract the new launcher into an empty folder, then retry.",
            ));
        }
    }
    Ok(())
}
