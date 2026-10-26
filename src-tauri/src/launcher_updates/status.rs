//! Launcher update status types and the local and network-checked status entry points.

use std::path::Path;

use bahamut_launcher::release;
use bahamut_launcher::update_helper::POINTER_FILE;
use serde::Serialize;

use super::config::LauncherUpdateConfig;
use super::fetch::{cached_launcher_artifact, fetch_signed_release};
use super::fs_io::{regular_file_exists, write_json_replace};
use super::locks::UpdateLock;
use super::pending::{read_pending, reject_pending_transaction};
use super::receipts::{
    ReceiptPointer, ensure_installed_receipt, load_receipt, persist_receipt, read_pointer,
};
use super::verify::{
    compare_versions, launcher_scope, verify_launcher_release, verify_new_launcher_offer,
};
use super::{INSTALLED_FILE, OFFER_FILE, STATE_FILE};

/// Wire values consumed by the launcher UI; renaming a variant changes the IPC contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LauncherUpdateState {
    Blocked,
    Ready,
    UpdateAvailable,
    UpdateDownloaded,
    Current,
    Handoff,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LauncherUpdateStatus {
    pub(crate) state: LauncherUpdateState,
    pub(crate) message: String,
    pub(crate) installed_version: Option<String>,
    pub(crate) offered_version: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LauncherUpdateResult {
    pub(crate) state: LauncherUpdateState,
    pub(crate) message: String,
}

/// Return local-only update state. This function never performs network I/O.
pub(crate) fn get_launcher_update_status(root: &Path) -> LauncherUpdateStatus {
    match local_status(root) {
        Ok(status) => status,
        Err(message) => LauncherUpdateStatus {
            state: LauncherUpdateState::Blocked,
            message,
            installed_version: None,
            offered_version: None,
        },
    }
}

/// Fetch and verify signed release metadata without downloading its artifact.
pub(crate) fn check_launcher_update(root: &Path) -> Result<LauncherUpdateStatus, String> {
    let config = LauncherUpdateConfig::load(root)?;
    let _lock = UpdateLock::acquire(root)?;
    reject_pending_transaction(root)?;
    let key = config.public_key(root)?;
    let installed = ensure_installed_receipt(root, &config, &key)?;
    let (metadata, signature) = fetch_signed_release(&config)?;
    let offer = verify_launcher_release(&metadata, &signature, &key)?;
    if offer.metadata_sha256 == installed.verified.metadata_sha256 {
        release::accept_offer(&root.join(STATE_FILE), &offer)
            .map_err(|error| format!("Launcher release offer was rejected: {error}"))?;
        persist_receipt(root, &metadata, &signature, &offer.metadata_sha256)?;
        return Ok(LauncherUpdateStatus {
            state: LauncherUpdateState::Current,
            message: format!(
                "Bahamut Launcher version {} is already installed.",
                offer.metadata.version
            ),
            installed_version: Some(installed.verified.metadata.version),
            offered_version: None,
        });
    }
    if compare_versions(
        &offer.metadata.version,
        &installed.verified.metadata.version,
    )? != std::cmp::Ordering::Greater
    {
        return Err(format!(
            "Launcher downgrade rejected: installed version {} is newer than offered version {}.",
            installed.verified.metadata.version, offer.metadata.version
        ));
    }
    verify_new_launcher_offer(&offer)?;
    release::accept_offer(&root.join(STATE_FILE), &offer)
        .map_err(|error| format!("Launcher release offer was rejected: {error}"))?;
    persist_receipt(root, &metadata, &signature, &offer.metadata_sha256)?;
    write_json_replace(
        &root.join(OFFER_FILE),
        &ReceiptPointer {
            schema_version: 1,
            metadata_sha256: offer.metadata_sha256.clone(),
        },
    )?;
    Ok(LauncherUpdateStatus {
        state: LauncherUpdateState::UpdateAvailable,
        message: format!(
            "Bahamut Launcher version {} is available. Choose Update Launcher to download and install it.",
            offer.metadata.version
        ),
        installed_version: Some(installed.verified.metadata.version),
        offered_version: Some(offer.metadata.version),
    })
}

fn local_status(root: &Path) -> Result<LauncherUpdateStatus, String> {
    let config = LauncherUpdateConfig::load(root)?;
    if regular_file_exists(&root.join(POINTER_FILE), "helper transaction pointer")?
        || read_pending(root)?.is_some()
    {
        return Err("A launcher update handoff is pending. Restart to confirm the update or recover the previous release.".into());
    }
    let key = config.public_key(root)?;
    let bootstrap = config.bootstrap_receipt(root, &key)?;
    let state_path = root.join(STATE_FILE);
    let installed = match release::accepted_state_summary(&state_path, launcher_scope()) {
        Ok((_, Some(version))) => {
            let pointer = read_pointer(&root.join(INSTALLED_FILE), "installed launcher receipt")?
                .ok_or_else(|| {
                "The installed launcher receipt pointer is missing.".to_owned()
            })?;
            let receipt = load_receipt(root, &key, &pointer.metadata_sha256)?;
            if receipt.verified.metadata.version != version {
                return Err(
                    "Installed launcher receipt does not match accepted release state.".into(),
                );
            }
            Some(receipt)
        }
        Ok((_, None)) => Some(bootstrap),
        Err(_) if !regular_file_exists(&state_path, "accepted launcher state")? => Some(bootstrap),
        Err(error) => {
            return Err(format!(
                "Accepted launcher release state is unavailable: {error}"
            ));
        }
    };
    let offer = match read_pointer(&root.join(OFFER_FILE), "launcher update offer")? {
        Some(pointer) => Some(load_receipt(root, &key, &pointer.metadata_sha256)?),
        None => None,
    };
    let installed_version = installed
        .as_ref()
        .map(|receipt| receipt.verified.metadata.version.clone());
    let offered_version = offer
        .as_ref()
        .map(|receipt| receipt.verified.metadata.version.clone());
    if let (Some(installed), Some(offer)) = (&installed, &offer)
        && compare_versions(
            &offer.verified.metadata.version,
            &installed.verified.metadata.version,
        )? == std::cmp::Ordering::Greater
    {
        let downloaded = cached_launcher_artifact(root, &offer.verified.metadata).is_some();
        return Ok(LauncherUpdateStatus {
            state: if downloaded {
                LauncherUpdateState::UpdateDownloaded
            } else {
                LauncherUpdateState::UpdateAvailable
            },
            message: if downloaded {
                format!(
                    "Bahamut Launcher version {} is downloaded and verified. Choose Update Launcher when other work is idle.",
                    offer.verified.metadata.version
                )
            } else {
                format!(
                    "Bahamut Launcher version {} is available. Choose Update Launcher to download and install it.",
                    offer.verified.metadata.version
                )
            },
            installed_version,
            offered_version,
        });
    }
    Ok(LauncherUpdateStatus {
        state: LauncherUpdateState::Ready,
        message: "A trusted stable launcher source is configured. Choose Check for updates to verify its signed release.".into(),
        installed_version,
        offered_version: None,
    })
}
