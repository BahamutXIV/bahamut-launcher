//! Pending helper transaction receipt, confirmation, and recovered-launch finalization.

use std::path::{Path, PathBuf};
use std::process::Command;

use bahamut_launcher::digest::is_lower_sha256_hex;
use bahamut_launcher::release;
use bahamut_launcher::update_helper::{PENDING_FILE, POINTER_FILE};
use serde::{Deserialize, Serialize};

use super::config::LauncherUpdateConfig;
use super::fs_io::{
    checked_root, ensure_existing_directory, normalize_path, read_regular_file,
    regular_file_exists, remove_empty_directory, remove_regular_file_if_present,
    write_json_replace,
};
use super::helper::staged_helper_path;
use super::locks::{UpdateLock, acquire_idle_helper_lock};
use super::receipts::{ReceiptPointer, load_receipt};
use super::verify::verify_installed_inventory;
use super::{INSTALLED_FILE, STATE_FILE, UPDATE_HELPER_FILE};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PendingUpdate {
    pub(super) schema_version: u32,
    pub(super) target: PathBuf,
    pub(super) stage: PathBuf,
    pub(super) transaction: PathBuf,
    pub(super) operation: HelperOperation,
    #[serde(default)]
    pub(super) helper_pid: Option<u32>,
    pub(super) previous_metadata_sha256: String,
    pub(super) new_metadata_sha256: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum HelperOperation {
    Update,
    Repair,
}

impl HelperOperation {
    pub(super) fn command(self) -> &'static str {
        match self {
            Self::Update => "apply",
            Self::Repair => "repair",
        }
    }

    pub(super) fn transaction_name(self) -> &'static str {
        match self {
            Self::Update => "bahamut-update",
            Self::Repair => "bahamut-repair",
        }
    }
}

/// Confirm the signed update passed on this launcher's helper-added argument.
pub(crate) fn confirm_pending_launcher_update(root: &Path) -> Result<(), String> {
    let transaction = update_transaction_argument()
        .ok_or_else(|| "No helper update transaction was supplied to this launch.".to_owned())?;
    let target = checked_root(root)?;
    let _lock = UpdateLock::acquire(&target)?;
    confirm_pending_launcher_update_locked(&target, &transaction)
}

pub(super) fn confirm_pending_launcher_update_locked(
    target: &Path,
    transaction: &Path,
) -> Result<(), String> {
    let helper_lock = acquire_idle_helper_lock(target)?;
    let pending =
        read_pending(target)?.ok_or_else(|| "The helper update receipt is missing.".to_owned())?;
    ensure_pending_matches_target(&pending, target)?;
    if normalize_path(&pending.transaction)? != normalize_path(transaction)? {
        return Err(
            "The helper transaction argument does not match the persisted launcher update receipt."
                .into(),
        );
    }
    let config = LauncherUpdateConfig::load(target)?;
    let key = config.public_key(target)?;
    let updated = load_receipt(target, &key, &pending.new_metadata_sha256)?;
    verify_installed_inventory(target, &updated.verified.metadata)?;
    let helper = staged_helper_path(&pending.stage, &updated.verified.metadata)?;

    // Record the new release before confirmation. Recovery uses the helper's
    // durable commit phase to choose the installed receipt if confirmation fails.
    release::record_installed_release(&target.join(STATE_FILE), &updated.verified)
        .map_err(|error| format!("Could not record the confirmed launcher release: {error}"))?;
    write_json_replace(
        &target.join(INSTALLED_FILE),
        &ReceiptPointer {
            schema_version: 1,
            metadata_sha256: updated.verified.metadata_sha256.clone(),
        },
    )?;

    drop(helper_lock);
    let output = Command::new(helper)
        .current_dir(target)
        .arg("confirm")
        .arg("--target")
        .arg(target)
        .arg("--transaction")
        .arg(&pending.transaction)
        .arg("--public-key")
        .arg(config.key_path(target)?)
        .output()
        .map_err(|error| format!("Could not run launcher update confirmation: {error}"))?;
    if !output.status.success() {
        return Err(helper_error("Launcher update confirmation failed", &output));
    }
    remove_regular_file_if_present(&target.join(PENDING_FILE))?;
    remove_regular_file_if_present(&pending.stage.join(UPDATE_HELPER_FILE))?;
    remove_empty_directory(&pending.stage);
    Ok(())
}

