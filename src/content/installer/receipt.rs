use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::content::INSTALL_RECEIPT_FILE as RECEIPT_NAME;
use crate::content::manifest::validate_hash;

use super::paths::{check_reparse, resolve_path, validate_tree_has_no_reparse_points};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallReceipt {
    schema_version: u32,
    manifest_sha256: String,
}

pub(super) fn validate_destination_state(destination: &Path, identity: &str) -> Result<(), String> {
    let resolved = resolve_path(destination)?;
    if !resolved.exists {
        return Ok(());
    }
    if !fs::symlink_metadata(&resolved.path)
        .map_err(|error| format!("Could not inspect the destination: {error}"))?
        .is_dir()
    {
        return Err("The selected destination is not a directory.".into());
    }
    let entries = fs::read_dir(&resolved.path)
        .map_err(|error| format!("Could not inspect the destination: {error}"))?;
    let mut found_receipt = false;
    for entry in entries {
        let entry = entry.map_err(|error| format!("Could not inspect the destination: {error}"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("Could not inspect the destination: {error}"))?;
        check_reparse(&entry.path(), &metadata)?;
        if entry.file_name().to_string_lossy() == RECEIPT_NAME {
            found_receipt = true;
            continue;
        }
        return Err("The selected destination is not empty.".into());
    }
    if found_receipt && read_receipt(&resolved.path)? == identity {
        return Ok(());
    }
    if found_receipt {
        return Err("The selected destination contains an unowned install receipt.".into());
    }
    Ok(())
}

pub(super) fn validate_quote_destination_state(
    destination: &Path,
    identity: &str,
) -> Result<(), String> {
    let resolved = resolve_path(destination)?;
    if !resolved.exists || !destination_has_receipt(&resolved.path)? {
        return validate_destination_state(destination, identity);
    }
    if read_receipt(&resolved.path)? != identity {
        return Err("The selected destination belongs to another package.".into());
    }
    validate_tree_has_no_reparse_points(&resolved.path)
}

pub(super) fn destination_has_receipt(destination: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(destination.join(RECEIPT_NAME)) {
        Ok(metadata) => {
            check_reparse(&destination.join(RECEIPT_NAME), &metadata)?;
            if !metadata.is_file() {
                return Err("The installation receipt is not a regular file.".into());
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "Could not inspect the installation receipt: {error}"
        )),
    }
}

pub(super) fn read_receipt(directory: &Path) -> Result<String, String> {
    let path = directory.join(RECEIPT_NAME);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("Could not read the installation receipt: {error}"))?;
    check_reparse(&path, &metadata)?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return Err("The installation receipt is invalid.".into());
    }
    let receipt: InstallReceipt = serde_json::from_reader(
        File::open(&path)
            .map_err(|error| format!("Could not read the installation receipt: {error}"))?,
    )
    .map_err(|error| format!("The installation receipt is invalid: {error}"))?;
    if receipt.schema_version != 1 {
        return Err("The installation receipt has an unsupported version.".into());
    }
    validate_hash(&receipt.manifest_sha256)?;
    Ok(receipt.manifest_sha256)
}

pub(super) fn write_receipt(directory: &Path, identity: &str) -> Result<(), String> {
    let receipt = InstallReceipt {
        schema_version: 1,
        manifest_sha256: identity.to_owned(),
    };
    let bytes = serde_json::to_vec(&receipt).expect("InstallReceipt is infallibly serializable");
    let path = directory.join(RECEIPT_NAME);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("Could not create the installation receipt: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Could not persist the installation receipt: {error}"))
}

pub(super) fn remove_receipt(directory: &Path, identity: &str) -> Result<(), String> {
    let path = directory.join(RECEIPT_NAME);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("Could not inspect the installation receipt: {error}"))?;
    check_reparse(&path, &metadata)?;
    if !metadata.is_file() || read_receipt(directory)? != identity {
        return Err("The installation receipt changed before removal.".into());
    }
    fs::remove_file(path)
        .map_err(|error| format!("Could not remove the installation receipt: {error}"))
}

pub(super) fn prepare_owned_stage(stage: &Path, identity: &str) -> Result<PathBuf, String> {
    let stage = resolve_path(stage)?;
    if stage.exists {
        if !fs::symlink_metadata(&stage.path)
            .map_err(|error| format!("Could not inspect staged installation: {error}"))?
            .is_dir()
        {
            return Err("The staging path is not a directory.".into());
        }
        if read_receipt(&stage.path)? != identity {
            return Err("The existing staging directory belongs to another package.".into());
        }
        validate_tree_has_no_reparse_points(&stage.path)?;
        fs::remove_dir_all(&stage.path)
            .map_err(|error| format!("Could not reset owned staging data: {error}"))?;
    }
    fs::create_dir(&stage.path)
        .map_err(|error| format!("Could not create same-volume staging: {error}"))?;
    if let Err(error) = write_receipt(&stage.path, identity) {
        validate_tree_has_no_reparse_points(&stage.path)?;
        let _ = fs::remove_dir(&stage.path);
        return Err(error);
    }
    Ok(stage.path)
}
