#![cfg(windows)]

use std::fs::{self, OpenOptions};
use std::io::{Cursor, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::Duration;

use bahamut_launcher::release::{
    ArtifactFormat, ArtifactIdentity, Channel, FileOwnership, InventoryFile, Product,
    ReleaseInventory, ReleaseMetadata, Target, sign_metadata,
};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

const HELPER: &str = env!("CARGO_BIN_EXE_bahamut-update-helper");
const TEST_NAME: &str = "helper_process_contract";
const TEST_PARENT_ENV: &str = "BAHAMUT_UPDATE_HELPER_TEST_PARENT";
const TEST_LOCK_FILE_ENV: &str = "BAHAMUT_UPDATE_HELPER_TEST_LOCK_FILE";
const TEST_CRASH_ENV: &str = "BAHAMUT_UPDATE_HELPER_TEST_CRASH_AFTER";
const TEST_CRASH_BOUNDARY_ENV: &str = "BAHAMUT_UPDATE_HELPER_TEST_CRASH_AT";

#[derive(Clone)]
struct PackageFile {
    path: String,
    bytes: Vec<u8>,
    ownership: FileOwnership,
}

struct SignedRelease {
    metadata_path: PathBuf,
    signature_path: PathBuf,
    artifact_path: PathBuf,
}

struct Fixture {
    _root: TempDir,
    target: PathBuf,
    stage: PathBuf,
    public_key_path: PathBuf,
    old: SignedRelease,
    new: SignedRelease,
}

#[derive(Serialize)]
struct InvalidScopeMetadata {
    schema_version: u32,
    product: Product,
    channel: Channel,
    target: Target,
    version: String,
    artifact: ArtifactIdentity,
    inventory: ReleaseInventory,
}

#[test]
fn helper_process_contract() {
    if let Some(path) = std::env::var_os(TEST_LOCK_FILE_ENV) {
        let _locked = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .unwrap();
        thread::sleep(Duration::from_millis(700));
        return;
    }
    if std::env::var_os(TEST_PARENT_ENV).is_some() {
        thread::sleep(Duration::from_millis(300));
        return;
    }

    update_adopts_official_overlay_and_preserves_unknown_files();
    independently_updated_overlay_manifest_is_adopted_and_restored();
    legacy_overlay_payload_is_preserved_and_blocks_adoption();
    interrupted_confirmation_can_finish_the_committed_release();
    launcher_can_reserve_exact_staged_transaction_path();
    repair_restores_damaged_managed_files_and_confirms();
    interrupted_repair_restores_exact_previous_bytes();
    repair_rejects_a_different_release_identity();
    interrupted_apply_is_discovered_and_rolled_back();
    early_attempt_interruptions_are_recoverable_and_retryable();
    corrupted_payload_is_rejected_before_mutation();
    signed_wrong_target_is_rejected();
    locked_launcher_file_rolls_back_partial_apply();
    startup_failure_recovery_waits_for_shell_exit();
    recovery_restart_launches_the_restored_launcher();
}

fn update_adopts_official_overlay_and_preserves_unknown_files() {
    let fixture = fixture();
    let output = run_apply(&fixture, false);
    assert!(output.status.success(), "{}", output_text(&output));
    let transaction = pointer_transaction(&fixture.target);
    assert!(transaction.join("backup/bahamut-launcher.exe").is_file());
    assert!(
        transaction
            .join("backup/plugins/dats/bahamut-dats-overlay/overlay.toml")
            .is_file()
    );
    assert_eq!(
        fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
        b"launcher-v2"
    );
    assert_eq!(
        fs::read(fixture.target.join("bahamut-update-helper.exe")).unwrap(),
        b"helper-v2"
    );
    assert!(!fixture.target.join("addons/fps/fps.lua").exists());
    assert_eq!(
        fs::read(fixture.target.join("scripts/default.txt")).unwrap(),
        b"player script"
    );
    assert_eq!(
        fs::read(
            fixture
                .target
                .join("plugins/dats/bahamut-dats-overlay/overlay.toml")
        )
        .unwrap(),
        b"release overlay config v2"
    );
    assert_eq!(
        fs::read(fixture.target.join("addons/custom/addon.toml")).unwrap(),
        b"user addon"
    );

    let output = run_helper(&[
        "confirm".into(),
        "--target".into(),
        as_arg(&fixture.target),
        "--public-key".into(),
        as_arg(&fixture.public_key_path),
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(!transaction.exists());
    assert!(
        !fixture
            .target
            .join("config/launcher-update-state.json")
            .exists()
    );
}

fn legacy_overlay_payload_is_preserved_and_blocks_adoption() {
    let fixture = fixture();
    let legacy_dat = fixture
        .target
        .join("plugins/dats/bahamut-dats-overlay/data/legacy.dat");
    fs::create_dir_all(legacy_dat.parent().unwrap()).unwrap();
    fs::write(&legacy_dat, b"older independent overlay payload").unwrap();

    let output = run_apply(&fixture, false);
    assert!(!output.status.success());
    assert!(output_text(&output).contains("legacy official overlay has extra files"));
    assert_eq!(
        fs::read(&legacy_dat).unwrap(),
        b"older independent overlay payload"
    );
    assert_eq!(
        fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
        b"launcher-v1"
    );
}

fn independently_updated_overlay_manifest_is_adopted_and_restored() {
    let fixture = fixture();
    let manifest = fixture
        .target
        .join("plugins/dats/bahamut-dats-overlay/overlay.toml");
    fs::write(&manifest, b"previous independent overlay manifest").unwrap();

    let output = run_apply(&fixture, false);
    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(fs::read(&manifest).unwrap(), b"release overlay config v2");

    let output = run_recover(&fixture);
    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(
        fs::read(&manifest).unwrap(),
        b"previous independent overlay manifest"
    );
}

fn interrupted_confirmation_can_finish_the_committed_release() {
    for action in ["confirm", "recover"] {
        let fixture = fixture();
        let output = run_apply(&fixture, false);
        assert!(output.status.success(), "{}", output_text(&output));
        let transaction = pointer_transaction(&fixture.target);
        let public_key = fs::read(&fixture.public_key_path).unwrap();
        let rollback = bahamut_launcher::update_helper::recovery_release(
            &fixture.target,
            &transaction,
            &public_key,
        )
        .unwrap();
        assert_eq!(rollback.metadata.version, "1.0.0");
        let journal_path = transaction.join("transaction.json");
        let mut journal: serde_json::Value =
            serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        // Confirmation can stop after persisting its phase but before clearing the pointer.
        journal["phase"] = "committed".into();
        fs::write(&journal_path, serde_json::to_vec(&journal).unwrap()).unwrap();
        let committed = bahamut_launcher::update_helper::recovery_release(
            &fixture.target,
            &transaction,
            &public_key,
        )
        .unwrap();
        assert_eq!(committed.metadata.version, "2.0.0");

        let output = run_helper(&[
            action.into(),
            "--target".into(),
            as_arg(&fixture.target),
            "--transaction".into(),
            as_arg(&transaction),
            "--public-key".into(),
            as_arg(&fixture.public_key_path),
        ]);
        assert!(output.status.success(), "{}", output_text(&output));
        assert_eq!(
            fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
            b"launcher-v2"
        );
        assert!(!transaction.exists());
        assert!(
            !fixture
                .target
                .join("config/launcher-update-state.json")
                .exists()
        );
    }
}

fn launcher_can_reserve_exact_staged_transaction_path() {
    let fixture = fixture();
    let metadata = fs::read(&fixture.new.metadata_path).unwrap();
    let canonical_stage = fs::canonicalize(&fixture.stage).unwrap();
    let transaction = canonical_stage.join(format!("bahamut-update-{}", sha256(&metadata)));
    let mut args = apply_args(&fixture);
    args.extend(["--transaction".into(), as_arg(&transaction)]);

    let output = run_helper_with_parent(&args, None);
    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(pointer_transaction(&fixture.target), transaction);

    let output = run_helper(&[
        "confirm".into(),
        "--target".into(),
        as_arg(&fixture.target),
        "--transaction".into(),
        as_arg(&transaction),
        "--public-key".into(),
        as_arg(&fixture.public_key_path),
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(!transaction.exists());
}

fn repair_restores_damaged_managed_files_and_confirms() {
    let fixture = fixture();
    fs::write(
        fixture.target.join("addons/fps/fps.lua"),
        b"corrupt addon bytes",
    )
    .unwrap();
    fs::remove_file(fixture.target.join("bahamut-update-helper.exe")).unwrap();

    let output = run_repair(&fixture, None);
    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(
        fs::read(fixture.target.join("addons/fps/fps.lua")).unwrap(),
        b"old fps"
    );
    assert_eq!(
        fs::read(fixture.target.join("bahamut-update-helper.exe")).unwrap(),
        b"helper-v1"
    );
    assert_eq!(
        fs::read(fixture.target.join("scripts/default.txt")).unwrap(),
        b"player script"
    );
    assert_eq!(
        fs::read(
            fixture
                .target
                .join("plugins/dats/bahamut-dats-overlay/overlay.toml")
        )
        .unwrap(),
        b"release overlay config v1"
    );
    assert_eq!(
        fs::read(fixture.target.join("addons/custom/addon.toml")).unwrap(),
        b"user addon"
    );

    let output = run_helper(&[
        "confirm".into(),
        "--target".into(),
        as_arg(&fixture.target),
        "--public-key".into(),
        as_arg(&fixture.public_key_path),
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(String::from_utf8_lossy(&output.stdout).contains("confirmed 1.0.0"));
    assert!(
        !fixture
            .target
            .join("config/launcher-update-state.json")
            .exists()
    );
}

fn interrupted_repair_restores_exact_previous_bytes() {
    if !cfg!(debug_assertions) {
        return;
    }
    let fixture = fixture();
    fs::write(
        fixture.target.join("bahamut-launcher.exe"),
        b"pre-repair corrupt launcher",
    )
    .unwrap();
    fs::remove_file(fixture.target.join("bahamut-update-helper.exe")).unwrap();

    let output = run_repair(&fixture, Some("2"));
    assert_eq!(output.status.code(), Some(86), "{}", output_text(&output));
    assert_eq!(
        fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
        b"launcher-v1"
    );
    assert_eq!(
        fs::read(fixture.target.join("bahamut-update-helper.exe")).unwrap(),
        b"helper-v1"
    );

    let output = run_recover(&fixture);
    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(
        fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
        b"pre-repair corrupt launcher"
    );
    assert!(!fixture.target.join("bahamut-update-helper.exe").exists());
    assert_eq!(
        fs::read(fixture.target.join("scripts/default.txt")).unwrap(),
        b"player script"
    );
    assert_eq!(
        fs::read(fixture.target.join("addons/custom/addon.toml")).unwrap(),
        b"user addon"
    );
    assert!(
        !fixture
            .target
            .join("config/launcher-update-state.json")
            .exists()
    );
}

fn repair_rejects_a_different_release_identity() {
    let fixture = fixture();
    let output = run_helper_with_parent(&repair_args(&fixture, &fixture.new, &fixture.old), None);
    assert!(!output.status.success());
    assert!(output_text(&output).contains("exact same trusted release metadata identity"));
    assert_eq!(
        fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
        b"launcher-v1"
    );
    assert!(
        !fixture
            .target
            .join("config/launcher-update-state.json")
            .exists()
    );
}

fn interrupted_apply_is_discovered_and_rolled_back() {
    if !cfg!(debug_assertions) {
        return;
    }
    let fixture = fixture();
    let output = run_apply(&fixture, true);
    assert_eq!(output.status.code(), Some(86), "{}", output_text(&output));
    let transaction = pointer_transaction(&fixture.target);
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(transaction.join("transaction.json")).unwrap()).unwrap();
    assert_eq!(journal["phase"], "applying");
    assert_eq!(
        fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
        b"launcher-v2"
    );

    for (filename, field) in [
        ("transaction.json", "mode"),
        ("snapshot.json", "previous_managed_files"),
    ] {
        let path = transaction.join(filename);
        let original = fs::read(&path).unwrap();
        let mut incomplete: serde_json::Value = serde_json::from_slice(&original).unwrap();
        incomplete.as_object_mut().unwrap().remove(field);
        let incomplete = serde_json::to_vec(&incomplete).unwrap();
        fs::write(&path, &incomplete).unwrap();

        let output = run_recover(&fixture);
        assert!(!output.status.success(), "{}", output_text(&output));
        assert!(
            output_text(&output).contains(&format!("missing field `{field}`")),
            "{}",
            output_text(&output)
        );
        assert_eq!(
            fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
            b"launcher-v2"
        );
        assert_eq!(pointer_transaction(&fixture.target), transaction);
        assert_eq!(fs::read(&path).unwrap(), incomplete);
        fs::write(path, original).unwrap();
    }

    let output = run_helper(&[
        "recover".into(),
        "--target".into(),
        as_arg(&fixture.target),
        "--public-key".into(),
        as_arg(&fixture.public_key_path),
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(
        fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
        b"launcher-v1"
    );
    assert_eq!(
        fs::read(fixture.target.join("bahamut-update-helper.exe")).unwrap(),
        b"helper-v1"
    );
    assert_eq!(
        fs::read(fixture.target.join("addons/fps/fps.lua")).unwrap(),
        b"old fps"
    );
    assert_eq!(
        fs::read(
            fixture
                .target
                .join("plugins/dats/bahamut-dats-overlay/overlay.toml")
        )
        .unwrap(),
        b"release overlay config v1"
    );
    assert!(
        !fixture
            .target
            .join("config/launcher-update-state.json")
            .exists()
    );
}

fn early_attempt_interruptions_are_recoverable_and_retryable() {
    if !cfg!(debug_assertions) {
        return;
    }
    for (boundary, pointer_expected) in [
        ("attempt-created", false),
        ("signed-inputs-copied", false),
        ("prepared-journal-written", false),
        ("prepared-pointer-published", true),
        ("payload-extracted", true),
        ("backup-snapshot-written", true),
    ] {
        let fixture = fixture();
        let output = run_apply_at(&fixture, boundary);
        assert_eq!(
            output.status.code(),
            Some(86),
            "boundary {boundary}: {}",
            output_text(&output)
        );
        assert_eq!(
            fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
            b"launcher-v1",
            "boundary {boundary} changed the target before Applying"
        );

        let attempt = if pointer_expected {
            let transaction = pointer_transaction(&fixture.target);
            let journal: serde_json::Value =
                serde_json::from_slice(&fs::read(transaction.join("transaction.json")).unwrap())
                    .unwrap();
            assert_eq!(journal["phase"], "prepared", "boundary {boundary}");
            let output = run_recover(&fixture);
            assert!(
                output.status.success(),
                "boundary {boundary}: {}",
                output_text(&output)
            );
            assert!(!transaction.exists(), "boundary {boundary}");
            transaction
        } else {
            let attempts = transaction_attempts(&fixture.stage);
            assert_eq!(attempts.len(), 1, "boundary {boundary}");
            let output = run_recover(&fixture);
            assert!(
                output.status.success(),
                "boundary {boundary}: {}",
                output_text(&output)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("no pending transaction"),
                "boundary {boundary}: {}",
                output_text(&output)
            );
            attempts.into_iter().next().unwrap()
        };

        let output = run_apply(&fixture, false);
        assert!(
            output.status.success(),
            "same-release retry after {boundary}: {}",
            output_text(&output)
        );
        let retry = pointer_transaction(&fixture.target);
        assert_ne!(retry, attempt, "boundary {boundary}");
        assert_eq!(
            fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
            b"launcher-v2",
            "same-release retry after {boundary}"
        );
        let output = run_helper(&[
            "confirm".into(),
            "--target".into(),
            as_arg(&fixture.target),
            "--public-key".into(),
            as_arg(&fixture.public_key_path),
        ]);
        assert!(
            output.status.success(),
            "confirm after {boundary}: {}",
            output_text(&output)
        );
    }
}

fn run_recover(fixture: &Fixture) -> Output {
    run_helper(&[
        "recover".into(),
        "--target".into(),
        as_arg(&fixture.target),
        "--public-key".into(),
        as_arg(&fixture.public_key_path),
    ])
}

fn corrupted_payload_is_rejected_before_mutation() {
    let fixture = fixture();
    fs::write(&fixture.new.artifact_path, b"tampered archive bytes").unwrap();
    let output = run_apply(&fixture, false);
    assert!(!output.status.success());
    assert!(output_text(&output).contains("Release artifact length or SHA-256 does not match"));
    assert_eq!(
        fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
        b"launcher-v1"
    );
    assert!(
        !fixture
            .target
            .join("config/launcher-update-state.json")
            .exists()
    );
}

fn signed_wrong_target_is_rejected() {
    let fixture = fixture();
    let bad_dir = fixture._root.path().join("other-scope");
    fs::create_dir(&bad_dir).unwrap();
    let archive = zip_bytes(&[]);
    let digest = sha256(&archive);
    let artifact = ArtifactIdentity {
        object_key: format!("launcher/3.0.0/linux-x86_64/{digest}.tar.gz"),
        format: ArtifactFormat::TarGz,
        length: archive.len() as u64,
        sha256: digest,
    };
    let metadata = InvalidScopeMetadata {
        schema_version: 1,
        product: Product::Launcher,
        channel: Channel::Stable,
        target: Target::LinuxX86_64,
        version: "3.0.0".into(),
        artifact,
        inventory: ReleaseInventory::Launcher {
            files: vec![InventoryFile {
                path: "LICENSE.md".into(),
                length: 0,
                sha256: sha256(b""),
                ownership: FileOwnership::Managed,
            }],
        },
    };
    let metadata_bytes = serde_json::to_vec(&metadata).unwrap();
    let seed = [0x4a; 32];
    let signature = sign_metadata(&metadata_bytes, &seed, None).unwrap();
    let metadata_path = bad_dir.join("metadata.json");
    let signature_path = bad_dir.join("signature.bin");
    let artifact_path = bad_dir.join("overlay.zip");
    fs::write(metadata_path, metadata_bytes).unwrap();
    fs::write(signature_path, signature).unwrap();
    fs::write(&artifact_path, archive).unwrap();

    let output = run_helper_with_parent(
        &[
            "apply".into(),
            "--target".into(),
            as_arg(&fixture.target),
            "--stage".into(),
            as_arg(&fixture.stage),
            "--metadata".into(),
            as_arg(&bad_dir.join("metadata.json")),
            "--signature".into(),
            as_arg(&bad_dir.join("signature.bin")),
            "--public-key".into(),
            as_arg(&fixture.public_key_path),
            "--artifact".into(),
            as_arg(&artifact_path),
            "--previous-metadata".into(),
            as_arg(&fixture.old.metadata_path),
            "--previous-signature".into(),
            as_arg(&fixture.old.signature_path),
        ],
        None,
    );
    assert!(!output.status.success());
    assert!(
        output_text(&output).contains("Signed release product, channel, or target does not match")
    );
    assert_eq!(
        fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
        b"launcher-v1"
    );
}

fn locked_launcher_file_rolls_back_partial_apply() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("install");
    let stage = root.path().join("stage");
    let work = root.path().join("inputs");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(&stage).unwrap();
    fs::create_dir_all(&work).unwrap();
    let seed = [0x4a; 32];
    let key_pair = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
    let public_key_path = work.join("public-key.bin");
    fs::write(&public_key_path, key_pair.public_key().as_ref()).unwrap();

    let old_files = vec![managed("bahamut-launcher.exe", b"launcher-v1")];
    let new_files = vec![
        managed("addons/fps/fps.lua", b"new standalone file"),
        managed("bahamut-launcher.exe", b"launcher-v2"),
    ];
    let old = create_release(&work, "1.0.0", &old_files, &seed, "old");
    let new = create_release(&work, "2.0.0", &new_files, &seed, "new");
    write_package_tree(&target, &old_files);
    let locked = OpenOptions::new()
        .read(true)
        .share_mode(1 | 2)
        .open(target.join("bahamut-launcher.exe"))
        .unwrap();

    let fixture = Fixture {
        _root: root,
        target,
        stage,
        public_key_path,
        old,
        new,
    };
    let output = run_apply(&fixture, false);
    drop(locked);
    assert!(!output.status.success(), "{}", output_text(&output));
    assert!(output_text(&output).contains("I/O failed"));
    assert_eq!(
        fs::read(fixture.target.join("bahamut-launcher.exe")).unwrap(),
        b"launcher-v1"
    );
    assert!(!fixture.target.join("addons/fps/fps.lua").exists());
    assert!(
        !fixture
            .target
            .join("config/launcher-update-state.json")
            .exists()
    );
}

fn startup_failure_recovery_waits_for_shell_exit() {
    let fixture = fixture();
    let output = run_apply(&fixture, false);
    assert!(output.status.success(), "{}", output_text(&output));
    let executable = fixture.target.join("bahamut-launcher.exe");
    let mut shell = spawn_locked_shell(&executable);
    let started = std::time::Instant::now();
    let output = run_helper(&[
        "recover".into(),
        "--target".into(),
        as_arg(&fixture.target),
        "--public-key".into(),
        as_arg(&fixture.public_key_path),
        "--wait-pid".into(),
        shell.id().to_string(),
    ]);
    let elapsed = started.elapsed();
    assert!(shell.wait().unwrap().success());
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(elapsed >= Duration::from_millis(500));
    assert_eq!(fs::read(executable).unwrap(), b"launcher-v1");
    assert_eq!(
        fs::read(fixture.target.join("bahamut-update-helper.exe")).unwrap(),
        b"helper-v1"
    );
    assert!(
        !fixture
            .target
            .join("config/launcher-update-state.json")
            .exists()
    );
}

fn recovery_restart_launches_the_restored_launcher() {
    let previous_launcher = fs::read(HELPER).unwrap();
    let fixture = fixture_with_launcher_bytes(previous_launcher.clone());
    let output = run_apply(&fixture, false);
    assert!(output.status.success(), "{}", output_text(&output));
    let executable = fixture.target.join("bahamut-launcher.exe");
    let mut shell = spawn_locked_shell(&executable);
    let args = vec![
        "recover".to_owned(),
        "--target".to_owned(),
        as_arg(&fixture.target),
        "--public-key".to_owned(),
        as_arg(&fixture.public_key_path),
        "--wait-pid".to_owned(),
        shell.id().to_string(),
        "--restart".to_owned(),
    ];
    let output = Command::new(HELPER)
        .args(args)
        .env(TEST_PARENT_ENV, "1")
        .output()
        .unwrap();
    assert!(shell.wait().unwrap().success());
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("restarted launcher process"),
        "{}",
        output_text(&output)
    );
    assert_eq!(fs::read(executable).unwrap(), previous_launcher);
    assert!(
        !fixture
            .target
            .join("config/launcher-update-state.json")
            .exists()
    );
}

fn fixture() -> Fixture {
    fixture_with_launcher_bytes(b"launcher-v1".to_vec())
}

fn fixture_with_launcher_bytes(launcher_bytes: Vec<u8>) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("install");
    let stage = root.path().join("stage");
    let work = root.path().join("inputs");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(target.join("config")).unwrap();
    fs::create_dir_all(&stage).unwrap();
    fs::create_dir_all(&work).unwrap();
    let seed = [0x4a; 32];
    let key_pair = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
    let public_key_path = work.join("public-key.bin");
    fs::write(&public_key_path, key_pair.public_key().as_ref()).unwrap();

    let old_files = vec![
        managed("bahamut-launcher.exe", &launcher_bytes),
        managed("bahamut-update-helper.exe", b"helper-v1"),
        managed("addons/fps/addon.toml", b"[addon]\nversion = 1"),
        managed("addons/fps/fps.lua", b"old fps"),
        seed_file("scripts/default.txt", b"release seed v1"),
        seed_file(
            "plugins/dats/bahamut-dats-overlay/overlay.toml",
            b"release overlay config v1",
        ),
    ];
    let new_files = vec![
        managed("bahamut-launcher.exe", b"launcher-v2"),
        managed("bahamut-update-helper.exe", b"helper-v2"),
        managed("addons/fps/addon.toml", b"[addon]\nversion = 2"),
        seed_file("scripts/default.txt", b"release seed v2"),
        managed(
            "plugins/dats/bahamut-dats-overlay/overlay.toml",
            b"release overlay config v2",
        ),
    ];
    let old = create_release(&work, "1.0.0", &old_files, &seed, "old");
    let new = create_release(&work, "2.0.0", &new_files, &seed, "new");
    write_package_tree(&target, &old_files);
    fs::write(target.join("scripts/default.txt"), b"player script").unwrap();
    fs::create_dir_all(target.join("addons/custom")).unwrap();
    fs::write(target.join("addons/custom/addon.toml"), b"user addon").unwrap();
    Fixture {
        _root: root,
        target,
        stage,
        public_key_path,
        old,
        new,
    }
}

fn managed(path: &str, bytes: &[u8]) -> PackageFile {
    PackageFile {
        path: path.into(),
        bytes: bytes.to_vec(),
        ownership: FileOwnership::Managed,
    }
}

fn seed_file(path: &str, bytes: &[u8]) -> PackageFile {
    PackageFile {
        path: path.into(),
        bytes: bytes.to_vec(),
        ownership: FileOwnership::Seed,
    }
}

fn create_release(
    root: &Path,
    version: &str,
    files: &[PackageFile],
    seed: &[u8; 32],
    prefix: &str,
) -> SignedRelease {
    let archive = zip_bytes(files);
    let artifact_sha256 = sha256(&archive);
    let inventory = files
        .iter()
        .map(|file| InventoryFile {
            path: file.path.clone(),
            length: file.bytes.len() as u64,
            sha256: sha256(&file.bytes),
            ownership: file.ownership,
        })
        .collect();
    let metadata = ReleaseMetadata {
        schema_version: 1,
        product: Product::Launcher,
        channel: Channel::Stable,
        target: Target::WindowsX86_64,
        version: version.into(),
        artifact: ArtifactIdentity {
            object_key: format!("launcher/{version}/windows-x86_64/{artifact_sha256}.zip"),
            format: ArtifactFormat::Zip,
            length: archive.len() as u64,
            sha256: artifact_sha256,
        },
        inventory: ReleaseInventory::Launcher { files: inventory },
    };
    let metadata_bytes = serde_json::to_vec(&metadata).unwrap();
    let signature = sign_metadata(&metadata_bytes, seed, None).unwrap();
    let metadata_path = root.join(format!("{prefix}-metadata.json"));
    let signature_path = root.join(format!("{prefix}-signature.bin"));
    let artifact_path = root.join(format!("{prefix}-artifact.zip"));
    fs::write(&metadata_path, metadata_bytes).unwrap();
    fs::write(&signature_path, signature).unwrap();
    fs::write(&artifact_path, archive).unwrap();
    SignedRelease {
        metadata_path,
        signature_path,
        artifact_path,
    }
}

fn zip_bytes(files: &[PackageFile]) -> Vec<u8> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    for file in files {
        archive
            .start_file(&file.path, SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&file.bytes).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

fn write_package_tree(root: &Path, files: &[PackageFile]) {
    for file in files {
        let path = root.join(&file.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, &file.bytes).unwrap();
    }
}

fn apply_args(fixture: &Fixture) -> Vec<String> {
    vec![
        "apply".into(),
        "--target".into(),
        as_arg(&fixture.target),
        "--stage".into(),
        as_arg(&fixture.stage),
        "--metadata".into(),
        as_arg(&fixture.new.metadata_path),
        "--signature".into(),
        as_arg(&fixture.new.signature_path),
        "--public-key".into(),
        as_arg(&fixture.public_key_path),
        "--artifact".into(),
        as_arg(&fixture.new.artifact_path),
        "--previous-metadata".into(),
        as_arg(&fixture.old.metadata_path),
        "--previous-signature".into(),
        as_arg(&fixture.old.signature_path),
    ]
}

fn repair_args(
    fixture: &Fixture,
    replacement: &SignedRelease,
    previous: &SignedRelease,
) -> Vec<String> {
    vec![
        "repair".into(),
        "--target".into(),
        as_arg(&fixture.target),
        "--stage".into(),
        as_arg(&fixture.stage),
        "--metadata".into(),
        as_arg(&replacement.metadata_path),
        "--signature".into(),
        as_arg(&replacement.signature_path),
        "--public-key".into(),
        as_arg(&fixture.public_key_path),
        "--artifact".into(),
        as_arg(&replacement.artifact_path),
        "--previous-metadata".into(),
        as_arg(&previous.metadata_path),
        "--previous-signature".into(),
        as_arg(&previous.signature_path),
    ]
}

fn run_apply(fixture: &Fixture, interrupt: bool) -> Output {
    run_helper_with_parent(&apply_args(fixture), interrupt.then_some("1"))
}

fn run_repair(fixture: &Fixture, interrupt_after: Option<&str>) -> Output {
    run_helper_with_parent(
        &repair_args(fixture, &fixture.old, &fixture.old),
        interrupt_after,
    )
}

fn run_apply_at(fixture: &Fixture, boundary: &str) -> Output {
    run_helper_with_parent_at(&apply_args(fixture), None, Some(boundary))
}

fn run_helper_with_parent(args: &[String], failpoint: Option<&str>) -> Output {
    run_helper_with_parent_at(args, failpoint, None)
}

fn run_helper_with_parent_at(
    args: &[String],
    failpoint: Option<&str>,
    boundary: Option<&str>,
) -> Output {
    let mut parent = spawn_short_lived_parent();
    let mut command = Command::new(HELPER);
    command
        .args(args)
        .arg("--wait-pid")
        .arg(parent.id().to_string());
    if let Some(failpoint) = failpoint {
        command.env(TEST_CRASH_ENV, failpoint);
    }
    if let Some(boundary) = boundary {
        command.env(TEST_CRASH_BOUNDARY_ENV, boundary);
    }
    let output = command.output().unwrap();
    assert!(parent.wait().unwrap().success());
    output
}

fn spawn_short_lived_parent() -> Child {
    Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg(TEST_NAME)
        .arg("--nocapture")
        .env(TEST_PARENT_ENV, "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn spawn_locked_shell(executable: &Path) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg(TEST_NAME)
        .arg("--nocapture")
        .env(TEST_LOCK_FILE_ENV, executable)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn run_helper(args: &[String]) -> Output {
    Command::new(HELPER).args(args).output().unwrap()
}

fn pointer_transaction(target: &Path) -> PathBuf {
    let value: serde_json::Value = serde_json::from_slice(
        &fs::read(target.join("config/launcher-update-state.json")).unwrap(),
    )
    .unwrap();
    PathBuf::from(value["transaction"].as_str().unwrap())
}

fn transaction_attempts(stage: &Path) -> Vec<PathBuf> {
    fs::read_dir(stage)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("bahamut-update-"))
        })
        .collect()
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn as_arg(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
