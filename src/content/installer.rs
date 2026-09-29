//! Verified, same-volume installation of a shipped base package.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use super::INSTALL_RECEIPT_FILE as RECEIPT_NAME;
use super::http::{ObjectSpec, download_object};
use super::manifest::{
    ArchiveLayout, BaseArchive, BasePackage, InstallFile, validate_hash, validate_relative_path,
};
use super::version_files::write_version_files;
use super::worker::{InstallShared, Phase, download_checkpoint};
use crate::diagnostics::free_disk_bytes;
use crate::version::{FFXIV_BOOT_VERSION, FFXIV_GAME_VERSION};

const IO_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallQuote {
    pub download_bytes: u64,
    pub staging_bytes: u64,
    pub destination_bytes: u64,
    pub available_cache_bytes: Option<u64>,
    pub available_destination_bytes: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallReceipt {
    schema_version: u32,
    manifest_sha256: String,
}

struct ResolvedPath {
    path: PathBuf,
    nearest_existing: PathBuf,
    exists: bool,
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
    if destination_path.exists && destination_has_receipt(&destination_path.path)? {
        if read_receipt(&destination_path.path)? != identity {
            return Err("The selected destination belongs to another package.".into());
        }
        shared.total_files.store(
            package.final_files.len(),
            std::sync::atomic::Ordering::Release,
        );
        shared.set_phase(Phase::ValidatingFiles);
        validate_final_tree(&destination_path.path, package, &identity, shared, true)?;
        remove_receipt(&destination_path.path, &identity)?;
        return Ok(());
    }
    if destination_path.exists {
        shared.total_files.store(
            package.final_files.len(),
            std::sync::atomic::Ordering::Release,
        );
        shared.set_phase(Phase::ValidatingFiles);
        if validate_final_tree(&destination_path.path, package, &identity, shared, false).is_ok() {
            return Ok(());
        }
    }
    validate_destination_state(&destination_path.path, &identity)?;

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
    let stage_path = prepare_owned_stage(&stage_path, &identity)?;
    let quote = quote(&destination_path.path, &cache_path, package)?;
    let required_cache_bytes = cache_space_requirement(package, &cache_path)?;
    check_space(
        &quote,
        same_volume(&cache_path, &destination_path.nearest_existing),
        required_cache_bytes,
    )?;
    validate_destination_state(&destination_path.path, &identity)?;

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
            &cache_path,
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
    for (archive, path) in package.archives.iter().zip(&archives) {
        extract_archive(shared, path, archive, package, &stage_path)?;
    }

    write_version_files(&stage_path)
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
    validate_final_tree(&stage_path, package, &identity, shared, true)?;

    shared.set_phase(Phase::Installing);
    validate_no_reparse_ancestors(&stage_path)?;
    let destination_path = resolve_path(&destination_path.path)?;
    validate_destination_state(&destination_path.path, &identity)?;
    if destination_path.exists {
        fs::remove_dir(&destination_path.path).map_err(|error| {
            format!("The selected destination changed and is no longer empty: {error}")
        })?;
    }
    fs::rename(&stage_path, &destination_path.path)
        .map_err(|error| format!("Could not publish the staged installation: {error}"))?;

    validate_no_reparse_ancestors(&destination_path.path)?;
    validate_tree_has_no_reparse_points(&destination_path.path)?;
    if read_receipt(&destination_path.path)? != identity {
        return Err("Published installation receipt changed unexpectedly.".into());
    }
    remove_receipt(&destination_path.path, &identity)?;
    Ok(())
}

fn cache_space_requirement(package: &BasePackage, cache: &Path) -> Result<u64, String> {
    let unit = allocation_unit(cache)?;
    let objects: Vec<ObjectSpec> = package
        .archives
        .iter()
        .map(|archive| archive.object.clone())
        .collect();

    let mut seen = HashSet::new();
    objects.into_iter().try_fold(0_u64, |required, object| {
        let identity = format!("{}-{}", object.sha256, object.length);
        let deduplication_key = if cfg!(windows) {
            identity.to_ascii_lowercase()
        } else {
            identity.clone()
        };
        if !seen.insert(deduplication_key) {
            return Ok(required);
        }

        let object_allocation = object
            .length
            .div_ceil(unit)
            .checked_mul(unit)
            .ok_or_else(|| "Install cache allocation overflow.".to_string())?;

        let complete = cache_file_footprint(&cache.join(format!("{identity}.object")))?;
        let partial = cache_file_footprint(&cache.join(format!("{identity}.part")))?;
        let complete_allocation = complete.map_or(0, |file| file.allocated_bytes);
        let partial_allocation =
            partial.map_or(0, |file| file.allocated_bytes.min(file.logical_bytes));
        let already_allocated = complete_allocation
            .saturating_add(partial_allocation)
            .min(object_allocation);
        required
            .checked_add(object_allocation - already_allocated)
            .ok_or_else(|| "Install cache space requirement overflow.".to_string())
    })
}

#[derive(Clone, Copy)]
struct CacheFileFootprint {
    logical_bytes: u64,
    allocated_bytes: u64,
}

fn cache_file_footprint(path: &Path) -> Result<Option<CacheFileFootprint>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Could not inspect cached object {}: {error}",
                path.display()
            ));
        }
    };
    check_reparse(path, &metadata)?;
    if !metadata.is_file() {
        return Err(format!(
            "Cached object is not a regular file: {}",
            path.display()
        ));
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|error| {
        format!(
            "Could not safely inspect cache file {}: {error}",
            path.display()
        )
    })?;
    let opened_metadata = file
        .metadata()
        .map_err(|error| format!("Could not inspect cache file {}: {error}", path.display()))?;
    check_reparse(path, &opened_metadata)?;
    if !opened_metadata.is_file() {
        return Err(format!(
            "Cached object is not a regular file: {}",
            path.display()
        ));
    }

    let (allocated_bytes, link_count) = cache_allocation_and_links(&file, &opened_metadata)?;
    if link_count != 1 {
        return Err(format!(
            "Cached object has unexpected hard links: {}",
            path.display()
        ));
    }
    Ok(Some(CacheFileFootprint {
        logical_bytes: opened_metadata.len(),
        allocated_bytes,
    }))
}

#[cfg(unix)]
fn cache_allocation_and_links(_file: &File, metadata: &fs::Metadata) -> Result<(u64, u64), String> {
    use std::os::unix::fs::MetadataExt;
    Ok((metadata.blocks().saturating_mul(512), metadata.nlink()))
}

#[cfg(windows)]
fn cache_allocation_and_links(file: &File, _metadata: &fs::Metadata) -> Result<(u64, u64), String> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandleEx,
    };

    let mut info = FILE_STANDARD_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileStandardInfo,
            &mut info as *mut _ as _,
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    }
    .map_err(|error| format!("Could not inspect cache allocation: {error}"))?;
    let allocated_bytes = u64::try_from(info.AllocationSize)
        .map_err(|_| "Cached object has an invalid allocation size.".to_string())?;
    Ok((allocated_bytes, u64::from(info.NumberOfLinks)))
}

#[cfg(not(any(unix, windows)))]
fn cache_allocation_and_links(_file: &File, metadata: &fs::Metadata) -> Result<(u64, u64), String> {
    Ok((metadata.len(), 1))
}

fn inventory_bytes(files: &[InstallFile]) -> Result<u64, String> {
    files.iter().try_fold(0_u64, |sum, file| {
        sum.checked_add(file.length)
            .ok_or_else(|| "Install inventory length overflow.".into())
    })
}

fn staging_space_requirement(package: &BasePackage, destination: &Path) -> Result<u64, String> {
    let unit = allocation_unit(destination)?;
    staging_space_bound(package, unit)
}

