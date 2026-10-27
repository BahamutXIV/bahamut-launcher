use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::path::Path;

use crate::content::http::ObjectSpec;
use crate::content::manifest::BasePackage;
use crate::content::manifest::InstallFile;
use crate::content::require_space;

use super::InstallQuote;
use super::archive::expected_directories;
use super::paths::check_reparse;

pub(super) fn cache_space_requirement(package: &BasePackage, cache: &Path) -> Result<u64, String> {
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
pub(super) struct CacheFileFootprint {
    pub(super) logical_bytes: u64,
    pub(super) allocated_bytes: u64,
}

pub(super) fn cache_file_footprint(path: &Path) -> Result<Option<CacheFileFootprint>, String> {
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

pub(super) fn inventory_bytes(files: &[InstallFile]) -> Result<u64, String> {
    files.iter().try_fold(0_u64, |sum, file| {
        sum.checked_add(file.length)
            .ok_or_else(|| "Install inventory length overflow.".into())
    })
}

pub(super) fn staging_space_requirement(
    package: &BasePackage,
    destination: &Path,
) -> Result<u64, String> {
    let unit = allocation_unit(destination)?;
    staging_space_bound(package, unit)
}

pub(super) fn staging_space_bound(package: &BasePackage, unit: u64) -> Result<u64, String> {
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
pub(super) fn allocation_unit(destination: &Path) -> Result<u64, String> {
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
pub(super) fn allocation_unit(destination: &Path) -> Result<u64, String> {
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

pub(super) fn check_space(
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
