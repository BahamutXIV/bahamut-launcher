//! Signed portable launcher updates coordinated by the packaged helper.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bahamut_launcher::patcher::http::{self, CheckpointAction, ObjectSpec};
use bahamut_launcher::release::{
    self, Channel, FileOwnership, Product, ReleaseInventory, ReleaseMetadata, ReleaseScope, Target,
    VerifiedRelease,
};
use reqwest::{Url, blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

const CONFIG_FILE: &str = "launcher-updates.json";
const CONFIG_SCHEMA: u32 = 1;
const RECEIPT_DIR: &str = "config/launcher-release-receipts";
const STATE_FILE: &str = "config/launcher-release-state.json";
const INSTALLED_FILE: &str = "config/launcher-installed.json";
const OFFER_FILE: &str = "config/launcher-update-offer.json";
const PENDING_FILE: &str = "config/launcher-update-pending.json";
const HELPER_POINTER: &str = "config/launcher-update-state.json";
const HELPER_LOCK_FILE: &str = "config/launcher-update-helper.lock";
const ARTIFACT_CACHE: &str = "cache/launcher-updates";
const UPDATE_HELPER_FILE: &str = "bahamut-update-helper.exe";
const OFFICIAL_OVERLAY_ROOT: &str = "plugins/dats/bahamut-dats-overlay/";
const OFFICIAL_OVERLAY_MANIFEST: &str = "plugins/dats/bahamut-dats-overlay/overlay.toml";
const MAX_CONFIG_BYTES: usize = 16 * 1024;
const MAX_METADATA_BYTES: usize = release::MAX_METADATA_BYTES;
const MAX_SIGNATURE_BYTES: usize = release::ED25519_SIGNATURE_BYTES;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const IO_CHUNK: usize = 64 * 1024;
static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LauncherUpdateConfig {
    schema_version: u32,
    metadata_url: String,
    signature_url: String,
    artifact_root_url: String,
    trusted_public_key_path: String,
    bootstrap: Option<BootstrapConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BootstrapConfig {
    version: String,
    metadata_path: String,
    signature_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptPointer {
    schema_version: u32,
    metadata_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingUpdate {
    schema_version: u32,
    target: PathBuf,
    stage: PathBuf,
    transaction: PathBuf,
    operation: HelperOperation,
    #[serde(default)]
    helper_pid: Option<u32>,
    previous_metadata_sha256: String,
    new_metadata_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LauncherUpdateStatus {
    pub(crate) state: String,
    pub(crate) message: String,
    pub(crate) installed_version: Option<String>,
    pub(crate) offered_version: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LauncherUpdateResult {
    pub(crate) state: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug)]
struct Receipt {
    verified: VerifiedRelease,
    metadata: Vec<u8>,
    signature: Vec<u8>,
}

impl LauncherUpdateConfig {
    fn load(root: &Path) -> Result<Self, String> {
        ensure_existing_directory(root, "launcher directory")?;
        let config_dir = root.join("config");
        match fs::symlink_metadata(&config_dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err("Launcher updates are blocked: provision config/launcher-updates.json with explicit stable HTTPS endpoints, a trustedPublicKeyPath, and a signed bootstrap release (metadata, signature, and version).".into());
            }
            Err(error) => {
                return Err(format!(
                    "Could not inspect launcher config directory: {error}"
                ));
            }
            Ok(details) if details.is_dir() && !is_reparse(&details) => {}
            Ok(_) => return Err("Launcher config directory is not a plain directory.".into()),
        }
        let path = root.join("config").join(CONFIG_FILE);
        let bytes = read_regular_file(&path, MAX_CONFIG_BYTES, "launcher update configuration")
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    "Launcher updates are blocked: provision config/launcher-updates.json with explicit stable HTTPS endpoints, a trustedPublicKeyPath, and a signed bootstrap release (metadata, signature, and version).".to_owned()
                } else {
                    format!("Could not read config/{CONFIG_FILE}: {error}")
                }
            })?;
        let config: Self = serde_json::from_slice(&bytes)
            .map_err(|_| format!("config/{CONFIG_FILE} is not valid launcher update JSON."))?;
        config.validate(root)?;
        Ok(config)
    }

    fn validate(&self, root: &Path) -> Result<(), String> {
        if self.schema_version != CONFIG_SCHEMA {
            return Err(format!("Unsupported config/{CONFIG_FILE} schema version."));
        }
        validate_endpoint(&self.metadata_url, "metadata URL")?;
        validate_endpoint(&self.signature_url, "signature URL")?;
        validate_endpoint(&self.artifact_root_url, "artifact root URL")?;
        let key_path = resolve_local_path(root, &self.trusted_public_key_path)?;
        let key = read_regular_file(
            &key_path,
            release::ED25519_PUBLIC_KEY_BYTES,
            "trusted launcher public key",
        )
        .map_err(|error| format!("Could not read the configured trusted public key: {error}"))?;
        if key.len() != release::ED25519_PUBLIC_KEY_BYTES {
            return Err("The configured launcher public key must be exactly 32 bytes.".into());
        }
        let bootstrap = self.bootstrap.as_ref().ok_or_else(|| {
            "Launcher updates are blocked: config/launcher-updates.json must explicitly provision a signed bootstrap metadata file, signature file, and version.".to_owned()
        })?;
        validate_version(&bootstrap.version)?;
        resolve_local_path(root, &bootstrap.metadata_path)?;
        resolve_local_path(root, &bootstrap.signature_path)?;
        Ok(())
    }

    fn key_path(&self, root: &Path) -> Result<PathBuf, String> {
        resolve_local_path(root, &self.trusted_public_key_path)
    }

    fn public_key(&self, root: &Path) -> Result<Vec<u8>, String> {
        release::read_public_key_file(&self.key_path(root)?)
            .map_err(|error| format!("Could not read the configured trusted public key: {error}"))
    }

    fn bootstrap_receipt(&self, root: &Path, key: &[u8]) -> Result<Receipt, String> {
        let bootstrap = self.bootstrap.as_ref().ok_or_else(|| {
            "Launcher updates are blocked because no signed bootstrap release is configured."
                .to_owned()
        })?;
        let metadata_path = resolve_local_path(root, &bootstrap.metadata_path)?;
        let signature_path = resolve_local_path(root, &bootstrap.signature_path)?;
        let metadata = release::read_metadata_file(&metadata_path).map_err(|error| {
            format!("Could not read the configured bootstrap metadata: {error}")
        })?;
        let signature = release::read_signature_file(&signature_path).map_err(|error| {
            format!("Could not read the configured bootstrap signature: {error}")
        })?;
        let verified = verify_launcher_release(&metadata, &signature, key)?;
        if verified.metadata.version != bootstrap.version {
            return Err(
                "The configured bootstrap version does not match its signed metadata.".into(),
            );
        }
        Ok(Receipt {
            verified,
            metadata,
            signature,
        })
    }
}

/// Return local-only update state. This function never performs network I/O.
pub(crate) fn get_launcher_update_status(root: &Path) -> LauncherUpdateStatus {
    match local_status(root) {
        Ok(status) => status,
        Err(message) => LauncherUpdateStatus {
            state: "blocked".into(),
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
            state: "current".into(),
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
        state: "update_available".into(),
        message: format!(
            "Bahamut Launcher version {} is available. Choose Update Launcher to download and install it.",
            offer.metadata.version
        ),
        installed_version: Some(installed.verified.metadata.version),
        offered_version: Some(offer.metadata.version),
    })
}

fn download_launcher_artifact(
    target: &Path,
    config: &LauncherUpdateConfig,
    offer: &Receipt,
) -> Result<PathBuf, String> {
    let artifact = http::download_object(
        &config.artifact_root_url,
        &ObjectSpec {
            object_key: offer.verified.metadata.artifact.object_key.clone(),
            length: offer.verified.metadata.artifact.length,
            sha256: offer.verified.metadata.artifact.sha256.clone(),
        },
        &target.join(ARTIFACT_CACHE),
        |_| CheckpointAction::Continue,
        |_, _| {},
    )
    .map_err(|error| format!("Could not download the signed launcher artifact: {error}"))?;
    release::verify_artifact_file(&offer.verified.metadata, &artifact)
        .map_err(|error| format!("Launcher artifact verification failed: {error}"))?;
    Ok(artifact)
}

/// Download a freshly revalidated offer and hand it to the helper.
pub(crate) fn apply_launcher_update(root: &Path) -> Result<LauncherUpdateResult, String> {
    apply_launcher_update_with(root, start_helper)
}

fn apply_launcher_update_with(
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum HelperOperation {
    Update,
    Repair,
}

impl HelperOperation {
    fn command(self) -> &'static str {
        match self {
            Self::Update => "apply",
            Self::Repair => "repair",
        }
    }

    fn transaction_name(self) -> &'static str {
        match self {
            Self::Update => "bahamut-update",
            Self::Repair => "bahamut-repair",
        }
    }
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
    let stage = create_sibling_stage(target)?;
    let helper = match stage_trusted_helper_from_artifact(
        artifact,
        &stage,
        &replacement.verified.metadata,
    ) {
        Ok(helper) => helper,
        Err(error) => {
            remove_empty_directory(&stage);
            return Err(error);
        }
    };
    let transaction = stage.join(format!(
        "{}-{}",
        operation.transaction_name(),
        replacement.verified.metadata_sha256
    ));
    let mut pending = PendingUpdate {
        schema_version: 1,
        target: target.to_path_buf(),
        stage: stage.clone(),
        transaction: transaction.clone(),
        operation,
        helper_pid: None,
        previous_metadata_sha256: previous.verified.metadata_sha256.clone(),
        new_metadata_sha256: replacement.verified.metadata_sha256.clone(),
    };
    if let Err(error) = write_json_new(&target.join(PENDING_FILE), &pending) {
        remove_regular_file_if_present(&helper)?;
        remove_empty_directory(&stage);
        return Err(error);
    }

    let result = Command::new(&helper)
        .current_dir(target)
        .arg(operation.command())
        .arg("--target")
        .arg(target)
        .arg("--stage")
        .arg(&stage)
        .arg("--transaction")
        .arg(&transaction)
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
        .spawn();
    let mut child = match result {
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
        state: "handoff".into(),
        message: match operation {
            HelperOperation::Update => format!(
                "Launcher update to version {} is ready. The launcher will close and restart.",
                replacement.verified.metadata.version
            ),
            HelperOperation::Repair => format!(
                "Launcher version {} is ready to repair. The launcher will close and restart.",
                replacement.verified.metadata.version
            ),
        },
    })
}

