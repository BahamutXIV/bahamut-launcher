//! Signed launcher release, installed inventory, and version checks.

use std::fs;
use std::io;
use std::path::{Component, Path};

use bahamut_launcher::atomic_fs::is_link_or_reparse;
use bahamut_launcher::release::{
    self, Channel, FileOwnership, Product, ReleaseInventory, ReleaseMetadata, ReleaseScope, Target,
    VerifiedRelease,
};
use bahamut_launcher::update_helper::OFFICIAL_OVERLAY_MANIFEST;

use super::UPDATE_HELPER_FILE;
use super::fs_io::{hash_file, safe_join};

const OFFICIAL_OVERLAY_ROOT: &str = "plugins/dats/bahamut-dats-overlay/";

pub(super) fn verify_launcher_release(
    metadata: &[u8],
    signature: &[u8],
    key: &[u8],
) -> Result<VerifiedRelease, String> {
    let verified = release::verify_metadata(metadata, signature, key, launcher_scope(), None)
        .map_err(|error| format!("Signed launcher release verification failed: {error}"))?;
    let ReleaseInventory::Launcher { files } = &verified.metadata.inventory else {
        return Err("Signed launcher release inventory is missing.".into());
    };
    if !files
        .iter()
        .any(|file| file.path == "bahamut-launcher.exe" && file.ownership == FileOwnership::Managed)
        || !files
            .iter()
            .any(|file| file.path == UPDATE_HELPER_FILE && file.ownership == FileOwnership::Managed)
    {
        return Err(
            "Signed Windows launcher release must manage bahamut-launcher.exe and bahamut-update-helper.exe in a ZIP artifact.".into(),
        );
    }
    Ok(verified)
}

pub(super) fn verify_new_launcher_offer(offer: &VerifiedRelease) -> Result<(), String> {
    let ReleaseInventory::Launcher { files } = &offer.metadata.inventory else {
        return Err("Signed launcher release inventory is missing.".into());
    };
    let mut has_managed_manifest = false;
    for file in files {
        if !file.path.starts_with(OFFICIAL_OVERLAY_ROOT) {
            continue;
        }
        if file.ownership != FileOwnership::Managed {
            return Err("New launcher offers must manage every official DAT overlay file.".into());
        }
        if file.path == OFFICIAL_OVERLAY_MANIFEST {
            has_managed_manifest = true;
        }
    }
    if !has_managed_manifest {
        return Err("New launcher offers must manage the official DAT overlay manifest.".into());
    }
    Ok(())
}

fn inspect_launcher_file(
    root: &Path,
    path: &Path,
    expected: &bahamut_launcher::release::InventoryFile,
) -> Result<Option<&'static str>, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Managed launcher file has no parent.".to_owned())?;
    let relative_parent = parent
        .strip_prefix(root)
        .map_err(|_| "Managed launcher file escaped its root.".to_owned())?;
    let mut current = root.to_path_buf();
    for component in relative_parent.components() {
        let Component::Normal(name) = component else {
            return Ok(Some("unresolved"));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(details) if details.is_dir() && !is_link_or_reparse(&details) => {}
            Ok(_) => return Ok(Some("unresolved")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Some("missing"));
            }
            Err(_) => return Ok(Some("unresolved")),
        }
    }
    let details = match fs::symlink_metadata(path) {
        Ok(details) => details,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Some("missing")),
        Err(_) => return Ok(Some("unresolved")),
    };
    if !details.is_file() || is_link_or_reparse(&details) {
        return Ok(Some("unresolved"));
    }
    let (length, hash) = match hash_file(path) {
        Ok(identity) => identity,
        Err(_) => return Ok(Some("unresolved")),
    };
    if length != expected.length || hash != expected.sha256 {
        return Ok(Some("corrupt"));
    }
    Ok(None)
}

pub(super) fn verify_installed_inventory(
    root: &Path,
    metadata: &ReleaseMetadata,
) -> Result<(), String> {
    let ReleaseInventory::Launcher { files } = &metadata.inventory else {
        return Err("Signed launcher inventory is missing.".into());
    };
    for file in files
        .iter()
        .filter(|file| file.ownership == FileOwnership::Managed)
    {
        let path = safe_join(root, &file.path)?;
        if let Some(state) = inspect_launcher_file(root, &path, file)? {
            return Err(format!("Installed launcher file {} is {state}.", file.path));
        }
    }
    Ok(())
}

pub(super) fn launcher_scope() -> ReleaseScope {
    ReleaseScope {
        product: Product::Launcher,
        channel: Channel::Stable,
        target: Target::WindowsX86_64,
    }
}

pub(super) fn validate_version(value: &str) -> Result<(), String> {
    version_tuple(value).map(|_| ())
}

fn version_tuple(value: &str) -> Result<(u64, u64, u64), String> {
    let mut parts = value.split('.');
    let parse = |part: Option<&str>| -> Result<u64, String> {
        let part = part.ok_or_else(|| {
            "Launcher version must be a canonical stable semantic version.".to_owned()
        })?;
        let number = part.parse::<u64>().map_err(|_| {
            "Launcher version must be a canonical stable semantic version.".to_owned()
        })?;
        if number.to_string() != part {
            return Err("Launcher version must be a canonical stable semantic version.".into());
        }
        Ok(number)
    };
    let version = (
        parse(parts.next())?,
        parse(parts.next())?,
        parse(parts.next())?,
    );
    if parts.next().is_some() {
        return Err("Launcher version must be a canonical stable semantic version.".into());
    }
    Ok(version)
}

pub(super) fn compare_versions(left: &str, right: &str) -> Result<std::cmp::Ordering, String> {
    Ok(version_tuple(left)?.cmp(&version_tuple(right)?))
}
