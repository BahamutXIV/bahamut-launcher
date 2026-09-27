//! Bounded portable backups for retail user data and writable extension state.

use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Local;
use zip::write::SimpleFileOptions;

use crate::config::dirs;
use crate::config::extension_config::{
    DatsConfig, ExtensionsConfig, screenshot_settings_from_ini_str,
};
use crate::config::launcher_ini::LauncherConfig;
use crate::config::preferences::GameSettings;
use crate::config::retail_game;

const BACKUP_KEEP: usize = 5;
const COPY_BUFFER_SIZE: usize = 64 * 1024;
const USER_DIRECTORY: &str = "user";
const USER_CONFIG_FILES: &[&str] = &["config.lng", "config.pad", "config.rgn", "config.sys"];
const USER_CHARACTER_FILES: &[&str] = &["game", "mcr0", "ui"];
const EXTENSION_UNITS: &[&str] = &[
    "addons",
    "config/extensions.ini",
    "config/dats.ini",
    "config/addons",
    "config/plugins",
    "plugins/dats",
    "scripts/default.txt",
];
const EXTENSION_FILE_UNITS: &[&str] = &[
    "config/extensions.ini",
    "config/dats.ini",
    "config/plugins/screenshot/settings.ini",
    "scripts/default.txt",
];

static TRANSACTION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupTarget {
    UserSettings,
    Extensions,
}

impl BackupTarget {
    pub fn parse(value: &str) -> Result<Self, BackupError> {
        match value {
            "user-settings" => Ok(Self::UserSettings),
            "extensions" => Ok(Self::Extensions),
            _ => Err(BackupError::UnknownTarget(value.to_owned())),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::UserSettings => "User Settings and Macros",
            Self::Extensions => "Extensions",
        }
    }

    fn file_prefix(self) -> &'static str {
        match self {
            Self::UserSettings => "UserSettings",
            Self::Extensions => "Extensions",
        }
    }
}

#[derive(Debug, Clone)]
struct BackupLayout {
    backups_root: PathBuf,
    user_settings_root: PathBuf,
    launcher_root: PathBuf,
    launcher_config_path: PathBuf,
}

impl BackupLayout {
    fn resolve(target: BackupTarget) -> Result<Self, BackupError> {
        let launcher_root = dirs::current_exe_dir().map_err(|source| BackupError::Io {
            operation: "resolving the launcher directory",
            path: PathBuf::new(),
            source,
        })?;
        Ok(Self {
            backups_root: dirs::backups_dir()?,
            user_settings_root: if target == BackupTarget::UserSettings {
                dirs::retail_user_dir()?
            } else {
                PathBuf::new()
            },
            launcher_root,
            launcher_config_path: dirs::launcher_config_path()?,
        })
    }

