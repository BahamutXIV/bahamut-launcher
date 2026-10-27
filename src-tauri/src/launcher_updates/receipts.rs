//! Trusted release receipts, receipt pointers, and accepted installed state.

use std::path::{Path, PathBuf};

use bahamut_launcher::digest::{is_lower_sha256_hex, sha256_hex};
use bahamut_launcher::release::{self, VerifiedRelease};
use serde::{Deserialize, Serialize};

use super::config::LauncherUpdateConfig;
use super::fs_io::{
    ensure_plain_relative_directory, publish_immutable, read_regular_file, regular_file_exists,
    write_json_replace,
};
use super::verify::{launcher_scope, verify_launcher_release};
use super::{ARTIFACT_CACHE, INSTALLED_FILE, STATE_FILE};

const RECEIPT_DIR: &str = "config/launcher-release-receipts";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ReceiptPointer {
    pub(super) schema_version: u32,
    pub(super) metadata_sha256: String,
}

#[derive(Clone, Debug)]
pub(super) struct Receipt {
    pub(super) verified: VerifiedRelease,
    pub(super) metadata: Vec<u8>,
    pub(super) signature: Vec<u8>,
}

pub(super) fn ensure_installed_receipt(
    root: &Path,
    config: &LauncherUpdateConfig,
    key: &[u8],
) -> Result<Receipt, String> {
    let state_path = root.join(STATE_FILE);
    let bootstrap = config.bootstrap_receipt(root, key)?;
    persist_receipt(
        root,
        &bootstrap.metadata,
        &bootstrap.signature,
        &bootstrap.verified.metadata_sha256,
    )?;
    let state = release::accepted_state_summary(&state_path, launcher_scope());
    match state {
        Ok((_, Some(version))) => {
            let pointer = read_pointer(&root.join(INSTALLED_FILE), "installed launcher receipt")?
                .ok_or_else(|| {
                "The installed launcher receipt pointer is missing.".to_owned()
            })?;
            let installed = load_receipt(root, key, &pointer.metadata_sha256)?;
            if installed.verified.metadata.version != version {
                return Err(
                    "Installed launcher receipt does not match accepted release state.".into(),
                );
            }
            Ok(installed)
        }
        Ok((highest, None)) => {
            let installed =
                match read_pointer(&root.join(INSTALLED_FILE), "installed launcher receipt")? {
                    Some(pointer) => load_receipt(root, key, &pointer.metadata_sha256)?,
                    None if highest == bootstrap.verified.metadata.version => bootstrap.clone(),
                    None => {
                        return Err(
                            "Accepted launcher state has no installed release receipt.".into()
                        );
                    }
                };
            write_json_replace(
                &root.join(INSTALLED_FILE),
                &ReceiptPointer {
                    schema_version: 1,
                    metadata_sha256: installed.verified.metadata_sha256.clone(),
                },
            )?;
            release::record_installed_release(&state_path, &installed.verified).map_err(
                |error| format!("Could not record the installed launcher baseline: {error}"),
            )?;
            Ok(installed)
        }
        Err(_) => {
            release::bootstrap_accepted_state(
                &state_path,
                &bootstrap.verified,
                &bootstrap.verified.metadata.version,
            )
            .map_err(|error| {
                format!("Could not bootstrap accepted launcher release state: {error}")
            })?;
            write_json_replace(
                &root.join(INSTALLED_FILE),
                &ReceiptPointer {
                    schema_version: 1,
                    metadata_sha256: bootstrap.verified.metadata_sha256.clone(),
                },
            )?;
            release::record_installed_release(&state_path, &bootstrap.verified).map_err(
                |error| format!("Could not record the installed launcher baseline: {error}"),
            )?;
            Ok(bootstrap)
        }
    }
}

pub(super) fn persist_receipt(
    root: &Path,
    metadata: &[u8],
    signature: &[u8],
    hash: &str,
) -> Result<(), String> {
    ensure_plain_relative_directory(root, Path::new(RECEIPT_DIR))?;
    publish_immutable(&receipt_metadata_path(root, hash), metadata)?;
    publish_immutable(&receipt_signature_path(root, hash), signature)
}

pub(super) fn load_receipt(root: &Path, key: &[u8], hash: &str) -> Result<Receipt, String> {
    if !is_lower_sha256_hex(hash) {
        return Err("Launcher release receipt identity is invalid.".into());
    }
    let metadata = release::read_metadata_file(&receipt_metadata_path(root, hash))
        .map_err(|error| format!("Could not read trusted launcher metadata receipt: {error}"))?;
    if sha256_hex(&metadata) != hash {
        return Err("Launcher metadata receipt identity does not match.".into());
    }
    let signature = release::read_signature_file(&receipt_signature_path(root, hash))
        .map_err(|error| format!("Could not read trusted launcher signature receipt: {error}"))?;
    let verified = verify_launcher_release(&metadata, &signature, key)?;
    Ok(Receipt {
        verified,
        metadata,
        signature,
    })
}

pub(super) fn ensure_update_directories(root: &Path) -> Result<(), String> {
    ensure_plain_relative_directory(root, Path::new("config"))?;
    ensure_plain_relative_directory(root, Path::new(RECEIPT_DIR))?;
    ensure_plain_relative_directory(root, Path::new(ARTIFACT_CACHE))
}

pub(super) fn receipt_metadata_path(root: &Path, hash: &str) -> PathBuf {
    root.join(RECEIPT_DIR).join(format!("{hash}.json"))
}

pub(super) fn receipt_signature_path(root: &Path, hash: &str) -> PathBuf {
    root.join(RECEIPT_DIR).join(format!("{hash}.sig"))
}

pub(super) fn read_pointer(path: &Path, label: &str) -> Result<Option<ReceiptPointer>, String> {
    if !regular_file_exists(path, label)? {
        return Ok(None);
    }
    let bytes = read_regular_file(path, 16 * 1024, label)
        .map_err(|error| format!("Could not read {label}: {error}"))?;
    let pointer: ReceiptPointer =
        serde_json::from_slice(&bytes).map_err(|_| format!("{label} JSON is invalid."))?;
    if pointer.schema_version != 1 || !is_lower_sha256_hex(&pointer.metadata_sha256) {
        return Err(format!("{label} fields are invalid."));
    }
    Ok(Some(pointer))
}
