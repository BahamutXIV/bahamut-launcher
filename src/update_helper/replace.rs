//! The `apply` and `repair` commands.

use std::path::{Path, PathBuf};

use crate::release::{VerifiedRelease, verify_artifact_file};

use super::apply::{ApplyContext, prepare_and_apply};
use super::cli::{Options, require_only, take_optional_path, take_path, take_text};
use super::fsutil::{
    acquire_transaction_lock, checked_existing_directory, ensure_outside_target, ensure_same_volume,
};
use super::hooks::maybe_test_boundary;
use super::pointer::{clear_pointer, ensure_no_pending_pointer, validate_launcher_handoff};
use super::process::{spawn_launcher, wait_for_process};
use super::rollback::recover_transaction;
use super::transaction::{create_transaction_directory, remove_transaction};
use super::verify::{
    ensure_launcher_executable, ensure_newer, ensure_same_release, read_trusted_key,
    verified_release,
};
use super::{JOURNAL_FILE, Result, TransactionMode, UpdateError, invalid};

struct ReplacementArgs {
    target: PathBuf,
    stage: PathBuf,
    requested_transaction: Option<PathBuf>,
    metadata_path: PathBuf,
    signature_path: PathBuf,
    key_path: PathBuf,
    artifact_path: PathBuf,
    previous_metadata_path: PathBuf,
    previous_signature_path: PathBuf,
    launcher_handoff: bool,
    wait_pid: u32,
    restart: bool,
}

struct VerifiedInputs {
    public_key: Vec<u8>,
    new_release: VerifiedRelease,
    previous_release: VerifiedRelease,
}

pub(super) fn replacement_command(options: &mut Options, mode: TransactionMode) -> Result<()> {
    let args = parse_replacement_args(options)?;

    let target = checked_existing_directory(&args.target, "installation directory")?;
    let _transaction_lock = acquire_transaction_lock(&target)?;
    let stage = checked_existing_directory(&args.stage, "external stage directory")?;
    ensure_outside_target(&stage, &target)?;
    ensure_same_volume(&stage, &target)?;
    ensure_no_pending_pointer(&target)?;

    let VerifiedInputs {
        public_key,
        new_release,
        previous_release,
    } = verify_replacement_inputs(&args, mode, &target, &stage)?;

    wait_for_process(args.wait_pid)?;
    let transaction = create_transaction_directory(
        &stage,
        &new_release.metadata_sha256,
        mode,
        args.requested_transaction.as_deref(),
    )?;
    maybe_test_boundary("attempt-created");
    let operation = prepare_and_apply(ApplyContext {
        mode,
        target: &target,
        transaction: &transaction,
        metadata_path: &args.metadata_path,
        signature_path: &args.signature_path,
        previous_metadata_path: &args.previous_metadata_path,
        previous_signature_path: &args.previous_signature_path,
        new_release: &new_release,
        previous_release: &previous_release,
        artifact_path: &args.artifact_path,
        public_key: &public_key,
    });
    if let Err(error) = operation {
        return Err(roll_back_failed_attempt(
            error,
            &target,
            &transaction,
            &public_key,
            &new_release,
            &previous_release,
        ));
    }

    println!("transaction {}", transaction.display());
    if args.restart {
        let child =
            spawn_launcher(&target, "--update-transaction", &transaction).map_err(|error| {
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

fn verify_replacement_inputs(
    args: &ReplacementArgs,
    mode: TransactionMode,
    target: &Path,
    stage: &Path,
) -> Result<VerifiedInputs> {
    let public_key = read_trusted_key(&args.key_path)?;
    let new_release = verified_release(&args.metadata_path, &args.signature_path, &public_key)?;
    let previous_release = verified_release(
        &args.previous_metadata_path,
        &args.previous_signature_path,
        &public_key,
    )?;
    if args.launcher_handoff {
        let transaction = args
            .requested_transaction
            .as_deref()
            .ok_or_else(|| invalid("Launcher handoff is missing its transaction path."))?;
        validate_launcher_handoff(
            target,
            stage,
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
    verify_artifact_file(&new_release.metadata, &args.artifact_path)?;
    Ok(VerifiedInputs {
        public_key,
        new_release,
        previous_release,
    })
}

/// Undo a failed attempt and return the error to report. A rollback that
/// cannot complete leaves the transaction and pointer in place for recovery.
fn roll_back_failed_attempt(
    error: UpdateError,
    target: &Path,
    transaction: &Path,
    public_key: &[u8],
    new_release: &VerifiedRelease,
    previous_release: &VerifiedRelease,
) -> UpdateError {
    let rollback = if transaction.join(JOURNAL_FILE).exists() {
        recover_transaction(target, transaction, public_key)
    } else {
        Ok(())
    };
    match rollback {
        Ok(()) => match clear_pointer(
            target,
            transaction,
            &new_release.metadata_sha256,
            &previous_release.metadata_sha256,
        )
        .and_then(|()| remove_transaction(transaction))
        {
            Ok(()) => error,
            Err(cleanup_error) => cleanup_error,
        },
        Err(rollback_error) => invalid(&format!(
            "{error}; rollback is incomplete and recoverable at {}: {rollback_error}",
            transaction.display(),
        )),
    }
}

fn parse_replacement_args(options: &mut Options) -> Result<ReplacementArgs> {
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
    Ok(ReplacementArgs {
        target,
        stage,
        requested_transaction,
        metadata_path,
        signature_path,
        key_path,
        artifact_path,
        previous_metadata_path,
        previous_signature_path,
        launcher_handoff,
        wait_pid,
        restart,
    })
}