    fn live_root(&self, target: BackupTarget) -> &Path {
        match target {
            BackupTarget::UserSettings => &self.user_settings_root,
            BackupTarget::Extensions => &self.launcher_root,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("unknown backup target {0:?}")]
    UnknownTarget(String),
    #[error("{0} data was not found")]
    NoSource(&'static str),
    #[error("no {0} backup exists yet")]
    NoBackup(&'static str),
    #[error("backup contains an unsafe or unexpected entry: {0}")]
    UnsafeEntry(String),
    #[error("backup contains duplicate entry {0}")]
    DuplicateEntry(String),
    #[error("backup contains no restorable files")]
    EmptyArchive,
    #[error("backup cannot include symbolic link {0}")]
    SymbolicLink(PathBuf),
    #[error("restore destination contains a symbolic link or reparse point: {0}")]
    UnsafeDestination(PathBuf),
    #[error("backup path has an unexpected file type: {0}")]
    UnexpectedPathType(PathBuf),
    #[error("could not encode backup path {0}")]
    InvalidPath(PathBuf),
    #[error("could not resolve backup directories: {0}")]
    Directory(#[from] dirs::ConfigDirError),
    #[error("could not {operation} at {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not read or write backup archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("backup retail settings are invalid: {0}")]
    RetailSettings(#[from] retail_game::RetailGameError),
    #[error("launcher settings could not be reconciled after restore: {0}")]
    LauncherConfig(#[from] crate::config::launcher_ini::LauncherConfigError),
    #[error("extension backup is invalid: {0}")]
    ExtensionConfig(#[from] crate::config::extension_config::ExtensionConfigError),
    #[error("restore failed: {primary}; rollback was retained at {rollback}: {rollback_error}")]
    RollbackFailed {
        primary: String,
        rollback: PathBuf,
        rollback_error: String,
    },
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> BackupError {
    BackupError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

pub fn create_backup(target: BackupTarget) -> Result<PathBuf, BackupError> {
    create_backup_in(target, &BackupLayout::resolve(target)?, &backup_timestamp())
}

pub fn restore_latest_backup(target: BackupTarget) -> Result<PathBuf, BackupError> {
    restore_latest_backup_in(target, &BackupLayout::resolve(target)?)
}

/// Back up the current User Settings files, then remove each regular per-character HUD layout.
///
/// The client recreates these `ui` files with its retail defaults on the next launch. No backup
/// or filesystem mutation occurs when there are no matching layout files.
pub fn reset_hud_layouts() -> Result<usize, BackupError> {
    reset_hud_layouts_in(&BackupLayout::resolve(BackupTarget::UserSettings)?)
}

fn reset_hud_layouts_in(layout: &BackupLayout) -> Result<usize, BackupError> {
    let files = collect_hud_layout_files(layout)?;
    if files.is_empty() {
        return Ok(0);
    }

    create_backup_in(BackupTarget::UserSettings, layout, &backup_timestamp())?;

    let mut removed = 0_usize;
    for path in files {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_error("reading HUD layout metadata", &path, error)),
        };
        if metadata_is_link(&metadata) || !metadata.is_file() {
            continue;
        }
        fs::remove_file(&path).map_err(|error| io_error("removing a HUD layout", &path, error))?;
        removed += 1;
    }
    Ok(removed)
}

fn collect_hud_layout_files(layout: &BackupLayout) -> Result<Vec<PathBuf>, BackupError> {
    let user_root = layout.user_settings_root.join(USER_DIRECTORY);
    let metadata = match fs::symlink_metadata(&user_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(io_error(
                "reading the user settings directory",
                &user_root,
                error,
            ));
        }
    };
    if metadata_is_link(&metadata) || !metadata.is_dir() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for character in read_sorted_dir(&user_root)? {
        let character_name = character.file_name();
        let metadata = fs::symlink_metadata(character.path()).map_err(|error| {
            io_error("reading character data metadata", &character.path(), error)
        })?;
        if metadata_is_link(&metadata)
            || !metadata.is_dir()
            || !is_character_directory_name(&character_name)
        {
            continue;
        }

        let path = character.path().join("ui");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_error("reading HUD layout metadata", &path, error)),
        };
        if !metadata_is_link(&metadata) && metadata.is_file() {
            files.push(path);
        }
    }
    Ok(files)
}

fn create_backup_in(
    target: BackupTarget,
    layout: &BackupLayout,
    timestamp: &str,
) -> Result<PathBuf, BackupError> {
    let files = collect_backup_files(target, layout)?;
    if files.is_empty() {
        return Err(BackupError::NoSource(target.label()));
    }

    ensure_no_link_ancestors(&layout.launcher_root, &layout.backups_root)?;
    fs::create_dir_all(&layout.backups_root)
        .map_err(|error| io_error("creating the backup folder", &layout.backups_root, error))?;
    ensure_no_link_ancestors(&layout.launcher_root, &layout.backups_root)?;
    let (partial_path, file) = create_backup_partial(&layout.backups_root)?;
    let result = write_archive(file, &partial_path, &files)
        .and_then(|()| publish_backup(target, &layout.backups_root, timestamp, &partial_path));
    let archive_path = match result {
        Ok(path) => path,
        Err(error) => {
            // This operation exclusively created the partial file.
            let _ = fs::remove_file(&partial_path);
            return Err(error);
        }
    };
    if let Err(error) = prune_backups(target, &layout.backups_root) {
        tracing::warn!(
            path = %layout.backups_root.display(),
            error = %error,
            "backup completed but old-backup cleanup failed"
        );
    }
    Ok(archive_path)
}

fn restore_latest_backup_in(
    target: BackupTarget,
    layout: &BackupLayout,
) -> Result<PathBuf, BackupError> {
    let archive_path = latest_backup(target, &layout.backups_root)?
        .ok_or_else(|| BackupError::NoBackup(target.label()))?;
    let live_root = layout.live_root(target);
    ensure_no_link_ancestors(live_root, live_root)?;
    let transaction_parent = live_root.parent().unwrap_or(live_root);
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

    let restore_result = (|| {
        extract_archive(target, &archive_path, &staging_root)?;
        validate_staged_backup(target, &staging_root)?;
        let units = replacement_units(target, &staging_root)?;
        preflight_restore_destinations(target, layout, live_root, &staging_root, &units)?;

        let launcher_state = if target == BackupTarget::UserSettings {
            prepare_launcher_reconciliation(&staging_root)?
        } else {
            None
        };

        let transaction = apply_staged_units(live_root, &staging_root, &rollback_root, &units)?;
        if let Some(restored_game) = launcher_state
            && let Err(error) = reconcile_launcher_config(layout, restored_game)
        {
            let primary = error.to_string();
            return match transaction.rollback() {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(BackupError::RollbackFailed {
                    primary,
                    rollback: rollback_root.clone(),
                    rollback_error: rollback_error.to_string(),
                }),
            };
        }
        transaction.commit();
        Ok(())
    })();

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

fn backup_timestamp() -> String {
    Local::now().format("%Y%m%d-%H%M%S-%3f").to_string()
}

fn create_backup_partial(root: &Path) -> Result<(PathBuf, File), BackupError> {
    loop {
        let counter = TRANSACTION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(
            ".bahamut-backup-{}-{counter}.partial",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error("creating a backup", &path, error)),
        }
    }
}

fn publish_backup(
    target: BackupTarget,
    root: &Path,
    timestamp: &str,
    partial: &Path,
) -> Result<PathBuf, BackupError> {
    loop {
        let destination = unique_archive_path(target, root, timestamp)?;
        match crate::atomic_fs::rename_noreplace(partial, &destination) {
            Ok(()) => return Ok(destination),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error("publishing the backup", &destination, error)),
        }
    }
}

fn unique_archive_path(
    target: BackupTarget,
    root: &Path,
    timestamp: &str,
) -> Result<PathBuf, BackupError> {
    let base = format!("{}_{}", target.file_prefix(), timestamp);
    let mut highest = 0_u32;
    for entry in read_sorted_dir(root)? {
        let name = entry.file_name();
        let Some(tail) = name
            .to_str()
            .and_then(|name| name.strip_prefix(&base))
            .and_then(|name| name.strip_suffix(".zip"))
        else {
            continue;
        };
        let sequence = if tail.is_empty() {
            Some(1)
        } else {
            tail.strip_prefix('-')
                .and_then(|value| value.parse::<u32>().ok())
        };
        highest = highest.max(sequence.unwrap_or(0));
    }
    // Do not reuse a lower sequence removed by retention at the same timestamp.
    let mut sequence = highest.checked_add(1).ok_or_else(|| {
        io_error(
            "allocating a backup name",
            root,
            io::Error::other("backup sequence exhausted"),
        )
    })?;
    loop {
        let name = if sequence == 1 {
            format!("{base}.zip")
        } else {
            format!("{base}-{sequence}.zip")
        };
        let candidate = root.join(name);
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(candidate),
            Err(error) => return Err(io_error("checking a backup name", &candidate, error)),
            Ok(_) => {
                // Include dangling links and case aliases on case-insensitive filesystems.
                sequence = sequence.checked_add(1).ok_or_else(|| {
                    io_error(
                        "allocating a backup name",
                        root,
                        io::Error::other("backup sequence exhausted"),
                    )
                })?;
            }
        }
    }
}

fn backup_order(path: &Path) -> (String, u32) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let stem = name.strip_suffix(".zip").unwrap_or(name);
    if let Some((base, suffix)) = stem.rsplit_once('-')
        && let Some((_, timestamp)) = base.split_once('_')
        && timestamp.len() == 19
        && timestamp.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 15) {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
        && let Ok(sequence) = suffix.parse::<u32>()
        && sequence >= 2
    {
        return (base.to_owned(), sequence);
    }
    (stem.to_owned(), 1)
}

fn unique_transaction_root(parent: &Path) -> PathBuf {
    loop {
        let counter = TRANSACTION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".bahamut-restore-{}-{counter}", std::process::id()));
        if !candidate.exists() {
            return candidate;
        }
    }
}

