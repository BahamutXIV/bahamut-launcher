//! Release archive extraction and atomic file installation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path};

use zip::ZipArchive;

use crate::release::ReleaseMetadata;

use super::fsutil::{ensure_safe_parent, safe_join, unique_path};
use super::inventory::managed_files;
use super::{Result, UpdateError, invalid};

pub(super) fn extract_managed_payload(
    artifact: &Path,
    payload: &Path,
    metadata: &ReleaseMetadata,
) -> Result<()> {
    let expected = managed_files(metadata)?
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    let input = File::open(artifact)?;
    let mut archive = ZipArchive::new(input)
        .map_err(|error| invalid(&format!("Could not open verified release archive: {error}")))?;
    let mut seen = BTreeSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            invalid(&format!(
                "Verified release archive entry is invalid: {error}"
            ))
        })?;
        if entry.is_dir() || entry.name().ends_with('/') {
            continue;
        }
        let name = entry.name().to_owned();
        if !expected.contains_key(&name) {
            continue;
        }
        if !seen.insert(name.clone()) {
            return Err(invalid("Release archive repeats a managed path."));
        }
        let expected_file = &expected[&name];
        let output_path = safe_join(payload, &name)?;
        if let Some(parent) = output_path.parent() {
            create_safe_directories(payload, parent)?;
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)?;
        let copied = io::copy(&mut entry, &mut output)?;
        output.sync_all()?;
        if copied != expected_file.length {
            return Err(invalid("Extracted launcher file length changed."));
        }
    }
    if seen.len() != expected.len() {
        return Err(invalid(
            "Release archive is missing a managed launcher file.",
        ));
    }
    Ok(())
}

pub(super) fn install_file_atomically(
    transaction: &Path,
    target: &Path,
    source: &Path,
    destination: &Path,
) -> Result<()> {
    ensure_safe_parent(target, destination)?;
    let temporary = unique_path(transaction, ".bahamut-replace-");
    copy_new_file(source, &temporary)?;
    let result = crate::atomic_fs::rename_replace(&temporary, destination);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(UpdateError::from)
}

pub(super) fn copy_new_file(source: &Path, destination: &Path) -> Result<()> {
    let details = fs::symlink_metadata(source)?;
    if !details.is_file() || crate::atomic_fs::is_link_or_reparse(&details) {
        return Err(invalid("Transaction input is not a regular file."));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("Staged file has no parent directory."))?;
    fs::create_dir_all(parent)?;
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    crate::atomic_fs::sync_directory(parent)?;
    Ok(())
}

fn create_safe_directories(root: &Path, destination: &Path) -> Result<()> {
    if !destination.starts_with(root) {
        return Err(invalid("Managed directory escaped its staging root."));
    }
    let relative = destination
        .strip_prefix(root)
        .map_err(|_| invalid("Managed directory escaped its staging root."))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(invalid("Managed directory contains an unsafe component."));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(details) => {
                if !details.is_dir() || crate::atomic_fs::is_link_or_reparse(&details) {
                    return Err(invalid("Managed path contains a non-directory parent."));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&current)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