fn staging_space_bound(package: &BasePackage, unit: u64) -> Result<u64, String> {
    let mut directories =
        expected_directories(package.final_files.iter().map(|file| file.path.as_str()));
    for archive in &package.archives {
        directories.extend(expected_directories(
            archive.empty_directories.iter().map(String::as_str),
        ));
        for directory in &archive.empty_directories {
            directories.insert(directory.to_ascii_lowercase());
        }
    }
    let file_bytes = package.final_files.iter().try_fold(0_u64, |total, file| {
        let blocks = file.length.div_ceil(unit);
        total
            .checked_add(
                blocks
                    .checked_mul(unit)
                    .ok_or("Install file allocation overflow.")?,
            )
            .ok_or_else(|| "Install file allocation overflow.".to_string())
    })?;
    // Reserve one allocation unit per file/directory record, the root, and the receipt.
    let records = u64::try_from(package.final_files.len() + directories.len() + 2)
        .map_err(|_| "Install metadata allocation overflow.".to_string())?;
    let metadata_bytes = records
        .checked_mul(unit)
        .ok_or_else(|| "Install metadata allocation overflow.".to_string())?;
    Ok(package.staging_bytes.max(
        file_bytes
            .checked_add(metadata_bytes)
            .ok_or_else(|| "Install staging allocation overflow.".to_string())?,
    ))
}

#[cfg(windows)]
fn allocation_unit(destination: &Path) -> Result<u64, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceW;
    use windows::core::PCWSTR;

    let path = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut sectors = 0_u32;
    let mut bytes = 0_u32;
    unsafe {
        GetDiskFreeSpaceW(
            PCWSTR(path.as_ptr()),
            Some(&mut sectors),
            Some(&mut bytes),
            None,
            None,
        )
    }
    .map_err(|error| format!("Could not inspect installation allocation unit: {error}"))?;
    u64::from(sectors)
        .checked_mul(u64::from(bytes))
        .filter(|unit| *unit > 0)
        .ok_or_else(|| "Invalid installation allocation unit.".to_string())
}

#[cfg(unix)]
fn allocation_unit(destination: &Path) -> Result<u64, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| "Installation path contains a NUL byte.".to_string())?;
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return Err(format!(
            "Could not inspect installation allocation unit: {}",
            std::io::Error::last_os_error()
        ));
    }
    let info = unsafe { info.assume_init() };
    let unit = if info.f_frsize > 0 {
        info.f_frsize
    } else {
        info.f_bsize
    };
    // statvfs block sizes vary in width by target; the i128 hop keeps the narrowing lint-clean.
    u64::try_from(i128::from(unit))
        .ok()
        .filter(|unit| *unit > 0)
        .ok_or_else(|| "Invalid installation allocation unit.".to_string())
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
        let expected_hash = format!("{:x}", Sha256::digest(contents.as_bytes()));
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

fn check_space(
    quote: &InstallQuote,
    same_volume: bool,
    required_cache_bytes: u64,
) -> Result<(), String> {
    if same_volume {
        let available = quote
            .available_cache_bytes
            .zip(quote.available_destination_bytes)
            .map(|(cache, destination)| cache.min(destination))
            .or(quote.available_cache_bytes)
            .or(quote.available_destination_bytes);
        let required = required_cache_bytes
            .checked_add(quote.staging_bytes)
            .ok_or_else(|| "Install space requirement overflow.".to_string())?;
        return require_space(available, required, "download and staging volume");
    }
    require_space(
        quote.available_cache_bytes,
        required_cache_bytes,
        "download cache",
    )?;
    require_space(
        quote.available_destination_bytes,
        quote.staging_bytes,
        "installation destination",
    )
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

fn require_space(available: Option<u64>, required: u64, volume: &str) -> Result<(), String> {
    if let Some(available) = available.filter(|available| *available < required) {
        return Err(format!(
            "Insufficient free space on the {volume}: need {required} bytes, {available} bytes available."
        ));
    }
    Ok(())
}

fn stage_path_for(destination: &Path) -> Result<PathBuf, String> {
    let leaf = destination
        .file_name()
        .ok_or_else(|| "The installation destination has no final path component.".to_string())?;
    let mut stage_leaf = leaf.to_os_string();
    stage_leaf.push(".bahamut-stage");
    Ok(destination.with_file_name(stage_leaf))
}

fn ensure_destination_parent(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "The installation destination has no parent directory.".to_string())?;
    resolve_path(parent)?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the installation parent: {error}"))?;
    let resolved = resolve_path(parent)?;
    if !resolved.exists || !resolved.path.is_dir() {
        return Err("The installation parent changed while it was being created.".into());
    }
    Ok(())
}

