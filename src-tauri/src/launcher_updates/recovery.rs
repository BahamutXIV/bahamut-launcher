//! Startup recovery of a failed pending helper transaction.

use std::path::Path;
use std::process::{Child, Command, Stdio};

use bahamut_launcher::release;
use bahamut_launcher::update_helper::{self, PENDING_FILE, POINTER_FILE};

use super::config::LauncherUpdateConfig;
use super::fs_io::{
    checked_root, regular_file_exists, remove_empty_directory, remove_regular_file_if_present,
    write_json_replace,
};
use super::helper::staged_helper_path;
use super::locks::{UpdateLock, acquire_idle_helper_lock};
use super::pending::{
    PendingUpdate, confirm_pending_launcher_update_locked, ensure_pending_matches_target,
    read_pending,
};
use super::receipts::{ReceiptPointer, load_receipt};
use super::verify::verify_installed_inventory;
use super::{INSTALLED_FILE, STATE_FILE, UPDATE_HELPER_FILE};

const POINTER_LABEL: &str = "helper transaction pointer";
const MISSING_PENDING: &str = "The pending launcher helper receipt is missing.";

/// Start helper recovery for a failed pending transaction; true means exit now.
pub(crate) fn recover_failed_launcher_update(
    root: &Path,
    force_rollback: bool,
) -> Result<bool, String> {
    let target = checked_root(root)?;
    let _lock = UpdateLock::acquire(&target)?;
    let mut helper_lock = Some(acquire_idle_helper_lock(&target)?);
    let mut pending = read_matching_pending(&target)?;
    let pointer_path = target.join(POINTER_FILE);
    if !regular_file_exists(&pointer_path, POINTER_LABEL)? {
        if let Some(pending) = pending {
            discard_unstarted_transaction(&target, &pending)?;
        }
        return Ok(false);
    }
    let config = LauncherUpdateConfig::load(&target)?;
    let key = config.public_key(&target)?;
    if !force_rollback
        && let Some(current_pending) = &pending
        && is_fully_installed(&target, &key, current_pending)
    {
        drop(helper_lock.take());
        match confirm_pending_launcher_update_locked(&target, &current_pending.transaction) {
            Ok(()) => return Ok(false),
            Err(error) => {
                tracing::warn!(%error, "could not confirm the fully installed launcher on startup");
            }
        }

        helper_lock = Some(acquire_idle_helper_lock(&target)?);
        pending = read_matching_pending(&target)?;
        if !regular_file_exists(&pointer_path, POINTER_LABEL)? {
            if let Some(pending) = pending {
                settle_finished_transaction(&target, &pending)?;
            }
            return Ok(false);
        }
    }
    let Some(mut pending) = pending else {
        return Err(MISSING_PENDING.into());
    };
    record_recovered_release(&target, &pending, &key)?;
    let replacement = load_receipt(&target, &key, &pending.new_metadata_sha256)?;
    let helper = staged_helper_path(&pending.stage, &replacement.verified.metadata)?;
    let mut child = spawn_recovery_helper(&helper, &target, &pending.transaction, &config)?;
    pending.helper_pid = Some(child.id());
    if let Err(error) = write_json_replace(&target.join(PENDING_FILE), &pending) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "Could not record the launcher recovery helper process: {error}"
        ));
    }
    tracing::info!(
        pid = child.id(),
        "launcher update recovery handed off to helper"
    );
    drop(helper_lock.take());
    Ok(true)
}

fn read_matching_pending(target: &Path) -> Result<Option<PendingUpdate>, String> {
    let pending = read_pending(target)?;
    if let Some(pending) = &pending {
        ensure_pending_matches_target(pending, target)?;
    }
    Ok(pending)
}

fn is_fully_installed(target: &Path, key: &[u8], pending: &PendingUpdate) -> bool {
    load_receipt(target, key, &pending.new_metadata_sha256)
        .is_ok_and(|updated| verify_installed_inventory(target, &updated.verified.metadata).is_ok())
}

/// The helper publishes its transaction pointer before changing any managed
/// file, so a pending receipt without one never touched the installation.
fn discard_unstarted_transaction(target: &Path, pending: &PendingUpdate) -> Result<(), String> {
    remove_regular_file_if_present(&target.join(PENDING_FILE))?;
    remove_regular_file_if_present(&pending.stage.join(UPDATE_HELPER_FILE))?;
    remove_empty_directory(&pending.stage);
    Ok(())
}

/// The pointer is gone after a failed confirmation: keep the recorded release
/// only when the new files are fully installed, otherwise restore the previous one.
fn settle_finished_transaction(target: &Path, pending: &PendingUpdate) -> Result<(), String> {
    let config = LauncherUpdateConfig::load(target)?;
    let key = config.public_key(target)?;
    if !is_fully_installed(target, &key, pending) {
        let previous = load_receipt(target, &key, &pending.previous_metadata_sha256)?;
        release::record_installed_release(&target.join(STATE_FILE), &previous.verified).map_err(
            |error| format!("Could not restore the previous accepted launcher release: {error}"),
        )?;
        write_json_replace(
            &target.join(INSTALLED_FILE),
            &ReceiptPointer {
                schema_version: 1,
                metadata_sha256: previous.verified.metadata_sha256.clone(),
            },
        )?;
    }
    remove_regular_file_if_present(&target.join(PENDING_FILE))?;
    let _ = remove_regular_file_if_present(&pending.stage.join(UPDATE_HELPER_FILE));
    remove_empty_directory(&pending.stage);
    Ok(())
}

fn record_recovered_release(
    target: &Path,
    pending: &PendingUpdate,
    key: &[u8],
) -> Result<(), String> {
    let recovered = update_helper::recovery_release(target, &pending.transaction, key)
        .map_err(|error| format!("Could not inspect the launcher recovery transaction: {error}"))?;
    release::record_installed_release(&target.join(STATE_FILE), &recovered)
        .map_err(|error| format!("Could not record the recovered launcher release: {error}"))?;
    write_json_replace(
        &target.join(INSTALLED_FILE),
        &ReceiptPointer {
            schema_version: 1,
            metadata_sha256: recovered.metadata_sha256,
        },
    )
}

fn spawn_recovery_helper(
    helper: &Path,
    target: &Path,
    transaction: &Path,
    config: &LauncherUpdateConfig,
) -> Result<Child, String> {
    Command::new(helper)
        .current_dir(target)
        .arg("recover")
        .arg("--target")
        .arg(target)
        .arg("--transaction")
        .arg(transaction)
        .arg("--public-key")
        .arg(config.key_path(target)?)
        .arg("--launcher-handoff")
        .arg("--wait-pid")
        .arg(std::process::id().to_string())
        .arg("--restart")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not start launcher update recovery: {error}"))
}
