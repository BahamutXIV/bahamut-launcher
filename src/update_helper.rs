//! Out-of-process portable launcher replacement.
//!
//! The helper accepts only a local Ed25519 public key and exact signed
//! metadata/artifact files. Its scope is fixed to the Windows launcher. The
//! helper does not fetch keys or release data and does not manage user files.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::release::{
    Channel, FileOwnership, InventoryFile, Product, ReleaseError, ReleaseInventory,
    ReleaseMetadata, ReleaseScope, Target, VerifiedRelease, read_metadata_file,
    read_public_key_file, read_signature_file, verify_artifact_file, verify_metadata,
};

const JOURNAL_FILE: &str = "transaction.json";
const NEW_METADATA_FILE: &str = "new-metadata.json";
const NEW_SIGNATURE_FILE: &str = "new-signature.bin";
const OLD_METADATA_FILE: &str = "old-metadata.json";
const OLD_SIGNATURE_FILE: &str = "old-signature.bin";
const SNAPSHOT_FILE: &str = "snapshot.json";
const POINTER_FILE: &str = "config/launcher-update-state.json";
const PENDING_FILE: &str = "config/launcher-update-pending.json";
const HELPER_LOCK_FILE: &str = "config/launcher-update-helper.lock";
const OFFICIAL_OVERLAY_MANIFEST: &str = "plugins/dats/bahamut-dats-overlay/overlay.toml";
const PAYLOAD_DIR: &str = "payload";
const BACKUP_DIR: &str = "backup";
const JOURNAL_SCHEMA: u32 = 1;
const MAX_JOURNAL_BYTES: usize = 16 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

