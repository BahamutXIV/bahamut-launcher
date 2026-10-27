use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::content::INSTALL_RECEIPT_FILE as RECEIPT_NAME;
use crate::content::manifest::BasePackage;
use crate::content::manifest::InstallFile;
use crate::content::manifest::validate_relative_path;
use crate::content::worker::InstallShared;
use crate::version::FFXIV_BOOT_VERSION;
use crate::version::FFXIV_GAME_VERSION;

use super::IO_BUFFER_BYTES;
use super::archive::expected_directories;
use super::paths::{check_reparse, validate_tree_has_no_reparse_points};
use super::receipt::{destination_has_receipt, read_receipt};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FinalContentCheck {
    Hash,
    ExtractionVerified,
}

fn extraction_identities(package: &BasePackage) -> HashMap<String, (u64, String)> {
    let mut identities: HashMap<String, (u64, String)> = package
        .archives
        .iter()
        .flat_map(|archive| archive.files.iter())
        .map(|file| {
            (
                file.path.to_ascii_lowercase(),
                (file.length, file.sha256.to_ascii_lowercase()),
            )
        })
        .collect();
    for (name, contents) in [
        ("boot.ver", FFXIV_BOOT_VERSION),
        ("game.ver", FFXIV_GAME_VERSION),
    ] {
        identities.insert(
            name.to_owned(),
            (
                contents.len() as u64,
                crate::digest::sha256_hex(contents.as_bytes()),
            ),
        );
    }
    identities
}

pub(super) fn validate_final_tree(
    root: &Path,
    package: &BasePackage,
    identity: &str,
    shared: &InstallShared,
    receipt_required: bool,
    content_check: FinalContentCheck,
) -> Result<(), String> {
    validate_tree_has_no_reparse_points(root)?;
    if receipt_required && read_receipt(root)? != identity {
        return Err("Staging receipt does not match the selected package.".into());
    }
    if !receipt_required && destination_has_receipt(root)? {
        return Err("A staged installation receipt is present in the destination.".into());
    }
    let expected: HashMap<String, &InstallFile> = package
        .final_files
        .iter()
        .map(|file| (file.path.to_ascii_lowercase(), file))
        .collect();
    let verified = (content_check == FinalContentCheck::ExtractionVerified)
        .then(|| extraction_identities(package));
    let mut expected_dirs = expected_directories(expected.values().map(|file| file.path.as_str()));
    let empty_dirs: HashSet<String> = package
        .archives
        .iter()
        .flat_map(|archive| archive.empty_directories.iter())
        .map(|directory| directory.to_ascii_lowercase())
        .collect();
    expected_dirs.extend(expected_directories(empty_dirs.iter().map(String::as_str)));
    expected_dirs.extend(empty_dirs.iter().cloned());
    let mut found_empty_dirs = HashSet::new();
    let mut found = HashSet::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, relative)) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("Could not validate staged files: {error}"))?
        {
            if shared.wait_if_paused() {
                return Err("Installation was cancelled.".into());
            }
            let entry =
                entry.map_err(|error| format!("Could not validate staged files: {error}"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            if relative.is_empty() && name == RECEIPT_NAME && receipt_required {
                continue;
            }
            validate_relative_path(&child)?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("Could not inspect staged entry: {error}"))?;
            check_reparse(&path, &metadata)?;
            let folded = child.to_ascii_lowercase();
            if metadata.is_dir() {
                if !expected_dirs.contains(&folded) {
                    return Err(format!("Unexpected staged directory: {child}"));
                }
                if empty_dirs.contains(&folded) {
                    found_empty_dirs.insert(folded.clone());
                }
                pending.push((path, child));
            } else if metadata.is_file() {
                let spec = expected
                    .get(&folded)
                    .ok_or_else(|| format!("Unexpected staged file: {child}"))?;
                if !found.insert(folded.clone()) {
                    return Err(format!("Duplicate staged destination: {child}"));
                }
                let valid = match &verified {
                    Some(sources) => {
                        metadata.len() == spec.length
                            && sources.get(&folded).is_some_and(|(length, sha256)| {
                                *length == spec.length && spec.sha256.eq_ignore_ascii_case(sha256)
                            })
                    }
                    None => hash_file(shared, &path, &spec.path, spec.length)?
                        .eq_ignore_ascii_case(&spec.sha256),
                };
                if !valid {
                    return Err(format!(
                        "Final install file failed validation: {}",
                        spec.path
                    ));
                }
                shared
                    .file_idx
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            } else {
                return Err(format!("Unexpected special file in installation: {child}"));
            }
        }
    }
    if found.len() != expected.len() {
        let missing = expected
            .keys()
            .find(|name| !found.contains(*name))
            .cloned()
            .unwrap_or_else(|| "unknown file".into());
        return Err(format!("Final installation is missing: {missing}"));
    }
    if found_empty_dirs.len() != empty_dirs.len() {
        return Err("Final installation is missing an empty source directory.".into());
    }
    Ok(())
}

pub(super) fn hash_file(
    shared: &InstallShared,
    path: &Path,
    label: &str,
    expected_length: u64,
) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("Could not open final install file {label}: {error}"))?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; IO_BUFFER_BYTES];
    while total < expected_length {
        if shared.wait_if_paused() {
            return Err("Installation was cancelled.".into());
        }
        let remaining = expected_length - total;
        let limit = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = file
            .read(&mut buffer[..limit])
            .map_err(|error| format!("Could not validate final install file {label}: {error}"))?;
        if read == 0 {
            return Err(format!(
                "Final install file ended before its manifest length: {label}"
            ));
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
    }
    let mut excess = [0_u8; 1];
    if file
        .read(&mut excess)
        .map_err(|error| format!("Could not check final install file length {label}: {error}"))?
        != 0
    {
        return Err(format!(
            "Final install file exceeded its manifest length: {label}"
        ));
    }
    Ok(format!("{:x}", hasher.finalize()))
}