/// Confirm the signed update passed on this launcher's helper-added argument.
pub(crate) fn confirm_pending_launcher_update(root: &Path) -> Result<(), String> {
    let transaction = update_transaction_argument()
        .ok_or_else(|| "No helper update transaction was supplied to this launch.".to_owned())?;
    let target = checked_root(root)?;
    let _lock = UpdateLock::acquire(&target)?;
    confirm_pending_launcher_update_locked(&target, &transaction)
}

fn confirm_pending_launcher_update_locked(target: &Path, transaction: &Path) -> Result<(), String> {
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

/// Start helper recovery for a failed pending transaction; true means exit now.
pub(crate) fn recover_failed_launcher_update(
    root: &Path,
    force_rollback: bool,
) -> Result<bool, String> {
    let target = checked_root(root)?;
    let _lock = UpdateLock::acquire(&target)?;
    let mut helper_lock = Some(acquire_idle_helper_lock(&target)?);
    let mut pending = read_pending(&target)?;
    if let Some(pending) = &pending {
        ensure_pending_matches_target(pending, &target)?;
    }
    let pointer_path = target.join(HELPER_POINTER);
    let pointer_exists = regular_file_exists(&pointer_path, "helper transaction pointer")?;
    if !pointer_exists {
        if let Some(pending) = pending {
            // The helper publishes this pointer before changing any managed file.
            remove_regular_file_if_present(&target.join(PENDING_FILE))?;
            remove_regular_file_if_present(&pending.stage.join(UPDATE_HELPER_FILE))?;
            remove_empty_directory(&pending.stage);
        }
        return Ok(false);
    }
    let config = LauncherUpdateConfig::load(&target)?;
    let key = config.public_key(&target)?;
    if !force_rollback
        && let Some(current_pending) = &pending
        && let Ok(updated) = load_receipt(&target, &key, &current_pending.new_metadata_sha256)
        && verify_installed_inventory(&target, &updated.verified.metadata).is_ok()
    {
        drop(helper_lock.take());
        match confirm_pending_launcher_update_locked(&target, &current_pending.transaction) {
            Ok(()) => return Ok(false),
            Err(error) => {
                tracing::warn!(%error, "could not confirm the fully installed launcher on startup");
            }
        }

        helper_lock = Some(acquire_idle_helper_lock(&target)?);
        pending = read_pending(&target)?;
        if let Some(pending) = &pending {
            ensure_pending_matches_target(pending, &target)?;
        }
        if !regular_file_exists(&pointer_path, "helper transaction pointer")? {
            if let Some(pending) = pending {
                let config = LauncherUpdateConfig::load(&target)?;
                let key = config.public_key(&target)?;
                let installed_new = load_receipt(&target, &key, &pending.new_metadata_sha256)
                    .is_ok_and(|receipt| {
                        verify_installed_inventory(&target, &receipt.verified.metadata).is_ok()
                    });
                if !installed_new {
                    let previous = load_receipt(&target, &key, &pending.previous_metadata_sha256)?;
                    release::record_installed_release(&target.join(STATE_FILE), &previous.verified)
                        .map_err(|error| {
                            format!(
                                "Could not restore the previous accepted launcher release: {error}"
                            )
                        })?;
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
            }
            return Ok(false);
        }
    }
    if let Some(pending) = &pending {
        let recovered =
            bahamut_launcher::update_helper::recovery_release(&target, &pending.transaction, &key)
                .map_err(|error| {
                    format!("Could not inspect the launcher recovery transaction: {error}")
                })?;
        release::record_installed_release(&target.join(STATE_FILE), &recovered)
            .map_err(|error| format!("Could not record the recovered launcher release: {error}"))?;
        write_json_replace(
            &target.join(INSTALLED_FILE),
            &ReceiptPointer {
                schema_version: 1,
                metadata_sha256: recovered.metadata_sha256,
            },
        )?;
    }
    let helper_release = if let Some(pending) = &pending {
        Some(load_receipt(&target, &key, &pending.new_metadata_sha256)?)
    } else {
        None
    };
    let helper = match (&pending, &helper_release) {
        (Some(pending), Some(receipt)) => {
            staged_helper_path(&pending.stage, &receipt.verified.metadata)?
        }
        _ => return Err("The pending launcher helper receipt is missing.".into()),
    };
    let transaction = pending
        .as_ref()
        .ok_or_else(|| "The pending launcher helper receipt is missing.".to_owned())?
        .transaction
        .clone();
    let mut command = Command::new(helper);
    command
        .current_dir(&target)
        .arg("recover")
        .arg("--target")
        .arg(&target)
        .arg("--transaction")
        .arg(&transaction)
        .arg("--public-key")
        .arg(config.key_path(&target)?)
        .arg("--launcher-handoff")
        .arg("--wait-pid")
        .arg(std::process::id().to_string())
        .arg("--restart")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start launcher update recovery: {error}"))?;
    if let Some(mut pending) = pending {
        pending.helper_pid = Some(child.id());
        if let Err(error) = write_json_replace(&target.join(PENDING_FILE), &pending) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "Could not record the launcher recovery helper process: {error}"
            ));
        }
    }
    tracing::info!(
        pid = child.id(),
        "launcher update recovery handed off to helper"
    );
    drop(helper_lock.take());
    Ok(true)
}