fn reject_overlapping_paths(destination: &Path, cache: &Path, stage: &Path) -> Result<(), String> {
    for (left, right, label) in [
        (destination, cache, "destination and download cache"),
        (stage, cache, "staging directory and download cache"),
    ] {
        if paths_overlap(left, right) {
            return Err(format!("The {label} must be separate paths."));
        }
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    let left = folded_components(left);
    let right = folded_components(right);
    left.starts_with(&right) || right.starts_with(&left)
}

fn folded_components(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

fn resolve_path(path: &Path) -> Result<ResolvedPath, String> {
    if !path.is_absolute() {
        return Err(format!("Expected an absolute path: {}", path.display()));
    }
    if path
        .as_os_str()
        .to_string_lossy()
        .split(['\\', '/'])
        .any(|part| matches!(part, "." | ".."))
    {
        return Err(format!(
            "The path contains a relative alias: {}",
            path.display()
        ));
    }
    let mut probe = path.to_path_buf();
    let mut missing = Vec::new();
    let deepest = loop {
        match fs::symlink_metadata(&probe) {
            Ok(metadata) => {
                check_reparse(&probe, &metadata)?;
                break probe;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let leaf = probe.file_name().ok_or_else(|| {
                    format!("The path has no existing parent: {}", path.display())
                })?;
                missing.push(leaf.to_os_string());
                if !probe.pop() {
                    return Err(format!("Could not resolve the path: {}", path.display()));
                }
            }
            Err(error) => return Err(format!("Could not inspect {}: {error}", probe.display())),
        }
    };
    if !fs::metadata(&deepest)
        .map_err(|error| format!("Could not inspect {}: {error}", deepest.display()))?
        .is_dir()
    {
        return Err(format!(
            "A path ancestor is not a directory: {}",
            deepest.display()
        ));
    }
    inspect_existing_ancestors(&deepest)?;
    let nearest_existing = fs::canonicalize(&deepest)
        .map_err(|error| format!("Could not resolve {}: {error}", deepest.display()))?;
    let mut resolved = nearest_existing.clone();
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(ResolvedPath {
        path: resolved,
        nearest_existing,
        exists: missing.is_empty(),
    })
}

fn inspect_existing_ancestors(path: &Path) -> Result<(), String> {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let metadata = fs::symlink_metadata(&prefix)
            .map_err(|error| format!("Could not inspect {}: {error}", prefix.display()))?;
        check_reparse(&prefix, &metadata)?;
        if !metadata.is_dir() {
            return Err(format!(
                "A path ancestor is not a directory: {}",
                prefix.display()
            ));
        }
    }
    Ok(())
}

fn validate_no_reparse_ancestors(path: &Path) -> Result<(), String> {
    let resolved = resolve_path(path)?;
    if !resolved.exists || resolved.path != path {
        return Err(format!("The owned path changed: {}", path.display()));
    }
    Ok(())
}

fn check_reparse(path: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    if metadata.file_type().is_symlink() || is_windows_reparse(metadata) {
        return Err(format!(
            "Refusing a link or reparse point: {}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn is_windows_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_windows_reparse(_metadata: &fs::Metadata) -> bool {
    false
}

fn ensure_cache_directory(path: &Path) -> Result<PathBuf, String> {
    let before = resolve_path(path)?;
    fs::create_dir_all(&before.path)
        .map_err(|error| format!("Could not create the download cache: {error}"))?;
    let after = resolve_path(&before.path)?;
    if !after.exists || after.path != before.path {
        return Err("The download cache path changed while it was being created.".into());
    }
    Ok(after.path)
}

fn validate_destination_state(destination: &Path, identity: &str) -> Result<(), String> {
    let resolved = resolve_path(destination)?;
    if !resolved.exists {
        return Ok(());
    }
    if !fs::symlink_metadata(&resolved.path)
        .map_err(|error| format!("Could not inspect the destination: {error}"))?
        .is_dir()
    {
        return Err("The selected destination is not a directory.".into());
    }
    let entries = fs::read_dir(&resolved.path)
        .map_err(|error| format!("Could not inspect the destination: {error}"))?;
    let mut found_receipt = false;
    for entry in entries {
        let entry = entry.map_err(|error| format!("Could not inspect the destination: {error}"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("Could not inspect the destination: {error}"))?;
        check_reparse(&entry.path(), &metadata)?;
        if entry.file_name().to_string_lossy() == RECEIPT_NAME {
            found_receipt = true;
            continue;
        }
        return Err("The selected destination is not empty.".into());
    }
    if found_receipt && read_receipt(&resolved.path)? == identity {
        return Ok(());
    }
    if found_receipt {
        return Err("The selected destination contains an unowned install receipt.".into());
    }
    Ok(())
}

fn validate_quote_destination_state(destination: &Path, identity: &str) -> Result<(), String> {
    let resolved = resolve_path(destination)?;
    if !resolved.exists || !destination_has_receipt(&resolved.path)? {
        return validate_destination_state(destination, identity);
    }
    if read_receipt(&resolved.path)? != identity {
        return Err("The selected destination belongs to another package.".into());
    }
    validate_tree_has_no_reparse_points(&resolved.path)
}

fn destination_has_receipt(destination: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(destination.join(RECEIPT_NAME)) {
        Ok(metadata) => {
            check_reparse(&destination.join(RECEIPT_NAME), &metadata)?;
            if !metadata.is_file() {
                return Err("The installation receipt is not a regular file.".into());
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "Could not inspect the installation receipt: {error}"
        )),
    }
}

fn read_receipt(directory: &Path) -> Result<String, String> {
    let path = directory.join(RECEIPT_NAME);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("Could not read the installation receipt: {error}"))?;
    check_reparse(&path, &metadata)?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return Err("The installation receipt is invalid.".into());
    }
    let receipt: InstallReceipt = serde_json::from_reader(
        File::open(&path)
            .map_err(|error| format!("Could not read the installation receipt: {error}"))?,
    )
    .map_err(|error| format!("The installation receipt is invalid: {error}"))?;
    if receipt.schema_version != 1 {
        return Err("The installation receipt has an unsupported version.".into());
    }
    validate_hash(&receipt.manifest_sha256)?;
    Ok(receipt.manifest_sha256)
}

fn write_receipt(directory: &Path, identity: &str) -> Result<(), String> {
    let receipt = InstallReceipt {
        schema_version: 1,
        manifest_sha256: identity.to_owned(),
    };
    let bytes = serde_json::to_vec(&receipt).expect("InstallReceipt is infallibly serializable");
    let path = directory.join(RECEIPT_NAME);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("Could not create the installation receipt: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Could not persist the installation receipt: {error}"))
}

fn remove_receipt(directory: &Path, identity: &str) -> Result<(), String> {
    let path = directory.join(RECEIPT_NAME);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("Could not inspect the installation receipt: {error}"))?;
    check_reparse(&path, &metadata)?;
    if !metadata.is_file() || read_receipt(directory)? != identity {
        return Err("The installation receipt changed before removal.".into());
    }
    fs::remove_file(path)
        .map_err(|error| format!("Could not remove the installation receipt: {error}"))
}

fn prepare_owned_stage(stage: &Path, identity: &str) -> Result<PathBuf, String> {
    let stage = resolve_path(stage)?;
    if stage.exists {
        if !fs::symlink_metadata(&stage.path)
            .map_err(|error| format!("Could not inspect staged installation: {error}"))?
            .is_dir()
        {
            return Err("The staging path is not a directory.".into());
        }
        if read_receipt(&stage.path)? != identity {
            return Err("The existing staging directory belongs to another package.".into());
        }
        validate_tree_has_no_reparse_points(&stage.path)?;
        fs::remove_dir_all(&stage.path)
            .map_err(|error| format!("Could not reset owned staging data: {error}"))?;
    }
    fs::create_dir(&stage.path)
        .map_err(|error| format!("Could not create same-volume staging: {error}"))?;
    if let Err(error) = write_receipt(&stage.path, identity) {
        validate_tree_has_no_reparse_points(&stage.path)?;
        let _ = fs::remove_dir(&stage.path);
        return Err(error);
    }
    Ok(stage.path)
}

fn validate_tree_has_no_reparse_points(root: &Path) -> Result<(), String> {
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("Could not inspect {}: {error}", root.display()))?;
    check_reparse(root, &root_metadata)?;
    if !root_metadata.is_dir() {
        return Err(format!("Expected a directory: {}", root.display()));
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("Could not inspect {}: {error}", directory.display()))?
        {
            let entry =
                entry.map_err(|error| format!("Could not inspect staging data: {error}"))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
            check_reparse(&path, &metadata)?;
            if metadata.is_dir() {
                pending.push(path);
            } else if !metadata.is_file() {
                return Err(format!(
                    "Unexpected special file in owned staging: {}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn extract_archive(
    shared: &Arc<InstallShared>,
    archive_path: &Path,
    archive_spec: &BaseArchive,
    package: &BasePackage,
    stage: &Path,
) -> Result<(), String> {
    let expected = archive_file_map(archive_spec);
    let ignored = archive_ignored_file_map(archive_spec, package)?;
    validate_archive_inventory(archive_path, archive_spec, &expected, &ignored)?;
    let file = File::open(archive_path)
        .map_err(|error| format!("Could not open the verified base archive: {error}"))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("Could not read the base archive: {error}"))?;
    let mut extracted = 0_u64;
    for index in 0..archive.len() {
        if shared.wait_if_paused() {
            return Err("Installation was cancelled.".into());
        }
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Could not read a base archive entry: {error}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_owned();
        if archive_spec.layout == ArchiveLayout::FinalFantasyXivWrapper
            && name.starts_with("__MACOSX/")
        {
            continue;
        }
        let relative = archive_payload_name(archive_spec.layout, &name)?;
        if let Some(spec) = ignored.get(&relative.to_ascii_lowercase()) {
            let digest = copy_and_hash(
                shared,
                &mut entry,
                &mut std::io::sink(),
                &spec.path,
                spec.length,
            )?;
            if !digest.eq_ignore_ascii_case(&spec.sha256) {
                return Err(format!(
                    "Excluded source file failed integrity verification: {}",
                    spec.path
                ));
            }
            continue;
        }
        let spec = expected
            .get(&relative.to_ascii_lowercase())
            .ok_or_else(|| format!("Unexpected base archive entry: {name}"))?;
        let path = joined_content_path(stage, &spec.path);
        create_stage_parents(stage, path.parent().expect("a file path has a parent"))?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("Could not create staged file {}: {error}", spec.path))?;
        let digest = copy_and_hash(shared, &mut entry, &mut output, &spec.path, spec.length)?;
        if !digest.eq_ignore_ascii_case(&spec.sha256) {
            return Err(format!(
                "Base archive file failed integrity verification: {}",
                spec.path
            ));
        }
        extracted = extracted
            .checked_add(spec.length)
            .ok_or_else(|| "Extracted base size overflow.".to_string())?;
        shared
            .file_idx
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
    let expected_bytes = inventory_bytes(&archive_spec.files)?;
    if extracted != expected_bytes {
        return Err("Extracted base archive size differs from its manifest.".into());
    }
    for directory in &archive_spec.empty_directories {
        let path = joined_content_path(stage, directory);
        create_stage_parents(stage, &path)?;
    }
    Ok(())
}

fn archive_payload_name(layout: ArchiveLayout, name: &str) -> Result<&str, String> {
    match layout {
        ArchiveLayout::Flat => Ok(name),
        ArchiveLayout::FinalFantasyXivWrapper => name
            .strip_prefix("FINAL FANTASY XIV/")
            .filter(|relative| !relative.is_empty())
            .ok_or_else(|| format!("Unexpected wrapped archive entry: {name}")),
    }
}

fn archive_ignored_file_map<'a>(
    archive: &'a BaseArchive,
    package: &'a BasePackage,
) -> Result<HashMap<String, &'a InstallFile>, String> {
    let mut ignored = HashMap::new();
    for file in &archive.excluded_files {
        let _ = ignored.insert(file.path.to_ascii_lowercase(), file);
    }
    if archive.layout == ArchiveLayout::FinalFantasyXivWrapper {
        for name in ["boot.ver", "game.ver"] {
            let file = package
                .final_files
                .iter()
                .find(|file| file.path.eq_ignore_ascii_case(name))
                .ok_or_else(|| format!("Missing managed version identity: {name}"))?;
            let _ = ignored.insert(name.to_owned(), file);
        }
    }
    Ok(ignored)
}

fn archive_file_map(archive: &BaseArchive) -> HashMap<String, &InstallFile> {
    let mut files = HashMap::new();
    for file in &archive.files {
        let _ = files.insert(file.path.to_ascii_lowercase(), file);
    }
    files
}

fn validate_archive_inventory(
    path: &Path,
    archive_spec: &BaseArchive,
    expected: &HashMap<String, &InstallFile>,
    ignored: &HashMap<String, &InstallFile>,
) -> Result<(), String> {
    let file = File::open(path).map_err(|error| format!("Could not open base archive: {error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("Could not read base archive: {error}"))?;
    let mut expected_dirs = expected_directories(
        expected
            .values()
            .chain(ignored.values())
            .map(|file| file.path.as_str()),
    );
    for directory in &archive_spec.empty_directories {
        expected_dirs.insert(directory.to_ascii_lowercase());
        expected_dirs.extend(expected_directories(std::iter::once(directory.as_str())));
    }
    let mut seen = HashSet::new();
    let mut found_files = HashSet::new();
    let mut found_ignored = HashSet::new();
    let mut found_directories = HashSet::new();
    let mut metadata_files = Vec::new();
    let mut metadata_directories = HashSet::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("Could not inspect a base archive entry: {error}"))?;
        let is_dir = entry.is_dir();
        let raw_name = entry.name();
        let name = if is_dir {
            raw_name
                .strip_suffix('/')
                .ok_or_else(|| format!("Invalid directory entry in base archive: {raw_name}"))?
        } else {
            raw_name
        };
        validate_relative_path(name)?;
        if name.eq_ignore_ascii_case(RECEIPT_NAME) {
            return Err("Base archive contains a reserved launcher receipt path.".into());
        }
        validate_entry_mode(entry.unix_mode(), is_dir, name)?;
        let folded = name.to_ascii_lowercase();
        if !seen.insert(folded.clone()) {
            return Err(format!("Duplicate normalized archive destination: {name}"));
        }
        if archive_spec.layout == ArchiveLayout::FinalFantasyXivWrapper
            && (name == "__MACOSX" || name.starts_with("__MACOSX/"))
        {
            if is_dir {
                metadata_directories.insert(folded);
            } else {
                metadata_files.push(name.to_owned());
            }
            continue;
        }
        if archive_spec.layout == ArchiveLayout::FinalFantasyXivWrapper
            && name == "FINAL FANTASY XIV"
            && is_dir
        {
            found_directories.insert(String::new());
            continue;
        }
        let relative = archive_payload_name(archive_spec.layout, name)?;
        validate_relative_path(relative)?;
        let relative_folded = relative.to_ascii_lowercase();
        if is_dir {
            if !expected_dirs.contains(&relative_folded) {
                return Err(format!("Unexpected directory in base archive: {name}"));
            }
            found_directories.insert(relative_folded);
            continue;
        }
        let spec = expected
            .get(&relative_folded)
            .or_else(|| ignored.get(&relative_folded))
            .ok_or_else(|| format!("Unexpected file in base archive: {name}"))?;
        if entry.size() != spec.length {
            return Err(format!("Declared length mismatch for base file: {name}"));
        }
        if expected.contains_key(&relative_folded) {
            found_files.insert(relative_folded);
        } else {
            found_ignored.insert(relative_folded);
        }
    }
    if found_files.len() != expected.len() {
        let missing = expected
            .keys()
            .find(|name| !found_files.contains(*name))
            .cloned()
            .unwrap_or_else(|| "unknown file".into());
        return Err(format!("Base archive is missing inventory file: {missing}"));
    }
    if found_ignored.len() != ignored.len() {
        let missing = ignored
            .keys()
            .find(|name| !found_ignored.contains(*name))
            .cloned()
            .unwrap_or_else(|| "unknown file".into());
        return Err(format!(
            "Base archive is missing excluded or managed source file: {missing}"
        ));
    }
    for directory in &archive_spec.empty_directories {
        if !found_directories.contains(&directory.to_ascii_lowercase()) {
            return Err(format!(
                "Base archive is missing empty directory: {directory}"
            ));
        }
    }
    if archive_spec.layout == ArchiveLayout::FinalFantasyXivWrapper {
        if metadata_files.len() as u64 != archive_spec.apple_metadata_files {
            return Err("Apple metadata entry count differs from the manifest.".into());
        }
        let expected_metadata_dirs =
            expected_directories(metadata_files.iter().map(String::as_str));
        for metadata in &metadata_files {
            let counterpart = apple_metadata_counterpart(metadata)?;
            if !seen.contains(&counterpart.to_ascii_lowercase()) {
                return Err(format!(
                    "Apple metadata has no source counterpart: {metadata}"
                ));
            }
        }
        if !metadata_directories.is_subset(&expected_metadata_dirs) {
            return Err("Apple metadata contains an unexpected directory.".into());
        }
    }
    Ok(())
}

fn apple_metadata_counterpart(name: &str) -> Result<String, String> {
    let relative = name
        .strip_prefix("__MACOSX/")
        .ok_or_else(|| format!("Unexpected Apple metadata path: {name}"))?;
    let (parent, leaf) = relative.rsplit_once('/').unwrap_or(("", relative));
    let counterpart_leaf = leaf
        .strip_prefix("._")
        .filter(|leaf| !leaf.is_empty())
        .ok_or_else(|| format!("Unexpected Apple metadata path: {name}"))?;
    let counterpart = if parent.is_empty() {
        counterpart_leaf.to_owned()
    } else {
        format!("{parent}/{counterpart_leaf}")
    };
    if counterpart != "FINAL FANTASY XIV" && !counterpart.starts_with("FINAL FANTASY XIV/") {
        return Err(format!(
            "Apple metadata is outside the selected source: {name}"
        ));
    }
    Ok(counterpart)
}

fn validate_entry_mode(mode: Option<u32>, is_dir: bool, name: &str) -> Result<(), String> {
    if let Some(mode) = mode {
        let kind = mode & 0o170000;
        if mode & 0o7000 != 0
            || (is_dir && kind != 0 && kind != 0o040000)
            || (!is_dir && kind != 0 && kind != 0o100000)
        {
            return Err(format!(
                "Links or special modes are not allowed in base archives: {name}"
            ));
        }
    }
    Ok(())
}

fn expected_directories<'a>(paths: impl Iterator<Item = &'a str>) -> HashSet<String> {
    let mut directories = HashSet::new();
    for path in paths {
        for (index, _) in path.match_indices('/') {
            directories.insert(path[..index].to_ascii_lowercase());
        }
    }
    directories
}

