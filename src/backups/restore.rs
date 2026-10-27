//! Staged restore of the latest backup with launcher reconciliation.

use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::atomic_fs::is_link_or_reparse;
use crate::config::extension_config::{
    DatsConfig, ExtensionsConfig, screenshot_settings_from_ini_str,
};
use crate::config::launcher_ini::LauncherConfig;
use crate::config::preferences::GameSettings;
use crate::config::{dirs, retail_game};

use super::catalog::latest_backup;
use super::paths::{
    archive_path_allowed, character_data_files, ensure_no_link_ancestors, extension_unit_is_file,
};
use super::transaction::{apply_staged_units, remove_path, unique_transaction_root};
use super::{
    BackupError, BackupLayout, BackupTarget, EXTENSION_FILE_UNITS, EXTENSION_UNITS,
    USER_CONFIG_FILES, USER_DIRECTORY, io_error,
};

pub(super) fn restore_latest_backup_in(
    target: BackupTarget,
    layout: &BackupLayout,
) -> Result<PathBuf, BackupError> {
    let archive_path = latest_backup(target, &layout.backups_root)?
        .ok_or_else(|| BackupError::NoBackup(target.label()))?;
    let live_root = layout.live_root(target);
    ensure_no_link_ancestors(live_root, live_root)?;
    let transaction_parent = layout.transaction_parent(target);
    fs::create_dir_all(transaction_parent).map_err(|error| {
        io_error(
            "creating the restore transaction parent",
            transaction_parent,
            error,
        )
    })?;
    let transaction_root = unique_transaction_root(transaction_parent);
    let staging_root = transaction_root.join("staging");
    let rollback_root = transaction_root.join("rollback");
    fs::create_dir_all(&staging_root)
        .map_err(|error| io_error("creating restore staging", &staging_root, error))?;

    let restore_result = stage_and_install(
        target,
        layout,
        live_root,
        &archive_path,
        &staging_root,
        &rollback_root,
    );

    match restore_result {
        Ok(()) => Ok(archive_path),
        Err(error) => {
            if !matches!(error, BackupError::RollbackFailed { .. }) {
                let _ = remove_path(&transaction_root);
            }
            Err(error)
        }
    }
}

/// Extract and validate the archive in staging, then swap it into the live
/// root, rolling back if the launcher configuration cannot be reconciled.
fn stage_and_install(
    target: BackupTarget,
    layout: &BackupLayout,
    live_root: &Path,
    archive_path: &Path,
    staging_root: &Path,
    rollback_root: &Path,
) -> Result<(), BackupError> {
    extract_archive(target, archive_path, staging_root)?;
    validate_staged_backup(target, staging_root)?;
    let units = replacement_units(target, staging_root)?;
    preflight_restore_destinations(target, layout, live_root, staging_root, &units)?;

    let launcher_state = if target == BackupTarget::UserSettings {
        prepare_launcher_reconciliation(staging_root)?
    } else {
        None
    };

    let transaction = apply_staged_units(live_root, staging_root, rollback_root, &units)?;
    if let Some(restored_game) = launcher_state
        && let Err(error) = reconcile_launcher_config(layout, restored_game)
    {
        let primary = error.to_string();
        return match transaction.rollback() {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(BackupError::RollbackFailed {
                primary,
                rollback: rollback_root.to_path_buf(),
                rollback_error: rollback_error.to_string(),
            }),
        };
    }
    transaction.commit();
    Ok(())
}

fn extract_archive(
    target: BackupTarget,
    archive_path: &Path,
    staging_root: &Path,
) -> Result<(), BackupError> {
    let file = File::open(archive_path)
        .map_err(|error| io_error("opening a backup archive", archive_path, error))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))?;
    let mut seen = HashSet::new();
    let mut restored_files = 0_usize;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_owned();
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| BackupError::UnsafeEntry(name.clone()))?;
        if !archive_path_allowed(target, &relative) {
            return Err(BackupError::UnsafeEntry(name));
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(BackupError::UnsafeEntry(name));
        }
        if entry.is_dir() {
            return Err(BackupError::UnsafeEntry(name));
        }
        if !seen.insert(relative.clone()) {
            return Err(BackupError::DuplicateEntry(name));
        }
        let destination = staging_root.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| io_error("creating a staged backup parent", parent, error))?;
        }
        let mut output = BufWriter::new(
            File::create(&destination)
                .map_err(|error| io_error("creating a staged backup file", &destination, error))?,
        );
        io::copy(&mut entry, &mut output)
            .map_err(|error| io_error("extracting a staged backup file", &destination, error))?;
        output
            .flush()
            .map_err(|error| io_error("flushing a staged backup file", &destination, error))?;
        restored_files += 1;
    }
    if restored_files == 0 {
        return Err(BackupError::EmptyArchive);
    }
    Ok(())
}

