//! Plain-directory, regular-file, and atomic publication helpers.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::Ordering;

use bahamut_launcher::atomic_fs::{is_link_or_reparse, rename_replace};
use bahamut_launcher::digest::sha256_reader;
use serde::Serialize;

use super::UNIQUE_COUNTER;
use super::fetch::MAX_METADATA_BYTES;

pub(super) fn write_json_new<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    write_new_file(
        path,
        &serde_json::to_vec(value).map_err(|error| error.to_string())?,
    )
}

pub(super) fn write_json_replace<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    write_replace_file(
        path,
        &serde_json::to_vec(value).map_err(|error| error.to_string())?,
    )
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Launcher state path has no parent.".to_owned())?;
    let temp = unique_sibling(parent, ".launcher-update-state-");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| format!("Could not stage launcher update state: {error}"))?;
    if let Err(error) = output.write_all(bytes).and_then(|()| output.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(format!("Could not write launcher update state: {error}"));
    }
    drop(output);
    match move_new_file(&temp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            if error.kind() == io::ErrorKind::AlreadyExists {
                Err("Launcher update state already exists.".into())
            } else {
                Err(format!("Could not publish launcher update state: {error}"))
            }
        }
    }
}

fn write_replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Launcher state path has no parent.".to_owned())?;
    let temp = unique_sibling(parent, ".launcher-update-state-");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| format!("Could not stage launcher update state: {error}"))?;
    if let Err(error) = output.write_all(bytes).and_then(|()| output.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(format!("Could not write launcher update state: {error}"));
    }
    drop(output);
    if let Err(error) = rename_replace(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(format!("Could not publish launcher update state: {error}"));
    }
    Ok(())
}

#[cfg(not(windows))]
fn move_new_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::hard_link(source, destination)?;
    fs::remove_file(source)
}

#[cfg(windows)]
fn move_new_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

    let source = source
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            windows::core::PCWSTR(source.as_ptr()),
            windows::core::PCWSTR(destination.as_ptr()),
            MOVEFILE_WRITE_THROUGH,
        )
        .map_err(io::Error::other)
    }
}

pub(super) fn publish_immutable(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if regular_file_exists(path, "launcher release receipt")? {
        let existing = read_regular_file(path, MAX_METADATA_BYTES, "launcher release receipt")
            .map_err(|error| format!("Could not read existing launcher receipt: {error}"))?;
        return if existing == bytes {
            Ok(())
        } else {
            Err("An existing launcher release receipt has conflicting bytes.".into())
        };
    }
    write_new_file(path, bytes)
}

fn unique_sibling(parent: &Path, prefix: &str) -> PathBuf {
    let serial = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!("{prefix}{}-{serial}", std::process::id()))
}

pub(super) fn remove_regular_file_if_present(path: &Path) -> Result<(), String> {
    if regular_file_exists(path, "launcher update state")? {
        fs::remove_file(path)
            .map_err(|error| format!("Could not remove launcher update state: {error}"))?;
    }
    Ok(())
}

pub(super) fn remove_empty_directory(path: &Path) {
    if let Ok(details) = fs::symlink_metadata(path)
        && details.is_dir()
        && !is_link_or_reparse(&details)
    {
        let _ = fs::remove_dir(path);
    }
}

pub(super) fn regular_file_exists(path: &Path, label: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(details) if details.is_file() && !is_link_or_reparse(&details) => Ok(true),
        Ok(_) => Err(format!("{label} is not a regular file.")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Could not inspect {label}: {error}")),
    }
}

pub(super) fn read_regular_file(path: &Path, maximum: usize, label: &str) -> io::Result<Vec<u8>> {
    let details = fs::symlink_metadata(path)?;
    if !details.is_file() || is_link_or_reparse(&details) || details.len() > maximum as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{label} is not a bounded regular file"),
        ));
    }
    let mut bytes = Vec::with_capacity(details.len() as usize);
    File::open(path)?
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{label} exceeds its size limit"),
        ));
    }
    Ok(bytes)
}

pub(super) fn resolve_local_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(
            "Configured launcher local paths must be relative to the launcher directory.".into(),
        );
    }
    let resolved = root.join(path);
    if let Some(parent) = resolved.parent() {
        ensure_path_has_no_reparse_ancestors(parent)?;
    }
    Ok(resolved)
}

pub(super) fn ensure_plain_relative_directory(root: &Path, relative: &Path) -> Result<(), String> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("Launcher directory path is unsafe.".into());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(details) if details.is_dir() && !is_link_or_reparse(&details) => {}
            Ok(_) => {
                return Err(format!(
                    "Launcher directory is not a plain directory: {}",
                    current.display()
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&current)
                .map_err(|error| {
                    format!(
                        "Could not create launcher directory {}: {error}",
                        current.display()
                    )
                })?,
            Err(error) => {
                return Err(format!(
                    "Could not inspect launcher directory {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn ensure_existing_directory(path: &Path, label: &str) -> Result<(), String> {
    ensure_path_has_no_reparse_ancestors(path)?;
    let details = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect {label}: {error}"))?;
    if !details.is_dir() || is_link_or_reparse(&details) {
        return Err(format!("{label} is not a plain directory."));
    }
    Ok(())
}

fn ensure_path_has_no_reparse_ancestors(path: &Path) -> Result<(), String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::Normal(name) => {
                current.push(name);
                let details = fs::symlink_metadata(&current).map_err(|error| {
                    format!(
                        "Could not inspect launcher path {}: {error}",
                        current.display()
                    )
                })?;
                if !details.is_dir() || is_link_or_reparse(&details) {
                    return Err(format!(
                        "Launcher path contains a reparse point or non-directory parent: {}",
                        current.display()
                    ));
                }
            }
            Component::CurDir | Component::ParentDir => {
                return Err("Launcher path contains an unsafe component.".into());
            }
        }
    }
    Ok(())
}

pub(super) fn checked_root(root: &Path) -> Result<PathBuf, String> {
    ensure_existing_directory(root, "launcher install directory")?;
    fs::canonicalize(root)
        .map_err(|error| format!("Could not resolve launcher install directory: {error}"))
}

pub(super) fn safe_join(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("Signed launcher inventory contains an unsafe path.".into());
    }
    Ok(root.join(relative))
}

pub(super) fn normalize_path(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path)
        .or_else(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Ok(path.to_path_buf())
            } else {
                Err(error)
            }
        })
        .map_err(|error| format!("Could not resolve launcher transaction path: {error}"))
}

pub(super) fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let mut file =
        File::open(path).map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    sha256_reader(&mut file).map_err(|error| format!("Could not hash {}: {error}", path.display()))
}