fn joined_content_path(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |mut path, part| {
            path.push(part);
            path
        })
}

fn create_stage_parents(stage: &Path, directory: &Path) -> Result<(), String> {
    let relative = directory
        .strip_prefix(stage)
        .map_err(|_| "Staged file escaped its owned directory.".to_string())?;
    let mut current = stage.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("Invalid directory component in staged path.".into());
        };
        current.push(name);
        match fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&current).map_err(|error| {
                    format!(
                        "Could not inspect staged directory {}: {error}",
                        current.display()
                    )
                })?;
                check_reparse(&current, &metadata)?;
                if !metadata.is_dir() {
                    return Err(format!(
                        "Staged parent is not a directory: {}",
                        current.display()
                    ));
                }
            }
            Err(error) => {
                return Err(format!(
                    "Could not create staged directory {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

fn copy_and_hash(
    shared: &InstallShared,
    reader: &mut impl Read,
    writer: &mut impl Write,
    label: &str,
    expected_length: u64,
) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; IO_BUFFER_BYTES];
    while total < expected_length {
        if shared.wait_if_paused() {
            return Err("Installation was cancelled.".into());
        }
        let remaining = expected_length - total;
        let limit = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = reader
            .read(&mut buffer[..limit])
            .map_err(|error| format!("Could not read staged file {label}: {error}"))?;
        if read == 0 {
            return Err(format!(
                "Staged file ended before its manifest length: {label}"
            ));
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .map_err(|error| format!("Could not write staged file {label}: {error}"))?;
    }
    let mut excess = [0_u8; 1];
    if reader
        .read(&mut excess)
        .map_err(|error| format!("Could not check staged file length {label}: {error}"))?
        != 0
    {
        return Err(format!("Staged file exceeded its manifest length: {label}"));
    }
    writer
        .flush()
        .map_err(|error| format!("Could not flush staged file {label}: {error}"))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_final_tree(
    root: &Path,
    package: &BasePackage,
    identity: &str,
    shared: &InstallShared,
    receipt_required: bool,
) -> Result<(), String> {
    validate_tree_has_no_reparse_points(root)?;
    if receipt_required && read_receipt(root)? != identity {
        return Err("Staging receipt does not match the selected package.".into());
    }
    if !receipt_required && destination_has_receipt(root)? {
        return Err("A staged installation receipt is present in the destination.".into());
    }
    let expected: HashMap<String, &InstallFile> = package
        .final_files
        .iter()
        .map(|file| (file.path.to_ascii_lowercase(), file))
        .collect();
    let mut expected_dirs = expected_directories(expected.values().map(|file| file.path.as_str()));
    let empty_dirs: HashSet<String> = package
        .archives
        .iter()
        .flat_map(|archive| archive.empty_directories.iter())
        .map(|directory| directory.to_ascii_lowercase())
        .collect();
    expected_dirs.extend(expected_directories(empty_dirs.iter().map(String::as_str)));
    expected_dirs.extend(empty_dirs.iter().cloned());
    let mut found_empty_dirs = HashSet::new();
    let mut found = HashSet::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, relative)) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("Could not validate staged files: {error}"))?
        {
            if shared.wait_if_paused() {
                return Err("Installation was cancelled.".into());
            }
            let entry =
                entry.map_err(|error| format!("Could not validate staged files: {error}"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            if relative.is_empty() && name == RECEIPT_NAME && receipt_required {
                continue;
            }
            validate_relative_path(&child)?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("Could not inspect staged entry: {error}"))?;
            check_reparse(&path, &metadata)?;
            let folded = child.to_ascii_lowercase();
            if metadata.is_dir() {
                if !expected_dirs.contains(&folded) {
                    return Err(format!("Unexpected staged directory: {child}"));
                }
                if empty_dirs.contains(&folded) {
                    found_empty_dirs.insert(folded.clone());
                }
                pending.push((path, child));
            } else if metadata.is_file() {
                let spec = expected
                    .get(&folded)
                    .ok_or_else(|| format!("Unexpected staged file: {child}"))?;
                if !found.insert(folded) {
                    return Err(format!("Duplicate staged destination: {child}"));
                }
                let digest = hash_file(shared, &path, &spec.path, spec.length)?;
                if !digest.eq_ignore_ascii_case(&spec.sha256) {
                    return Err(format!(
                        "Final install file failed validation: {}",
                        spec.path
                    ));
                }
                shared
                    .file_idx
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            } else {
                return Err(format!("Unexpected special file in installation: {child}"));
            }
        }
    }
    if found.len() != expected.len() {
        let missing = expected
            .keys()
            .find(|name| !found.contains(*name))
            .cloned()
            .unwrap_or_else(|| "unknown file".into());
        return Err(format!("Final installation is missing: {missing}"));
    }
    if found_empty_dirs.len() != empty_dirs.len() {
        return Err("Final installation is missing an empty source directory.".into());
    }
    Ok(())
}

