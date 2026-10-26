//! Verified, same-volume installation of a shipped base package.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::INSTALL_RECEIPT_FILE as RECEIPT_NAME;
use super::http::download_object;
use super::manifest::BasePackage;
use super::version_files::write_version_files;
use super::worker::{InstallShared, Phase, download_checkpoint};
use crate::diagnostics::free_disk_bytes;
use crate::version::{FFXIV_BOOT_VERSION, FFXIV_GAME_VERSION};

mod archive;
mod final_tree;
mod paths;
mod receipt;
mod space;
#[cfg(test)]
mod tests;

use archive::extract_archive;
use final_tree::{FinalContentCheck, validate_final_tree};
use paths::{
    ResolvedPath, ensure_cache_directory, ensure_destination_parent, reject_overlapping_paths,
    resolve_path, stage_path_for, validate_no_reparse_ancestors,
    validate_tree_has_no_reparse_points,
};
use receipt::{
    destination_has_receipt, prepare_owned_stage, read_receipt, remove_receipt,
    validate_destination_state, validate_quote_destination_state,
};
use space::{cache_space_requirement, check_space, inventory_bytes, staging_space_requirement};

const IO_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallQuote {
    pub download_bytes: u64,
    pub staging_bytes: u64,
    pub destination_bytes: u64,
    pub available_cache_bytes: Option<u64>,
    pub available_destination_bytes: Option<u64>,
}

/// Quote transfer, peak staging, final inventory, and currently available space.
pub fn quote(
    destination: &Path,
    cache: &Path,
    package: &BasePackage,
) -> Result<InstallQuote, String> {
    package.validate()?;
    validate_managed_version_files(package)?;
    let identity = package_identity(package);
    let destination_path = resolve_path(destination)?;
    validate_quote_destination_state(&destination_path.path, &identity)?;
    let cache_path = resolve_path(cache)?;
    if !cache_path.nearest_existing.is_dir() {
        return Err("The download cache parent is not a directory.".into());
    }
    let stage_path = stage_path_for(&destination_path.path)?;
    reject_overlapping_paths(&destination_path.path, &cache_path.path, &stage_path)?;

    let download_bytes = package.download_bytes()?;
    let destination_bytes = inventory_bytes(&package.final_files)?;
    Ok(InstallQuote {
        download_bytes,
        staging_bytes: staging_space_requirement(package, &destination_path.nearest_existing)?,
        destination_bytes,
        available_cache_bytes: free_disk_bytes(&cache_path.nearest_existing),
        available_destination_bytes: free_disk_bytes(&destination_path.nearest_existing),
    })
}

/// Download, stage, validate, and publish a new base installation.
pub fn install(
    shared: &Arc<InstallShared>,
    destination: &Path,
    root: &str,
    cache: &Path,
    package: &BasePackage,
) -> Result<(), String> {
    package.validate()?;
    validate_managed_version_files(package)?;
    let identity = package_identity(package);
    let destination_path = resolve_path(destination)?;
    if complete_existing_destination(shared, &destination_path, package, &identity)? {
        return Ok(());
    }
    validate_destination_state(&destination_path.path, &identity)?;

    let (cache_path, stage_path) =
        prepare_install_paths(shared, &destination_path, cache, package, &identity)?;
    let archives = download_archives(shared, root, &cache_path, package)?;
    stage_installation(shared, package, &archives, &stage_path, &identity)?;
    publish_stage(shared, &stage_path, &destination_path.path, &identity)
}