fn collect_backup_files(
    target: BackupTarget,
    layout: &BackupLayout,
) -> Result<Vec<(PathBuf, PathBuf)>, BackupError> {
    let mut roots = Vec::new();
    match target {
        BackupTarget::UserSettings => {
            if !layout.user_settings_root.is_dir() {
                return Err(BackupError::NoSource(target.label()));
            }
            for name in USER_CONFIG_FILES {
                let source = layout.user_settings_root.join(name);
                if source.is_file() {
                    roots.push((source, PathBuf::from(name)));
                }
            }
            let user_root = layout.user_settings_root.join(USER_DIRECTORY);
            if user_root.is_dir() {
                for character in read_sorted_dir(&user_root)? {
                    let character_name = character.file_name();
                    let metadata = fs::symlink_metadata(character.path()).map_err(|error| {
                        io_error("reading character data metadata", &character.path(), error)
                    })?;
                    if metadata_is_link(&metadata)
                        || !metadata.is_dir()
                        || !is_character_directory_name(&character_name)
                    {
                        continue;
                    }
                    for entry in read_sorted_dir(&character.path())? {
                        let name = entry.file_name();
                        if entry.path().is_file() && is_character_data_file(&name) {
                            roots.push((
                                entry.path(),
                                PathBuf::from(USER_DIRECTORY)
                                    .join(&character_name)
                                    .join(name),
                            ));
                        }
                    }
                }
            }
        }
        BackupTarget::Extensions => {
            for relative in EXTENSION_UNITS {
                let relative = PathBuf::from(relative);
                let source = layout.launcher_root.join(&relative);
                if source.exists() {
                    roots.push((source, relative));
                }
            }
        }
    }

    let mut files = Vec::new();
    for (source, relative) in roots {
        collect_path(&source, &relative, &mut files)?;
    }
    files.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(files)
}

fn collect_path(
    source: &Path,
    relative: &Path,
    output: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), BackupError> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| io_error("reading backup source metadata", source, error))?;
    if metadata_is_link(&metadata) {
        return Err(BackupError::SymbolicLink(source.to_path_buf()));
    }
    if metadata.is_file() {
        output.push((source.to_path_buf(), relative.to_path_buf()));
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in read_sorted_dir(source)? {
            collect_path(&entry.path(), &relative.join(entry.file_name()), output)?;
        }
    }
    Ok(())
}

fn read_sorted_dir(path: &Path) -> Result<Vec<fs::DirEntry>, BackupError> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| io_error("reading a backup directory", path, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| io_error("reading a backup directory entry", path, error))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn write_archive(file: File, path: &Path, files: &[(PathBuf, PathBuf)]) -> Result<(), BackupError> {
    let mut archive = zip::ZipWriter::new(BufWriter::new(file));
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o600);
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];

    for (source, relative) in files {
        let name = archive_name(relative)?;
        archive.start_file(name, options)?;
        let mut input = BufReader::new(
            File::open(source)
                .map_err(|error| io_error("opening a backup source file", source, error))?,
        );
        loop {
            let read = input
                .read(&mut buffer)
                .map_err(|error| io_error("reading a backup source file", source, error))?;
            if read == 0 {
                break;
            }
            archive
                .write_all(&buffer[..read])
                .map_err(|error| io_error("writing a backup archive", path, error))?;
        }
    }
    let mut output = archive.finish()?;
    output
        .flush()
        .map_err(|error| io_error("flushing a completed backup archive", path, error))?;
    output
        .get_ref()
        .sync_all()
        .map_err(|error| io_error("synchronizing a completed backup archive", path, error))?;
    Ok(())
}

fn archive_name(path: &Path) -> Result<String, BackupError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => return Err(BackupError::InvalidPath(path.to_path_buf())),
        }
    }
    if parts.is_empty() {
        return Err(BackupError::InvalidPath(path.to_path_buf()));
    }
    Ok(parts.join("/"))
}

fn latest_backup(
    target: BackupTarget,
    backups_root: &Path,
) -> Result<Option<PathBuf>, BackupError> {
    let listing = match fs::read_dir(backups_root) {
        Ok(listing) => listing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(io_error("reading the backup folder", backups_root, error));
        }
    };
    let prefix = format!("{}_", target.file_prefix());
    let mut paths = listing
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".zip"))
        })
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| std::cmp::Reverse(backup_order(path)));
    Ok(paths.into_iter().next())
}

fn prune_backups(target: BackupTarget, backups_root: &Path) -> Result<(), BackupError> {
    let prefix = format!("{}_", target.file_prefix());
    let mut backups = read_sorted_dir(backups_root)?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".zip"))
        })
        .collect::<Vec<_>>();
    backups.sort_by_key(|path| std::cmp::Reverse(backup_order(path)));
    for stale in backups.into_iter().skip(BACKUP_KEEP) {
        fs::remove_file(&stale)
            .map_err(|error| io_error("pruning an old backup", &stale, error))?;
    }
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
            .ok_or_else(|| BackupError::UnsafeEntry(name.clone()))?
            .to_path_buf();
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

