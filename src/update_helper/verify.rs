//! Signed release verification and transaction loading.

use std::path::Path;

use crate::release::{
    Channel, Product, ReleaseMetadata, ReleaseScope, Target, VerifiedRelease, read_metadata_file,
    read_public_key_file, read_signature_file, verify_metadata,
};

use super::fsutil::{canonicalize_target, read_regular_bounded, reject_reparse};
use super::inventory::managed_files;
use super::{
    JOURNAL_FILE, JOURNAL_SCHEMA, Journal, MAX_JOURNAL_BYTES, NEW_METADATA_FILE,
    NEW_SIGNATURE_FILE, OLD_METADATA_FILE, OLD_SIGNATURE_FILE, Result, TransactionMode,
    UpdateError, invalid,
};

pub(super) fn load_transaction(
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

pub(super) fn verified_release(
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

pub(super) fn read_trusted_key(path: &Path) -> Result<Vec<u8>> {
    reject_reparse(path)?;
    read_public_key_file(path).map_err(UpdateError::from)
}

pub(super) fn ensure_launcher_executable(metadata: &ReleaseMetadata) -> Result<()> {
    let files = managed_files(metadata)?;
    if files.iter().any(|file| file.path == "bahamut-launcher.exe") {
        Ok(())
    } else {
        Err(invalid(
            "Launcher update inventory must include bahamut-launcher.exe.",
        ))
    }
}

pub(super) fn ensure_newer(
    new_release: &VerifiedRelease,
    old_release: &VerifiedRelease,
) -> Result<()> {
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

pub(super) fn ensure_same_release(
    new_release: &VerifiedRelease,
    old_release: &VerifiedRelease,
) -> Result<()> {
    if new_release.metadata_sha256 != old_release.metadata_sha256 {
        return Err(invalid(
            "Repair requires the exact same trusted release metadata identity.",
        ));
    }
    Ok(())
}
