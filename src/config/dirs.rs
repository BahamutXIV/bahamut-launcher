//! Launcher-owned configuration, WebView state, and regenerable patch data stay beside the executable.

use std::io;
use std::path::{Path, PathBuf};

#[cfg(not(target_os = "windows"))]
use directories::ProjectDirs;

#[cfg(not(target_os = "windows"))]
pub(crate) const QUALIFIER: &str = "com";

#[cfg(not(target_os = "windows"))]
pub(crate) const ORGANIZATION: &str = "BahamutXIV";

#[cfg(not(target_os = "windows"))]
pub(crate) const APPLICATION: &str = "Launcher";

pub const LAUNCHER_CONFIG_FILE: &str = "bahamut.ini";

pub const EXTENSIONS_CONFIG_FILE: &str = "extensions.ini";

pub const DATS_CONFIG_FILE: &str = "dats.ini";

pub const PORTABLE_CONFIG_DIR_NAME: &str = "config";

pub const PORTABLE_CACHE_DIR_NAME: &str = "cache";

pub const PORTABLE_DATA_DIR_NAME: &str = "data";

pub const PORTABLE_BACKUPS_DIR_NAME: &str = "backups";

pub const RETAIL_CONFIG_SYS_FILE: &str = "config.sys";

const RETAIL_GAME_DOCUMENTS_DIR: &str = "FINAL FANTASY XIV";

const LAUNCHER_LOGS_DIR_NAME: &str = "logs";

const LAUNCHER_LOGS_SCOPE_NAME: &str = "launcher";

const LAUNCHER_LOG_FILE_NAME: &str = "bahamut-launcher.log";

const DEFAULT_PATCH_STORAGE_DIR_NAME: &str = "XIVLegacy_Patches";

#[derive(Debug, thiserror::Error)]
pub enum ConfigDirError {
    #[error("could not resolve the running executable's directory: {0}")]
    NoExeDir(#[from] io::Error),

    #[error("no platform-appropriate per-user directory is available for this user")]
    NotAvailable,
}

/// Resolve the directory containing the running launcher executable.
pub fn current_exe_dir() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other("current_exe has no parent directory"))
}

/// Resolve the portable `<exe-dir>/config/` directory used by profiles and news.
pub fn current_exe_config_dir() -> io::Result<PathBuf> {
    let parent = current_exe_dir()?;
    Ok(parent.join(PORTABLE_CONFIG_DIR_NAME))
}

pub fn portable_config_dir() -> Result<PathBuf, ConfigDirError> {
    Ok(current_exe_config_dir()?)
}

/// Resolve the canonical portable launcher configuration path.
pub fn launcher_config_path() -> Result<PathBuf, ConfigDirError> {
    portable_config_dir().map(|dir| dir.join(LAUNCHER_CONFIG_FILE))
}

pub fn extensions_config_path() -> Result<PathBuf, ConfigDirError> {
    portable_config_dir().map(|dir| dir.join(EXTENSIONS_CONFIG_FILE))
}

pub fn dats_config_path() -> Result<PathBuf, ConfigDirError> {
    portable_config_dir().map(|dir| dir.join(DATS_CONFIG_FILE))
}

pub fn screenshot_settings_path() -> Result<PathBuf, ConfigDirError> {
    portable_config_dir().map(|dir| dir.join("plugins/screenshot/settings.ini"))
}

/// Resolve the portable folder containing bounded user-created backup archives.
pub fn backups_dir() -> Result<PathBuf, ConfigDirError> {
    Ok(current_exe_dir()?.join(PORTABLE_BACKUPS_DIR_NAME))
}

/// Resolve the retail client's per-user `config.sys` under Documents.
///
/// The retail configuration utility owns `Documents/My Games/FINAL FANTASY XIV`,
/// independently of the selected client installation directory.
pub fn retail_config_sys_path() -> Result<PathBuf, ConfigDirError> {
    let documents = directories::UserDirs::new()
        .and_then(|dirs| dirs.document_dir().map(Path::to_path_buf))
        .ok_or(ConfigDirError::NotAvailable)?;
    Ok(retail_config_sys_path_under(&documents))
}

/// Resolve the retail client's per-user settings and character-data directory.
pub fn retail_user_dir() -> Result<PathBuf, ConfigDirError> {
    retail_config_sys_path()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or(ConfigDirError::NotAvailable)
}

fn retail_config_sys_path_under(documents: &Path) -> PathBuf {
    documents
        .join("My Games")
        .join(RETAIL_GAME_DOCUMENTS_DIR)
        .join(RETAIL_CONFIG_SYS_FILE)
}

/// Resolve launcher data for the managed Wine prefix and runtime.
///
/// | Platform | Location |
/// |----------|----------|
/// | Windows  | `<exe-dir>\data` |
/// | macOS    | `~/Library/Application Support/com.BahamutXIV.Launcher` |
/// | Linux    | `${XDG_DATA_HOME:-~/.local/share}/launcher` |
#[cfg(target_os = "windows")]
pub fn data_dir() -> Result<PathBuf, ConfigDirError> {
    Ok(current_exe_dir()?.join(PORTABLE_DATA_DIR_NAME))
}

#[cfg(not(target_os = "windows"))]
pub fn data_dir() -> Result<PathBuf, ConfigDirError> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .map(|dirs| dirs.data_dir().to_path_buf())
        .ok_or(ConfigDirError::NotAvailable)
}

