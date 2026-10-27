//! Download a checked offer and hand it to the packaged update helper.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use bahamut_launcher::release;
use bahamut_launcher::update_helper::PENDING_FILE;

use super::config::LauncherUpdateConfig;
use super::fetch::{cached_launcher_artifact, download_launcher_artifact, fetch_signed_release};
use super::fs_io::{
    checked_root, remove_empty_directory, remove_regular_file_if_present, write_json_new,
    write_json_replace,
};
use super::helper::{create_sibling_stage, stage_trusted_helper_from_artifact};
use super::locks::{UpdateLock, acquire_helper_lock};
use super::pending::{HelperOperation, PendingUpdate, reject_pending_transaction};
use super::receipts::{
    Receipt, ensure_installed_receipt, ensure_update_directories, load_receipt, read_pointer,
    receipt_metadata_path, receipt_signature_path,
};
use super::status::{LauncherUpdateResult, LauncherUpdateState};
use super::verify::{compare_versions, verify_launcher_release, verify_new_launcher_offer};
use super::{OFFER_FILE, STATE_FILE};

/// Download a freshly revalidated offer and hand it to the helper.
pub(crate) fn apply_launcher_update(root: &Path) -> Result<LauncherUpdateResult, String> {
    apply_launcher_update_with(root, start_helper)
}

pub(super) fn apply_launcher_update_with(
    root: &Path,
    handoff: impl FnOnce(
        &Path,
        &LauncherUpdateConfig,
        &Receipt,
        &Receipt,
        &Path,
        HelperOperation,
    ) -> Result<LauncherUpdateResult, String>,
) -> Result<LauncherUpdateResult, String> {
    let config = LauncherUpdateConfig::load(root)?;
    let _lock = UpdateLock::acquire(root)?;
    ensure_update_directories(root)?;
    reject_pending_transaction(root)?;
    let target = checked_root(root)?;
    let key = config.public_key(&target)?;
    let installed = ensure_installed_receipt(&target, &config, &key)?;
    let pointer = read_pointer(&target.join(OFFER_FILE), "launcher update offer")?
        .ok_or_else(|| "Check for launcher updates first.".to_owned())?;
    let offer = load_receipt(&target, &key, &pointer.metadata_sha256)?;
    if compare_versions(
        &offer.verified.metadata.version,
        &installed.verified.metadata.version,
    )? != std::cmp::Ordering::Greater
    {
        return Err("The checked launcher offer is no longer newer than the installed release. Check again.".into());
    }

    let (remote_metadata, remote_signature) = fetch_signed_release(&config)?;
    let fresh = verify_launcher_release(&remote_metadata, &remote_signature, &key)?;
    if fresh.metadata_sha256 != offer.verified.metadata_sha256 {
        return Err(
            "The stable launcher offer changed after Check. Check again before installing.".into(),
        );
    }
    verify_new_launcher_offer(&fresh)?;
    release::accept_offer(&target.join(STATE_FILE), &fresh)
        .map_err(|error| format!("Launcher release offer was rejected: {error}"))?;
    let artifact = match cached_launcher_artifact(&target, &offer.verified.metadata) {
        Some(path) => path,
        None => download_launcher_artifact(&target, &config, &offer)?,
    };

    handoff(
        &target,
        &config,
        &installed,
        &offer,
        &artifact,
        HelperOperation::Update,
    )
}

