//! Path resolution and reparse-point safety for the game, cache, and transaction directories.

use std::fs;
use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};

use crate::atomic_fs::{is_link_or_reparse, sync_directory};
use crate::content::manifest::{BasePackage, InstallFile};

const TRANSACTION_SUFFIX: &str = ".bahamut-repair";

pub(super) fn open_regular_for_identity(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    set_no_follow(&mut options);
    let file = options
        .open(path)
        .map_err(|error| format!("Could not inspect repair file {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Could not inspect repair file {}: {error}", path.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "Refusing a non-regular repair file: {}",
            path.display()
        ));
    }
    Ok(file)
}

pub(super) struct ResolvedCachePath {
    pub(super) path: PathBuf,
    pub(super) nearest_existing: PathBuf,
}

pub(super) fn resolve_cache_path(path: &Path) -> Result<ResolvedCachePath, String> {
    if !path.is_absolute() || has_parent_alias(path) {
        return Err(format!(
            "Expected a clean absolute cache path: {}",
            path.display()
        ));
    }
    let mut probe = path.to_path_buf();
    let mut missing = Vec::new();
    let deepest = loop {
        match fs::symlink_metadata(&probe) {
            Ok(metadata) => {
                if is_link_or_reparse(&metadata) {
                    return Err(format!(
                        "Refusing a cache reparse point: {}",
                        probe.display()
                    ));
                }
                if !metadata.is_dir() {
                    return Err(format!(
                        "Cache ancestor is not a directory: {}",
                        probe.display()
                    ));
                }
                break probe;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let leaf = probe
                    .file_name()
                    .ok_or_else(|| "Could not resolve the cache path.".to_string())?;
                missing.push(leaf.to_os_string());
                if !probe.pop() {
                    return Err("Could not resolve the cache path.".into());
                }
            }
            Err(error) => {
                return Err(format!(
                    "Could not inspect cache path {}: {error}",
                    probe.display()
                ));
            }
        }
    };
    inspect_directory_ancestors(&deepest)?;
    let nearest_existing = fs::canonicalize(&deepest)
        .map_err(|error| format!("Could not resolve cache directory: {error}"))?;
    let mut resolved = nearest_existing.clone();
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(ResolvedCachePath {
        path: resolved,
        nearest_existing,
    })
}

pub(super) fn resolve_existing_directory(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || has_parent_alias(path) {
        return Err(format!(
            "Expected a clean absolute game directory: {}",
            path.display()
        ));
    }
    inspect_directory_ancestors(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "Could not inspect game directory {}: {error}",
            path.display()
        )
    })?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err("The selected game path is not a safe directory.".into());
    }
    fs::canonicalize(path).map_err(|error| format!("Could not resolve game directory: {error}"))
}

pub(super) fn inspect_directory_ancestors(path: &Path) -> Result<(), String> {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let metadata = fs::symlink_metadata(&prefix).map_err(|error| {
            format!("Could not inspect directory {}: {error}", prefix.display())
        })?;
        if is_link_or_reparse(&metadata) {
            return Err(format!(
                "Refusing a directory reparse point: {}",
                prefix.display()
            ));
        }
        if !metadata.is_dir() {
            return Err(format!(
                "A path ancestor is not a directory: {}",
                prefix.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn check_no_reparse_between(root: &Path, path: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "Cache object escaped the cache directory.".to_string())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("Invalid cache object path.".into());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_link_or_reparse(&metadata) => {
                return Err(format!(
                    "Refusing a cache reparse point: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("Could not inspect cache object: {error}")),
        }
    }
    Ok(())
}

pub(super) fn transaction_path(game: &Path) -> Result<PathBuf, String> {
    let parent = game
        .parent()
        .ok_or_else(|| "The game directory has no parent for repair recovery.".to_string())?;
    let leaf = game
        .file_name()
        .ok_or_else(|| "The game directory has no final path component.".to_string())?;
    let mut recovery_leaf = leaf.to_os_string();
    recovery_leaf.push(TRANSACTION_SUFFIX);
    Ok(parent.join(recovery_leaf))
}

pub(super) fn path_exists_safely(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Could not inspect repair recovery file: {error}")),
        Ok(metadata) if is_link_or_reparse(&metadata) || !metadata.is_file() => Err(format!(
            "Repair recovery copy is unsafe: {}",
            path.display()
        )),
        Ok(_) => Ok(true),
    }
}

pub(super) fn ensure_live_parent(game: &Path, path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "A managed file has no parent directory.".to_string())?;
    let relative = parent
        .strip_prefix(game)
        .map_err(|_| "A managed file parent escaped the game directory.".to_string())?;
    let mut current = game.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("Invalid managed parent path.".into());
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            format!(
                "Could not inspect managed parent {}: {error}",
                current.display()
            )
        })?;
        if is_link_or_reparse(&metadata) || !metadata.is_dir() {
            return Err(format!(
                "Unsafe managed parent directory: {}",
                current.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn find_file<'a>(
    package: &'a BasePackage,
    path: &str,
) -> Result<&'a InstallFile, String> {
    package
        .final_files
        .iter()
        .find(|file| file.path.eq_ignore_ascii_case(path))
        .ok_or_else(|| format!("Path is not in the managed game catalog: {path}"))
}

pub(super) fn join_relative(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |mut path, part| {
            path.push(part);
            path
        })
}

pub(super) fn parent_paths(path: &str) -> Vec<String> {
    path.match_indices('/')
        .map(|(index, _)| path[..index].to_owned())
        .collect()
}

pub(super) fn has_parent_alias(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
}

pub(super) fn set_no_follow(options: &mut OpenOptions) {
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
}

pub(super) fn validate_regular_handle(file: &File, path: &Path) -> Result<(), String> {
    let metadata = file
        .metadata()
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(format!("Refusing a non-regular file: {}", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(format!(
                "Refusing a hard-linked managed file: {}",
                path.display()
            ));
        }
    }
    #[cfg(windows)]
    {
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
        .map_err(|error| format!("Could not inspect {} links: {error}", path.display()))?;
        if info.NumberOfLinks != 1 {
            return Err(format!(
                "Refusing a hard-linked managed file: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn sync_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = sync_directory(parent);
    }
}