fn archive_path_allowed(target: BackupTarget, relative: &Path) -> bool {
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

fn extension_unit_is_file(unit: &Path) -> bool {
    unit.to_str()
        .is_some_and(|unit| EXTENSION_FILE_UNITS.contains(&unit))
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
    if metadata_is_link(&metadata) || !metadata.is_file() {
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
        for character in read_sorted_dir(&user_root)? {
            let character_name = character.file_name();
            let metadata = fs::symlink_metadata(character.path()).map_err(|error| {
                io_error("reading character data metadata", &character.path(), error)
            })?;
            if metadata_is_link(&metadata)
                || !metadata.is_dir()
                || !is_character_directory_name(&character_name)
            {
                continue;
            }
            for entry in read_sorted_dir(&character.path())? {
                let name = entry.file_name();
                if entry.path().is_file() && is_character_data_file(&name) {
                    units.insert(
                        PathBuf::from(USER_DIRECTORY)
                            .join(&character_name)
                            .join(name),
                    );
                }
            }
        }
    }
    Ok(())
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

fn metadata_is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    false
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

fn ensure_no_link_ancestors(root: &Path, destination: &Path) -> Result<(), BackupError> {
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
        Ok(metadata) if metadata_is_link(&metadata) => {
            Err(BackupError::UnsafeDestination(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error("checking the restore destination", path, error)),
    }
}

struct AppliedTransaction {
    live_root: PathBuf,
    transaction_root: PathBuf,
    rollback_root: PathBuf,
    moved_live: Vec<PathBuf>,
    installed: Vec<PathBuf>,
}

impl AppliedTransaction {
    fn rollback(&self) -> Result<(), BackupError> {
        for relative in self.installed.iter().rev() {
            remove_path(&self.live_root.join(relative))?;
        }
        for relative in self.moved_live.iter().rev() {
            let source = self.rollback_root.join(relative);
            let destination = self.live_root.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    io_error("recreating a rollback destination", parent, error)
                })?;
            }
            fs::rename(&source, &destination).map_err(|error| {
                io_error(
                    "restoring original files after restore failure",
                    &destination,
                    error,
                )
            })?;
        }
        if let Err(error) = remove_path(&self.transaction_root) {
            tracing::warn!(
                path = %self.transaction_root.display(),
                error = %error,
                "restore rollback completed but transaction cleanup failed"
            );
        }
        Ok(())
    }

    fn commit(self) {
        if let Err(error) = remove_path(&self.transaction_root) {
            tracing::warn!(
                path = %self.transaction_root.display(),
                error = %error,
                "restore completed but transaction cleanup failed"
            );
        }
    }
}

fn apply_staged_units(
    live_root: &Path,
    staging_root: &Path,
    rollback_root: &Path,
    units: &[PathBuf],
) -> Result<AppliedTransaction, BackupError> {
    fs::create_dir_all(live_root)
        .map_err(|error| io_error("creating the restore destination", live_root, error))?;
    fs::create_dir_all(rollback_root)
        .map_err(|error| io_error("creating restore rollback", rollback_root, error))?;
    let transaction_root = rollback_root
        .parent()
        .expect("rollback always has a transaction parent")
        .to_path_buf();
    let mut transaction = AppliedTransaction {
        live_root: live_root.to_path_buf(),
        transaction_root,
        rollback_root: rollback_root.to_path_buf(),
        moved_live: Vec::new(),
        installed: Vec::new(),
    };

    let result = (|| {
        for relative in units {
            let live = live_root.join(relative);
            if !live.exists() {
                continue;
            }
            let rollback = rollback_root.join(relative);
            if let Some(parent) = rollback.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| io_error("creating a rollback parent", parent, error))?;
            }
            fs::rename(&live, &rollback)
                .map_err(|error| io_error("moving live data into rollback", &live, error))?;
            transaction.moved_live.push(relative.clone());
        }
        for relative in units {
            let staged = staging_root.join(relative);
            if !staged.exists() {
                continue;
            }
            let live = live_root.join(relative);
            if let Some(parent) = live.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    io_error("creating a restore destination parent", parent, error)
                })?;
            }
            fs::rename(&staged, &live)
                .map_err(|error| io_error("installing staged backup data", &live, error))?;
            transaction.installed.push(relative.clone());
        }
        Ok(())
    })();

    if let Err(primary) = result {
        return match transaction.rollback() {
            Ok(()) => Err(primary),
            Err(rollback_error) => Err(BackupError::RollbackFailed {
                primary: primary.to_string(),
                rollback: rollback_root.to_path_buf(),
                rollback_error: rollback_error.to_string(),
            }),
        };
    }
    Ok(transaction)
}