fn local_status(root: &Path) -> Result<LauncherUpdateStatus, String> {
    let config = LauncherUpdateConfig::load(root)?;
    if regular_file_exists(&root.join(HELPER_POINTER), "helper transaction pointer")?
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
                "update_downloaded"
            } else {
                "update_available"
            }
            .into(),
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
        state: "ready".into(),
        message: "A trusted stable launcher source is configured. Choose Check for updates to verify its signed release.".into(),
        installed_version,
        offered_version: None,
    })
}

fn cached_launcher_artifact(root: &Path, metadata: &ReleaseMetadata) -> Option<PathBuf> {
    let path = root.join(ARTIFACT_CACHE).join(format!(
        "{}-{}.object",
        metadata.artifact.sha256, metadata.artifact.length
    ));
    let details = fs::symlink_metadata(&path).ok()?;
    if !details.is_file() || is_reparse(&details) {
        return None;
    }
    release::verify_artifact_file(metadata, &path).ok()?;
    Some(path)
}

fn ensure_installed_receipt(
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

fn verify_launcher_release(
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

fn verify_new_launcher_offer(offer: &VerifiedRelease) -> Result<(), String> {
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

fn fetch_signed_release(config: &LauncherUpdateConfig) -> Result<(Vec<u8>, Vec<u8>), String> {
    let metadata = fetch_bounded(&config.metadata_url, MAX_METADATA_BYTES, "signed metadata")?;
    let signature = fetch_bounded(&config.signature_url, MAX_SIGNATURE_BYTES, "signature")?;
    if signature.len() != MAX_SIGNATURE_BYTES {
        return Err("Launcher release signature must be exactly 64 bytes.".into());
    }
    Ok((metadata, signature))
}

fn fetch_bounded(url: &str, maximum: usize, label: &str) -> Result<Vec<u8>, String> {
    let client = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(Policy::none())
        .no_proxy()
        .build()
        .map_err(|error| format!("Could not configure launcher update transport: {error}"))?;
    let mut response = client
        .get(url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .map_err(|error| format!("Could not fetch launcher {label}: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Launcher {label} endpoint returned HTTP {}.",
            response.status().as_u16()
        ));
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(format!(
            "Launcher {label} endpoint used an unsupported content encoding."
        ));
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/html"))
    {
        return Err(format!("Launcher {label} endpoint returned HTML."));
    }
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(format!("Launcher {label} exceeds its size limit."));
    }
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read launcher {label}: {error}"))?;
    if bytes.len() > maximum {
        return Err(format!("Launcher {label} exceeds its size limit."));
    }
    if looks_like_html(&bytes) {
        return Err(format!("Launcher {label} endpoint returned HTML."));
    }
    Ok(bytes)
}

fn validate_endpoint(value: &str, label: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| format!("Configured launcher {label} is invalid."))?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(format!(
            "Configured launcher {label} must not contain credentials or a fragment."
        ));
    }
    match url.scheme() {
        "https" if url.host_str().is_some() => Ok(url),
        "http" if is_loopback_url(&url) => Ok(url),
        _ => Err(format!("Configured launcher {label} must use HTTPS.")),
    }
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback()),
        None => false,
    }
}

fn looks_like_html(bytes: &[u8]) -> bool {
    let prefix = &bytes[..bytes.len().min(128)];
    let text = String::from_utf8_lossy(prefix)
        .trim_start()
        .to_ascii_lowercase();
    text.starts_with("<!doctype html") || text.starts_with("<html")
}

fn persist_receipt(
    root: &Path,
    metadata: &[u8],
    signature: &[u8],
    hash: &str,
) -> Result<(), String> {
    ensure_plain_relative_directory(root, Path::new(RECEIPT_DIR))?;
    publish_immutable(&receipt_metadata_path(root, hash), metadata)?;
    publish_immutable(&receipt_signature_path(root, hash), signature)
}