fn hash_file(
    shared: &InstallShared,
    path: &Path,
    label: &str,
    expected_length: u64,
) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("Could not open final install file {label}: {error}"))?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; IO_BUFFER_BYTES];
    while total < expected_length {
        if shared.wait_if_paused() {
            return Err("Installation was cancelled.".into());
        }
        let remaining = expected_length - total;
        let limit = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = file
            .read(&mut buffer[..limit])
            .map_err(|error| format!("Could not validate final install file {label}: {error}"))?;
        if read == 0 {
            return Err(format!(
                "Final install file ended before its manifest length: {label}"
            ));
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
    }
    let mut excess = [0_u8; 1];
    if file
        .read(&mut excess)
        .map_err(|error| format!("Could not check final install file length {label}: {error}"))?
        != 0
    {
        return Err(format!(
            "Final install file exceeded its manifest length: {label}"
        ));
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::content::manifest::{BaseArchive, InstallFile};
    use crate::content::test_support::tempdir;
    use zip::write::{SimpleFileOptions, ZipWriter};

    struct Entry<'a> {
        path: &'a str,
        contents: &'a [u8],
        mode: Option<u32>,
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("simulated write failure"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn zip_bytes(entries: &[Entry<'_>]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for entry in entries {
            let options = entry
                .mode
                .map(|mode| SimpleFileOptions::default().unix_permissions(mode))
                .unwrap_or_default();
            if entry.path.ends_with('/') {
                zip.add_directory(entry.path, options).unwrap();
            } else {
                zip.start_file(entry.path, options).unwrap();
                zip.write_all(entry.contents).unwrap();
            }
        }
        zip.finish().unwrap().into_inner()
    }

    fn with_unix_mode(mut bytes: Vec<u8>, name: &str, mode: u32) -> Vec<u8> {
        let signature = b"PK\x01\x02";
        let mut cursor = 0;
        while let Some(offset) = bytes[cursor..]
            .windows(signature.len())
            .position(|window| window == signature)
        {
            let start = cursor + offset;
            let name_length = u16::from_le_bytes([bytes[start + 28], bytes[start + 29]]) as usize;
            let extra_length = u16::from_le_bytes([bytes[start + 30], bytes[start + 31]]) as usize;
            let comment_length =
                u16::from_le_bytes([bytes[start + 32], bytes[start + 33]]) as usize;
            let entry_name = &bytes[start + 46..start + 46 + name_length];
            if entry_name == name.as_bytes() {
                bytes[start + 38..start + 42].copy_from_slice(&(mode << 16).to_le_bytes());
                return bytes;
            }
            cursor = start + 46 + name_length + extra_length + comment_length;
        }
        panic!("central-directory entry {name} not found");
    }

    fn file(path: &str, contents: &[u8]) -> InstallFile {
        InstallFile {
            path: path.to_owned(),
            length: contents.len() as u64,
            sha256: format!("{:x}", Sha256::digest(contents)),
        }
    }

    fn fixture(
        root: &Path,
        entries: &[Entry<'_>],
        expected: &[(&str, &[u8])],
        final_boot_hash: Option<&str>,
    ) -> (BasePackage, PathBuf, PathBuf) {
        let cache = root.join("cache");
        fs::create_dir(&cache).unwrap();
        let destination = root.join("game");
        let archive_bytes = zip_bytes(entries);
        let digest = format!("{:x}", Sha256::digest(&archive_bytes));
        let archive_path = cache.join(format!("{digest}-{}.object", archive_bytes.len()));
        fs::write(&archive_path, &archive_bytes).unwrap();

        let archive_files = expected
            .iter()
            .map(|(path, contents)| file(path, contents))
            .collect::<Vec<_>>();
        let mut final_files = archive_files.clone();
        final_files.push(file("boot.ver", FFXIV_BOOT_VERSION.as_bytes()));
        final_files.push(file("game.ver", FFXIV_GAME_VERSION.as_bytes()));
        if let Some(hash) = final_boot_hash {
            final_files
                .iter_mut()
                .find(|entry| entry.path == "ffxivboot.exe")
                .unwrap()
                .sha256 = hash.to_owned();
        }
        let total = archive_files.iter().map(|file| file.length).sum::<u64>();
        let package = BasePackage {
            target_version: FFXIV_GAME_VERSION.to_owned(),
            archives: vec![BaseArchive {
                object: super::super::http::ObjectSpec {
                    object_key: "game/test/archive.zip".to_owned(),
                    length: archive_bytes.len() as u64,
                    sha256: digest,
                },
                files: archive_files,
                layout: ArchiveLayout::Flat,
                excluded_files: Vec::new(),
                empty_directories: Vec::new(),
                apple_metadata_files: 0,
            }],
            final_files,
            staging_bytes: total
                + FFXIV_BOOT_VERSION.len() as u64
                + FFXIV_GAME_VERSION.len() as u64
                + 64,
        };
        (package, cache, destination)
    }

    fn valid_fixture(root: &Path) -> (BasePackage, PathBuf, PathBuf) {
        let entries = [
            Entry {
                path: "ffxivboot.exe",
                contents: b"boot",
                mode: None,
            },
            Entry {
                path: "ffxivgame.exe",
                contents: b"game",
                mode: None,
            },
        ];
        fixture(
            root,
            &entries,
            &[("ffxivboot.exe", b"boot"), ("ffxivgame.exe", b"game")],
            None,
        )
    }

    #[test]
    fn wrapped_final_archive_maps_stamps_metadata_and_empty_directories() {
        let temporary = tempdir().unwrap();
        let entries = [
            Entry {
                path: "FINAL FANTASY XIV/",
                contents: b"",
                mode: None,
            },
            Entry {
                path: "FINAL FANTASY XIV/ffxivboot.exe",
                contents: b"boot",
                mode: None,
            },
            Entry {
                path: "FINAL FANTASY XIV/ffxivgame.exe",
                contents: b"game",
                mode: None,
            },
            Entry {
                path: "FINAL FANTASY XIV/patch.ver",
                contents: b"1.23b",
                mode: None,
            },
            Entry {
                path: "FINAL FANTASY XIV/boot.ver",
                contents: FFXIV_BOOT_VERSION.as_bytes(),
                mode: None,
            },
            Entry {
                path: "FINAL FANTASY XIV/game.ver",
                contents: FFXIV_GAME_VERSION.as_bytes(),
                mode: None,
            },
            Entry {
                path: "FINAL FANTASY XIV/.DS_Store",
                contents: b"finder",
                mode: None,
            },
            Entry {
                path: "FINAL FANTASY XIV/empty/",
                contents: b"",
                mode: None,
            },
            Entry {
                path: "__MACOSX/FINAL FANTASY XIV/._ffxivgame.exe",
                contents: b"apple",
                mode: None,
            },
        ];
        let (mut package, cache, destination) = fixture(
            temporary.path(),
            &entries,
            &[
                ("ffxivboot.exe", b"boot"),
                ("ffxivgame.exe", b"game"),
                ("patch.ver", b"1.23b"),
            ],
            None,
        );
        let archive = &mut package.archives[0];
        archive.layout = ArchiveLayout::FinalFantasyXivWrapper;
        archive.excluded_files = vec![file(".DS_Store", b"finder")];
        archive.empty_directories = vec!["empty".to_owned()];
        archive.apple_metadata_files = 1;
        let shared = InstallShared::with_totals(
            package.download_bytes().unwrap(),
            package.final_files.len(),
        );
        run_install(&shared, &destination, &cache, package)
            .join()
            .unwrap();
        assert_eq!(shared.phase(), Phase::Done, "{:?}", shared.error());
        assert_eq!(fs::read(destination.join("patch.ver")).unwrap(), b"1.23b");
        assert_eq!(
            fs::read(destination.join("boot.ver")).unwrap(),
            FFXIV_BOOT_VERSION.as_bytes()
        );
        assert!(destination.join("empty").is_dir());
        assert!(!destination.join(".DS_Store").exists());
        assert!(!destination.join("__MACOSX").exists());
    }

    #[test]
    fn wrapped_archive_rejects_unlisted_content() {
        let temporary = tempdir().unwrap();
        let entries = [
            Entry {
                path: "FINAL FANTASY XIV/ffxivboot.exe",
                contents: b"boot",
                mode: None,
            },
            Entry {
                path: "FINAL FANTASY XIV/ffxivgame.exe",
                contents: b"game",
                mode: None,
            },
            Entry {
                path: "FINAL FANTASY XIV/boot.ver",
                contents: FFXIV_BOOT_VERSION.as_bytes(),
                mode: None,
            },
            Entry {
                path: "FINAL FANTASY XIV/game.ver",
                contents: FFXIV_GAME_VERSION.as_bytes(),
                mode: None,
            },
            Entry {
                path: "FINAL FANTASY XIV/unreviewed.txt",
                contents: b"unexpected",
                mode: None,
            },
        ];
        let (mut package, cache, destination) = fixture(
            temporary.path(),
            &entries,
            &[("ffxivboot.exe", b"boot"), ("ffxivgame.exe", b"game")],
            None,
        );
        package.archives[0].layout = ArchiveLayout::FinalFantasyXivWrapper;
        let shared = InstallShared::with_totals(
            package.download_bytes().unwrap(),
            package.final_files.len(),
        );
        run_install(&shared, &destination, &cache, package)
            .join()
            .unwrap();
        assert_eq!(shared.phase(), Phase::Error);
        assert!(!destination.exists());
    }

    fn run_install(
        shared: &Arc<InstallShared>,
        destination: &Path,
        cache: &Path,
        package: BasePackage,
    ) -> thread::JoinHandle<()> {
        let shared = Arc::clone(shared);
        let destination = destination.to_path_buf();
        let cache = cache.to_path_buf();
        thread::spawn(move || {
            crate::content::worker::drive(
                shared,
                crate::content::worker::InstallRequest {
                    destination,
                    content_root: "http://127.0.0.1:9".to_owned(),
                    cache_dir: cache,
                    package,
                },
            )
        })
    }

    #[test]
    fn empty_destination_reaches_ready_after_inventory_and_version_checks() {
        let temporary = tempdir().unwrap();
        let (package, cache, destination) = valid_fixture(temporary.path());
        let shared = InstallShared::with_totals(
            package.download_bytes().unwrap(),
            package.final_files.len(),
        );
        run_install(&shared, &destination, &cache, package)
            .join()
            .unwrap();

        assert_eq!(shared.phase(), Phase::Done);
        assert_eq!(
            fs::read(destination.join("ffxivboot.exe")).unwrap(),
            b"boot"
        );
        assert_eq!(
            fs::read(destination.join("ffxivgame.exe")).unwrap(),
            b"game"
        );
        assert_eq!(
            fs::read_to_string(destination.join("game.ver")).unwrap(),
            FFXIV_GAME_VERSION
        );
        assert!(!destination.join(RECEIPT_NAME).exists());
    }

    #[test]
    fn matching_owned_staging_is_reset_and_recovered_after_interruption() {
        let temporary = tempdir().unwrap();
        let (package, cache, destination) = valid_fixture(temporary.path());
        let stage = stage_path_for(&destination).unwrap();
        fs::create_dir(&stage).unwrap();
        write_receipt(&stage, &package_identity(&package)).unwrap();
        fs::write(stage.join("partial.bin"), b"unfinished extraction").unwrap();

        let shared = InstallShared::with_totals(
            package.download_bytes().unwrap(),
            package.final_files.len(),
        );
        run_install(&shared, &destination, &cache, package)
            .join()
            .unwrap();

        assert_eq!(shared.phase(), Phase::Done);
        assert!(!stage.exists());
        assert!(destination.join("ffxivgame.exe").is_file());
    }

    #[test]
    fn retry_reclaims_owned_staging_before_space_preflight() {
        let temporary = tempdir().unwrap();
        let (mut package, cache, destination) = valid_fixture(temporary.path());
        package.staging_bytes = u64::MAX - cache_space_requirement(&package, &cache).unwrap();
        let identity = package_identity(&package);
        let stage = stage_path_for(&destination).unwrap();
        fs::create_dir(&stage).unwrap();
        write_receipt(&stage, &identity).unwrap();
        fs::write(stage.join("partial.bin"), b"unfinished extraction").unwrap();
        let shared = InstallShared::with_totals(
            package.download_bytes().unwrap(),
            package.final_files.len(),
        );

        let error = install(
            &shared,
            &destination,
            "http://127.0.0.1:9",
            &cache,
            &package,
        )
        .unwrap_err();

        assert!(error.contains("Insufficient free space"), "{error}");
        assert!(!stage.join("partial.bin").exists());
        assert_eq!(read_receipt(&stage).unwrap(), identity);
        assert!(!destination.exists());
    }

    #[test]
    fn nonempty_destination_is_rejected_without_changing_user_files() {
        let temporary = tempdir().unwrap();
        let (package, cache, destination) = valid_fixture(temporary.path());
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("notes.txt"), b"owner data").unwrap();
        let shared = InstallShared::with_totals(
            package.download_bytes().unwrap(),
            package.final_files.len(),
        );
        run_install(&shared, &destination, &cache, package)
            .join()
            .unwrap();

        assert_eq!(shared.phase(), Phase::Error);
        assert_eq!(
            fs::read(destination.join("notes.txt")).unwrap(),
            b"owner data"
        );
        assert!(!destination.join("ffxivgame.exe").exists());
    }

    #[test]
    fn changed_destination_is_not_removed_at_publish() {
        let temporary = tempdir().unwrap();
        let (package, cache, destination) = valid_fixture(temporary.path());
        fs::create_dir(&destination).unwrap();
        let shared = InstallShared::with_totals(
            package.download_bytes().unwrap(),
            package.final_files.len(),
        );
        shared.request_pause();
        let worker = run_install(&shared, &destination, &cache, package);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !shared.is_paused() {
            assert!(
                Instant::now() < deadline,
                "download did not reach its durable pause"
            );
            thread::sleep(Duration::from_millis(1));
        }
        fs::write(destination.join("owner.txt"), b"changed after start").unwrap();
        shared.request_resume();
        worker.join().unwrap();

        assert_eq!(shared.phase(), Phase::Error);
        assert_eq!(
            fs::read(destination.join("owner.txt")).unwrap(),
            b"changed after start"
        );
        assert!(!destination.join("ffxivgame.exe").exists());
    }

    #[test]
    fn unsafe_archive_path_is_rejected_before_publish() {
        let temporary = tempdir().unwrap();
        let entries = [
            Entry {
                path: "ffxivboot.exe",
                contents: b"boot",
                mode: None,
            },
            Entry {
                path: "ffxivgame.exe",
                contents: b"game",
                mode: None,
            },
            Entry {
                path: "../escape.txt",
                contents: b"escape",
                mode: None,
            },
        ];
        let expected = [
            ("ffxivboot.exe", b"boot".as_slice()),
            ("ffxivgame.exe", b"game".as_slice()),
        ];
        let (package, cache, destination) = fixture(temporary.path(), &entries, &expected, None);
        let error = install(
            &InstallShared::with_totals(1, 1),
            &destination,
            "http://127.0.0.1:9",
            &cache,
            &package,
        )
        .unwrap_err();

        assert!(error.contains("Unsafe content path"), "{error}");
        assert!(!destination.exists());
        assert!(!destination.join("game.ver").exists());
    }

    #[test]
    fn archive_links_and_duplicate_destinations_are_rejected() {
        let temporary = tempdir().unwrap();
        let entries = [
            Entry {
                path: "ffxivboot.exe",
                contents: b"boot",
                mode: None,
            },
            Entry {
                path: "ffxivgame.exe",
                contents: b"game",
                mode: None,
            },
            Entry {
                path: "link",
                contents: b"ffxivgame.exe",
                mode: Some(0o120777),
            },
        ];
        let (mut package, cache, destination) = valid_fixture(temporary.path());
        let bytes = with_unix_mode(zip_bytes(&entries), "link", 0o120777);
        let digest = format!("{:x}", Sha256::digest(&bytes));
        package.archives[0].object.sha256 = digest.clone();
        package.archives[0].object.length = bytes.len() as u64;
        let path = cache.join(format!("{digest}-{}.object", bytes.len()));
        fs::write(&path, &bytes).unwrap();
        let error = install(
            &InstallShared::with_totals(1, 1),
            &destination,
            "http://127.0.0.1:9",
            &cache,
            &package,
        )
        .unwrap_err();
        assert!(error.contains("Links or special modes"), "{error}");
        assert!(!destination.exists());

        let duplicate_root = temporary.path().join("duplicate");
        fs::create_dir(&duplicate_root).unwrap();
        let duplicate_entries = [
            Entry {
                path: "ffxivboot.exe",
                contents: b"boot",
                mode: None,
            },
            Entry {
                path: "FFXIVBOOT.EXE",
                contents: b"boot",
                mode: None,
            },
            Entry {
                path: "ffxivgame.exe",
                contents: b"game",
                mode: None,
            },
        ];
        let (package, cache, destination) = fixture(
            &duplicate_root,
            &duplicate_entries,
            &[("ffxivboot.exe", b"boot"), ("ffxivgame.exe", b"game")],
            None,
        );
        let error = install(
            &InstallShared::with_totals(1, 1),
            &destination,
            "http://127.0.0.1:9",
            &cache,
            &package,
        )
        .unwrap_err();
        assert!(
            error.contains("Duplicate normalized archive destination"),
            "{error}"
        );
        assert!(!destination.exists());
    }

    #[test]
    fn declared_size_hash_and_missing_inventory_are_rejected() {
        let temporary = tempdir().unwrap();
        let entries = [
            Entry {
                path: "ffxivboot.exe",
                contents: b"boot",
                mode: None,
            },
            Entry {
                path: "ffxivgame.exe",
                contents: b"game",
                mode: None,
            },
        ];
        let (mut package, cache, destination) = valid_fixture(temporary.path());
        package.archives[0].files[0].length += 1;
        package.staging_bytes += 1;
        let error = install(
            &InstallShared::with_totals(1, 1),
            &destination,
            "http://127.0.0.1:9",
            &cache,
            &package,
        )
        .unwrap_err();
        assert!(error.contains("Declared length mismatch"), "{error}");
        assert!(!destination.exists());

        let hash_root = temporary.path().join("wrong-hash");
        fs::create_dir(&hash_root).unwrap();
        let wrong_hash = "0".repeat(64);
        let (package, cache, destination) = fixture(
            &hash_root,
            &entries,
            &[("ffxivboot.exe", b"boot"), ("ffxivgame.exe", b"game")],
            Some(&wrong_hash),
        );
        let mut package = package;
        package.archives[0].files[0].sha256 = wrong_hash;
        let error = install(
            &InstallShared::with_totals(1, 1),
            &destination,
            "http://127.0.0.1:9",
            &cache,
            &package,
        )
        .unwrap_err();
        assert!(
            error.contains("Base archive file failed integrity verification"),
            "{error}"
        );
        assert!(!destination.exists());

        let missing_root = temporary.path().join("missing");
        fs::create_dir(&missing_root).unwrap();
        let (mut package, cache, destination) = valid_fixture(&missing_root);
        package.archives[0]
            .files
            .push(file("sqpack/missing.bin", b"missing"));
        package
            .final_files
            .push(file("sqpack/missing.bin", b"missing"));
        package.staging_bytes += b"missing".len() as u64;
        let error = install(
            &InstallShared::with_totals(1, 1),
            &destination,
            "http://127.0.0.1:9",
            &cache,
            &package,
        )
        .unwrap_err();
        assert!(error.contains("missing inventory file"), "{error}");
        assert!(!destination.exists());
    }

    #[test]
    fn invalid_final_identity_never_publishes_a_version_marker() {
        let temporary = tempdir().unwrap();
        let wrong_hash = "0".repeat(64);
        let (package, cache, destination) = valid_fixture(temporary.path());
        let mut package = package;
        package.final_files[0].sha256 = wrong_hash;
        let error = install(
            &InstallShared::with_totals(1, 1),
            &destination,
            "http://127.0.0.1:9",
            &cache,
            &package,
        )
        .unwrap_err();

        assert!(
            error.contains("Final install file failed validation"),
            "{error}"
        );
        assert!(!destination.exists());
        assert!(!destination.join("game.ver").exists());
    }

    #[test]
    fn insufficient_space_fails_before_download_or_staging() {
        let quote = InstallQuote {
            download_bytes: 10,
            staging_bytes: 20,
            destination_bytes: 18,
            available_cache_bytes: Some(5),
            available_destination_bytes: Some(100),
        };
        assert!(
            check_space(&quote, false, quote.download_bytes)
                .unwrap_err()
                .contains("Insufficient free space")
        );
        assert!(
            check_space(&quote, true, quote.download_bytes)
                .unwrap_err()
                .contains("Insufficient free space")
        );
    }

    #[test]
    fn cache_space_preflight_credits_existing_allocations_without_changing_transfer_quote() {
        let temporary = tempdir().unwrap();
        let mut state = 0x7a31_4421_u32;
        let large_payload = (0..128 * 1024)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect::<Vec<_>>();
        let entries = [
            Entry {
                path: "ffxivboot.exe",
                contents: b"boot",
                mode: None,
            },
            Entry {
                path: "ffxivgame.exe",
                contents: b"game",
                mode: None,
            },
            Entry {
                path: "large.dat",
                contents: &large_payload,
                mode: None,
            },
        ];
        let expected = [
            ("ffxivboot.exe", b"boot".as_slice()),
            ("ffxivgame.exe", b"game".as_slice()),
            ("large.dat", large_payload.as_slice()),
        ];
        let (package, original_cache, destination) =
            fixture(temporary.path(), &entries, &expected, None);
        let cache = temporary.path().join("resumable-cache");
        fs::create_dir(&cache).unwrap();
        let object = &package.archives[0].object;
        let identity = format!("{}-{}", object.sha256, object.length);
        let original = original_cache.join(format!("{identity}.object"));
        let bytes = fs::read(original).unwrap();
        assert!(object.length > 64 * 1024);
        let full_requirement = object.length.div_ceil(allocation_unit(&cache).unwrap())
            * allocation_unit(&cache).unwrap();
        assert_eq!(
            cache_space_requirement(&package, &cache).unwrap(),
            full_requirement
        );

        fs::write(
            cache.join(format!("{identity}.part")),
            &bytes[..(bytes.len() / 2)],
        )
        .unwrap();
        let partial_requirement = cache_space_requirement(&package, &cache).unwrap();
        assert!(partial_requirement > 0 && partial_requirement < full_requirement);
        let display_quote = quote(&destination, &cache, &package).unwrap();
        assert_eq!(display_quote.download_bytes, object.length);
        check_space(&display_quote, false, partial_requirement).unwrap();

        fs::write(cache.join(format!("{identity}.object")), &bytes).unwrap();
        let complete_allocation = cache_file_footprint(&cache.join(format!("{identity}.object")))
            .unwrap()
            .unwrap()
            .allocated_bytes;
        assert_eq!(
            cache_space_requirement(&package, &cache).unwrap(),
            full_requirement - complete_allocation.min(full_requirement)
        );
        assert_eq!(
            quote(&destination, &cache, &package)
                .unwrap()
                .download_bytes,
            object.length
        );
    }

    #[test]
    fn corrupt_complete_cache_with_low_allocation_fails_space_preflight() {
        let temporary = tempdir().unwrap();
        let (package, cache, destination) = valid_fixture(temporary.path());
        let object = &package.archives[0].object;
        let path = cache.join(format!("{}-{}.object", object.sha256, object.length));
        fs::remove_file(&path).unwrap();
        File::create(&path).unwrap().set_len(object.length).unwrap();

        let footprint = cache_file_footprint(&path).unwrap().unwrap();
        let unit = allocation_unit(&cache).unwrap();
        let full_allocation = object.length.div_ceil(unit) * unit;
        assert_eq!(footprint.logical_bytes, object.length);
        assert!(footprint.allocated_bytes < full_allocation);
        assert_ne!(
            format!("{:x}", Sha256::digest(fs::read(&path).unwrap())),
            object.sha256
        );

        let required = cache_space_requirement(&package, &cache).unwrap();
        assert_eq!(required, full_allocation - footprint.allocated_bytes);
        let mut display_quote = quote(&destination, &cache, &package).unwrap();
        assert_eq!(display_quote.download_bytes, object.length);
        display_quote.available_cache_bytes = Some(required - 1);
        display_quote.available_destination_bytes = Some(display_quote.staging_bytes);
        assert!(
            check_space(&display_quote, false, required)
                .unwrap_err()
                .contains("Insufficient free space on the download cache")
        );
    }

    #[test]
    fn extraction_and_hashing_bound_length_and_propagate_write_errors() {
        let shared = InstallShared::with_totals(0, 0);
        let mut output = Vec::new();
        let error = copy_and_hash(&shared, &mut Cursor::new(b"x"), &mut output, "short.bin", 2)
            .unwrap_err();
        assert!(error.contains("ended before"));

        output.clear();
        let error = copy_and_hash(
            &shared,
            &mut Cursor::new(b"xyz"),
            &mut output,
            "excess.bin",
            2,
        )
        .unwrap_err();
        assert!(error.contains("exceeded its manifest length"));
        assert_eq!(output, b"xy");

        let error = copy_and_hash(
            &shared,
            &mut Cursor::new(b"ok"),
            &mut FailingWriter,
            "write-error.bin",
            2,
        )
        .unwrap_err();
        assert!(error.contains("simulated write failure"));

        let temporary = tempdir().unwrap();
        let oversized = temporary.path().join("oversized.bin");
        fs::write(&oversized, b"xyz").unwrap();
        let error = hash_file(&shared, &oversized, "oversized.bin", 2).unwrap_err();
        assert!(error.contains("exceeded its manifest length"));
    }

    #[test]
    fn quote_reports_transfer_stage_final_and_available_space() {
        let temporary = tempdir().unwrap();
        let (package, cache, destination) = valid_fixture(temporary.path());
        let quote = quote(&destination, &cache, &package).unwrap();
        assert_eq!(quote.download_bytes, package.download_bytes().unwrap());
        assert_eq!(
            quote.staging_bytes,
            staging_space_bound(&package, allocation_unit(temporary.path()).unwrap()).unwrap()
        );
        assert!(quote.staging_bytes >= package.staging_bytes);
        assert_eq!(
            quote.destination_bytes,
            inventory_bytes(&package.final_files).unwrap()
        );
        let object = &package.archives[0].object;
        let unit = allocation_unit(&cache).unwrap();
        let full_requirement = object.length.div_ceil(unit) * unit;
        let complete = cache.join(format!("{}-{}.object", object.sha256, object.length));
        let complete_allocation = cache_file_footprint(&complete)
            .unwrap()
            .unwrap()
            .allocated_bytes;
        assert_eq!(
            cache_space_requirement(&package, &cache).unwrap(),
            full_requirement - complete_allocation.min(full_requirement)
        );
        assert!(
            quote.available_cache_bytes.is_some() && quote.available_destination_bytes.is_some()
        );
    }

    #[test]
    fn full_client_staging_bound_accounts_for_filesystem_allocation_units() {
        let package = crate::content::manifest::shipped_manifest()
            .unwrap()
            .base
            .unwrap();
        assert_eq!(package.staging_bytes, 14_000_000_000);
        let four_k = staging_space_bound(&package, 4096).unwrap();
        let eight_k = staging_space_bound(&package, 8192).unwrap();
        assert!(four_k > package.staging_bytes);
        assert!(eight_k > four_k);
    }

    #[test]
    fn install_creates_missing_games_parent_after_quote() {
        let temporary = tempdir().unwrap();
        let (package, cache, _) = valid_fixture(temporary.path());
        let destination = temporary.path().join("Games").join("FINAL FANTASY XIV");
        assert!(!destination.parent().unwrap().exists());
        quote(&destination, &cache, &package).unwrap();
        assert!(!destination.parent().unwrap().exists());
        let shared = InstallShared::with_totals(
            package.download_bytes().unwrap(),
            package.final_files.len(),
        );
        run_install(&shared, &destination, &cache, package)
            .join()
            .unwrap();
        assert!(destination.is_dir());
    }

    #[test]
    fn quote_and_install_recover_a_receipt_owned_published_tree() {
        let temporary = tempdir().unwrap();
        let (package, cache, destination) = valid_fixture(temporary.path());
        let shared = InstallShared::with_totals(
            package.download_bytes().unwrap(),
            package.final_files.len(),
        );
        run_install(&shared, &destination, &cache, package.clone())
            .join()
            .unwrap();
        let identity = package_identity(&package);
        write_receipt(&destination, &identity).unwrap();
        let quote = quote(&destination, &cache, &package).unwrap();
        assert_eq!(
            quote.destination_bytes,
            inventory_bytes(&package.final_files).unwrap()
        );
        let second = InstallShared::with_totals(
            package.download_bytes().unwrap(),
            package.final_files.len(),
        );
        run_install(&second, &destination, &cache, package)
            .join()
            .unwrap();
        assert_eq!(second.phase(), Phase::Done, "{:?}", second.error());
        assert!(!destination.join(RECEIPT_NAME).exists());
    }

    #[test]
    fn free_space_uses_additional_cache_bytes_not_the_display_transfer_total() {
        let quote = InstallQuote {
            download_bytes: 100,
            staging_bytes: 20,
            destination_bytes: 18,
            available_cache_bytes: Some(20),
            available_destination_bytes: Some(20),
        };
        assert!(check_space(&quote, false, 20).is_ok());
        assert!(check_space(&quote, false, 21).is_err());
        assert!(check_space(&quote, true, 0).is_ok());
    }
}