fn validate_staged_backup(target: BackupTarget, staging_root: &Path) -> Result<(), BackupError> {
    if target == BackupTarget::Extensions {
        validate_ini_if_present(&staging_root.join("config/extensions.ini"), |text| {
            ExtensionsConfig::from_ini_str(text).map(|_| ())
        })?;
        validate_ini_if_present(&staging_root.join("config/dats.ini"), |text| {
            DatsConfig::from_ini_str(text).map(|_| ())
        })?;
        validate_ini_if_present(
            &staging_root.join("config/plugins/screenshot/settings.ini"),
            |text| screenshot_settings_from_ini_str(text).map(|_| ()),
        )?;
        let script = staging_root.join("scripts/default.txt");
        if script.is_file() {
            fs::read_to_string(&script).map_err(|error| {
                io_error("validating the extension startup script", &script, error)
            })?;
        }
    }
    Ok(())
}

fn validate_ini_if_present(
    path: &Path,
    parse: impl FnOnce(&str) -> Result<(), crate::config::extension_config::ExtensionConfigError>,
) -> Result<(), BackupError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(io_error(
                "reading staged extension settings metadata",
                path,
                error,
            ));
        }
    };
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(BackupError::UnexpectedPathType(path.to_path_buf()));
    }
    let text = fs::read_to_string(path)
        .map_err(|error| io_error("reading staged extension settings", path, error))?;
    parse(&text)?;
    Ok(())
}

fn replacement_units(
    target: BackupTarget,
    staging_root: &Path,
) -> Result<Vec<PathBuf>, BackupError> {
    let mut units = BTreeSet::new();
    match target {
        BackupTarget::UserSettings => {
            collect_user_units(staging_root, &mut units)?;
        }
        BackupTarget::Extensions => {
            units.extend(
                EXTENSION_UNITS
                    .iter()
                    .map(PathBuf::from)
                    .filter(|relative| staging_root.join(relative).exists()),
            );
        }
    }
    Ok(units.into_iter().collect())
}

fn collect_user_units(root: &Path, units: &mut BTreeSet<PathBuf>) -> Result<(), BackupError> {
    if !root.is_dir() {
        return Ok(());
    }
    for name in USER_CONFIG_FILES {
        if root.join(name).is_file() {
            units.insert(PathBuf::from(name));
        }
    }
    let user_root = root.join(USER_DIRECTORY);
    if user_root.is_dir() {
        units.extend(
            character_data_files(&user_root)?
                .into_iter()
                .map(|(_, relative)| relative),
        );
    }
    Ok(())
}

fn prepare_launcher_reconciliation(
    staging_root: &Path,
) -> Result<Option<GameSettings>, BackupError> {
    let restored_config = staging_root.join(dirs::RETAIL_CONFIG_SYS_FILE);
    if !restored_config.is_file() {
        return Ok(None);
    }
    let restored_game = retail_game::import_from_path(&restored_config)?;
    Ok(Some(restored_game))
}

fn reconcile_launcher_config(
    layout: &BackupLayout,
    restored_game: GameSettings,
) -> Result<(), BackupError> {
    let _write = LauncherConfig::write_lock()?;
    let mut config = match fs::read_to_string(&layout.launcher_config_path) {
        Ok(text) => LauncherConfig::from_ini_str(&text)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => LauncherConfig::defaults(),
        Err(error) => {
            return Err(io_error(
                "reading launcher settings before restore",
                &layout.launcher_config_path,
                error,
            ));
        }
    };
    config.preferences.game = restored_game;
    config.save_to(&layout.launcher_config_path)?;
    Ok(())
}

fn preflight_restore_destinations(
    target: BackupTarget,
    layout: &BackupLayout,
    live_root: &Path,
    staging_root: &Path,
    units: &[PathBuf],
) -> Result<(), BackupError> {
    let mut affected_paths: Vec<&Path> = units.iter().map(PathBuf::as_path).collect();
    if target == BackupTarget::Extensions {
        // Known files inside replaced directories need checks even when absent from the archive.
        affected_paths.extend(
            EXTENSION_FILE_UNITS
                .iter()
                .map(Path::new)
                .filter(|relative| {
                    !units.iter().any(|unit| *relative == unit)
                        && units.iter().any(|unit| relative.starts_with(unit))
                }),
        );
    }
    for relative in affected_paths {
        let destination = live_root.join(relative);
        ensure_no_link_ancestors(live_root, &destination)?;
        if target == BackupTarget::Extensions
            && let Ok(metadata) = fs::symlink_metadata(&destination)
        {
            let expected_file = extension_unit_is_file(relative);
            let type_matches = if expected_file {
                metadata.is_file()
            } else {
                metadata.is_dir()
            };
            if !type_matches {
                return Err(BackupError::UnexpectedPathType(destination));
            }
        }
    }
    if target == BackupTarget::UserSettings
        && staging_root.join(dirs::RETAIL_CONFIG_SYS_FILE).is_file()
    {
        ensure_no_link_ancestors(&layout.launcher_root, &layout.launcher_config_path)?;
    }
    Ok(())
}