fn load_receipt(root: &Path, key: &[u8], hash: &str) -> Result<Receipt, String> {
    if !valid_sha256(hash) {
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
            Ok(details) if details.is_dir() && !is_reparse(&details) => {}
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
    if !details.is_file() || is_reparse(&details) {
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

fn verify_installed_inventory(root: &Path, metadata: &ReleaseMetadata) -> Result<(), String> {
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

fn staged_helper_path(stage: &Path, metadata: &ReleaseMetadata) -> Result<PathBuf, String> {
    ensure_existing_directory(stage, "launcher update stage")?;
    let ReleaseInventory::Launcher { files } = &metadata.inventory else {
        return Err("Signed launcher helper inventory is missing.".into());
    };
    let expected = files
        .iter()
        .find(|file| file.path == UPDATE_HELPER_FILE && file.ownership == FileOwnership::Managed)
        .ok_or_else(|| "Signed launcher release does not manage its update helper.".to_owned())?;
    let path = stage.join(UPDATE_HELPER_FILE);
    let details = fs::symlink_metadata(&path)
        .map_err(|_| "The staged bahamut-update-helper.exe is missing.".to_owned())?;
    if !details.is_file() || is_reparse(&details) {
        return Err("The staged bahamut-update-helper.exe is not a regular file.".into());
    }
    let (length, hash) = hash_file(&path)?;
    if length != expected.length || hash != expected.sha256 {
        return Err(
            "The staged bahamut-update-helper.exe failed signed inventory verification.".into(),
        );
    }
    Ok(path)
}

fn stage_trusted_helper_from_artifact(
    artifact: &Path,
    stage: &Path,
    metadata: &ReleaseMetadata,
) -> Result<PathBuf, String> {
    ensure_existing_directory(stage, "launcher update stage")?;
    let ReleaseInventory::Launcher { files } = &metadata.inventory else {
        return Err("Signed launcher helper inventory is missing.".into());
    };
    let expected = files
        .iter()
        .find(|file| file.path == UPDATE_HELPER_FILE && file.ownership == FileOwnership::Managed)
        .ok_or_else(|| "Signed launcher release does not manage its update helper.".to_owned())?;
    let input = File::open(artifact)
        .map_err(|error| format!("Could not open the verified launcher ZIP: {error}"))?;
    let mut archive = ZipArchive::new(input)
        .map_err(|error| format!("Could not read the verified launcher ZIP: {error}"))?;
    let mut entry = archive
        .by_name(UPDATE_HELPER_FILE)
        .map_err(|_| "Verified launcher ZIP is missing bahamut-update-helper.exe.".to_owned())?;
    if entry.is_dir() || entry.size() != expected.length {
        return Err("Verified launcher helper entry changed after validation.".into());
    }
    let path = stage.join(UPDATE_HELPER_FILE);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("Could not stage bahamut-update-helper.exe: {error}"))?;
    let result = (|| {
        let mut digest = Sha256::new();
        let mut length = 0_u64;
        let mut buffer = [0_u8; IO_CHUNK];
        loop {
            let read = entry
                .read(&mut buffer)
                .map_err(|error| format!("Could not read the launcher helper entry: {error}"))?;
            if read == 0 {
                break;
            }
            length = length
                .checked_add(read as u64)
                .ok_or_else(|| "Launcher helper entry length overflowed.".to_owned())?;
            if length > expected.length {
                return Err("Launcher helper entry exceeded its signed length.".into());
            }
            output
                .write_all(&buffer[..read])
                .map_err(|error| format!("Could not write staged launcher helper: {error}"))?;
            digest.update(&buffer[..read]);
        }
        output
            .flush()
            .and_then(|()| output.sync_all())
            .map_err(|error| format!("Could not flush staged launcher helper: {error}"))?;
        if length != expected.length || format!("{:x}", digest.finalize()) != expected.sha256 {
            return Err("Staged launcher helper failed signed inventory verification.".into());
        }
        Ok(())
    })();
    drop(output);
    if let Err(error) = result {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

fn create_sibling_stage(target: &Path) -> Result<PathBuf, String> {
    let parent = target
        .parent()
        .ok_or_else(|| "Launcher install directory has no parent.".to_owned())?;
    ensure_existing_directory(parent, "launcher install parent")?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| format!("Could not resolve launcher install parent: {error}"))?;
    for _ in 0..64 {
        let serial = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let stage = canonical_parent.join(format!(
            ".bahamut-launcher-update-{}-{serial}",
            std::process::id()
        ));
        match fs::create_dir(&stage) {
            Ok(()) => {
                ensure_existing_directory(&stage, "launcher update stage")?;
                return Ok(stage);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Could not create same-volume launcher update stage: {error}"
                ));
            }
        }
    }
    Err("Could not allocate a unique launcher update stage.".into())
}

fn reject_pending_transaction(root: &Path) -> Result<(), String> {
    if regular_file_exists(&root.join(HELPER_POINTER), "helper transaction pointer")? {
        return Err("A launcher update transaction is pending. Restart the launcher to confirm it or recover the previous version first.".into());
    }
    if read_pending(root)?.is_some() {
        return Err("A launcher update handoff is already pending. Restart the launcher to finish it before checking again.".into());
    }
    Ok(())
}

fn read_pending(root: &Path) -> Result<Option<PendingUpdate>, String> {
    let path = root.join(PENDING_FILE);
    if !regular_file_exists(&path, "launcher update receipt")? {
        return Ok(None);
    }
    let bytes = read_regular_file(&path, 32 * 1024, "launcher update receipt")
        .map_err(|error| format!("Could not read launcher update receipt: {error}"))?;
    let pending: PendingUpdate = serde_json::from_slice(&bytes)
        .map_err(|_| "Launcher update receipt JSON is invalid.".to_owned())?;
    if pending.schema_version != 1
        || !valid_sha256(&pending.previous_metadata_sha256)
        || !valid_sha256(&pending.new_metadata_sha256)
        || pending.helper_pid == Some(0)
    {
        return Err("Launcher update receipt fields are invalid.".into());
    }
    Ok(Some(pending))
}

fn ensure_pending_matches_target(pending: &PendingUpdate, target: &Path) -> Result<(), String> {
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
    if regular_file_exists(&target.join(HELPER_POINTER), "helper transaction pointer")? {
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

fn process_matches_executable(pid: u32, expected_path: &Path) -> Result<bool, String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        };
        use windows::core::PWSTR;

        let process = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
            Ok(process) => process,
            Err(error)
                if error.code().0 == 0x8007_0057_u32 as i32
                    || error.code().0 == 0x8007_0005_u32 as i32 =>
            {
                return Ok(false);
            }
            Err(error) => {
                return Err(format!(
                    "Could not inspect launcher update helper process {pid}: {error}"
                ));
            }
        };
        let mut buffer = vec![0_u16; 32768];
        let mut length = buffer.len() as u32;
        let queried = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        };
        let _ = unsafe { CloseHandle(process) };
        queried.map_err(|error| {
            format!("Could not read launcher update helper process path: {error}")
        })?;
        let actual_path = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]));
        let comparable = |path: &Path| {
            let value = path.to_string_lossy();
            if let Some(rest) = value.strip_prefix("\\\\?\\UNC\\") {
                format!("\\\\{rest}")
            } else if let Some(rest) = value.strip_prefix("\\\\?\\") {
                rest.to_owned()
            } else {
                value.into_owned()
            }
        };
        Ok(comparable(&actual_path).eq_ignore_ascii_case(&comparable(expected_path)))
    }
    #[cfg(not(windows))]
    {
        let _ = (pid, expected_path);
        // Managed launcher updates are currently scoped to Windows x86_64.
        Ok(false)
    }
}

fn ensure_update_directories(root: &Path) -> Result<(), String> {
    ensure_plain_relative_directory(root, Path::new("config"))?;
    ensure_plain_relative_directory(root, Path::new(RECEIPT_DIR))?;
    ensure_plain_relative_directory(root, Path::new(ARTIFACT_CACHE))
}

fn receipt_metadata_path(root: &Path, hash: &str) -> PathBuf {
    root.join(RECEIPT_DIR).join(format!("{hash}.json"))
}
fn receipt_signature_path(root: &Path, hash: &str) -> PathBuf {
    root.join(RECEIPT_DIR).join(format!("{hash}.sig"))
}
fn launcher_scope() -> ReleaseScope {
    ReleaseScope {
        product: Product::Launcher,
        channel: Channel::Stable,
        target: Target::WindowsX86_64,
    }
}

