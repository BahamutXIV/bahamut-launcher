//! Bounded backups for retail user data and the writable extension state under the launcher state root.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

use chrono::Local;

use crate::config::{dirs, retail_game};

mod catalog;
mod create;
mod paths;
mod restore;
#[cfg(test)]
mod tests;
mod transaction;

use create::create_backup_in;
use restore::restore_latest_backup_in;

const USER_DIRECTORY: &str = "user";

const USER_CONFIG_FILES: &[&str] = &["config.lng", "config.pad", "config.rgn", "config.sys"];

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
        let launcher_root = dirs::state_root()?;
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

    /// Restore work shares the live data's volume and never sits above the launcher state root.
    fn transaction_parent(&self, target: BackupTarget) -> &Path {
        match target {
            BackupTarget::UserSettings => self
                .user_settings_root
                .parent()
                .unwrap_or(&self.user_settings_root),
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

fn backup_timestamp() -> String {
    Local::now().format("%Y%m%d-%H%M%S-%3f").to_string()
}
