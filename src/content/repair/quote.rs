//! Verification summaries, download quotes, and space preflight.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use super::archive::RepairSource;
use super::paths::{check_no_reparse_between, find_file, resolve_cache_path};
use super::scan::{hash_regular_file, is_repairable};
use super::types::{FileState, ManagedFileStatus, RepairQuote, VerificationSummary};
use crate::atomic_fs::is_link_or_reparse;
use crate::content::http::ObjectSpec;
use crate::content::manifest::BasePackage;
use crate::content::require_space;
use crate::diagnostics::free_disk_bytes;

pub(super) struct CacheStatus {
    pub(super) valid: bool,
    pub(super) problem: Option<String>,
}

pub(super) fn summary(
    files: &[ManagedFileStatus],
    recovery_required: bool,
    recovery_detail: Option<String>,
    quote: RepairQuote,
) -> VerificationSummary {
    VerificationSummary {
        valid_count: files
            .iter()
            .filter(|file| file.state == FileState::Valid)
            .count(),
        missing_count: files
            .iter()
            .filter(|file| file.state == FileState::Missing)
            .count(),
        corrupt_count: files
            .iter()
            .filter(|file| file.state == FileState::Corrupt)
            .count(),
        unresolved_count: files
            .iter()
            .filter(|file| file.state == FileState::Unresolved)
            .count(),
        files: files
            .iter()
            .filter(|file| file.state != FileState::Valid)
            .cloned()
            .collect(),
        recovery_required,
        recovery_detail,
        quote,
    }
}

pub(super) fn quote_for_statuses(
    game: &Path,
    cache: &Path,
    cache_volume_root: &Path,
    package: &BasePackage,
    sources: &HashMap<String, RepairSource>,
    statuses: &[ManagedFileStatus],
) -> Result<RepairQuote, String> {
    quote_for_statuses_with_cache(game, cache, cache_volume_root, package, sources, statuses)
        .map(|(quote, _)| quote)
}

pub(super) fn quote_for_statuses_with_cache(
    game: &Path,
    cache: &Path,
    cache_volume_root: &Path,
    package: &BasePackage,
    sources: &HashMap<String, RepairSource>,
    statuses: &[ManagedFileStatus],
) -> Result<(RepairQuote, HashSet<usize>), String> {
    let full_archive_bytes = package.download_bytes()?;
    let mut replacement_bytes = 0_u64;
    let mut archive_indices = HashSet::new();
    for status in statuses {
        if !is_repairable(status.state) {
            continue;
        }
        let file = find_file(package, &status.path)?;
        replacement_bytes = replacement_bytes
            .checked_add(file.length)
            .ok_or_else(|| "Repair staging size overflow.".to_string())?;
        if let Some(source) = sources.get(&status.path.to_ascii_lowercase()) {
            archive_indices.insert(source.archive_index);
        }
    }
    let mut download_bytes = 0_u64;
    let mut cache_problem = None;
    let mut cached_archive_indices = HashSet::new();
    for index in archive_indices {
        let object = &package.archives[index].object;
        match cache_object_status(cache, object) {
            Ok(CacheStatus { valid: true, .. }) => {
                cached_archive_indices.insert(index);
            }
            Ok(CacheStatus {
                valid: false,
                problem,
            }) => {
                download_bytes = download_bytes
                    .checked_add(object.length)
                    .ok_or_else(|| "Repair download size overflow.".to_string())?;
                if cache_problem.is_none() {
                    cache_problem = problem;
                }
            }
            Err(error) => {
                download_bytes = download_bytes
                    .checked_add(object.length)
                    .ok_or_else(|| "Repair download size overflow.".to_string())?;
                if cache_problem.is_none() {
                    cache_problem = Some(error);
                }
            }
        }
    }
    Ok((
        RepairQuote {
            full_archive_bytes,
            download_bytes,
            replacement_bytes,
            available_cache_bytes: free_disk_bytes(cache_volume_root),
            available_game_bytes: free_disk_bytes(game),
            cache_problem,
        },
        cached_archive_indices,
    ))
}

pub(super) fn preflight_space(
    quote: &RepairQuote,
    cache: &Path,
    game: &Path,
) -> Result<(), String> {
    if same_volume(cache, game) {
        let available = quote
            .available_cache_bytes
            .zip(quote.available_game_bytes)
            .map(|(cache, game)| cache.min(game))
            .or(quote.available_cache_bytes)
            .or(quote.available_game_bytes);
        let required = quote
            .download_bytes
            .checked_add(quote.replacement_bytes)
            .ok_or_else(|| "Repair space requirement overflow.".to_string())?;
        require_space(available, required, "download and repair staging volume")?;
    } else {
        require_space(
            quote.available_cache_bytes,
            quote.download_bytes,
            "download cache",
        )?;
        require_space(
            quote.available_game_bytes,
            quote.replacement_bytes,
            "game installation",
        )?;
    }
    Ok(())
}

pub(super) fn same_volume(left: &Path, right: &Path) -> bool {
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
        fs::metadata(left)
            .ok()
            .zip(fs::metadata(right).ok())
            .is_some_and(|(left, right)| left.dev() == right.dev())
    }
    #[cfg(not(any(windows, unix)))]
    {
        left.components().next() == right.components().next()
    }
}

pub(super) fn cache_object_status(
    cache: &Path,
    object: &ObjectSpec,
) -> Result<CacheStatus, String> {
    let path = cache.join(format!(
        "{}-{}.object",
        object.sha256.to_ascii_lowercase(),
        object.length
    ));
    let resolved = resolve_cache_path(cache)?;
    check_no_reparse_between(&resolved.nearest_existing, &path)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(CacheStatus {
            valid: false,
            problem: None,
        }),
        Err(error) => Ok(CacheStatus {
            valid: false,
            problem: Some(format!("Could not inspect cached archive: {error}")),
        }),
        Ok(metadata) if is_link_or_reparse(&metadata) || !metadata.is_file() => Ok(CacheStatus {
            valid: false,
            problem: Some("A cached archive path is linked or is not a regular file.".into()),
        }),
        Ok(_) => match hash_regular_file(&path) {
            Ok((length, digest)) => Ok(CacheStatus {
                valid: length == object.length && digest.eq_ignore_ascii_case(&object.sha256),
                problem: None,
            }),
            Err(error) => Ok(CacheStatus {
                valid: false,
                problem: Some(error),
            }),
        },
    }
}
