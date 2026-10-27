//! Prepare a transaction and apply the new managed files.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::release::{InventoryFile, VerifiedRelease, verify_artifact_file};

use super::fsutil::{ensure_safe_inventory_path, safe_join, write_json_atomic};
use super::hooks::{maybe_test_boundary, maybe_test_interruption};
use super::inventory::{
    inspect_file, managed_files, same_bytes, same_file_identity, validate_installed_inventory,
    validate_transition_destinations, validate_tree_inventory,
};
use super::overlay::{
    adopt_legacy_overlay_manifest, reject_legacy_overlay_extras, transition_old_files,
};
use super::payload::{copy_new_file, extract_managed_payload, install_file_atomically};
use super::pointer::publish_pointer;
use super::snapshot::{capture_previous_files, repair_snapshot_identity_map};
use super::transaction::write_journal;
use super::verify::verified_release;
use super::{
    BACKUP_DIR, JOURNAL_SCHEMA, Journal, NEW_METADATA_FILE, NEW_SIGNATURE_FILE, OLD_METADATA_FILE,
    OLD_SIGNATURE_FILE, PAYLOAD_DIR, Phase, Result, SNAPSHOT_FILE, Snapshot, TransactionMode,
    invalid,
};

pub(super) struct ApplyContext<'a> {
    pub(super) mode: TransactionMode,
    pub(super) target: &'a Path,
    pub(super) transaction: &'a Path,
    pub(super) metadata_path: &'a Path,
    pub(super) signature_path: &'a Path,
    pub(super) previous_metadata_path: &'a Path,
    pub(super) previous_signature_path: &'a Path,
    pub(super) new_release: &'a VerifiedRelease,
    pub(super) previous_release: &'a VerifiedRelease,
    pub(super) artifact_path: &'a Path,
    pub(super) public_key: &'a [u8],
}

pub(super) fn prepare_and_apply(context: ApplyContext<'_>) -> Result<()> {
    let ApplyContext {
        mode,
        target,
        transaction,
        metadata_path,
        signature_path,
        previous_metadata_path,
        previous_signature_path,
        new_release,
        previous_release,
        artifact_path,
        public_key,
    } = context;
    copy_new_file(metadata_path, &transaction.join(NEW_METADATA_FILE))?;
    copy_new_file(signature_path, &transaction.join(NEW_SIGNATURE_FILE))?;
    copy_new_file(previous_metadata_path, &transaction.join(OLD_METADATA_FILE))?;
    copy_new_file(
        previous_signature_path,
        &transaction.join(OLD_SIGNATURE_FILE),
    )?;
    maybe_test_boundary("signed-inputs-copied");
    let staged_new = verified_release(
        &transaction.join(NEW_METADATA_FILE),
        &transaction.join(NEW_SIGNATURE_FILE),
        public_key,
    )?;
    let staged_previous = verified_release(
        &transaction.join(OLD_METADATA_FILE),
        &transaction.join(OLD_SIGNATURE_FILE),
        public_key,
    )?;
    if staged_new.metadata_sha256 != new_release.metadata_sha256
        || staged_previous.metadata_sha256 != previous_release.metadata_sha256
    {
        return Err(invalid(
            "Signed release inputs changed while preparing the transaction.",
        ));
    }
    let mut journal = Journal {
        schema_version: JOURNAL_SCHEMA,
        phase: Phase::Prepared,
        mode,
        target: target.to_path_buf(),
        new_metadata_sha256: staged_new.metadata_sha256.clone(),
        old_metadata_sha256: staged_previous.metadata_sha256.clone(),
        trusted_key_sha256: staged_new.trusted_key_sha256.clone(),
    };
    write_journal(transaction, &journal)?;
    maybe_test_boundary("prepared-journal-written");
    publish_pointer(
        target,
        transaction,
        &journal.new_metadata_sha256,
        &journal.old_metadata_sha256,
    )?;
    maybe_test_boundary("prepared-pointer-published");

    verify_artifact_file(&staged_new.metadata, artifact_path)?;
    let payload = transaction.join(PAYLOAD_DIR);
    fs::create_dir(&payload)?;
    let new_files = managed_files(&staged_new.metadata)?;
    extract_managed_payload(artifact_path, &payload, &staged_new.metadata)?;
    validate_tree_inventory(&payload, &new_files)?;
    maybe_test_boundary("payload-extracted");

    let mut old_files = if mode == TransactionMode::Update {
        transition_old_files(&staged_previous.metadata, &staged_new.metadata)?
    } else {
        managed_files(&staged_previous.metadata)?
    };
    let legacy_overlay = if mode == TransactionMode::Update {
        reject_legacy_overlay_extras(target, &staged_previous.metadata, &staged_new.metadata)?;
        adopt_legacy_overlay_manifest(target, &mut old_files)?
    } else {
        None
    };
    validate_transition_destinations(target, &staged_new.metadata, &old_files)?;
    let backup_root = transaction.join(BACKUP_DIR);
    fs::create_dir(&backup_root)?;
    let mut snapshot = capture_previous_files(target, &backup_root, &old_files, mode)?;
    if let Some(identity) = legacy_overlay {
        snapshot.previous_managed_files.push(identity);
    }
    write_json_atomic(&transaction.join(SNAPSHOT_FILE), &snapshot)?;
    maybe_test_boundary("backup-snapshot-written");
    journal.phase = Phase::Applying;
    write_journal(transaction, &journal)?;

    apply_managed_files(
        target,
        transaction,
        &payload,
        &old_files,
        &new_files,
        &snapshot,
        mode,
    )?;
    validate_installed_inventory(target, &staged_new.metadata)?;
    journal.phase = Phase::AwaitingValidation;
    write_journal(transaction, &journal)?;
    Ok(())
}

