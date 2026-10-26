//! The `recover` command and recovery release inspection.

use std::path::Path;

use crate::release::VerifiedRelease;

use super::cli::{Options, require_only, take_optional_path, take_optional_pid, take_path};
use super::fsutil::{acquire_transaction_lock, checked_existing_directory};
use super::pointer::{
    clear_pointer, resolve_transaction, validate_launcher_handoff, validate_pointer_identity,
};
use super::process::{spawn_launcher, wait_for_process};
use super::rollback::recover_transaction;
use super::transaction::remove_transaction;
use super::verify::{load_transaction, read_trusted_key};
use super::{Phase, Result, invalid};

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

pub(super) fn recover_command(mut options: Options) -> Result<()> {
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
        let child =
            spawn_launcher(&target, "--update-recovered", &transaction).map_err(|error| {
                invalid(&format!(
                    "Recovered the previous release but could not restart the launcher: {error}"
                ))
            })?;
        println!("restarted launcher process {}", child.id());
    }
    println!("recovered previous launcher release");
    Ok(())
}