fn start_helper(
    target: &Path,
    config: &LauncherUpdateConfig,
    previous: &Receipt,
    replacement: &Receipt,
    artifact: &Path,
    operation: HelperOperation,
) -> Result<LauncherUpdateResult, String> {
    release::verify_artifact_file(&replacement.verified.metadata, artifact)
        .map_err(|error| format!("Launcher artifact verification failed: {error}"))?;

    let _helper_lock = acquire_helper_lock(target)?;
    let trusted_key = config.key_path(target)?;
    let (stage, helper) = stage_helper(target, artifact, replacement)?;
    let mut pending = new_pending(target, &stage, operation, previous, replacement);
    if let Err(error) = write_json_new(&target.join(PENDING_FILE), &pending) {
        remove_regular_file_if_present(&helper)?;
        remove_empty_directory(&stage);
        return Err(error);
    }

    let mut child = match spawn_helper(
        &helper,
        &pending,
        &trusted_key,
        artifact,
        previous,
        replacement,
    ) {
        Ok(child) => child,
        Err(error) => {
            remove_regular_file_if_present(&target.join(PENDING_FILE))?;
            remove_regular_file_if_present(&helper)?;
            remove_empty_directory(&stage);
            return Err(format!(
                "Could not start bahamut-update-helper.exe: {error}"
            ));
        }
    };
    pending.helper_pid = Some(child.id());
    if let Err(error) = write_json_replace(&target.join(PENDING_FILE), &pending) {
        let _ = child.kill();
        let _ = child.wait();
        let _ = remove_regular_file_if_present(&target.join(PENDING_FILE));
        let _ = remove_regular_file_if_present(&helper);
        remove_empty_directory(&stage);
        return Err(format!(
            "Could not record the launcher update helper process: {error}"
        ));
    }
    Ok(LauncherUpdateResult {
        state: LauncherUpdateState::Handoff,
        message: handoff_message(operation, &replacement.verified.metadata.version),
    })
}

/// Create the same-volume stage and extract the verified helper into it,
/// removing the empty stage if extraction fails.
fn stage_helper(
    target: &Path,
    artifact: &Path,
    replacement: &Receipt,
) -> Result<(PathBuf, PathBuf), String> {
    let stage = create_sibling_stage(target)?;
    match stage_trusted_helper_from_artifact(artifact, &stage, &replacement.verified.metadata) {
        Ok(helper) => Ok((stage, helper)),
        Err(error) => {
            remove_empty_directory(&stage);
            Err(error)
        }
    }
}

fn new_pending(
    target: &Path,
    stage: &Path,
    operation: HelperOperation,
    previous: &Receipt,
    replacement: &Receipt,
) -> PendingUpdate {
    let transaction = stage.join(format!(
        "{}-{}",
        operation.transaction_name(),
        replacement.verified.metadata_sha256
    ));
    PendingUpdate {
        schema_version: 1,
        target: target.to_path_buf(),
        stage: stage.to_path_buf(),
        transaction,
        operation,
        helper_pid: None,
        previous_metadata_sha256: previous.verified.metadata_sha256.clone(),
        new_metadata_sha256: replacement.verified.metadata_sha256.clone(),
    }
}

fn spawn_helper(
    helper: &Path,
    pending: &PendingUpdate,
    trusted_key: &Path,
    artifact: &Path,
    previous: &Receipt,
    replacement: &Receipt,
) -> io::Result<Child> {
    let target = pending.target.as_path();
    Command::new(helper)
        .current_dir(target)
        .arg(pending.operation.command())
        .arg("--target")
        .arg(target)
        .arg("--stage")
        .arg(&pending.stage)
        .arg("--transaction")
        .arg(&pending.transaction)
        .arg("--launcher-handoff")
        .arg("--metadata")
        .arg(receipt_metadata_path(
            target,
            &replacement.verified.metadata_sha256,
        ))
        .arg("--signature")
        .arg(receipt_signature_path(
            target,
            &replacement.verified.metadata_sha256,
        ))
        .arg("--public-key")
        .arg(trusted_key)
        .arg("--artifact")
        .arg(artifact)
        .arg("--previous-metadata")
        .arg(receipt_metadata_path(
            target,
            &previous.verified.metadata_sha256,
        ))
        .arg("--previous-signature")
        .arg(receipt_signature_path(
            target,
            &previous.verified.metadata_sha256,
        ))
        .arg("--wait-pid")
        .arg(std::process::id().to_string())
        .arg("--restart")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

fn handoff_message(operation: HelperOperation, version: &str) -> String {
    match operation {
        HelperOperation::Update => format!(
            "Launcher update to version {version} is ready. The launcher will close and restart."
        ),
        HelperOperation::Repair => format!(
            "Launcher version {version} is ready to repair. The launcher will close and restart."
        ),
    }
}
