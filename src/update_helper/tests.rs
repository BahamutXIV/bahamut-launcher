use std::fs;
use std::path::Path;

use super::pointer::{publish_pointer, validate_launcher_handoff, validate_pointer_identity};
use super::{JOURNAL_SCHEMA, Journal, PENDING_FILE, Phase, TransactionMode};

fn test_journal(target: &Path) -> Journal {
    Journal {
        schema_version: JOURNAL_SCHEMA,
        phase: Phase::AwaitingValidation,
        mode: TransactionMode::Update,
        target: target.to_path_buf(),
        new_metadata_sha256: "a".repeat(64),
        old_metadata_sha256: "b".repeat(64),
        trusted_key_sha256: "c".repeat(64),
    }
}

#[test]
fn explicit_transaction_rejects_a_pointer_to_another_existing_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    let stage = directory.path().join("stage");
    let pointed_transaction = stage.join("transaction-a");
    let explicit_transaction = stage.join("transaction-b");
    fs::create_dir_all(target.join("config")).unwrap();
    fs::create_dir_all(&pointed_transaction).unwrap();
    fs::create_dir_all(&explicit_transaction).unwrap();
    let journal = test_journal(&target);
    publish_pointer(
        &target,
        &pointed_transaction,
        &journal.new_metadata_sha256,
        &journal.old_metadata_sha256,
    )
    .unwrap();

    assert!(validate_pointer_identity(&target, &explicit_transaction, &journal).is_err());
}

#[test]
fn explicit_transaction_rejects_a_pointer_to_another_missing_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    let stage = directory.path().join("stage");
    let pointed_transaction = stage.join("transaction-a");
    let explicit_transaction = stage.join("transaction-b");
    fs::create_dir_all(target.join("config")).unwrap();
    fs::create_dir_all(&pointed_transaction).unwrap();
    fs::create_dir_all(&explicit_transaction).unwrap();
    let journal = test_journal(&target);
    publish_pointer(
        &target,
        &pointed_transaction,
        &journal.new_metadata_sha256,
        &journal.old_metadata_sha256,
    )
    .unwrap();
    fs::remove_dir(&pointed_transaction).unwrap();

    assert!(validate_pointer_identity(&target, &explicit_transaction, &journal).is_err());
}

#[test]
fn launcher_handoff_requires_ownership_by_the_recorded_helper_process() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    let stage = directory.path().join("stage");
    fs::create_dir_all(target.join("config")).unwrap();
    fs::create_dir(&stage).unwrap();
    let new_metadata_sha256 = "a".repeat(64);
    let old_metadata_sha256 = "b".repeat(64);
    let transaction = stage.join(format!("bahamut-update-{new_metadata_sha256}"));
    let receipt = serde_json::json!({
        "schemaVersion": 1,
        "target": target,
        "stage": stage,
        "transaction": transaction,
        "operation": "update",
        "helperPid": std::process::id(),
        "previousMetadataSha256": old_metadata_sha256,
        "newMetadataSha256": new_metadata_sha256,
    });
    let receipt_path = target.join(PENDING_FILE);
    fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();

    validate_launcher_handoff(
        &target,
        &stage,
        &transaction,
        TransactionMode::Update,
        &new_metadata_sha256,
        &old_metadata_sha256,
    )
    .unwrap();

    let mut stale_receipt = receipt;
    stale_receipt["helperPid"] = serde_json::json!(std::process::id().wrapping_add(1));
    fs::write(&receipt_path, serde_json::to_vec(&stale_receipt).unwrap()).unwrap();
    assert!(
        validate_launcher_handoff(
            &target,
            &stage,
            &transaction,
            TransactionMode::Update,
            &new_metadata_sha256,
            &old_metadata_sha256,
        )
        .is_err()
    );
}
