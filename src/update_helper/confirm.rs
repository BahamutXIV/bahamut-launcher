//! The `confirm` command: commit a validated transaction.

use super::cli::{Options, require_only, take_optional_path, take_path};
use super::fsutil::{acquire_transaction_lock, checked_existing_directory};
use super::inventory::validate_installed_inventory;
use super::pointer::{clear_pointer, resolve_transaction, validate_pointer_identity};
use super::transaction::{remove_transaction, write_journal};
use super::verify::{load_transaction, read_trusted_key};
use super::{Phase, Result, invalid};

pub(super) fn confirm_command(mut options: Options) -> Result<()> {
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