fn apply_managed_files(
    target: &Path,
    transaction: &Path,
    payload: &Path,
    old_files: &[InventoryFile],
    new_files: &[InventoryFile],
    snapshot: &Snapshot,
    mode: TransactionMode,
) -> Result<()> {
    if mode == TransactionMode::Repair {
        return apply_repair_managed_files(target, transaction, payload, new_files, snapshot);
    }

    let present_old = snapshot
        .present_old_managed
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let old = old_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let new = new_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();

    let mut applied_steps = 0;
    for new_file in new_files {
        let destination = safe_join(target, &new_file.path)?;
        ensure_safe_inventory_path(target, &new_file.path, false)?;
        let current = inspect_file(&destination)?;
        if let Some(previous) = old.get(new_file.path.as_str()) {
            let was_present = present_old.contains(new_file.path.as_str());
            if (was_present
                && current
                    .as_ref()
                    .is_none_or(|identity| !same_bytes(identity, previous)))
                || (!was_present && current.is_some())
            {
                return Err(invalid("A previously managed file changed during update."));
            }
        } else if current.is_some() {
            return Err(invalid(
                "A new managed path became occupied during update; preserving it.",
            ));
        }
        let source = safe_join(payload, &new_file.path)?;
        install_file_atomically(transaction, target, &source, &destination)?;
        applied_steps += 1;
        maybe_test_interruption(applied_steps);
    }

    for old_file in old_files {
        if new.contains_key(old_file.path.as_str()) {
            continue;
        }
        let destination = safe_join(target, &old_file.path)?;
        ensure_safe_inventory_path(target, &old_file.path, false)?;
        if let Some(current) = inspect_file(&destination)? {
            if !same_bytes(&current, old_file) {
                return Err(invalid("An obsolete managed file changed during update."));
            }
            fs::remove_file(destination)?;
            applied_steps += 1;
            maybe_test_interruption(applied_steps);
        }
    }
    Ok(())
}

fn apply_repair_managed_files(
    target: &Path,
    transaction: &Path,
    payload: &Path,
    new_files: &[InventoryFile],
    snapshot: &Snapshot,
) -> Result<()> {
    let previous = repair_snapshot_identity_map(snapshot, new_files)?;
    let mut applied_steps = 0;
    for new_file in new_files {
        let destination = safe_join(target, &new_file.path)?;
        ensure_safe_inventory_path(target, &new_file.path, false)?;
        let current = inspect_file(&destination)?;
        match (previous.get(&new_file.path), current.as_ref()) {
            (Some(expected), Some(actual)) if same_file_identity(actual, expected) => {}
            (None, None) => {}
            (Some(_), _) | (None, Some(_)) => {
                return Err(invalid(
                    "A managed file changed while repair was being prepared; preserving it.",
                ));
            }
        }
        if current
            .as_ref()
            .is_some_and(|identity| same_bytes(identity, new_file))
        {
            continue;
        }
        let source = safe_join(payload, &new_file.path)?;
        install_file_atomically(transaction, target, &source, &destination)?;
        applied_steps += 1;
        maybe_test_interruption(applied_steps);
    }
    Ok(())
}
