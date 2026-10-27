//! Backup path allow-lists and link-free destination checks.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::atomic_fs::is_link_or_reparse;

use super::{
    BackupError, BackupTarget, EXTENSION_FILE_UNITS, EXTENSION_UNITS, USER_CONFIG_FILES,
    USER_DIRECTORY, io_error,
};

const USER_CHARACTER_FILES: &[&str] = &["game", "mcr0", "ui"];

pub(super) fn read_sorted_dir(path: &Path) -> Result<Vec<fs::DirEntry>, BackupError> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| io_error("reading a backup directory", path, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| io_error("reading a backup directory entry", path, error))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

pub(super) fn archive_path_allowed(target: BackupTarget, relative: &Path) -> bool {
    match target {
        BackupTarget::UserSettings => {
            let mut components = relative.components();
            let Some(Component::Normal(first)) = components.next() else {
                return false;
            };
            if first == USER_DIRECTORY {
                let Some(Component::Normal(character)) = components.next() else {
                    return false;
                };
                let Some(Component::Normal(file)) = components.next() else {
                    return false;
                };
                return components.next().is_none()
                    && is_character_directory_name(character)
                    && is_character_data_file(file);
            }
            components.next().is_none()
                && first
                    .to_str()
                    .is_some_and(|name| USER_CONFIG_FILES.contains(&name))
        }
        BackupTarget::Extensions => {
            if EXTENSION_FILE_UNITS
                .iter()
                .map(Path::new)
                .any(|unit| relative != unit && relative.starts_with(unit))
            {
                return false;
            }
            EXTENSION_FILE_UNITS
                .iter()
                .map(Path::new)
                .any(|unit| relative == unit)
                || EXTENSION_UNITS
                    .iter()
                    .map(Path::new)
                    .filter(|unit| !extension_unit_is_file(unit))
                    .any(|unit| relative != unit && relative.starts_with(unit))
        }
    }
}

pub(super) fn extension_unit_is_file(unit: &Path) -> bool {
    unit.to_str()
        .is_some_and(|unit| EXTENSION_FILE_UNITS.contains(&unit))
}

/// Character data files under `user_root` as (source, path relative to the
/// user-settings root), in directory-name order.
pub(super) fn character_data_files(
    user_root: &Path,
) -> Result<Vec<(PathBuf, PathBuf)>, BackupError> {
    let mut files = Vec::new();
    for character in read_sorted_dir(user_root)? {
        let character_name = character.file_name();
        let metadata = fs::symlink_metadata(character.path()).map_err(|error| {
            io_error("reading character data metadata", &character.path(), error)
        })?;
        if is_link_or_reparse(&metadata)
            || !metadata.is_dir()
            || !is_character_directory_name(&character_name)
        {
            continue;
        }
        for entry in read_sorted_dir(&character.path())? {
            let name = entry.file_name();
            if entry.path().is_file() && is_character_data_file(&name) {
                files.push((
                    entry.path(),
                    PathBuf::from(USER_DIRECTORY)
                        .join(&character_name)
                        .join(name),
                ));
            }
        }
    }
    Ok(files)
}

fn is_character_directory_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.len() == 8 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn is_character_data_file(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        USER_CHARACTER_FILES.contains(&name)
            || name
                .rsplit_once('.')
                .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("cmb"))
    })
}

pub(super) fn ensure_no_link_ancestors(root: &Path, destination: &Path) -> Result<(), BackupError> {
    let relative = destination
        .strip_prefix(root)
        .map_err(|_| BackupError::UnsafeDestination(destination.to_path_buf()))?;
    let mut current = root.to_path_buf();
    check_restore_destination_component(&current)?;
    for component in relative.components() {
        current.push(component.as_os_str());
        check_restore_destination_component(&current)?;
    }
    Ok(())
}

fn check_restore_destination_component(path: &Path) -> Result<(), BackupError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_link_or_reparse(&metadata) => {
            Err(BackupError::UnsafeDestination(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error("checking the restore destination", path, error)),
    }
}