/// Resolve the portable native launcher transcript path.
pub fn launcher_log_path() -> Result<PathBuf, ConfigDirError> {
    Ok(current_exe_dir()?
        .join(LAUNCHER_LOGS_DIR_NAME)
        .join(LAUNCHER_LOGS_SCOPE_NAME)
        .join(LAUNCHER_LOG_FILE_NAME))
}

/// Resolve the portable cache for patch scratch files.
pub(crate) fn cache_dir() -> Result<PathBuf, ConfigDirError> {
    Ok(current_exe_dir()?.join(PORTABLE_CACHE_DIR_NAME))
}

/// Resolve the portable WebView profile used for local storage and browser caches.
pub fn webview_data_dir() -> Result<PathBuf, ConfigDirError> {
    Ok(current_exe_dir()?.join(PORTABLE_DATA_DIR_NAME))
}

/// Resolve the long-lived `XIVLegacy_Patches/` download cache from Documents, falling back to [`data_dir`].
pub fn default_patch_storage_dir() -> Result<PathBuf, ConfigDirError> {
    match directories::UserDirs::new().and_then(|u| u.document_dir().map(|d| d.to_path_buf())) {
        Some(documents) => Ok(default_patch_storage_dir_under(&documents)),
        None => data_dir().map(|dir| dir.join(DEFAULT_PATCH_STORAGE_DIR_NAME)),
    }
}

fn default_patch_storage_dir_under(documents: &Path) -> PathBuf {
    documents.join(DEFAULT_PATCH_STORAGE_DIR_NAME)
}

/// Resolve the cache path for temporary local patch extraction.
pub fn patch_staging_dir() -> Result<PathBuf, ConfigDirError> {
    cache_dir().map(|dir| dir.join("patch-staging"))
}

/// Write `default` only when `dir/file_name` is absent, creating `dir` first; return whether it was written.
pub(crate) fn bootstrap_file_if_missing(
    dir: &Path,
    file_name: &str,
    default: &str,
) -> io::Result<bool> {
    let path = dir.join(file_name);
    if path.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, default)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_config_dir_lives_next_to_the_binary() {
        let dir = portable_config_dir().expect("test host has a current_exe");
        assert_eq!(
            dir.file_name().and_then(|n| n.to_str()),
            Some(PORTABLE_CONFIG_DIR_NAME),
            "portable config dir should be the {PORTABLE_CONFIG_DIR_NAME:?} folder \
             next to the binary; got {dir:?}",
        );
        let exe = std::env::current_exe().unwrap();
        assert_eq!(dir.parent(), exe.parent());
    }

    #[test]
    fn cache_dir_is_portable() {
        let cache = cache_dir().expect("resolvable");
        assert_eq!(cache.parent(), current_exe_dir().ok().as_deref());
        assert_eq!(
            cache.file_name().and_then(|name| name.to_str()),
            Some(PORTABLE_CACHE_DIR_NAME)
        );
    }

    #[test]
    fn webview_data_dir_is_portable() {
        assert_eq!(
            webview_data_dir().unwrap(),
            current_exe_dir().unwrap().join(PORTABLE_DATA_DIR_NAME)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn data_dir_is_portable_on_windows() {
        assert_eq!(
            data_dir().unwrap(),
            current_exe_dir().unwrap().join(PORTABLE_DATA_DIR_NAME)
        );
    }

    #[test]
    fn patch_storage_uses_a_named_documents_folder() {
        assert_eq!(
            default_patch_storage_dir_under(Path::new("Documents")),
            Path::new("Documents").join(DEFAULT_PATCH_STORAGE_DIR_NAME)
        );
    }

    #[test]
    fn launcher_config_path_ends_with_ini_file_under_portable_dir() {
        let path = launcher_config_path().expect("resolvable");
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some(LAUNCHER_CONFIG_FILE),
        );
        assert_eq!(
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str()),
            Some(PORTABLE_CONFIG_DIR_NAME),
        );
    }

    #[test]
    fn extension_configuration_paths_are_portable() {
        let config = portable_config_dir().unwrap();
        assert_eq!(
            extensions_config_path().unwrap(),
            config.join("extensions.ini")
        );
        assert_eq!(dats_config_path().unwrap(), config.join("dats.ini"));
        assert_eq!(
            screenshot_settings_path().unwrap(),
            config.join("plugins/screenshot/settings.ini")
        );
    }

    #[test]
    fn backups_dir_is_portable() {
        assert_eq!(
            backups_dir().unwrap(),
            current_exe_dir().unwrap().join(PORTABLE_BACKUPS_DIR_NAME)
        );
    }

    #[test]
    fn retail_config_path_is_under_documents_not_the_install() {
        assert_eq!(
            retail_config_sys_path_under(Path::new("Documents")),
            Path::new("Documents")
                .join("My Games")
                .join(RETAIL_GAME_DOCUMENTS_DIR)
                .join(RETAIL_CONFIG_SYS_FILE)
        );
    }

    #[test]
    fn launcher_log_path_is_fixed_under_portable_root() {
        let root = current_exe_dir().expect("resolvable");
        let log = launcher_log_path().expect("resolvable");
        assert_eq!(
            log,
            root.join(LAUNCHER_LOGS_DIR_NAME)
                .join(LAUNCHER_LOGS_SCOPE_NAME)
                .join(LAUNCHER_LOG_FILE_NAME)
        );
    }
}