fn read_pointer(path: &Path, label: &str) -> Result<Option<ReceiptPointer>, String> {
    if !regular_file_exists(path, label)? {
        return Ok(None);
    }
    let bytes = read_regular_file(path, 16 * 1024, label)
        .map_err(|error| format!("Could not read {label}: {error}"))?;
    let pointer: ReceiptPointer =
        serde_json::from_slice(&bytes).map_err(|_| format!("{label} JSON is invalid."))?;
    if pointer.schema_version != 1 || !valid_sha256(&pointer.metadata_sha256) {
        return Err(format!("{label} fields are invalid."));
    }
    Ok(Some(pointer))
}

fn write_json_new<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    write_new_file(
        path,
        &serde_json::to_vec(value).map_err(|error| error.to_string())?,
    )
}

fn write_json_replace<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    write_replace_file(
        path,
        &serde_json::to_vec(value).map_err(|error| error.to_string())?,
    )
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Launcher state path has no parent.".to_owned())?;
    let temp = unique_sibling(parent, ".launcher-update-state-");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| format!("Could not stage launcher update state: {error}"))?;
    if let Err(error) = output.write_all(bytes).and_then(|()| output.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(format!("Could not write launcher update state: {error}"));
    }
    drop(output);
    match move_new_file(&temp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            if error.kind() == io::ErrorKind::AlreadyExists {
                Err("Launcher update state already exists.".into())
            } else {
                Err(format!("Could not publish launcher update state: {error}"))
            }
        }
    }
}

fn write_replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Launcher state path has no parent.".to_owned())?;
    let temp = unique_sibling(parent, ".launcher-update-state-");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| format!("Could not stage launcher update state: {error}"))?;
    if let Err(error) = output.write_all(bytes).and_then(|()| output.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(format!("Could not write launcher update state: {error}"));
    }
    drop(output);
    if let Err(error) = replace_file(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(format!("Could not publish launcher update state: {error}"));
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(not(windows))]
fn move_new_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::hard_link(source, destination)?;
    fs::remove_file(source)
}

#[cfg(windows)]
fn move_new_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

    let source = source
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            windows::core::PCWSTR(source.as_ptr()),
            windows::core::PCWSTR(destination.as_ptr()),
            MOVEFILE_WRITE_THROUGH,
        )
        .map_err(io::Error::other)
    }
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            windows::core::PCWSTR(source.as_ptr()),
            windows::core::PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(io::Error::other)
    }
}

fn publish_immutable(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if regular_file_exists(path, "launcher release receipt")? {
        let existing = read_regular_file(path, MAX_METADATA_BYTES, "launcher release receipt")
            .map_err(|error| format!("Could not read existing launcher receipt: {error}"))?;
        return if existing == bytes {
            Ok(())
        } else {
            Err("An existing launcher release receipt has conflicting bytes.".into())
        };
    }
    write_new_file(path, bytes)
}

fn unique_sibling(parent: &Path, prefix: &str) -> PathBuf {
    let serial = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!("{prefix}{}-{serial}", std::process::id()))
}

fn remove_regular_file_if_present(path: &Path) -> Result<(), String> {
    if regular_file_exists(path, "launcher update state")? {
        fs::remove_file(path)
            .map_err(|error| format!("Could not remove launcher update state: {error}"))?;
    }
    Ok(())
}

fn remove_empty_directory(path: &Path) {
    if let Ok(details) = fs::symlink_metadata(path)
        && details.is_dir()
        && !is_reparse(&details)
    {
        let _ = fs::remove_dir(path);
    }
}

fn regular_file_exists(path: &Path, label: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(details) if details.is_file() && !is_reparse(&details) => Ok(true),
        Ok(_) => Err(format!("{label} is not a regular file.")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Could not inspect {label}: {error}")),
    }
}

fn read_regular_file(path: &Path, maximum: usize, label: &str) -> io::Result<Vec<u8>> {
    let details = fs::symlink_metadata(path)?;
    if !details.is_file() || is_reparse(&details) || details.len() > maximum as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{label} is not a bounded regular file"),
        ));
    }
    let mut bytes = Vec::with_capacity(details.len() as usize);
    File::open(path)?
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{label} exceeds its size limit"),
        ));
    }
    Ok(bytes)
}

fn resolve_local_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(
            "Configured launcher local paths must be relative to the launcher directory.".into(),
        );
    }
    let resolved = root.join(path);
    if let Some(parent) = resolved.parent() {
        ensure_path_has_no_reparse_ancestors(parent)?;
    }
    Ok(resolved)
}

fn ensure_plain_relative_directory(root: &Path, relative: &Path) -> Result<(), String> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("Launcher directory path is unsafe.".into());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(details) if details.is_dir() && !is_reparse(&details) => {}
            Ok(_) => {
                return Err(format!(
                    "Launcher directory is not a plain directory: {}",
                    current.display()
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&current)
                .map_err(|error| {
                    format!(
                        "Could not create launcher directory {}: {error}",
                        current.display()
                    )
                })?,
            Err(error) => {
                return Err(format!(
                    "Could not inspect launcher directory {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

fn ensure_existing_directory(path: &Path, label: &str) -> Result<(), String> {
    ensure_path_has_no_reparse_ancestors(path)?;
    let details = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect {label}: {error}"))?;
    if !details.is_dir() || is_reparse(&details) {
        return Err(format!("{label} is not a plain directory."));
    }
    Ok(())
}

fn ensure_path_has_no_reparse_ancestors(path: &Path) -> Result<(), String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::Normal(name) => {
                current.push(name);
                let details = fs::symlink_metadata(&current).map_err(|error| {
                    format!(
                        "Could not inspect launcher path {}: {error}",
                        current.display()
                    )
                })?;
                if !details.is_dir() || is_reparse(&details) {
                    return Err(format!(
                        "Launcher path contains a reparse point or non-directory parent: {}",
                        current.display()
                    ));
                }
            }
            Component::CurDir | Component::ParentDir => {
                return Err("Launcher path contains an unsafe component.".into());
            }
        }
    }
    Ok(())
}

fn checked_root(root: &Path) -> Result<PathBuf, String> {
    ensure_existing_directory(root, "launcher install directory")?;
    fs::canonicalize(root)
        .map_err(|error| format!("Could not resolve launcher install directory: {error}"))
}

fn safe_join(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("Signed launcher inventory contains an unsafe path.".into());
    }
    Ok(root.join(relative))
}

fn normalize_path(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path)
        .or_else(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Ok(path.to_path_buf())
            } else {
                Err(error)
            }
        })
        .map_err(|error| format!("Could not resolve launcher transaction path: {error}"))
}

fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let mut file =
        File::open(path).map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let mut digest = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = [0_u8; IO_CHUNK];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Could not hash {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(read as u64)
            .ok_or_else(|| "Launcher file length overflowed.".to_owned())?;
        digest.update(&buffer[..read]);
    }
    Ok((length, format!("{:x}", digest.finalize())))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_version(value: &str) -> Result<(), String> {
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

fn compare_versions(left: &str, right: &str) -> Result<std::cmp::Ordering, String> {
    Ok(version_tuple(left)?.cmp(&version_tuple(right)?))
}

#[cfg(windows)]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

struct UpdateLock(File);

fn acquire_helper_lock(
    root: &Path,
) -> Result<bahamut_launcher::atomic_fs::ExclusiveProcessLock, String> {
    ensure_plain_relative_directory(root, Path::new("config"))?;
    let path = root.join(HELPER_LOCK_FILE);
    let lock = bahamut_launcher::atomic_fs::try_lock_exclusive(&path).map_err(|error| {
        if error.kind() == io::ErrorKind::WouldBlock {
            "Another launcher update helper is active. Close this window and retry after the update completes.".to_owned()
        } else {
            format!("Could not acquire launcher update helper lock: {error}")
        }
    })?;
    let details = fs::symlink_metadata(&path)
        .map_err(|error| format!("Could not inspect launcher update helper lock: {error}"))?;
    if !details.is_file() || is_reparse(&details) {
        return Err("Launcher update helper lock is not a regular file.".into());
    }
    Ok(lock)
}

fn acquire_idle_helper_lock(
    root: &Path,
) -> Result<bahamut_launcher::atomic_fs::ExclusiveProcessLock, String> {
    loop {
        let lock = acquire_helper_lock_wait(root)?;
        let pending = read_pending(root)?;
        if let Some(pending) = pending
            && let Some(pid) = pending.helper_pid
        {
            let helper_path = pending.stage.join(UPDATE_HELPER_FILE);
            if process_matches_executable(pid, &helper_path)? {
                drop(lock);
                while process_matches_executable(pid, &helper_path)? {
                    std::thread::sleep(Duration::from_millis(50));
                }
                continue;
            }
        }
        return Ok(lock);
    }
}

fn acquire_helper_lock_wait(
    root: &Path,
) -> Result<bahamut_launcher::atomic_fs::ExclusiveProcessLock, String> {
    ensure_plain_relative_directory(root, Path::new("config"))?;
    let path = root.join(HELPER_LOCK_FILE);
    let lock = bahamut_launcher::atomic_fs::lock_exclusive(&path)
        .map_err(|error| format!("Could not wait for launcher update helper lock: {error}"))?;
    let details = fs::symlink_metadata(&path)
        .map_err(|error| format!("Could not inspect launcher update helper lock: {error}"))?;
    if !details.is_file() || is_reparse(&details) {
        return Err("Launcher update helper lock is not a regular file.".into());
    }
    Ok(lock)
}

impl UpdateLock {
    fn acquire(root: &Path) -> Result<Self, String> {
        ensure_plain_relative_directory(root, Path::new("config"))?;
        let path = root.join("config/launcher-update.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("Could not open launcher update lock: {error}"))?;
        let details = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if !details.is_file() || is_reparse(&details) {
            return Err("Launcher update lock is not a regular file.".into());
        }
        file.lock()
            .map_err(|error| format!("Could not acquire launcher update lock: {error}"))?;
        Ok(Self(file))
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bahamut_launcher::release::ArtifactFormat;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::thread;

    #[test]
    fn pending_update_requires_an_explicit_operation() {
        let root = crate::test_support::tempdir().unwrap();
        let pending = PendingUpdate {
            schema_version: 1,
            target: root.path().to_path_buf(),
            stage: root.path().join("stage"),
            transaction: root.path().join("stage/transaction"),
            operation: HelperOperation::Repair,
            helper_pid: None,
            previous_metadata_sha256: "a".repeat(64),
            new_metadata_sha256: "a".repeat(64),
        };
        let path = root.path().join(PENDING_FILE);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_json_new(&path, &pending).unwrap();
        assert_eq!(
            read_pending(root.path()).unwrap().unwrap().operation,
            HelperOperation::Repair
        );

        let mut incomplete = serde_json::to_value(&pending).unwrap();
        incomplete.as_object_mut().unwrap().remove("operation");
        let bytes = serde_json::to_vec(&incomplete).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert_eq!(
            read_pending(root.path()).unwrap_err(),
            "Launcher update receipt JSON is invalid."
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn missing_config_is_blocked_without_network() {
        let root = crate::test_support::tempdir().unwrap();
        let status = get_launcher_update_status(root.path());
        assert_eq!(status.state, "blocked");
        assert!(status.message.contains("launcher-updates.json"));
        assert!(
            check_launcher_update(root.path())
                .unwrap_err()
                .contains("launcher-updates.json")
        );
    }

    #[test]
    fn missing_bootstrap_blocks_before_configured_loopback_endpoints_are_read() {
        let root = crate::test_support::tempdir().unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        fs::write(root.path().join("config/key.bin"), [0; 32]).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let (url, server) = counted_fixture_server(Arc::clone(&requests));
        let config = serde_json::json!({
            "schemaVersion":1,"metadataUrl":url,"signatureUrl":url,"artifactRootUrl":url,
            "trustedPublicKeyPath":"config/key.bin"
        });
        fs::write(
            root.path().join("config/launcher-updates.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        assert!(
            check_launcher_update(root.path())
                .unwrap_err()
                .contains("bootstrap")
        );
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        server.join().unwrap();
    }

    #[test]
    fn redirects_and_html_metadata_are_rejected() {
        for (response, expected) in [
            (
                "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                "HTTP 302",
            ),
            (
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 13\r\nConnection: close\r\n\r\n<html></html>",
                "HTML",
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let response = response.to_owned();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request);
                stream.write_all(response.as_bytes()).unwrap();
            });
            let error = fetch_bounded(
                &format!("http://{address}/metadata"),
                MAX_METADATA_BYTES,
                "metadata",
            )
            .unwrap_err();
            assert!(error.contains(expected), "{error}");
            server.join().unwrap();
        }
    }

    #[test]
    fn signed_downgrade_is_rejected_before_offer_is_persisted() {
        let root = crate::test_support::tempdir().unwrap();
        let fixture = SignedFixture::new();
        fixture.prepare_root(root.path(), "1.0.0");
        let lower = fixture.release("0.9.0");
        let (url, server) = fixture.serve(&[lower.metadata.clone(), lower.signature.clone()]);
        fixture.write_config(root.path(), &url, Some("1.0.0"));
        let error = check_launcher_update(root.path()).unwrap_err();
        assert!(
            error.contains("Older remote release offers") || error.contains("downgrade rejected")
        );
        assert!(!root.path().join(OFFER_FILE).exists());
        server.join().unwrap();
    }

    #[test]
    fn new_signed_offer_rejects_legacy_seed_overlay_manifest() {
        let root = crate::test_support::tempdir().unwrap();
        let fixture = SignedFixture::new();
        fixture.prepare_root(root.path(), "1.0.0");
        let next = fixture.legacy_release("1.1.0");
        let (url, server) = fixture.serve(&[next.metadata.clone(), next.signature.clone()]);
        fixture.write_config(root.path(), &url, Some("1.0.0"));

        let error = check_launcher_update(root.path()).unwrap_err();

        assert!(error.contains("manage every official DAT overlay file"));
        assert!(!root.path().join(OFFER_FILE).exists());
        assert_eq!(
            release::accepted_state_summary(&root.path().join(STATE_FILE), launcher_scope())
                .unwrap(),
            ("1.0.0".into(), Some("1.0.0".into()))
        );
        server.join().unwrap();
    }

    #[test]
    fn new_signed_offer_requires_the_official_overlay_manifest() {
        let root = crate::test_support::tempdir().unwrap();
        let fixture = SignedFixture::new();
        fixture.prepare_root(root.path(), "1.0.0");
        let mut next = fixture.release("1.1.0");
        let mut metadata: ReleaseMetadata = serde_json::from_slice(&next.metadata).unwrap();
        let ReleaseInventory::Launcher { files } = &mut metadata.inventory else {
            panic!("fixture must use a launcher inventory");
        };
        files.retain(|file| file.path != OFFICIAL_OVERLAY_MANIFEST);
        next.metadata = serde_json::to_vec(&metadata).unwrap();
        next.signature = fixture.key.sign(&next.metadata).as_ref().to_vec();
        let (url, server) = fixture.serve(&[next.metadata.clone(), next.signature.clone()]);
        fixture.write_config(root.path(), &url, Some("1.0.0"));

        let error = check_launcher_update(root.path()).unwrap_err();

        assert!(error.contains("manage the official DAT overlay manifest"));
        assert!(!root.path().join(OFFER_FILE).exists());
        server.join().unwrap();
    }

    #[test]
    fn apply_rejects_legacy_seed_overlay_manifest_on_a_new_offer() {
        let root = crate::test_support::tempdir().unwrap();
        let fixture = SignedFixture::new();
        fixture.prepare_root(root.path(), "1.0.0");
        let next = fixture.legacy_release("1.1.0");
        let offer = verify_launcher_release(
            &next.metadata,
            &next.signature,
            fixture.key.public_key().as_ref(),
        )
        .unwrap();
        persist_receipt(
            root.path(),
            &next.metadata,
            &next.signature,
            &offer.metadata_sha256,
        )
        .unwrap();
        write_json_replace(
            &root.path().join(OFFER_FILE),
            &ReceiptPointer {
                schema_version: 1,
                metadata_sha256: offer.metadata_sha256.clone(),
            },
        )
        .unwrap();
        let (url, server) = fixture.serve(&[next.metadata.clone(), next.signature.clone()]);
        fixture.write_config(root.path(), &url, Some("1.0.0"));

        let error = apply_launcher_update_with(root.path(), |_, _, _, _, _, _| {
            panic!("invalid launcher offer must not be handed to the helper")
        })
        .unwrap_err();

        assert!(error.contains("manage every official DAT overlay file"));
        assert!(cached_launcher_artifact(root.path(), &offer.metadata).is_none());
        assert_eq!(
            release::accepted_state_summary(&root.path().join(STATE_FILE), launcher_scope())
                .unwrap(),
            ("1.0.0".into(), Some("1.0.0".into()))
        );
        server.join().unwrap();
    }

    #[test]
    fn verified_monotonic_offer_is_persisted_with_its_signed_receipt() {
        let root = crate::test_support::tempdir().unwrap();
        let fixture = SignedFixture::new();
        fixture.prepare_root(root.path(), "1.0.0");
        let next = fixture.release("1.1.0");
        let (url, server) = fixture.serve(&[next.metadata.clone(), next.signature.clone()]);
        fixture.write_config(root.path(), &url, Some("1.0.0"));

        let status = check_launcher_update(root.path()).unwrap();
        assert_eq!(status.state, "update_available");
        assert_eq!(status.installed_version.as_deref(), Some("1.0.0"));
        assert_eq!(status.offered_version.as_deref(), Some("1.1.0"));
        let pointer = read_pointer(&root.path().join(OFFER_FILE), "test offer")
            .unwrap()
            .unwrap();
        let key = fixture.key.public_key();
        let receipt = load_receipt(root.path(), key.as_ref(), &pointer.metadata_sha256).unwrap();
        let artifact_path = root.path().join("candidate.zip");
        fs::write(&artifact_path, next.artifact).unwrap();
        release::verify_artifact_file(&receipt.verified.metadata, &artifact_path).unwrap();
        assert_eq!(
            release::accepted_state_summary(&root.path().join(STATE_FILE), launcher_scope())
                .unwrap(),
            ("1.1.0".into(), Some("1.0.0".into()))
        );
        server.join().unwrap();
    }

    #[test]
    fn metadata_check_is_download_free_and_apply_downloads_before_handoff() {
        let root = crate::test_support::tempdir().unwrap();
        let fixture = SignedFixture::new();
        fixture.prepare_root(root.path(), "1.0.0");
        let next = fixture.release("1.1.0");
        let (url, server) = fixture.serve(&[
            next.metadata.clone(),
            next.signature.clone(),
            next.metadata.clone(),
            next.signature.clone(),
            next.artifact.clone(),
        ]);
        fixture.write_config(root.path(), &url, Some("1.0.0"));

        let status = check_launcher_update(root.path()).unwrap();
        assert_eq!(status.state, "update_available");
        assert_eq!(status.installed_version.as_deref(), Some("1.0.0"));
        assert_eq!(status.offered_version.as_deref(), Some("1.1.0"));
        assert!(!root.path().join(ARTIFACT_CACHE).exists());

        let expected_artifact = next.artifact.clone();
        let result = apply_launcher_update_with(
            root.path(),
            |target, _, previous, replacement, artifact, operation| {
                assert_eq!(operation, HelperOperation::Update);
                assert_eq!(previous.verified.metadata.version, "1.0.0");
                assert_eq!(replacement.verified.metadata.version, "1.1.0");
                assert_eq!(
                    cached_launcher_artifact(target, &replacement.verified.metadata).as_deref(),
                    Some(artifact)
                );
                assert_eq!(fs::read(artifact).unwrap(), expected_artifact);
                Ok(LauncherUpdateResult {
                    state: "handoff".into(),
                    message: "Launcher update helper started.".into(),
                })
            },
        )
        .unwrap();
        assert_eq!(result.state, "handoff");

        let pointer = read_pointer(&root.path().join(OFFER_FILE), "test offer")
            .unwrap()
            .unwrap();
        let receipt = load_receipt(
            root.path(),
            fixture.key.public_key().as_ref(),
            &pointer.metadata_sha256,
        )
        .unwrap();
        let cached = cached_launcher_artifact(root.path(), &receipt.verified.metadata).unwrap();
        assert_eq!(fs::read(&cached).unwrap(), next.artifact);
        server.join().unwrap();

        fs::write(&cached, b"corrupted cached artifact").unwrap();
        assert_eq!(
            get_launcher_update_status(root.path()).state,
            "update_available"
        );
    }

    #[test]
    fn tampered_signature_is_rejected_and_stage_is_a_sibling() {
        let root = crate::test_support::tempdir().unwrap();
        let fixture = SignedFixture::new();
        fixture.prepare_root(root.path(), "1.0.0");
        let mut offer = fixture.release("1.1.0");
        offer.signature[0] ^= 0x80;
        let (url, server) = fixture.serve(&[offer.metadata.clone(), offer.signature.clone()]);
        fixture.write_config(root.path(), &url, Some("1.0.0"));
        assert!(
            check_launcher_update(root.path())
                .unwrap_err()
                .contains("signature is invalid")
        );
        assert!(!root.path().join(OFFER_FILE).exists());
        server.join().unwrap();

        let install = root.path().join("install");
        fs::create_dir(&install).unwrap();
        let canonical = checked_root(&install).unwrap();
        let stage = create_sibling_stage(&canonical).unwrap();
        assert_eq!(stage.parent(), canonical.parent());
        assert!(!stage.starts_with(&canonical));
    }

    #[test]
    fn staged_helper_matches_the_signed_launcher_inventory() {
        let fixture = SignedFixture::new();
        let release = fixture.release("1.0.0");
        let verified = verify_launcher_release(
            &release.metadata,
            &release.signature,
            fixture.key.public_key().as_ref(),
        )
        .unwrap();
        let temp = crate::test_support::tempdir().unwrap();
        let target = temp.path().join("install");
        let stage = temp.path().join("stage");
        fs::create_dir(&target).unwrap();
        fs::create_dir(&stage).unwrap();
        let artifact = temp.path().join("launcher.zip");
        fs::write(&artifact, release.artifact).unwrap();

        let helper =
            stage_trusted_helper_from_artifact(&artifact, &stage, &verified.metadata).unwrap();

        assert_eq!(helper, stage.join(UPDATE_HELPER_FILE));
        assert_eq!(fs::read(helper).unwrap(), b"fixture trusted update helper");
        assert!(!stage.starts_with(&target));
    }

    #[cfg(windows)]
    #[test]
    fn helper_process_identity_rejects_a_reused_pid_for_another_executable() {
        let current = std::env::current_exe().unwrap();
        assert!(process_matches_executable(std::process::id(), &current).unwrap());
        assert!(
            !process_matches_executable(
                std::process::id(),
                &current.with_file_name("bahamut-update-helper.exe")
            )
            .unwrap()
        );
    }

    struct FixtureRelease {
        metadata: Vec<u8>,
        signature: Vec<u8>,
        artifact: Vec<u8>,
    }
    struct SignedFixture {
        key: Ed25519KeyPair,
    }

    impl SignedFixture {
        fn new() -> Self {
            Self {
                key: Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap(),
            }
        }

        fn release(&self, version: &str) -> FixtureRelease {
            self.release_with_overlay_ownership(version, FileOwnership::Managed)
        }

        fn legacy_release(&self, version: &str) -> FixtureRelease {
            self.release_with_overlay_ownership(version, FileOwnership::Seed)
        }

        fn release_with_overlay_ownership(
            &self,
            version: &str,
            overlay_ownership: FileOwnership,
        ) -> FixtureRelease {
            let temp = crate::test_support::tempdir().unwrap();
            let archive_path = temp.path().join("launcher.zip");
            let exe = b"fixture launcher executable";
            let helper = b"fixture trusted update helper";
            let overlay_manifest = b"fixture official overlay manifest";
            let file = File::create(&archive_path).unwrap();
            let mut archive = zip::ZipWriter::new(file);
            archive
                .start_file(
                    "bahamut-launcher.exe",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            archive.write_all(exe).unwrap();
            archive
                .start_file(UPDATE_HELPER_FILE, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(helper).unwrap();
            archive
                .start_file(
                    OFFICIAL_OVERLAY_MANIFEST,
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            archive.write_all(overlay_manifest).unwrap();
            archive.finish().unwrap();
            let artifact = fs::read(&archive_path).unwrap();
            let artifact_hash = sha256_hex(&artifact);
            let metadata = ReleaseMetadata {
                schema_version: 1,
                product: Product::Launcher,
                channel: Channel::Stable,
                target: Target::WindowsX86_64,
                version: version.into(),
                artifact: release::ArtifactIdentity {
                    object_key: format!("launcher/{version}/windows-x86_64/{artifact_hash}.zip"),
                    format: ArtifactFormat::Zip,
                    length: artifact.len() as u64,
                    sha256: artifact_hash,
                },
                inventory: ReleaseInventory::Launcher {
                    files: vec![
                        release::InventoryFile {
                            path: "bahamut-launcher.exe".into(),
                            length: exe.len() as u64,
                            sha256: sha256_hex(exe),
                            ownership: FileOwnership::Managed,
                        },
                        release::InventoryFile {
                            path: UPDATE_HELPER_FILE.into(),
                            length: helper.len() as u64,
                            sha256: sha256_hex(helper),
                            ownership: FileOwnership::Managed,
                        },
                        release::InventoryFile {
                            path: OFFICIAL_OVERLAY_MANIFEST.into(),
                            length: overlay_manifest.len() as u64,
                            sha256: sha256_hex(overlay_manifest),
                            ownership: overlay_ownership,
                        },
                    ],
                },
            };
            let bytes = serde_json::to_vec(&metadata).unwrap();
            let signature = self.key.sign(&bytes).as_ref().to_vec();
            FixtureRelease {
                metadata: bytes,
                signature,
                artifact,
            }
        }

        fn prepare_root(&self, root: &Path, version: &str) {
            fs::create_dir(root.join("config")).unwrap();
            fs::write(root.join("config/key.bin"), self.key.public_key().as_ref()).unwrap();
            let bootstrap = self.legacy_release(version);
            fs::write(root.join("config/bootstrap.json"), bootstrap.metadata).unwrap();
            fs::write(root.join("config/bootstrap.sig"), bootstrap.signature).unwrap();
        }

        fn write_config(&self, root: &Path, base_url: &str, bootstrap_version: Option<&str>) {
            let config = if let Some(version) = bootstrap_version {
                serde_json::json!({"schemaVersion":1,"metadataUrl":format!("{base_url}/metadata"),"signatureUrl":format!("{base_url}/signature"),"artifactRootUrl":base_url,"trustedPublicKeyPath":"config/key.bin","bootstrap":{"version":version,"metadataPath":"config/bootstrap.json","signaturePath":"config/bootstrap.sig"}})
            } else {
                serde_json::json!({"schemaVersion":1,"metadataUrl":format!("{base_url}/metadata"),"signatureUrl":format!("{base_url}/signature"),"artifactRootUrl":base_url,"trustedPublicKeyPath":"config/key.bin"})
            };
            fs::write(
                root.join("config/launcher-updates.json"),
                serde_json::to_vec(&config).unwrap(),
            )
            .unwrap();
        }

        fn serve(&self, responses: &[Vec<u8>]) -> (String, thread::JoinHandle<()>) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let responses = responses.to_vec();
            let server = thread::spawn(move || {
                for body in responses {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut request = [0_u8; 4096];
                    let _ = stream.read(&mut request);
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                    stream.write_all(&body).unwrap();
                }
            });
            (format!("http://{address}"), server)
        }
    }

    fn counted_fixture_server(counter: Arc<AtomicUsize>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for _ in 0..40 {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        let response =
                            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = stream.write_all(response);
                        return;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => return,
                }
            }
        });
        (format!("http://{address}"), server)
    }
}