/// Finish when the destination already holds this package: a receipt left by an interrupted
/// publish is validated and cleared, and a valid tree without one is accepted as is.
fn complete_existing_destination(
    shared: &Arc<InstallShared>,
    destination_path: &ResolvedPath,
    package: &BasePackage,
    identity: &str,
) -> Result<bool, String> {
    if destination_path.exists && destination_has_receipt(&destination_path.path)? {
        if read_receipt(&destination_path.path)? != identity {
            return Err("The selected destination belongs to another package.".into());
        }
        shared.total_files.store(
            package.final_files.len(),
            std::sync::atomic::Ordering::Release,
        );
        shared.set_phase(Phase::ValidatingFiles);
        validate_final_tree(
            &destination_path.path,
            package,
            identity,
            shared,
            true,
            FinalContentCheck::Hash,
        )?;
        remove_receipt(&destination_path.path, identity)?;
        return Ok(true);
    }
    if destination_path.exists {
        shared.total_files.store(
            package.final_files.len(),
            std::sync::atomic::Ordering::Release,
        );
        shared.set_phase(Phase::ValidatingFiles);
        if validate_final_tree(
            &destination_path.path,
            package,
            identity,
            shared,
            false,
            FinalContentCheck::Hash,
        )
        .is_ok()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Validate the cache and stage locations, check free space, and return the resolved
/// cache directory and owned stage path.
fn prepare_install_paths(
    shared: &Arc<InstallShared>,
    destination_path: &ResolvedPath,
    cache: &Path,
    package: &BasePackage,
    identity: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let cache_path = resolve_path(cache)?;
    if !cache_path.nearest_existing.is_dir() {
        return Err("The download cache parent is not a directory.".into());
    }
    let stage_path = stage_path_for(&destination_path.path)?;
    reject_overlapping_paths(&destination_path.path, &cache_path.path, &stage_path)?;
    let cache_path = ensure_cache_directory(&cache_path.path)?;
    ensure_destination_parent(&destination_path.path)?;
    if shared.wait_if_paused() {
        return Err("Installation was cancelled.".into());
    }
    // Reclaim a verified previous attempt before measuring space for its replacement.
    let stage_path = prepare_owned_stage(&stage_path, identity)?;
    let quote = quote(&destination_path.path, &cache_path, package)?;
    let required_cache_bytes = cache_space_requirement(package, &cache_path)?;
    check_space(
        &quote,
        same_volume(&cache_path, &destination_path.nearest_existing),
        required_cache_bytes,
    )?;
    validate_destination_state(&destination_path.path, identity)?;
    Ok((cache_path, stage_path))
}

fn download_archives(
    shared: &Arc<InstallShared>,
    root: &str,
    cache_path: &Path,
    package: &BasePackage,
) -> Result<Vec<PathBuf>, String> {
    let archive_bytes = package.download_bytes()?;
    let mut archives = Vec::with_capacity(package.archives.len());
    shared.set_phase(Phase::Downloading);
    shared
        .previous_completed_bytes
        .store(0, std::sync::atomic::Ordering::Release);
    for (index, archive) in package.archives.iter().enumerate() {
        if shared.is_cancel_requested() {
            return Err("Installation was cancelled.".into());
        }
        shared
            .download_idx
            .store(index, std::sync::atomic::Ordering::Release);
        let path = download_object(
            root,
            &archive.object,
            cache_path,
            |checkpoint| download_checkpoint(shared, checkpoint),
            |bytes, _total| {
                shared
                    .download
                    .bytes_downloaded
                    .store(bytes, std::sync::atomic::Ordering::Release)
            },
        )
        .map_err(|error| error.to_string())?;
        archives.push(path);
        shared
            .previous_completed_bytes
            .fetch_add(archive.object.length, std::sync::atomic::Ordering::AcqRel);
        shared
            .download
            .bytes_downloaded
            .store(0, std::sync::atomic::Ordering::Release);
    }
    if shared.wait_if_paused() {
        return Err("Installation was cancelled.".into());
    }
    if shared
        .previous_completed_bytes
        .load(std::sync::atomic::Ordering::Acquire)
        > archive_bytes
    {
        return Err("Downloaded archive progress exceeded its manifest total.".into());
    }
    Ok(archives)
}

/// Extract the downloaded archives into the stage and validate the resulting tree.
fn stage_installation(
    shared: &Arc<InstallShared>,
    package: &BasePackage,
    archives: &[PathBuf],
    stage_path: &Path,
    identity: &str,
) -> Result<(), String> {
    shared.set_phase(Phase::Installing);
    let extracted_file_count = package
        .archives
        .iter()
        .map(|archive| archive.files.len())
        .sum();
    shared
        .total_files
        .store(extracted_file_count, std::sync::atomic::Ordering::Release);
    shared
        .file_idx
        .store(0, std::sync::atomic::Ordering::Release);
    for (archive, path) in package.archives.iter().zip(archives) {
        extract_archive(shared, path, archive, package, stage_path)?;
    }

    write_version_files(stage_path)
        .map_err(|error| format!("Could not write staged version files: {error}"))?;

    if shared.wait_if_paused() {
        return Err("Installation was cancelled.".into());
    }
    shared.total_files.store(
        package.final_files.len(),
        std::sync::atomic::Ordering::Release,
    );
    shared
        .file_idx
        .store(0, std::sync::atomic::Ordering::Release);
    shared.set_phase(Phase::ValidatingFiles);
    // Extraction already verified every staged byte against its archive identity.
    validate_final_tree(
        stage_path,
        package,
        identity,
        shared,
        true,
        FinalContentCheck::ExtractionVerified,
    )
}

/// Move the validated stage into place and confirm the published tree and receipt.
fn publish_stage(
    shared: &Arc<InstallShared>,
    stage_path: &Path,
    destination: &Path,
    identity: &str,
) -> Result<(), String> {
    shared.set_phase(Phase::Installing);
    validate_no_reparse_ancestors(stage_path)?;
    let destination_path = resolve_path(destination)?;
    validate_destination_state(&destination_path.path, identity)?;
    if destination_path.exists {
        fs::remove_dir(&destination_path.path).map_err(|error| {
            format!("The selected destination changed and is no longer empty: {error}")
        })?;
    }
    fs::rename(stage_path, &destination_path.path)
        .map_err(|error| format!("Could not publish the staged installation: {error}"))?;

    validate_no_reparse_ancestors(&destination_path.path)?;
    validate_tree_has_no_reparse_points(&destination_path.path)?;
    if read_receipt(&destination_path.path)? != identity {
        return Err("Published installation receipt changed unexpectedly.".into());
    }
    remove_receipt(&destination_path.path, identity)?;
    Ok(())
}

fn package_identity(package: &BasePackage) -> String {
    let encoded = serde_json::to_vec(package).expect("BasePackage is infallibly serializable");
    format!("{:x}", Sha256::digest(encoded))
}

fn validate_managed_version_files(package: &BasePackage) -> Result<(), String> {
    for (name, contents) in [
        ("boot.ver", FFXIV_BOOT_VERSION),
        ("game.ver", FFXIV_GAME_VERSION),
    ] {
        let expected_hash = crate::digest::sha256_hex(contents.as_bytes());
        let expected_length = contents.len() as u64;
        let file = package
            .final_files
            .iter()
            .find(|file| file.path.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("The final inventory must contain {name}."))?;
        if file.length != expected_length || !file.sha256.eq_ignore_ascii_case(&expected_hash) {
            return Err(format!(
                "The final inventory has an invalid {name} identity."
            ));
        }
    }
    for archive in &package.archives {
        if archive.files.iter().any(|file| {
            file.path.eq_ignore_ascii_case("boot.ver") || file.path.eq_ignore_ascii_case("game.ver")
        }) {
            return Err("Base archives cannot include managed version files.".into());
        }
    }
    if package
        .final_files
        .iter()
        .any(|file| file.path.eq_ignore_ascii_case(RECEIPT_NAME))
        || package.archives.iter().any(|archive| {
            archive
                .files
                .iter()
                .any(|file| file.path.eq_ignore_ascii_case(RECEIPT_NAME))
        })
    {
        return Err("The install inventory uses a reserved launcher receipt path.".into());
    }
    Ok(())
}

fn same_volume(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows::Win32::Storage::FileSystem::GetVolumePathNameW;
        use windows::core::PCWSTR;

        let volume_path = |path: &Path| {
            let input = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            let mut output = vec![0_u16; 32768];
            unsafe { GetVolumePathNameW(PCWSTR(input.as_ptr()), &mut output) }.ok()?;
            let length = output.iter().position(|unit| *unit == 0)?;
            Some(
                OsString::from_wide(&output[..length])
                    .to_string_lossy()
                    .to_lowercase(),
            )
        };
        volume_path(left)
            .zip(volume_path(right))
            .is_some_and(|(left, right)| left == right)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let left_device = fs::metadata(left).ok().map(|metadata| metadata.dev());
        let right_device = fs::metadata(right).ok().map(|metadata| metadata.dev());
        left_device.is_some() && left_device == right_device
    }
    #[cfg(not(any(windows, unix)))]
    {
        left.components().next() == right.components().next()
    }
}