fn remove_path(path: &Path) -> Result<(), BackupError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error("reading restore cleanup metadata", path, error)),
    };
    if metadata.is_dir() && !metadata_is_link(&metadata) {
        fs::remove_dir_all(path)
            .map_err(|error| io_error("removing restore work data", path, error))
    } else {
        fs::remove_file(path).map_err(|error| io_error("removing restore work data", path, error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_LABEL: &str = "fixture";

    fn fixture_layout(temp: &tempfile::TempDir) -> BackupLayout {
        let launcher_root = temp.path().join("launcher");
        BackupLayout {
            backups_root: launcher_root.join("backups"),
            user_settings_root: temp.path().join("documents/FINAL FANTASY XIV"),
            launcher_config_path: launcher_root.join("config/bahamut.ini"),
            launcher_root,
        }
    }

    fn zip_names(path: &Path) -> Vec<String> {
        let file = File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut names = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn write_valid_retail_config(path: &Path, width: u32, height: u32) {
        let settings = GameSettings {
            initialized: true,
            width,
            height,
            ..GameSettings::default()
        };
        retail_game::create_at_path(path, &settings).unwrap();
    }

    #[cfg(windows)]
    fn create_junction(link: &Path, target: &Path) {
        use std::process::Command;

        let output = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("mklink should start");
        assert!(
            output.status.success(),
            "mklink failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn user_backup_includes_only_config_and_character_data() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        fs::create_dir_all(layout.user_settings_root.join("user/00000001")).unwrap();
        fs::create_dir_all(layout.user_settings_root.join("user/00000001/log")).unwrap();
        fs::create_dir_all(layout.user_settings_root.join("screenshots")).unwrap();
        fs::write(layout.user_settings_root.join("config.sys"), b"config").unwrap();
        fs::write(layout.user_settings_root.join("config.pad"), b"pad").unwrap();
        fs::write(
            layout.user_settings_root.join("user/00000001/macros.dat"),
            b"macros",
        )
        .unwrap();
        fs::write(
            layout.user_settings_root.join("user/00000001/game"),
            b"game",
        )
        .unwrap();
        fs::write(
            layout.user_settings_root.join("user/00000001/log/chat.log"),
            b"chat",
        )
        .unwrap();
        fs::write(
            layout.user_settings_root.join("screenshots/private.png"),
            b"image",
        )
        .unwrap();

        let archive_path =
            create_backup_in(BackupTarget::UserSettings, &layout, FIXTURE_LABEL).unwrap();

        assert_eq!(
            zip_names(&archive_path),
            ["config.pad", "config.sys", "user/00000001/game"]
        );
    }

    #[test]
    fn hud_layout_reset_backs_up_then_removes_only_matching_regular_files() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        let valid_character = layout.user_settings_root.join("user/0aBc1234");
        let invalid_character = layout.user_settings_root.join("user/gggggggg");
        let short_character = layout.user_settings_root.join("user/1234567");
        let directory_layout = layout.user_settings_root.join("user/deadbeef/ui");
        fs::create_dir_all(&valid_character).unwrap();
        fs::create_dir_all(&invalid_character).unwrap();
        fs::create_dir_all(&short_character).unwrap();
        fs::create_dir_all(&directory_layout).unwrap();
        fs::write(valid_character.join("ui"), b"custom HUD").unwrap();
        fs::write(valid_character.join("game"), b"game").unwrap();
        fs::write(invalid_character.join("ui"), b"keep").unwrap();
        fs::write(short_character.join("ui"), b"keep").unwrap();

        assert_eq!(reset_hud_layouts_in(&layout).unwrap(), 1);
        assert!(!valid_character.join("ui").exists());
        assert_eq!(fs::read(valid_character.join("game")).unwrap(), b"game");
        assert_eq!(fs::read(invalid_character.join("ui")).unwrap(), b"keep");
        assert_eq!(fs::read(short_character.join("ui")).unwrap(), b"keep");
        assert!(directory_layout.is_dir());

        let archive = latest_backup(BackupTarget::UserSettings, &layout.backups_root)
            .unwrap()
            .expect("reset should leave a recoverable User Settings backup");
        assert_eq!(
            zip_names(&archive),
            ["user/0aBc1234/game", "user/0aBc1234/ui"]
        );
    }

    #[test]
    fn hud_layout_reset_without_matching_files_does_not_mutate() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        let invalid_layout = layout.user_settings_root.join("user/not-a-character");
        fs::create_dir_all(&invalid_layout).unwrap();
        fs::write(invalid_layout.join("ui"), b"keep").unwrap();

        assert_eq!(reset_hud_layouts_in(&layout).unwrap(), 0);
        assert_eq!(fs::read(invalid_layout.join("ui")).unwrap(), b"keep");
        assert!(!layout.backups_root.exists());
    }

    #[test]
    fn extension_backup_excludes_payloads_logs_and_launcher_config() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        for (relative, contents) in [
            ("config/extensions.ini", "[plugins]\n\n[addons]\n"),
            ("config/dats.ini", "[dats]\n"),
            ("config/addons/fps/settings.ini", "show = true\n"),
            (
                "config/plugins/screenshot/settings.ini",
                "[screenshot]\nformat = png\nhide_overlays = true\nhotkey = print_screen\n",
            ),
            ("scripts/default.txt", "/addon load fps\n"),
            ("config/bahamut.ini", "secret = no\n"),
            ("addons/fps/addon.toml", "id = 'fps'\n"),
            ("plugins/dats/world/overlay.toml", "id = 'world'\n"),
            ("plugins/screenshot.dll", "binary\n"),
            ("logs/chat/example.log", "log\n"),
        ] {
            let path = layout.launcher_root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }

        let archive_path =
            create_backup_in(BackupTarget::Extensions, &layout, FIXTURE_LABEL).unwrap();

        assert_eq!(
            zip_names(&archive_path),
            [
                "addons/fps/addon.toml",
                "config/addons/fps/settings.ini",
                "config/dats.ini",
                "config/extensions.ini",
                "config/plugins/screenshot/settings.ini",
                "plugins/dats/world/overlay.toml",
                "scripts/default.txt",
            ]
        );
    }

    #[test]
    fn backup_collision_order_survives_retention_and_double_digit_suffixes() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        fs::create_dir_all(&layout.user_settings_root).unwrap();
        let source = layout.user_settings_root.join("config.pad");
        let timestamp = "20000103-173524-123";
        let mut created = Vec::new();
        for sequence in 1..=12 {
            fs::write(&source, format!("backup {sequence}")).unwrap();
            let path = create_backup_in(BackupTarget::UserSettings, &layout, timestamp).unwrap();
            assert_eq!(
                latest_backup(BackupTarget::UserSettings, &layout.backups_root).unwrap(),
                Some(path.clone())
            );
            created.push(path);
        }
        for (index, path) in created.iter().enumerate() {
            assert_eq!(path.exists(), index >= 12 - BACKUP_KEEP);
        }
        fs::write(&source, b"changed after backup").unwrap();
        restore_latest_backup_in(BackupTarget::UserSettings, &layout).unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"backup 12");
        fs::write(&source, b"next millisecond").unwrap();
        let later =
            create_backup_in(BackupTarget::UserSettings, &layout, "20000103-173524-124").unwrap();
        assert_eq!(
            latest_backup(BackupTarget::UserSettings, &layout.backups_root).unwrap(),
            Some(later)
        );
    }

    #[test]
    fn concurrent_backups_have_distinct_complete_archives() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        fs::create_dir_all(&layout.user_settings_root).unwrap();
        fs::write(layout.user_settings_root.join("config.pad"), b"pad").unwrap();
        let timestamp = "20000103-173524-123";
        let paths = std::thread::scope(|scope| {
            let first =
                scope.spawn(|| create_backup_in(BackupTarget::UserSettings, &layout, timestamp));
            let second =
                scope.spawn(|| create_backup_in(BackupTarget::UserSettings, &layout, timestamp));
            [
                first.join().unwrap().unwrap(),
                second.join().unwrap().unwrap(),
            ]
        });
        assert_ne!(paths[0], paths[1]);
        for path in &paths {
            assert_eq!(zip_names(path), ["config.pad"]);
        }
        assert_eq!(fs::read_dir(&layout.backups_root).unwrap().count(), 2);
    }

    #[test]
    fn backup_does_not_truncate_an_existing_partial_file() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        fs::create_dir_all(&layout.user_settings_root).unwrap();
        fs::write(layout.user_settings_root.join("config.pad"), b"pad").unwrap();
        fs::create_dir_all(&layout.backups_root).unwrap();
        let partial = layout.backups_root.join("UserSettings_fixture.zip.partial");
        fs::write(&partial, b"another operation").unwrap();
        let archive = create_backup_in(BackupTarget::UserSettings, &layout, FIXTURE_LABEL).unwrap();
        assert_eq!(zip_names(&archive), ["config.pad"]);
        assert_eq!(fs::read(partial).unwrap(), b"another operation");
    }

    #[cfg(unix)]
    #[test]
    fn backup_preserves_partial_links_and_rejects_a_linked_output_root() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        fs::create_dir_all(&layout.user_settings_root).unwrap();
        fs::write(layout.user_settings_root.join("config.pad"), b"pad").unwrap();
        fs::create_dir_all(&layout.backups_root).unwrap();
        let sentinel = temp.path().join("sentinel");
        fs::write(&sentinel, b"keep").unwrap();
        let partial = layout.backups_root.join("UserSettings_fixture.zip.partial");
        std::os::unix::fs::symlink(&sentinel, &partial).unwrap();
        create_backup_in(BackupTarget::UserSettings, &layout, FIXTURE_LABEL).unwrap();
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
        assert!(
            fs::symlink_metadata(&partial)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        let other = tempfile::tempdir().unwrap();
        let linked_layout = fixture_layout(&other);
        fs::create_dir_all(&linked_layout.user_settings_root).unwrap();
        fs::write(linked_layout.user_settings_root.join("config.pad"), b"pad").unwrap();
        fs::create_dir_all(&linked_layout.launcher_root).unwrap();
        std::os::unix::fs::symlink(temp.path(), &linked_layout.backups_root).unwrap();
        assert!(
            create_backup_in(BackupTarget::UserSettings, &linked_layout, FIXTURE_LABEL).is_err()
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
    }

    #[test]
    fn backup_retention_keeps_newest_five_per_target() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        fs::create_dir_all(&layout.user_settings_root).unwrap();
        fs::write(layout.user_settings_root.join("config.pad"), b"pad").unwrap();
        for index in 0..7 {
            create_backup_in(
                BackupTarget::UserSettings,
                &layout,
                &format!("fixture-{index}"),
            )
            .unwrap();
        }
        let backups = read_sorted_dir(&layout.backups_root)
            .unwrap()
            .into_iter()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("UserSettings_")
            })
            .count();
        assert_eq!(backups, BACKUP_KEEP);
    }

    #[test]
    fn user_restore_preserves_screenshots_and_reconciles_launcher_settings() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        fs::create_dir_all(layout.user_settings_root.join("user/00000001/log")).unwrap();
        write_valid_retail_config(&layout.user_settings_root.join("config.sys"), 1280, 720);
        fs::write(
            layout.user_settings_root.join("user/00000001/game"),
            b"saved",
        )
        .unwrap();
        fs::write(
            layout.user_settings_root.join("user/00000001/log/chat.log"),
            b"keep",
        )
        .unwrap();
        fs::create_dir_all(layout.user_settings_root.join("screenshots")).unwrap();
        fs::write(
            layout.user_settings_root.join("screenshots/keep.png"),
            b"image",
        )
        .unwrap();
        LauncherConfig::defaults()
            .save_to(&layout.launcher_config_path)
            .unwrap();
        create_backup_in(BackupTarget::UserSettings, &layout, FIXTURE_LABEL).unwrap();

        fs::write(layout.user_settings_root.join("user/00000001/game"), b"new").unwrap();
        fs::write(
            layout.user_settings_root.join("user/00000001/new.cmb"),
            b"new",
        )
        .unwrap();
        remove_path(&layout.user_settings_root.join("config.sys")).unwrap();
        write_valid_retail_config(&layout.user_settings_root.join("config.sys"), 1920, 1080);

        restore_latest_backup_in(BackupTarget::UserSettings, &layout).unwrap();

        assert_eq!(
            fs::read(layout.user_settings_root.join("user/00000001/game")).unwrap(),
            b"saved"
        );
        assert!(
            layout
                .user_settings_root
                .join("user/00000001/new.cmb")
                .exists()
        );
        assert_eq!(
            fs::read(layout.user_settings_root.join("user/00000001/log/chat.log")).unwrap(),
            b"keep"
        );
        assert!(
            layout
                .user_settings_root
                .join("screenshots/keep.png")
                .is_file()
        );
        let config = LauncherConfig::from_ini_str(
            &fs::read_to_string(&layout.launcher_config_path).unwrap(),
        )
        .unwrap();
        assert_eq!(
            (
                config.preferences.game.width,
                config.preferences.game.height
            ),
            (1280, 720)
        );
        assert!(config.preferences.game.initialized);
    }

    #[test]
    fn user_restore_without_config_sys_preserves_launcher_settings() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        fs::create_dir_all(&layout.user_settings_root).unwrap();
        fs::write(layout.user_settings_root.join("config.pad"), b"saved").unwrap();

        let mut launcher_config = LauncherConfig::defaults();
        launcher_config.preferences.game.initialized = true;
        launcher_config.preferences.game.width = 1600;
        launcher_config.preferences.game.height = 900;
        launcher_config
            .save_to(&layout.launcher_config_path)
            .unwrap();
        create_backup_in(BackupTarget::UserSettings, &layout, FIXTURE_LABEL).unwrap();

        fs::write(layout.user_settings_root.join("config.pad"), b"current").unwrap();
        restore_latest_backup_in(BackupTarget::UserSettings, &layout).unwrap();

        assert_eq!(
            fs::read(layout.user_settings_root.join("config.pad")).unwrap(),
            b"saved"
        );
        let restored_launcher = LauncherConfig::from_ini_str(
            &fs::read_to_string(&layout.launcher_config_path).unwrap(),
        )
        .unwrap();
        assert_eq!(
            (
                restored_launcher.preferences.game.width,
                restored_launcher.preferences.game.height
            ),
            (1600, 900)
        );
    }

    #[test]
    fn extension_restore_replaces_owned_units_and_preserves_runtime_files() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        for (relative, contents) in [
            ("addons/fps/addon.toml", "saved addon\n"),
            ("config/extensions.ini", "[plugins]\n\n[addons]\n"),
            ("config/addons/fps/settings.ini", "saved settings\n"),
            ("plugins/dats/world/overlay.toml", "saved dat\n"),
            ("scripts/default.txt", "/addon load fps\n"),
            ("plugins/screenshot.dll", "runtime dll\n"),
            ("logs/chat/example.log", "runtime log\n"),
        ] {
            let path = layout.launcher_root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        create_backup_in(BackupTarget::Extensions, &layout, FIXTURE_LABEL).unwrap();

        fs::write(
            layout.launcher_root.join("addons/fps/addon.toml"),
            "current addon\n",
        )
        .unwrap();
        fs::create_dir_all(layout.launcher_root.join("addons/new")).unwrap();
        fs::write(
            layout.launcher_root.join("addons/new/addon.toml"),
            "not in snapshot\n",
        )
        .unwrap();
        fs::write(
            layout.launcher_root.join("plugins/screenshot.dll"),
            "new runtime dll\n",
        )
        .unwrap();
        fs::write(
            layout.launcher_root.join("logs/chat/example.log"),
            "new runtime log\n",
        )
        .unwrap();

        restore_latest_backup_in(BackupTarget::Extensions, &layout).unwrap();

        assert_eq!(
            fs::read_to_string(layout.launcher_root.join("addons/fps/addon.toml")).unwrap(),
            "saved addon\n"
        );
        assert!(!layout.launcher_root.join("addons/new/addon.toml").exists());
        assert_eq!(
            fs::read_to_string(layout.launcher_root.join("plugins/screenshot.dll")).unwrap(),
            "new runtime dll\n"
        );
        assert_eq!(
            fs::read_to_string(layout.launcher_root.join("logs/chat/example.log")).unwrap(),
            "new runtime log\n"
        );
    }

    #[cfg(windows)]
    #[test]
    fn extension_restore_rejects_junction_before_replacing_live_units() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        let external_root = temp.path().join("external");
        let external_config = external_root.join("config");
        fs::create_dir_all(layout.launcher_root.join("addons/fps")).unwrap();
        fs::create_dir_all(&layout.backups_root).unwrap();
        fs::create_dir_all(&external_config).unwrap();
        fs::write(external_config.join("extensions.ini"), b"external\n").unwrap();
        fs::write(external_config.join("external-only.txt"), b"keep\n").unwrap();
        fs::write(
            layout.launcher_root.join("addons/fps/addon.toml"),
            b"live addon\n",
        )
        .unwrap();
        create_junction(&layout.launcher_root.join("config"), &external_config);

        let archive_path = layout.backups_root.join("Extensions_fixture.zip");
        let file = File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file("addons/fps/addon.toml", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"restored addon\n").unwrap();
        archive
            .start_file("config/extensions.ini", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"[plugins]\n\n[addons]\n").unwrap();
        archive.finish().unwrap();

        let restore_result = restore_latest_backup_in(BackupTarget::Extensions, &layout);
        fs::remove_dir(layout.launcher_root.join("config")).unwrap();

        assert!(matches!(
            restore_result,
            Err(BackupError::UnsafeDestination(_))
        ));
        assert_eq!(
            fs::read(layout.launcher_root.join("addons/fps/addon.toml")).unwrap(),
            b"live addon\n"
        );
        assert_eq!(
            fs::read(external_config.join("extensions.ini")).unwrap(),
            b"external\n"
        );
        assert_eq!(
            fs::read(external_config.join("external-only.txt")).unwrap(),
            b"keep\n"
        );
    }

    #[cfg(windows)]
    #[test]
    fn user_restore_rejects_launcher_config_junction_before_replacing_live_units() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        let external_root = temp.path().join("external");
        let external_config = external_root.join("config");
        fs::create_dir_all(layout.user_settings_root.join("user/00000001")).unwrap();
        fs::create_dir_all(&layout.backups_root).unwrap();
        fs::create_dir_all(&external_config).unwrap();
        create_junction(&layout.launcher_root.join("config"), &external_config);
        fs::write(
            &layout.launcher_config_path,
            LauncherConfig::defaults().to_ini_string(),
        )
        .unwrap();
        fs::write(external_config.join("external-only.txt"), b"keep\n").unwrap();
        let external_launcher_config = fs::read(&layout.launcher_config_path).unwrap();
        write_valid_retail_config(&layout.user_settings_root.join("config.sys"), 1280, 720);
        fs::write(
            layout.user_settings_root.join("user/00000001/game"),
            b"saved\n",
        )
        .unwrap();
        create_backup_in(BackupTarget::UserSettings, &layout, FIXTURE_LABEL).unwrap();
        fs::write(
            layout.user_settings_root.join("user/00000001/game"),
            b"current\n",
        )
        .unwrap();

        let restore_result = restore_latest_backup_in(BackupTarget::UserSettings, &layout);
        fs::remove_dir(layout.launcher_root.join("config")).unwrap();

        assert!(matches!(
            restore_result,
            Err(BackupError::UnsafeDestination(_))
        ));
        assert_eq!(
            fs::read(layout.user_settings_root.join("user/00000001/game")).unwrap(),
            b"current\n"
        );
        assert_eq!(
            fs::read(external_config.join("bahamut.ini")).unwrap(),
            external_launcher_config
        );
        assert_eq!(
            fs::read(external_config.join("external-only.txt")).unwrap(),
            b"keep\n"
        );
    }

    #[test]
    fn invalid_extension_backup_leaves_live_files_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        fs::create_dir_all(layout.launcher_root.join("config")).unwrap();
        fs::create_dir_all(&layout.backups_root).unwrap();
        fs::write(
            layout.launcher_root.join("config/extensions.ini"),
            "[plugins]\n\n[addons]\n",
        )
        .unwrap();
        let archive_path = layout.backups_root.join("Extensions_fixture.zip");
        let file = File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file("config/extensions.ini", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"[unknown]\nvalue = true\n").unwrap();
        archive.finish().unwrap();

        assert!(restore_latest_backup_in(BackupTarget::Extensions, &layout).is_err());
        assert_eq!(
            fs::read_to_string(layout.launcher_root.join("config/extensions.ini")).unwrap(),
            "[plugins]\n\n[addons]\n"
        );
    }

    #[test]
    fn extension_file_unit_cannot_be_replaced_by_a_directory() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        fs::create_dir_all(layout.launcher_root.join("config")).unwrap();
        fs::create_dir_all(&layout.backups_root).unwrap();
        fs::write(
            layout.launcher_root.join("config/extensions.ini"),
            "[plugins]\n\n[addons]\n",
        )
        .unwrap();
        let archive_path = layout.backups_root.join("Extensions_fixture.zip");
        let file = File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file("config/extensions.ini/child", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"unexpected").unwrap();
        archive.finish().unwrap();

        assert!(matches!(
            restore_latest_backup_in(BackupTarget::Extensions, &layout),
            Err(BackupError::UnsafeEntry(_))
        ));
        assert_eq!(
            fs::read_to_string(layout.launcher_root.join("config/extensions.ini")).unwrap(),
            "[plugins]\n\n[addons]\n"
        );
    }

    #[test]
    fn screenshot_settings_file_unit_cannot_be_replaced_by_a_directory() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        let live_settings = layout
            .launcher_root
            .join("config/plugins/screenshot/settings.ini");
        fs::create_dir_all(live_settings.parent().unwrap()).unwrap();
        fs::write(&live_settings, "[screenshot]\nformat = png\n").unwrap();
        fs::create_dir_all(&layout.backups_root).unwrap();
        let archive_path = layout.backups_root.join("Extensions_fixture.zip");
        let file = File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file(
                "config/plugins/screenshot/settings.ini/child",
                SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"unexpected").unwrap();
        archive.finish().unwrap();

        assert!(matches!(
            restore_latest_backup_in(BackupTarget::Extensions, &layout),
            Err(BackupError::UnsafeEntry(_))
        ));
        assert_eq!(
            fs::read_to_string(live_settings).unwrap(),
            "[screenshot]\nformat = png\n"
        );
    }

    #[test]
    fn extension_restore_rejects_live_settings_directory_before_replacing_it() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        let live_settings = layout
            .launcher_root
            .join("config/plugins/screenshot/settings.ini");
        fs::create_dir_all(live_settings.join("nested")).unwrap();
        fs::write(live_settings.join("nested/current.ini"), b"keep\n").unwrap();
        fs::create_dir_all(&layout.backups_root).unwrap();
        let archive_path = layout.backups_root.join("Extensions_fixture.zip");
        let file = File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file(
                "config/plugins/screenshot/settings.ini",
                SimpleFileOptions::default(),
            )
            .unwrap();
        archive
            .write_all(b"[screenshot]\nformat = png\nhide_overlays = true\nhotkey = print_screen\n")
            .unwrap();
        archive.finish().unwrap();

        assert!(matches!(
            restore_latest_backup_in(BackupTarget::Extensions, &layout),
            Err(BackupError::UnexpectedPathType(path)) if path == live_settings
        ));
        assert_eq!(
            fs::read(live_settings.join("nested/current.ini")).unwrap(),
            b"keep\n"
        );
    }

    #[test]
    fn extension_restore_checks_omitted_file_paths_in_replaced_units() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        let live_settings = layout
            .launcher_root
            .join("config/plugins/screenshot/settings.ini");
        fs::create_dir_all(live_settings.join("nested")).unwrap();
        fs::write(live_settings.join("nested/current.ini"), b"keep\n").unwrap();
        let live_addon = layout.launcher_root.join("addons/fixture.txt");
        fs::create_dir_all(live_addon.parent().unwrap()).unwrap();
        fs::write(&live_addon, b"live addon\n").unwrap();
        fs::create_dir_all(&layout.backups_root).unwrap();
        let archive_path = layout.backups_root.join("Extensions_fixture.zip");
        let mut archive = zip::ZipWriter::new(File::create(&archive_path).unwrap());
        for entry in ["addons/fixture.txt", "config/plugins/discord/fixture.txt"] {
            archive
                .start_file(entry, SimpleFileOptions::default())
                .unwrap();
            archive.write_all(b"replacement\n").unwrap();
        }
        archive.finish().unwrap();

        assert!(matches!(
            restore_latest_backup_in(BackupTarget::Extensions, &layout),
            Err(BackupError::UnexpectedPathType(path)) if path == live_settings
        ));
        assert_eq!(fs::read(&live_addon).unwrap(), b"live addon\n");
        assert_eq!(
            fs::read(live_settings.join("nested/current.ini")).unwrap(),
            b"keep\n"
        );

        // An unrelated replacement must not inspect paths it will not replace.
        let mut archive = zip::ZipWriter::new(File::create(&archive_path).unwrap());
        archive
            .start_file("addons/fixture.txt", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"replacement\n").unwrap();
        archive.finish().unwrap();
        restore_latest_backup_in(BackupTarget::Extensions, &layout).unwrap();
        assert_eq!(fs::read(live_addon).unwrap(), b"replacement\n");
        assert_eq!(
            fs::read(live_settings.join("nested/current.ini")).unwrap(),
            b"keep\n"
        );
    }

    #[test]
    fn extension_directory_unit_cannot_be_replaced_by_a_file() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        fs::create_dir_all(layout.launcher_root.join("addons/fps")).unwrap();
        fs::create_dir_all(&layout.backups_root).unwrap();
        fs::write(
            layout.launcher_root.join("addons/fps/addon.toml"),
            "live addon\n",
        )
        .unwrap();
        let archive_path = layout.backups_root.join("Extensions_fixture.zip");
        let file = File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file("addons", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"unexpected").unwrap();
        archive.finish().unwrap();

        assert!(matches!(
            restore_latest_backup_in(BackupTarget::Extensions, &layout),
            Err(BackupError::UnsafeEntry(_))
        ));
        assert_eq!(
            fs::read_to_string(layout.launcher_root.join("addons/fps/addon.toml")).unwrap(),
            "live addon\n"
        );
    }

    #[test]
    fn extension_backup_rejects_explicit_directory_entries() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        fs::create_dir_all(layout.launcher_root.join("addons/fps")).unwrap();
        fs::create_dir_all(&layout.backups_root).unwrap();
        fs::write(
            layout.launcher_root.join("addons/fps/addon.toml"),
            "live addon\n",
        )
        .unwrap();
        let archive_path = layout.backups_root.join("Extensions_fixture.zip");
        let file = File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .add_directory("addons/empty/", SimpleFileOptions::default())
            .unwrap();
        archive.finish().unwrap();

        assert!(matches!(
            restore_latest_backup_in(BackupTarget::Extensions, &layout),
            Err(BackupError::UnsafeEntry(_))
        ));
        assert_eq!(
            fs::read_to_string(layout.launcher_root.join("addons/fps/addon.toml")).unwrap(),
            "live addon\n"
        );
    }

    #[test]
    fn traversal_entry_is_rejected_before_live_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let layout = fixture_layout(&temp);
        fs::create_dir_all(layout.user_settings_root.join("user")).unwrap();
        fs::write(layout.user_settings_root.join("user/live.dat"), b"live").unwrap();
        fs::create_dir_all(&layout.backups_root).unwrap();
        let archive_path = layout.backups_root.join("UserSettings_fixture.zip");
        let file = File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file("../outside.txt", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"unsafe").unwrap();
        archive.finish().unwrap();

        assert!(matches!(
            restore_latest_backup_in(BackupTarget::UserSettings, &layout),
            Err(BackupError::UnsafeEntry(_))
        ));
        assert_eq!(
            fs::read(layout.user_settings_root.join("user/live.dat")).unwrap(),
            b"live"
        );
        assert!(!temp.path().join("outside.txt").exists());
    }
}