type Result<T> = std::result::Result<T, UpdateError>;

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("{0}")]
    Invalid(String),
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("release authorization failed: {0}")]
    Release(String),
    #[error("JSON encoding or decoding failed: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<ReleaseError> for UpdateError {
    fn from(value: ReleaseError) -> Self {
        Self::Release(value.to_string())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    Applying,
    AwaitingValidation,
    Committed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    phase: Phase,
    mode: TransactionMode,
    target: PathBuf,
    new_metadata_sha256: String,
    old_metadata_sha256: String,
    trusted_key_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    schema_version: u32,
    present_old_managed: Vec<String>,
    previous_managed_files: Vec<FileIdentity>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct FileIdentity {
    path: String,
    length: u64,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TransactionPointer {
    schema_version: u32,
    transaction: PathBuf,
    new_metadata_sha256: String,
    old_metadata_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LauncherHandoffReceipt {
    schema_version: u32,
    target: PathBuf,
    stage: PathBuf,
    transaction: PathBuf,
    operation: TransactionMode,
    helper_pid: Option<u32>,
    previous_metadata_sha256: String,
    new_metadata_sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Apply,
    Repair,
    Confirm,
    Recover,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum TransactionMode {
    Update,
    Repair,
}

#[derive(Default)]
struct Options {
    values: BTreeMap<String, OsString>,
    flags: BTreeSet<String>,
}

struct ApplyContext<'a> {
    mode: TransactionMode,
    target: &'a Path,
    transaction: &'a Path,
    metadata_path: &'a Path,
    signature_path: &'a Path,
    previous_metadata_path: &'a Path,
    previous_signature_path: &'a Path,
    new_release: &'a VerifiedRelease,
    previous_release: &'a VerifiedRelease,
    artifact_path: &'a Path,
    public_key: &'a [u8],
}

/// Run the explicit helper protocol used by the separately packaged binary.
pub fn run_cli(args: impl IntoIterator<Item = OsString>) -> std::result::Result<(), UpdateError> {
    let mut args = args.into_iter();
    let Some(mode) = args.next() else {
        return Err(invalid(usage()));
    };
    let mode = match mode.to_str() {
        Some("apply") => Mode::Apply,
        Some("repair") => Mode::Repair,
        Some("confirm") => Mode::Confirm,
        Some("recover") => Mode::Recover,
        Some("--help" | "-h" | "help") => {
            println!("{}", usage());
            return Ok(());
        }
        _ => return Err(invalid(usage())),
    };
    let mut options = parse_options(args)?;
    match mode {
        Mode::Apply => replacement_command(&mut options, TransactionMode::Update),
        Mode::Repair => replacement_command(&mut options, TransactionMode::Repair),
        Mode::Confirm => confirm_command(options),
        Mode::Recover => recover_command(options),
    }
}

fn usage() -> &'static str {
    "Usage:\n  bahamut-update-helper apply --target DIR --stage DIR [--transaction DIR] --metadata FILE --signature FILE --public-key FILE --artifact FILE --previous-metadata FILE --previous-signature FILE --wait-pid PID [--restart]\n  bahamut-update-helper repair --target DIR --stage DIR [--transaction DIR] --metadata FILE --signature FILE --public-key FILE --artifact FILE --previous-metadata FILE --previous-signature FILE --wait-pid PID [--restart]\n  bahamut-update-helper confirm --target DIR [--transaction DIR] --public-key FILE\n  bahamut-update-helper recover --target DIR [--transaction DIR] --public-key FILE [--wait-pid PID] [--restart]\n\nThe helper handles only signed Windows x86_64 launcher releases. All keys, metadata, signatures, artifacts, and transaction paths are explicit local inputs."
}

fn parse_options(args: impl Iterator<Item = OsString>) -> Result<Options> {
    let mut args = args;
    let mut options = Options::default();
    while let Some(argument) = args.next() {
        let flag = argument
            .to_str()
            .ok_or_else(|| invalid("Option name must be UTF-8."))?;
        if matches!(flag, "--restart" | "--launcher-handoff") {
            if !options.flags.insert(flag.to_owned()) {
                return Err(invalid("Duplicate command option."));
            }
            continue;
        }
        if !flag.starts_with("--") {
            return Err(invalid(usage()));
        }
        let value = args
            .next()
            .ok_or_else(|| invalid("Command option is missing its value."))?;
        if options.values.insert(flag.to_owned(), value).is_some() {
            return Err(invalid("Duplicate command option."));
        }
    }
    Ok(options)
}

fn replacement_command(options: &mut Options, mode: TransactionMode) -> Result<()> {
    require_only(
        options,
        &[
            "--target",
            "--stage",
            "--transaction",
            "--metadata",
            "--signature",
            "--public-key",
            "--artifact",
            "--previous-metadata",
            "--previous-signature",
            "--wait-pid",
        ],
        &["--restart", "--launcher-handoff"],
    )?;
    let target = take_path(options, "--target")?;
    let stage = take_path(options, "--stage")?;
    let requested_transaction = take_optional_path(options, "--transaction");
    let metadata_path = take_path(options, "--metadata")?;
    let signature_path = take_path(options, "--signature")?;
    let key_path = take_path(options, "--public-key")?;
    let artifact_path = take_path(options, "--artifact")?;
    let previous_metadata_path = take_path(options, "--previous-metadata")?;
    let previous_signature_path = take_path(options, "--previous-signature")?;
    let launcher_handoff = options.flags.remove("--launcher-handoff");
    let wait_pid = take_text(options, "--wait-pid")?
        .parse::<u32>()
        .map_err(|_| invalid("Parent process ID must be a positive integer."))?;
    if wait_pid == 0 || wait_pid == std::process::id() {
        return Err(invalid("Parent process ID must identify another process."));
    }
    let restart = options.flags.remove("--restart");

    let target = checked_existing_directory(&target, "installation directory")?;
    let _transaction_lock = acquire_transaction_lock(&target)?;
    let stage = checked_existing_directory(&stage, "external stage directory")?;
    ensure_outside_target(&stage, &target)?;
    ensure_same_volume(&stage, &target)?;
    ensure_no_pending_pointer(&target)?;

    let public_key = read_trusted_key(&key_path)?;
    let new_release = verified_release(&metadata_path, &signature_path, &public_key)?;
    let previous_release = verified_release(
        &previous_metadata_path,
        &previous_signature_path,
        &public_key,
    )?;
    if launcher_handoff {
        let transaction = requested_transaction
            .as_deref()
            .ok_or_else(|| invalid("Launcher handoff is missing its transaction path."))?;
        validate_launcher_handoff(
            &target,
            &stage,
            transaction,
            mode,
            &new_release.metadata_sha256,
            &previous_release.metadata_sha256,
        )?;
    }
    ensure_launcher_executable(&new_release.metadata)?;
    match mode {
        TransactionMode::Update => ensure_newer(&new_release, &previous_release)?,
        TransactionMode::Repair => ensure_same_release(&new_release, &previous_release)?,
    }
    verify_artifact_file(&new_release.metadata, &artifact_path)?;

    wait_for_process(wait_pid)?;
    let transaction = create_transaction_directory(
        &stage,
        &new_release.metadata_sha256,
        mode,
        requested_transaction.as_deref(),
    )?;
    maybe_test_boundary("attempt-created");
    let operation = prepare_and_apply(ApplyContext {
        mode,
        target: &target,
        transaction: &transaction,
        metadata_path: &metadata_path,
        signature_path: &signature_path,
        previous_metadata_path: &previous_metadata_path,
        previous_signature_path: &previous_signature_path,
        new_release: &new_release,
        previous_release: &previous_release,
        artifact_path: &artifact_path,
        public_key: &public_key,
    });
    if let Err(error) = operation {
        let rollback = if transaction.join(JOURNAL_FILE).exists() {
            recover_transaction(&target, &transaction, &public_key)
        } else {
            Ok(())
        };
        if rollback.is_ok() {
            clear_pointer(
                &target,
                &transaction,
                &new_release.metadata_sha256,
                &previous_release.metadata_sha256,
            )?;
            remove_transaction(&transaction)?;
            return Err(error);
        }
        return Err(invalid(&format!(
            "{error}; rollback is incomplete and recoverable at {}: {}",
            transaction.display(),
            rollback.unwrap_err()
        )));
    }

    println!("transaction {}", transaction.display());
    if restart {
        let executable = target.join("bahamut-launcher.exe");
        let child = Command::new(executable)
            .current_dir(&target)
            .arg("--update-transaction")
            .arg(&transaction)
            .spawn()
            .map_err(|error| {
                let rollback = recover_transaction(&target, &transaction, &public_key);
                if rollback.is_ok() {
                    let _ = clear_pointer(
                        &target,
                        &transaction,
                        &new_release.metadata_sha256,
                        &previous_release.metadata_sha256,
                    );
                    let _ = remove_transaction(&transaction);
                }
                invalid(&format!("Could not restart the updated launcher: {error}"))
            })?;
        println!("restarted launcher process {}", child.id());
    }
    Ok(())
}

fn confirm_command(mut options: Options) -> Result<()> {
    require_only(
        &options,
        &["--target", "--transaction", "--public-key"],
        &[],
    )?;
    let target = checked_existing_directory(
        &take_path(&mut options, "--target")?,
        "installation directory",
    )?;
    let _transaction_lock = acquire_transaction_lock(&target)?;
    let explicit = take_optional_path(&mut options, "--transaction");
    let public_key = read_trusted_key(&take_path(&mut options, "--public-key")?)?;
    let transaction = resolve_transaction(&target, explicit)?;
    let Some(transaction) = transaction else {
        println!("no pending transaction");
        return Ok(());
    };
    let (journal, new_release, _) = load_transaction(&target, &transaction, &public_key)?;
    validate_pointer_identity(&target, &transaction, &journal)?;
    if !matches!(journal.phase, Phase::AwaitingValidation | Phase::Committed) {
        return Err(invalid(
            "Transaction is not awaiting post-update validation.",
        ));
    }
    validate_installed_inventory(&target, &new_release.metadata)?;
    let mut committed = journal;
    committed.phase = Phase::Committed;
    write_journal(&transaction, &committed)?;
    clear_pointer(
        &target,
        &transaction,
        &new_release.metadata_sha256,
        &committed.old_metadata_sha256,
    )?;
    remove_transaction(&transaction)?;
    println!("confirmed {}", new_release.metadata.version);
    Ok(())
}

/// Inspect the signed release that recovery will retain. Callers coordinating
/// a handoff must hold the helper transaction lock until the handoff is recorded.
pub fn recovery_release(
    target: &Path,
    transaction: &Path,
    public_key: &[u8],
) -> Result<VerifiedRelease> {
    let target = checked_existing_directory(target, "installation directory")?;
    let transaction = resolve_transaction(&target, Some(transaction.to_path_buf()))?
        .ok_or_else(|| invalid("The recovery transaction is missing."))?;
    let (journal, new_release, old_release) = load_transaction(&target, &transaction, public_key)?;
    validate_pointer_identity(&target, &transaction, &journal)?;
    Ok(if journal.phase == Phase::Committed {
        new_release
    } else {
        old_release
    })
}

fn recover_command(mut options: Options) -> Result<()> {
    require_only(
        &options,
        &["--target", "--transaction", "--public-key", "--wait-pid"],
        &["--restart", "--launcher-handoff"],
    )?;
    let target = checked_existing_directory(
        &take_path(&mut options, "--target")?,
        "installation directory",
    )?;
    let _transaction_lock = acquire_transaction_lock(&target)?;
    let explicit = take_optional_path(&mut options, "--transaction");
    let public_key = read_trusted_key(&take_path(&mut options, "--public-key")?)?;
    let wait_pid = take_optional_pid(&mut options, "--wait-pid")?;
    let restart = options.flags.remove("--restart");
    let launcher_handoff = options.flags.remove("--launcher-handoff");
    if launcher_handoff && explicit.is_none() {
        return Err(invalid(
            "Launcher handoff recovery requires its transaction path.",
        ));
    }
    let transaction = resolve_transaction(&target, explicit)?;
    let Some(transaction) = transaction else {
        println!("no pending transaction");
        return Ok(());
    };
    let (journal, _, _) = load_transaction(&target, &transaction, &public_key)?;
    validate_pointer_identity(&target, &transaction, &journal)?;
    if launcher_handoff {
        validate_launcher_handoff(
            &target,
            transaction
                .parent()
                .ok_or_else(|| invalid("Launcher transaction has no stage directory."))?,
            &transaction,
            journal.mode,
            &journal.new_metadata_sha256,
            &journal.old_metadata_sha256,
        )?;
    }
    if let Some(pid) = wait_pid {
        wait_for_process(pid)?;
    }
    recover_transaction(&target, &transaction, &public_key)?;
    clear_pointer(
        &target,
        &transaction,
        &journal.new_metadata_sha256,
        &journal.old_metadata_sha256,
    )?;
    remove_transaction(&transaction)?;
    if restart {
        let executable = target.join("bahamut-launcher.exe");
        let child = Command::new(executable)
            .current_dir(&target)
            .arg("--update-recovered")
            .arg(&transaction)
            .spawn()
            .map_err(|error| {
                invalid(&format!(
                    "Recovered the previous release but could not restart the launcher: {error}"
                ))
            })?;
        println!("restarted launcher process {}", child.id());
    }
    println!("recovered previous launcher release");
    Ok(())
}

fn prepare_and_apply(context: ApplyContext<'_>) -> Result<()> {
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

fn repair_snapshot_identity_map(
    snapshot: &Snapshot,
    expected_files: &[InventoryFile],
) -> Result<BTreeMap<String, FileIdentity>> {
    let present = snapshot
        .present_old_managed
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected_paths = expected_files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    if present.len() != snapshot.present_old_managed.len()
        || present.iter().any(|path| !expected_paths.contains(path))
    {
        return Err(invalid("Repair snapshot names an unexpected managed path."));
    }

    let mut identities = BTreeMap::new();
    for identity in &snapshot.previous_managed_files {
        if !present.contains(identity.path.as_str())
            || !valid_sha256(&identity.sha256)
            || identities
                .insert(identity.path.clone(), identity.clone())
                .is_some()
        {
            return Err(invalid("Repair snapshot file identities are invalid."));
        }
    }
    if identities.len() != present.len() {
        return Err(invalid(
            "Repair snapshot does not identify every previous managed file.",
        ));
    }
    Ok(identities)
}

fn same_file_identity(actual: &InventoryFile, expected: &FileIdentity) -> bool {
    actual.length == expected.length && actual.sha256 == expected.sha256
}

fn recover_transaction(target: &Path, transaction: &Path, public_key: &[u8]) -> Result<()> {
    let (journal, new_release, old_release) = load_transaction(target, transaction, public_key)?;
    match journal.phase {
        Phase::Prepared | Phase::Committed => return Ok(()),
        Phase::Applying | Phase::AwaitingValidation => {}
    }
    let snapshot_path = transaction.join(SNAPSHOT_FILE);
    let snapshot_bytes =
        read_regular_bounded(&snapshot_path, MAX_SNAPSHOT_BYTES, "transaction snapshot")?;
    let snapshot: Snapshot = serde_json::from_slice(&snapshot_bytes)?;
    if snapshot.schema_version != JOURNAL_SCHEMA {
        return Err(invalid("Transaction snapshot schema is unsupported."));
    }
    let mut old_files = if journal.mode == TransactionMode::Update {
        transition_old_files(&old_release.metadata, &new_release.metadata)?
    } else {
        managed_files(&old_release.metadata)?
    };
    let new_files = managed_files(&new_release.metadata)?;
    if journal.mode == TransactionMode::Update {
        restore_legacy_overlay_identity(&snapshot, &mut old_files, &new_files)?;
    }
    let present = snapshot
        .present_old_managed
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected_old = old_files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    if present.iter().any(|path| !expected_old.contains(path)) {
        return Err(invalid(
            "Transaction snapshot names a file outside the old inventory.",
        ));
    }
    if present.len() != snapshot.present_old_managed.len() {
        return Err(invalid("Transaction snapshot repeats a managed path."));
    }
    if journal.mode == TransactionMode::Repair {
        return recover_repair_transaction(target, transaction, &old_files, &new_files, &snapshot);
    }
    let backup_root = transaction.join(BACKUP_DIR);
    reject_reparse(&backup_root)?;
    for old_file in &old_files {
        ensure_safe_inventory_path(&backup_root, &old_file.path, false)?;
        let backup = safe_join(&backup_root, &old_file.path)?;
        if present.contains(old_file.path.as_str()) {
            ensure_file_matches(&backup, old_file)?;
        } else if fs::symlink_metadata(&backup).is_ok() {
            return Err(invalid("Transaction backup does not match its snapshot."));
        }
    }
    let old_by_path = old_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let new_by_path = new_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();

    for new_file in new_files.iter().rev() {
        let destination = safe_join(target, &new_file.path)?;
        ensure_safe_inventory_path(target, &new_file.path, false)?;
        if let Some(old_file) = old_by_path.get(new_file.path.as_str()) {
            if present.contains(new_file.path.as_str()) {
                let backup = safe_join(&transaction.join(BACKUP_DIR), &old_file.path)?;
                ensure_safe_inventory_path(&transaction.join(BACKUP_DIR), &old_file.path, false)?;
                ensure_file_matches(&backup, old_file)?;
                if let Some(current) = inspect_file(&destination)?
                    && !same_bytes(&current, old_file)
                    && !same_bytes(&current, new_file)
                {
                    return Err(invalid(
                        "Cannot safely restore a changed managed file; preserving it.",
                    ));
                }
                if inspect_file(&destination)?.is_none_or(|current| !same_bytes(&current, old_file))
                {
                    install_file_atomically(transaction, target, &backup, &destination)?;
                }
            } else if let Some(current) = inspect_file(&destination)? {
                if same_bytes(&current, new_file) {
                    fs::remove_file(destination)?;
                } else {
                    return Err(invalid(
                        "Cannot safely roll back a changed managed file; preserving it.",
                    ));
                }
            }
        } else if let Some(current) = inspect_file(&destination)? {
            if same_bytes(&current, new_file) {
                fs::remove_file(destination)?;
            } else {
                return Err(invalid(
                    "Cannot safely remove an unexpected file during rollback; preserving it.",
                ));
            }
        }
    }
    for old_file in old_files
        .iter()
        .filter(|file| !new_by_path.contains_key(file.path.as_str()))
    {
        if !present.contains(old_file.path.as_str()) {
            continue;
        }
        let backup = safe_join(&transaction.join(BACKUP_DIR), &old_file.path)?;
        ensure_safe_inventory_path(&transaction.join(BACKUP_DIR), &old_file.path, false)?;
        ensure_file_matches(&backup, old_file)?;
        let destination = safe_join(target, &old_file.path)?;
        ensure_safe_inventory_path(target, &old_file.path, false)?;
        if let Some(current) = inspect_file(&destination)? {
            if !same_bytes(&current, old_file) {
                return Err(invalid(
                    "Cannot safely restore a changed obsolete file; preserving it.",
                ));
            }
        } else {
            install_file_atomically(transaction, target, &backup, &destination)?;
        }
    }
    validate_old_managed_files(target, &old_files, &present)?;
    Ok(())
}

fn recover_repair_transaction(
    target: &Path,
    transaction: &Path,
    old_files: &[InventoryFile],
    new_files: &[InventoryFile],
    snapshot: &Snapshot,
) -> Result<()> {
    let previous = repair_snapshot_identity_map(snapshot, old_files)?;
    let backup_root = transaction.join(BACKUP_DIR);
    reject_reparse(&backup_root)?;
    let new_by_path = new_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();

    for old_file in old_files {
        ensure_safe_inventory_path(&backup_root, &old_file.path, false)?;
        let backup = safe_join(&backup_root, &old_file.path)?;
        match previous.get(&old_file.path) {
            Some(expected) => {
                let actual = inspect_file(&backup)?
                    .ok_or_else(|| invalid("Transaction repair backup is missing."))?;
                if !same_file_identity(&actual, expected) {
                    return Err(invalid("Transaction repair backup changed."));
                }
            }
            None if inspect_file(&backup)?.is_some() => {
                return Err(invalid("Transaction backup does not match its snapshot."));
            }
            None => {}
        }
    }

    for old_file in old_files.iter().rev() {
        let destination = safe_join(target, &old_file.path)?;
        ensure_safe_inventory_path(target, &old_file.path, false)?;
        let current = inspect_file(&destination)?;
        match previous.get(&old_file.path) {
            Some(expected) => {
                if current
                    .as_ref()
                    .is_some_and(|actual| same_file_identity(actual, expected))
                {
                    continue;
                }
                if current.as_ref().is_some_and(|actual| {
                    !new_by_path
                        .get(old_file.path.as_str())
                        .is_some_and(|new_file| same_bytes(actual, new_file))
                }) {
                    return Err(invalid(
                        "Cannot safely restore a changed repair file; preserving it.",
                    ));
                }
                let backup = safe_join(&backup_root, &old_file.path)?;
                install_file_atomically(transaction, target, &backup, &destination)?;
            }
            None => {
                if let Some(current) = current {
                    if new_by_path
                        .get(old_file.path.as_str())
                        .is_some_and(|new_file| same_bytes(&current, new_file))
                    {
                        fs::remove_file(destination)?;
                    } else {
                        return Err(invalid(
                            "Cannot safely remove an unexpected repair file; preserving it.",
                        ));
                    }
                }
            }
        }
    }

    for old_file in old_files {
        ensure_safe_inventory_path(target, &old_file.path, false)?;
        let path = safe_join(target, &old_file.path)?;
        match previous.get(&old_file.path) {
            Some(expected) => {
                let actual = inspect_file(&path)?
                    .ok_or_else(|| invalid("Repair recovery did not restore a managed file."))?;
                if !same_file_identity(&actual, expected) {
                    return Err(invalid("Repair recovery did not restore prior file bytes."));
                }
            }
            None if inspect_file(&path)?.is_some() => {
                return Err(invalid("Repair recovery found an unexpected prior file."));
            }
            None => {}
        }
    }
    Ok(())
}

fn load_transaction(
    target: &Path,
    transaction: &Path,
    public_key: &[u8],
) -> Result<(Journal, VerifiedRelease, VerifiedRelease)> {
    reject_reparse(transaction)?;
    let journal_path = transaction.join(JOURNAL_FILE);
    reject_reparse(&journal_path)?;
    let journal_bytes =
        read_regular_bounded(&journal_path, MAX_JOURNAL_BYTES, "transaction journal")?;
    let journal: Journal = serde_json::from_slice(&journal_bytes)?;
    if journal.schema_version != JOURNAL_SCHEMA || canonicalize_target(&journal.target)? != target {
        return Err(invalid(
            "Transaction does not match the requested installation.",
        ));
    }
    let new_metadata_path = transaction.join(NEW_METADATA_FILE);
    let new_signature_path = transaction.join(NEW_SIGNATURE_FILE);
    let old_metadata_path = transaction.join(OLD_METADATA_FILE);
    let old_signature_path = transaction.join(OLD_SIGNATURE_FILE);
    for path in [
        &new_metadata_path,
        &new_signature_path,
        &old_metadata_path,
        &old_signature_path,
    ] {
        reject_reparse(path)?;
    }
    let new_release = verified_release(&new_metadata_path, &new_signature_path, public_key)?;
    let old_release = verified_release(&old_metadata_path, &old_signature_path, public_key)?;
    ensure_launcher_executable(&new_release.metadata)?;
    match journal.mode {
        TransactionMode::Update => ensure_newer(&new_release, &old_release)?,
        TransactionMode::Repair => ensure_same_release(&new_release, &old_release)?,
    }
    if new_release.metadata_sha256 != journal.new_metadata_sha256
        || old_release.metadata_sha256 != journal.old_metadata_sha256
        || new_release.trusted_key_sha256 != journal.trusted_key_sha256
    {
        return Err(invalid(
            "Transaction release identity or trusted key changed.",
        ));
    }
    Ok((journal, new_release, old_release))
}

fn verified_release(
    metadata_path: &Path,
    signature_path: &Path,
    public_key: &[u8],
) -> Result<VerifiedRelease> {
    reject_reparse(metadata_path)?;
    reject_reparse(signature_path)?;
    let metadata_bytes = read_metadata_file(metadata_path)?;
    let signature = read_signature_file(signature_path)?;
    verify_metadata(
        &metadata_bytes,
        &signature,
        public_key,
        ReleaseScope {
            product: Product::Launcher,
            channel: Channel::Stable,
            target: Target::WindowsX86_64,
        },
        None,
    )
    .map_err(UpdateError::from)
}

fn read_trusted_key(path: &Path) -> Result<Vec<u8>> {
    reject_reparse(path)?;
    read_public_key_file(path).map_err(UpdateError::from)
}

fn ensure_launcher_executable(metadata: &ReleaseMetadata) -> Result<()> {
    let files = managed_files(metadata)?;
    if files.iter().any(|file| file.path == "bahamut-launcher.exe") {
        Ok(())
    } else {
        Err(invalid(
            "Launcher update inventory must include bahamut-launcher.exe.",
        ))
    }
}

fn ensure_newer(new_release: &VerifiedRelease, old_release: &VerifiedRelease) -> Result<()> {
    let new_version = semver::Version::parse(&new_release.metadata.version)
        .map_err(|_| invalid("New release version is invalid."))?;
    let old_version = semver::Version::parse(&old_release.metadata.version)
        .map_err(|_| invalid("Previous release version is invalid."))?;
    if new_version <= old_version {
        return Err(invalid(
            "Launcher updates must advance the installed version.",
        ));
    }
    Ok(())
}

fn ensure_same_release(new_release: &VerifiedRelease, old_release: &VerifiedRelease) -> Result<()> {
    if new_release.metadata_sha256 != old_release.metadata_sha256 {
        return Err(invalid(
            "Repair requires the exact same trusted release metadata identity.",
        ));
    }
    Ok(())
}

fn create_transaction_directory(
    stage: &Path,
    metadata_sha256: &str,
    mode: TransactionMode,
    requested: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(transaction) = requested {
        let expected_name = format!("{}-{metadata_sha256}", mode.transaction_name());
        if !transaction.is_absolute()
            || transaction.parent() != Some(stage)
            || transaction.file_name().and_then(|name| name.to_str()) != Some(&expected_name)
        {
            return Err(invalid(
                "Requested transaction path does not match the external stage and signed release.",
            ));
        }
        fs::create_dir(transaction).map_err(|error| {
            invalid(&format!(
                "Could not create launcher transaction directory: {error}"
            ))
        })?;
        return Ok(transaction.to_path_buf());
    }
    for _ in 0..128 {
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let transaction = stage.join(format!(
            "{}-{metadata_sha256}-{}-{sequence}",
            mode.transaction_name(),
            std::process::id()
        ));
        match fs::create_dir(&transaction) {
            Ok(()) => return Ok(transaction),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(invalid(
        "Could not allocate a unique update transaction directory.",
    ))
}

impl TransactionMode {
    fn transaction_name(self) -> &'static str {
        match self {
            Self::Update => "bahamut-update",
            Self::Repair => "bahamut-repair",
        }
    }
}

#[cfg(debug_assertions)]
fn maybe_test_boundary(boundary: &str) {
    if std::env::var("BAHAMUT_UPDATE_HELPER_TEST_CRASH_AT").as_deref() == Ok(boundary) {
        std::process::exit(86);
    }
}

#[cfg(not(debug_assertions))]
fn maybe_test_boundary(_boundary: &str) {}

#[cfg(debug_assertions)]
fn maybe_test_interruption(applied_steps: usize) {
    if std::env::var("BAHAMUT_UPDATE_HELPER_TEST_CRASH_AFTER")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(applied_steps)
    {
        std::process::exit(86);
    }
}

#[cfg(not(debug_assertions))]
fn maybe_test_interruption(_applied_steps: usize) {}

fn managed_files(metadata: &ReleaseMetadata) -> Result<Vec<InventoryFile>> {
    let ReleaseInventory::Launcher { files } = &metadata.inventory else {
        return Err(invalid("Launcher inventory is required."));
    };
    Ok(files
        .iter()
        .filter(|file| file.ownership == FileOwnership::Managed)
        .cloned()
        .collect())
}

fn transition_old_files(
    old_metadata: &ReleaseMetadata,
    new_metadata: &ReleaseMetadata,
) -> Result<Vec<InventoryFile>> {
    let mut files = managed_files(old_metadata)?;
    let new_managed = managed_files(new_metadata)?;
    let ReleaseInventory::Launcher {
        files: old_inventory,
    } = &old_metadata.inventory
    else {
        return Err(invalid("Launcher inventory is required."));
    };
    if let Some(seed) = old_inventory.iter().find(|file| {
        file.path.eq_ignore_ascii_case(OFFICIAL_OVERLAY_MANIFEST)
            && file.ownership == FileOwnership::Seed
            && new_managed
                .iter()
                .any(|new| new.path.eq_ignore_ascii_case(&file.path))
    }) {
        files.push(seed.clone());
    }
    Ok(files)
}

fn adopt_legacy_overlay_manifest(
    target: &Path,
    old_files: &mut [InventoryFile],
) -> Result<Option<FileIdentity>> {
    let Some(seed) = old_files.iter_mut().find(|file| {
        file.path == OFFICIAL_OVERLAY_MANIFEST && file.ownership == FileOwnership::Seed
    }) else {
        return Ok(None);
    };
    let Some(actual) = inspect_file(&target.join(OFFICIAL_OVERLAY_MANIFEST))? else {
        return Ok(None);
    };
    seed.length = actual.length;
    seed.sha256 = actual.sha256.clone();
    Ok(Some(FileIdentity {
        path: OFFICIAL_OVERLAY_MANIFEST.into(),
        length: actual.length,
        sha256: actual.sha256,
    }))
}

fn restore_legacy_overlay_identity(
    snapshot: &Snapshot,
    old_files: &mut [InventoryFile],
    new_files: &[InventoryFile],
) -> Result<()> {
    let Some(identity) = snapshot.previous_managed_files.first() else {
        return Ok(());
    };
    if snapshot.previous_managed_files.len() != 1
        || identity.path != OFFICIAL_OVERLAY_MANIFEST
        || identity.sha256.len() != 64
        || !identity
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !snapshot
            .present_old_managed
            .iter()
            .any(|path| path == OFFICIAL_OVERLAY_MANIFEST)
        || !new_files
            .iter()
            .any(|file| file.path == OFFICIAL_OVERLAY_MANIFEST)
    {
        return Err(invalid("Legacy overlay snapshot identity is invalid."));
    }
    let seed = old_files
        .iter_mut()
        .find(|file| {
            file.path == OFFICIAL_OVERLAY_MANIFEST && file.ownership == FileOwnership::Seed
        })
        .ok_or_else(|| invalid("Legacy overlay snapshot has no signed seed."))?;
    seed.length = identity.length;
    seed.sha256 = identity.sha256.clone();
    Ok(())
}

fn reject_legacy_overlay_extras(
    target: &Path,
    old_metadata: &ReleaseMetadata,
    new_metadata: &ReleaseMetadata,
) -> Result<()> {
    let ReleaseInventory::Launcher { files } = &old_metadata.inventory else {
        return Err(invalid("Launcher inventory is required."));
    };
    let has_legacy_seed = files.iter().any(|file| {
        file.path == OFFICIAL_OVERLAY_MANIFEST && file.ownership == FileOwnership::Seed
    });
    let adopts_overlay = managed_files(new_metadata)?
        .iter()
        .any(|file| file.path == OFFICIAL_OVERLAY_MANIFEST);
    if !has_legacy_seed || !adopts_overlay {
        return Ok(());
    }
    ensure_safe_inventory_path(target, OFFICIAL_OVERLAY_MANIFEST, false)?;
    let package = target.join("plugins/dats/bahamut-dats-overlay");
    let entries = match fs::read_dir(&package) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let details = fs::symlink_metadata(entry.path())?;
        if entry.file_name() != "overlay.toml" || !details.is_file() || is_reparse(&details) {
            return Err(invalid(
                "The legacy official overlay has extra files. Remove those files from plugins/dats/bahamut-dats-overlay (leave overlay.toml), or extract the new launcher into an empty folder, then retry.",
            ));
        }
    }
    Ok(())
}

fn validate_transition_destinations(
    target: &Path,
    new_metadata: &ReleaseMetadata,
    old_files: &[InventoryFile],
) -> Result<()> {
    let old_managed_paths = old_files
        .iter()
        .map(|file| file.path.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    for file in managed_files(new_metadata)? {
        let folded = file.path.to_ascii_lowercase();
        ensure_safe_inventory_path(target, &file.path, false)?;
        let path = safe_join(target, &file.path)?;
        if !old_managed_paths.contains(&folded) && inspect_file(&path)?.is_some() {
            return Err(invalid(
                "A new managed path is already occupied; preserving the existing file.",
            ));
        }
    }
    Ok(())
}

fn capture_previous_files(
    target: &Path,
    backup_root: &Path,
    old_files: &[InventoryFile],
    mode: TransactionMode,
) -> Result<Snapshot> {
    let mut present = Vec::new();
    let mut previous_managed_files = Vec::new();
    for file in old_files {
        ensure_safe_inventory_path(target, &file.path, false)?;
        let path = safe_join(target, &file.path)?;
        if inspect_file(&path)?.is_some() {
            if mode == TransactionMode::Update {
                ensure_file_matches(&path, file)?;
            }
            let backup = safe_join(backup_root, &file.path)?;
            ensure_safe_inventory_path(backup_root, &file.path, true)?;
            copy_new_file(&path, &backup)?;
            present.push(file.path.clone());
            if mode == TransactionMode::Repair {
                let backup_identity = inspect_file(&backup)?
                    .ok_or_else(|| invalid("Repair backup disappeared while being captured."))?;
                let current_identity = inspect_file(&path)?.ok_or_else(|| {
                    invalid("Managed file changed while repair backup was captured.")
                })?;
                if !same_bytes(&backup_identity, &current_identity) {
                    return Err(invalid(
                        "Managed file changed while repair backup was captured.",
                    ));
                }
                previous_managed_files.push(FileIdentity {
                    path: file.path.clone(),
                    length: backup_identity.length,
                    sha256: backup_identity.sha256,
                });
            }
        }
    }
    Ok(Snapshot {
        schema_version: JOURNAL_SCHEMA,
        present_old_managed: present,
        previous_managed_files,
    })
}

fn extract_managed_payload(
    artifact: &Path,
    payload: &Path,
    metadata: &ReleaseMetadata,
) -> Result<()> {
    let expected = managed_files(metadata)?
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    let input = File::open(artifact)?;
    let mut archive = ZipArchive::new(input)
        .map_err(|error| invalid(&format!("Could not open verified release archive: {error}")))?;
    let mut seen = BTreeSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            invalid(&format!(
                "Verified release archive entry is invalid: {error}"
            ))
        })?;
        if entry.is_dir() || entry.name().ends_with('/') {
            continue;
        }
        let name = entry.name().to_owned();
        if !expected.contains_key(&name) {
            continue;
        }
        if !seen.insert(name.clone()) {
            return Err(invalid("Release archive repeats a managed path."));
        }
        let expected_file = &expected[&name];
        let output_path = safe_join(payload, &name)?;
        if let Some(parent) = output_path.parent() {
            create_safe_directories(payload, parent)?;
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)?;
        let copied = io::copy(&mut entry, &mut output)?;
        output.sync_all()?;
        if copied != expected_file.length {
            return Err(invalid("Extracted launcher file length changed."));
        }
    }
    if seen.len() != expected.len() {
        return Err(invalid(
            "Release archive is missing a managed launcher file.",
        ));
    }
    Ok(())
}

fn validate_installed_inventory(target: &Path, metadata: &ReleaseMetadata) -> Result<()> {
    validate_tree_inventory(target, &managed_files(metadata)?)
}

fn validate_tree_inventory(root: &Path, files: &[InventoryFile]) -> Result<()> {
    for file in files {
        ensure_safe_inventory_path(root, &file.path, false)?;
        let path = safe_join(root, &file.path)?;
        ensure_file_matches(&path, file)?;
    }
    Ok(())
}

fn validate_old_managed_files(
    target: &Path,
    old_files: &[InventoryFile],
    present: &BTreeSet<&str>,
) -> Result<()> {
    for file in old_files {
        ensure_safe_inventory_path(target, &file.path, false)?;
        let path = safe_join(target, &file.path)?;
        if present.contains(file.path.as_str()) {
            ensure_file_matches(&path, file)?;
        } else if inspect_file(&path)?.is_some() {
            return Err(invalid("Rollback found an unexpected prior managed file."));
        }
    }
    Ok(())
}

fn inspect_file(path: &Path) -> Result<Option<InventoryFile>> {
    match fs::symlink_metadata(path) {
        Ok(details) => {
            if !details.is_file() || is_reparse(&details) {
                return Err(invalid("Managed path is not a regular file."));
            }
            let (length, sha256) = hash_file(path)?;
            Ok(Some(InventoryFile {
                path: String::new(),
                length,
                sha256,
                ownership: FileOwnership::Managed,
            }))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn ensure_file_matches(path: &Path, expected: &InventoryFile) -> Result<()> {
    let actual = inspect_file(path)?
        .ok_or_else(|| invalid(&format!("Managed file is missing: {}", expected.path)))?;
    if actual.length != expected.length || actual.sha256 != expected.sha256 {
        return Err(invalid(&format!(
            "Managed file differs from its signed inventory: {}",
            expected.path
        )));
    }
    Ok(())
}

fn same_bytes(actual: &InventoryFile, expected: &InventoryFile) -> bool {
    actual.length == expected.length && actual.sha256 == expected.sha256
}

fn hash_file(path: &Path) -> Result<(u64, String)> {
    let mut input = File::open(path)?;
    let mut digest = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(read as u64)
            .ok_or_else(|| invalid("Managed file length overflows."))?;
        digest.update(&buffer[..read]);
    }
    Ok((length, format!("{:x}", digest.finalize())))
}

fn install_file_atomically(
    transaction: &Path,
    target: &Path,
    source: &Path,
    destination: &Path,
) -> Result<()> {
    ensure_safe_parent(target, destination)?;
    let temporary = unique_path(transaction, ".bahamut-replace-");
    copy_new_file(source, &temporary)?;
    let result = replace_file(&temporary, destination);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(UpdateError::from)
}

fn copy_new_file(source: &Path, destination: &Path) -> Result<()> {
    let details = fs::symlink_metadata(source)?;
    if !details.is_file() || is_reparse(&details) {
        return Err(invalid("Transaction input is not a regular file."));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("Staged file has no parent directory."))?;
    fs::create_dir_all(parent)?;
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    sync_parent(parent)?;
    Ok(())
}

fn create_safe_directories(root: &Path, destination: &Path) -> Result<()> {
    if !destination.starts_with(root) {
        return Err(invalid("Managed directory escaped its staging root."));
    }
    let relative = destination
        .strip_prefix(root)
        .map_err(|_| invalid("Managed directory escaped its staging root."))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(invalid("Managed directory contains an unsafe component."));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(details) => {
                if !details.is_dir() || is_reparse(&details) {
                    return Err(invalid("Managed path contains a non-directory parent."));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&current)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty() || relative.contains('\\') || relative.contains(':') {
        return Err(invalid("Signed inventory path is unsafe."));
    }
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid("Signed inventory path is unsafe."));
    }
    Ok(root.join(path))
}

fn ensure_safe_parent(root: &Path, destination: &Path) -> Result<()> {
    let relative = destination
        .strip_prefix(root)
        .map_err(|_| invalid("Managed destination escaped the installation directory."))?;
    let parent = relative
        .parent()
        .ok_or_else(|| invalid("Managed destination has no parent directory."))?;
    let mut current = root.to_path_buf();
    for component in parent.components() {
        let Component::Normal(part) = component else {
            return Err(invalid("Managed destination has an unsafe parent."));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(details) => {
                if !details.is_dir() || is_reparse(&details) {
                    return Err(invalid("Managed destination has a reparse parent."));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&current)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn ensure_safe_inventory_path(root: &Path, relative: &str, create: bool) -> Result<()> {
    let path = safe_join(root, relative)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Managed path has no parent."))?;
    let relative_parent = parent
        .strip_prefix(root)
        .map_err(|_| invalid("Managed path escaped its root."))?;
    let mut current = root.to_path_buf();
    for component in relative_parent.components() {
        let Component::Normal(part) = component else {
            return Err(invalid("Managed path has an unsafe parent."));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(details) => {
                if !details.is_dir() || is_reparse(&details) {
                    return Err(invalid("Managed path has a reparse parent."));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
                fs::create_dir(&current)?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn acquire_transaction_lock(target: &Path) -> Result<crate::atomic_fs::ExclusiveProcessLock> {
    ensure_safe_inventory_path(target, HELPER_LOCK_FILE, true)?;
    let path = target.join(HELPER_LOCK_FILE);
    let lock = crate::atomic_fs::lock_exclusive(&path)?;
    let details = fs::symlink_metadata(&path)?;
    if !details.is_file() || is_reparse(&details) {
        return Err(invalid(
            "Launcher update helper lock is not a regular file.",
        ));
    }
    Ok(lock)
}

fn validate_launcher_handoff(
    target: &Path,
    stage: &Path,
    transaction: &Path,
    mode: TransactionMode,
    new_metadata_sha256: &str,
    old_metadata_sha256: &str,
) -> Result<()> {
    ensure_safe_inventory_path(target, PENDING_FILE, false)?;
    let bytes = read_regular_bounded(
        &target.join(PENDING_FILE),
        32 * 1024,
        "launcher update handoff receipt",
    )?;
    let receipt: LauncherHandoffReceipt = serde_json::from_slice(&bytes)?;
    let transaction_name = match mode {
        TransactionMode::Update => "bahamut-update",
        TransactionMode::Repair => "bahamut-repair",
    };
    let expected_transaction = stage.join(format!("{transaction_name}-{new_metadata_sha256}"));
    if receipt.schema_version != 1
        || receipt.helper_pid != Some(std::process::id())
        || receipt.operation != mode
        || !valid_sha256(&receipt.previous_metadata_sha256)
        || !valid_sha256(&receipt.new_metadata_sha256)
        || receipt.previous_metadata_sha256 != old_metadata_sha256
        || receipt.new_metadata_sha256 != new_metadata_sha256
        || !same_path(&receipt.target, target)?
        || !same_path(&receipt.stage, stage)?
        || !same_path(&receipt.transaction, &expected_transaction)?
        || !same_path(transaction, &expected_transaction)?
    {
        return Err(invalid(
            "Launcher helper handoff does not match its persisted signed receipt.",
        ));
    }
    Ok(())
}

fn same_path(left: &Path, right: &Path) -> Result<bool> {
    let resolve = |path: &Path| -> Result<PathBuf> {
        match fs::canonicalize(path) {
            Ok(path) => Ok(path),
            Err(error) if error.kind() == io::ErrorKind::NotFound && path.is_absolute() => {
                Ok(path.to_path_buf())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Ok(std::env::current_dir()?.join(path))
            }
            Err(error) => Err(error.into()),
        }
    };
    let left = resolve(left)?;
    let right = resolve(right)?;
    #[cfg(windows)]
    {
        Ok(left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy()))
    }
    #[cfg(not(windows))]
    {
        Ok(left == right)
    }
}

fn read_regular_bounded(path: &Path, max: usize, label: &str) -> Result<Vec<u8>> {
    let details = fs::symlink_metadata(path)?;
    if !details.is_file() || is_reparse(&details) || details.len() > max as u64 {
        return Err(invalid(&format!("{label} is not a bounded regular file.")));
    }
    let mut bytes = Vec::with_capacity(details.len() as usize);
    File::open(path)?
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(invalid(&format!("{label} exceeds its size limit.")));
    }
    Ok(bytes)
}

fn read_pointer(target: &Path) -> Result<Option<TransactionPointer>> {
    let pointer = target.join(POINTER_FILE);
    let config = pointer
        .parent()
        .ok_or_else(|| invalid("Update pointer path has no parent."))?;
    match fs::symlink_metadata(config) {
        Ok(details) => {
            if !details.is_dir() || is_reparse(&details) {
                return Err(invalid(
                    "Update pointer directory is not a regular directory.",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let details = match fs::symlink_metadata(&pointer) {
        Ok(details) => details,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !details.is_file() || is_reparse(&details) || details.len() > 16 * 1024 {
        return Err(invalid("Update pointer is not a bounded regular file."));
    }
    let mut bytes = Vec::with_capacity(details.len() as usize);
    File::open(pointer)?
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 {
        return Err(invalid("Update pointer exceeds its size limit."));
    }
    let pointer: TransactionPointer = serde_json::from_slice(&bytes)?;
    if pointer.schema_version != JOURNAL_SCHEMA
        || !pointer.transaction.is_absolute()
        || !valid_sha256(&pointer.new_metadata_sha256)
        || !valid_sha256(&pointer.old_metadata_sha256)
    {
        return Err(invalid("Update pointer fields are invalid."));
    }
    Ok(Some(pointer))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn publish_pointer(
    target: &Path,
    transaction: &Path,
    new_metadata_sha256: &str,
    old_metadata_sha256: &str,
) -> Result<()> {
    ensure_safe_inventory_path(target, POINTER_FILE, true)?;
    let path = target.join(POINTER_FILE);
    let transaction = fs::canonicalize(transaction)?;
    let pointer = TransactionPointer {
        schema_version: JOURNAL_SCHEMA,
        transaction: transaction.clone(),
        new_metadata_sha256: new_metadata_sha256.to_owned(),
        old_metadata_sha256: old_metadata_sha256.to_owned(),
    };
    if let Some(existing) = read_pointer(target)? {
        if existing.transaction == transaction
            && existing.new_metadata_sha256 == pointer.new_metadata_sha256
            && existing.old_metadata_sha256 == pointer.old_metadata_sha256
        {
            return Ok(());
        }
        return Err(invalid("Update pointer is already owned by another state."));
    }
    let bytes = serde_json::to_vec(&pointer)?;
    let temporary = unique_path(transaction.parent().unwrap_or(target), ".bahamut-pointer-");
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    let result = move_file_without_replace(&temporary, &path);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    sync_parent(path.parent().expect("pointer has parent"))?;
    Ok(())
}

fn resolve_transaction(target: &Path, explicit: Option<PathBuf>) -> Result<Option<PathBuf>> {
    let path = if let Some(path) = explicit {
        path
    } else {
        let Some(pointer) = read_pointer(target)? else {
            return Ok(None);
        };
        pointer.transaction
    };
    if !path.exists() {
        return Err(invalid(
            "Update transaction is missing; preserving the portable pointer for manual recovery.",
        ));
    }
    let transaction = checked_existing_directory(&path, "transaction directory")?;
    ensure_outside_target(&transaction, target)?;
    ensure_same_volume(&transaction, target)?;
    Ok(Some(transaction))
}

fn validate_pointer_identity(target: &Path, transaction: &Path, journal: &Journal) -> Result<()> {
    let Some(pointer) = read_pointer(target)? else {
        return Ok(());
    };
    if !same_path(&pointer.transaction, transaction)?
        || pointer.new_metadata_sha256 != journal.new_metadata_sha256
        || pointer.old_metadata_sha256 != journal.old_metadata_sha256
    {
        return Err(invalid(
            "Update pointer does not match its signed transaction.",
        ));
    }
    Ok(())
}

fn ensure_no_pending_pointer(target: &Path) -> Result<()> {
    if read_pointer(target)?.is_some() {
        return Err(invalid(
            "An update transaction is already pending; confirm or recover it first.",
        ));
    }
    Ok(())
}

fn clear_pointer(
    target: &Path,
    transaction: &Path,
    new_metadata_sha256: &str,
    old_metadata_sha256: &str,
) -> Result<()> {
    let Some(pointer) = read_pointer(target)? else {
        return Ok(());
    };
    if pointer.transaction != transaction {
        return Ok(());
    }
    if pointer.new_metadata_sha256 != new_metadata_sha256
        || pointer.old_metadata_sha256 != old_metadata_sha256
    {
        return Err(invalid("Refusing to remove a changed update pointer."));
    }
    fs::remove_file(target.join(POINTER_FILE))?;
    sync_parent(
        target
            .join(POINTER_FILE)
            .parent()
            .expect("pointer has parent"),
    )?;
    Ok(())
}

fn checked_existing_directory(path: &Path, label: &str) -> Result<PathBuf> {
    reject_reparse(path)?;
    let details = fs::metadata(path).map_err(|_| invalid(&format!("Could not read {label}.")))?;
    if !details.is_dir() {
        return Err(invalid(&format!("{label} is not a directory.")));
    }
    fs::canonicalize(path).map_err(UpdateError::from)
}

fn canonicalize_target(path: &Path) -> Result<PathBuf> {
    checked_existing_directory(path, "installation directory")
}

fn reject_reparse(path: &Path) -> Result<()> {
    let details =
        fs::symlink_metadata(path).map_err(|_| invalid("Could not inspect update path."))?;
    if is_reparse(&details) {
        return Err(invalid("Update path is a symbolic link or reparse point."));
    }
    Ok(())
}

fn is_reparse(details: &fs::Metadata) -> bool {
    if details.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        details.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn ensure_outside_target(stage: &Path, target: &Path) -> Result<()> {
    if stage == target || stage.starts_with(target) {
        return Err(invalid(
            "Update staging must be outside the installation tree.",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn ensure_same_volume(stage: &Path, target: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };
    use windows::core::PCWSTR;

    fn volume(path: &Path) -> Result<String> {
        let wide = path
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>();
        let mut mount = vec![0_u16; 32768];
        unsafe { GetVolumePathNameW(PCWSTR(wide.as_ptr()), &mut mount) }
            .map_err(|error| invalid(&format!("Could not identify update volume: {error}")))?;
        let mut name = vec![0_u16; 128];
        unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(mount.as_ptr()), &mut name) }
            .map_err(|error| invalid(&format!("Could not identify update volume: {error}")))?;
        let end = name
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(name.len());
        Ok(String::from_utf16_lossy(&name[..end]))
    }
    if volume(stage)? != volume(target)? {
        return Err(invalid(
            "Update staging must be on the installation volume.",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn ensure_same_volume(stage: &Path, target: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if fs::metadata(stage)?.dev() != fs::metadata(target)?.dev() {
        return Err(invalid(
            "Update staging must be on the installation volume.",
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn ensure_same_volume(stage: &Path, target: &Path) -> Result<()> {
    let stage_prefix = stage.components().next();
    let target_prefix = target.components().next();
    if stage_prefix != target_prefix {
        return Err(invalid(
            "Update staging must be on the installation volume.",
        ));
    }
    Ok(())
}

fn wait_for_process(pid: u32) -> Result<()> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };
        let process = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
            Ok(process) => process,
            // The launcher may exit between starting this helper and opening its handle.
            Err(error) if error.code().0 == 0x8007_0057_u32 as i32 => return Ok(()),
            Err(error) => {
                return Err(invalid(&format!(
                    "Could not wait for launcher process {pid}: {error}"
                )));
            }
        };
        let result = unsafe { WaitForSingleObject(process, u32::MAX) };
        let _ = unsafe { CloseHandle(process) };
        if result != WAIT_OBJECT_0 {
            return Err(invalid("Waiting for the launcher process failed."));
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        use std::thread;
        use std::time::Duration;
        loop {
            let result = unsafe { libc::kill(pid as i32, 0) };
            if result != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    return Ok(());
                }
                if error.raw_os_error() != Some(libc::EPERM) {
                    return Err(error.into());
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        Err(invalid(
            "Waiting for a parent process is unsupported on this platform.",
        ))
    }
}

fn write_journal(transaction: &Path, journal: &Journal) -> Result<()> {
    write_json_atomic(&transaction.join(JOURNAL_FILE), journal)
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Transaction journal has no parent."))?;
    let temporary = unique_path(parent, ".bahamut-journal-");
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    replace_file(&temporary, path)?;
    sync_parent(parent)?;
    Ok(())
}

fn unique_path(parent: &Path, prefix: &str) -> PathBuf {
    let serial = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!("{prefix}{}-{serial}", std::process::id()))
}

fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let temporary_wide = temporary
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>();
        let destination_wide = destination
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>();
        unsafe {
            MoveFileExW(
                windows::core::PCWSTR(temporary_wide.as_ptr()),
                windows::core::PCWSTR(destination_wide.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(io::Error::other)
    }
    #[cfg(not(windows))]
    {
        fs::rename(temporary, destination)
    }
}

fn move_file_without_replace(temporary: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
        let temporary_wide = temporary
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>();
        let destination_wide = destination
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>();
        unsafe {
            MoveFileExW(
                windows::core::PCWSTR(temporary_wide.as_ptr()),
                windows::core::PCWSTR(destination_wide.as_ptr()),
                MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(io::Error::other)
    }
    #[cfg(unix)]
    {
        fs::hard_link(temporary, destination)?;
        fs::remove_file(temporary)
    }
    #[cfg(not(any(unix, windows)))]
    {
        if destination.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "destination exists",
            ));
        }
        fs::rename(temporary, destination)
    }
}

fn sync_parent(parent: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(parent)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        Ok(())
    }
}

fn remove_transaction(transaction: &Path) -> Result<()> {
    reject_reparse(transaction)?;
    remove_entry_safely(transaction)?;
    Ok(())
}

fn remove_entry_safely(path: &Path) -> Result<()> {
    let mut pending = vec![(path.to_path_buf(), false)];
    while let Some((current, visited)) = pending.pop() {
        let details = fs::symlink_metadata(&current)?;
        if is_reparse(&details) {
            if details.is_dir() {
                fs::remove_dir(current)?;
            } else {
                fs::remove_file(current)?;
            }
        } else if details.is_dir() && !visited {
            pending.push((current.clone(), true));
            for entry in fs::read_dir(&current)? {
                pending.push((entry?.path(), false));
            }
        } else if details.is_dir() {
            fs::remove_dir(current)?;
        } else {
            fs::remove_file(current)?;
        }
    }
    Ok(())
}

fn require_only(options: &Options, values: &[&str], flags: &[&str]) -> Result<()> {
    if options
        .values
        .keys()
        .all(|key| values.contains(&key.as_str()))
        && options
            .flags
            .iter()
            .all(|flag| flags.contains(&flag.as_str()))
    {
        Ok(())
    } else {
        Err(invalid(usage()))
    }
}

fn take_path(options: &mut Options, key: &str) -> Result<PathBuf> {
    options
        .values
        .remove(key)
        .map(PathBuf::from)
        .ok_or_else(|| invalid(&format!("Missing required option {key}.")))
}

fn take_optional_path(options: &mut Options, key: &str) -> Option<PathBuf> {
    options.values.remove(key).map(PathBuf::from)
}

fn take_optional_pid(options: &mut Options, key: &str) -> Result<Option<u32>> {
    let Some(value) = options.values.remove(key) else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| invalid(&format!("Option {key} must be UTF-8.")))?;
    let pid = value
        .parse::<u32>()
        .map_err(|_| invalid("Parent process ID must be a positive integer."))?;
    if pid == 0 || pid == std::process::id() {
        return Err(invalid("Parent process ID must identify another process."));
    }
    Ok(Some(pid))
}

fn take_text(options: &mut Options, key: &str) -> Result<String> {
    options
        .values
        .remove(key)
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| invalid(&format!("Option {key} must be UTF-8.")))
}

fn invalid(message: &str) -> UpdateError {
    UpdateError::Invalid(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