pub(super) fn reject_pending_transaction(root: &Path) -> Result<(), String> {
    if regular_file_exists(&root.join(POINTER_FILE), "helper transaction pointer")? {
        return Err("A launcher update transaction is pending. Restart the launcher to confirm it or recover the previous version first.".into());
    }
    if read_pending(root)?.is_some() {
        return Err("A launcher update handoff is already pending. Restart the launcher to finish it before checking again.".into());
    }
    Ok(())
}

pub(super) fn read_pending(root: &Path) -> Result<Option<PendingUpdate>, String> {
    let path = root.join(PENDING_FILE);
    if !regular_file_exists(&path, "launcher update receipt")? {
        return Ok(None);
    }
    let bytes = read_regular_file(&path, 32 * 1024, "launcher update receipt")
        .map_err(|error| format!("Could not read launcher update receipt: {error}"))?;
    let pending: PendingUpdate = serde_json::from_slice(&bytes)
        .map_err(|_| "Launcher update receipt JSON is invalid.".to_owned())?;
    if pending.schema_version != 1
        || !is_lower_sha256_hex(&pending.previous_metadata_sha256)
        || !is_lower_sha256_hex(&pending.new_metadata_sha256)
        || pending.helper_pid == Some(0)
    {
        return Err("Launcher update receipt fields are invalid.".into());
    }
    Ok(Some(pending))
}

pub(super) fn ensure_pending_matches_target(
    pending: &PendingUpdate,
    target: &Path,
) -> Result<(), String> {
    let expected_transaction = pending.stage.join(format!(
        "{}-{}",
        pending.operation.transaction_name(),
        pending.new_metadata_sha256
    ));
    let expected_stage_parent = target
        .parent()
        .ok_or_else(|| "Launcher install directory has no parent.".to_owned())?;
    if normalize_path(&pending.target)? != normalize_path(target)?
        || pending.transaction.parent() != Some(pending.stage.as_path())
        || pending.stage.parent() != Some(expected_stage_parent)
        || pending.transaction != expected_transaction
        || (pending.operation == HelperOperation::Update
            && pending.previous_metadata_sha256 == pending.new_metadata_sha256)
        || (pending.operation == HelperOperation::Repair
            && pending.previous_metadata_sha256 != pending.new_metadata_sha256)
    {
        return Err("Launcher update receipt paths do not match the current installation.".into());
    }
    ensure_existing_directory(&pending.stage, "launcher update stage")
}

fn update_transaction_argument() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--update-transaction" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

pub(crate) fn has_update_transaction_argument() -> bool {
    update_transaction_argument().is_some()
}

fn update_recovered_transaction_argument() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--update-recovered" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

pub(crate) fn has_recovered_update_argument() -> bool {
    update_recovered_transaction_argument().is_some()
}

pub(crate) fn finalize_recovered_launcher_update(root: &Path) -> Result<(), String> {
    let transaction = update_recovered_transaction_argument()
        .ok_or_else(|| "No recovered launcher transaction was supplied.".to_owned())?;
    let target = checked_root(root)?;
    let _lock = UpdateLock::acquire(&target)?;
    if regular_file_exists(&target.join(POINTER_FILE), "helper transaction pointer")? {
        return Err("The recovered launcher transaction is still pending.".into());
    }
    let Some(pending) = read_pending(&target)? else {
        return Ok(());
    };
    ensure_pending_matches_target(&pending, &target)?;
    if normalize_path(&pending.transaction)? != normalize_path(&transaction)? {
        return Err(
            "The recovered transaction does not match the persisted launcher receipt.".into(),
        );
    }
    let config = LauncherUpdateConfig::load(&target)?;
    let key = config.public_key(&target)?;
    let replacement = load_receipt(&target, &key, &pending.new_metadata_sha256)?;
    let helper = staged_helper_path(&pending.stage, &replacement.verified.metadata)?;
    remove_regular_file_if_present(&target.join(PENDING_FILE))?;
    // The recovery helper may still have this staged executable open. Cleanup
    // is best effort; it must not prevent the restored launcher from opening.
    let _ = remove_regular_file_if_present(&helper);
    remove_empty_directory(&pending.stage);
    Ok(())
}

fn helper_error(label: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let detail = if stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    } else {
        stderr
    };
    if detail.is_empty() {
        format!("{label} (exit status {}).", output.status)
    } else {
        format!("{label}: {detail}")
    }
}
