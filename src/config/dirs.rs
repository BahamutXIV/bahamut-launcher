//! Launcher roots: shipped payload is read from the install root, and everything the launcher writes lives under the state root.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

pub const LAUNCHER_CONFIG_FILE: &str = "bahamut.ini";

pub const EXTENSIONS_CONFIG_FILE: &str = "extensions.ini";

pub const DATS_CONFIG_FILE: &str = "dats.ini";

pub const PORTABLE_CONFIG_DIR_NAME: &str = "config";

pub const PORTABLE_CACHE_DIR_NAME: &str = "cache";

pub const PORTABLE_DATA_DIR_NAME: &str = "data";

pub const PORTABLE_BACKUPS_DIR_NAME: &str = "backups";

pub const RETAIL_CONFIG_SYS_FILE: &str = "config.sys";

/// Absolute path that replaces `~/.bahamut-launcher` as the data directory off Windows.
#[cfg(not(target_os = "windows"))]
pub const LAUNCHER_HOME_ENVIRONMENT: &str = "BAHAMUT_LAUNCHER_HOME";

#[cfg(not(target_os = "windows"))]
const LAUNCHER_HOME_DIR_NAME: &str = ".bahamut-launcher";

const RETAIL_GAME_DOCUMENTS_DIR: &str = "FINAL FANTASY XIV";

const LAUNCHER_LOGS_DIR_NAME: &str = "logs";

const LAUNCHER_LOGS_SCOPE_NAME: &str = "launcher";

const LAUNCHER_LOG_FILE_NAME: &str = "bahamut-launcher.log";

const DEFAULT_PATCH_STORAGE_DIR_NAME: &str = "XIVLegacy_Patches";

const BUNDLE_EXTENSION: &str = "app";

const BUNDLE_CONTENTS_DIR_NAME: &str = "Contents";

const BUNDLE_EXECUTABLE_DIR_NAME: &str = "MacOS";

const BUNDLE_RESOURCES_DIR_NAME: &str = "Resources";

const BUNDLE_INFO_PLIST_FILE_NAME: &str = "Info.plist";

#[derive(Debug, thiserror::Error)]
pub enum ConfigDirError {
    #[error("could not resolve the running executable's directory: {0}")]
    NoExeDir(#[from] io::Error),

    #[error("no platform-appropriate per-user directory is available for this user")]
    NotAvailable,

    #[error("could not create the launcher state directory {}: {source}", path.display())]
    CreateStateRoot {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// The two launcher roots. They are the same directory except in a macOS app bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherRoots {
    /// Read-only payload: the loader, the client module, native plugins, and shipped packages.
    pub install: PathBuf,
    /// Writable state: configuration, backups, logs, caches, WebView data, and user packages.
    pub state: PathBuf,
}

impl LauncherRoots {
    /// Create the state root and its missing parents, never the install root, and return it.
    pub fn ensure_state(&self) -> Result<PathBuf, ConfigDirError> {
        create_private_dir(&self.state).map_err(|source| ConfigDirError::CreateStateRoot {
            path: self.state.clone(),
            source,
        })?;
        Ok(self.state.clone())
    }
}

/// Create `path` and its missing parents. New directories are private to the user on Unix
/// (chat logs, screenshots, and backups live here); an existing directory keeps its mode.
fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Resolve the directory containing the running launcher executable.
pub fn current_exe_dir() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other("current_exe has no parent directory"))
}

/// Resolve the running executable. macOS follows symlinks so a linked launch still finds its
/// bundle; other platforms keep the reported path because Windows canonicalization adds a
/// verbatim prefix.
fn launcher_executable() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    if cfg!(target_os = "macos") {
        return Ok(std::fs::canonicalize(&exe).unwrap_or(exe));
    }
    Ok(exe)
}

/// Resolve both launcher roots for the running executable; only a macOS app bundle splits them.
pub fn launcher_roots() -> Result<LauncherRoots, ConfigDirError> {
    let exe = launcher_executable()?;
    if cfg!(target_os = "macos") {
        resolve_roots(&exe, data_dir().ok())
    } else {
        portable_roots(&exe)
    }
}

/// Resolve the launcher roots for `exe` without touching the filesystem beyond bundle detection.
///
/// A bundled executable reads its payload from `Contents/Resources` and writes to
/// `per_user_data`; any other executable uses its own directory for both roots.
pub fn resolve_roots(
    exe: &Path,
    per_user_data: Option<PathBuf>,
) -> Result<LauncherRoots, ConfigDirError> {
    match bundle_contents_dir(exe) {
        Some(contents) => Ok(LauncherRoots {
            install: contents.join(BUNDLE_RESOURCES_DIR_NAME),
            state: per_user_data.ok_or(ConfigDirError::NotAvailable)?,
        }),
        None => portable_roots(exe),
    }
}

fn portable_roots(exe: &Path) -> Result<LauncherRoots, ConfigDirError> {
    let dir = exe
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other("launcher executable has no parent directory"))?;
    Ok(LauncherRoots {
        install: dir.clone(),
        state: dir,
    })
}

/// Return `<X>.app/Contents` when `exe` is `<X>.app/Contents/MacOS/<name>` and the bundle has
/// an `Info.plist` file.
pub(crate) fn bundle_contents_dir(exe: &Path) -> Option<PathBuf> {
    let executable_dir = exe.parent()?;
    let contents = executable_dir.parent()?;
    let bundle = contents.parent()?;
    let bundled = executable_dir.file_name() == Some(OsStr::new(BUNDLE_EXECUTABLE_DIR_NAME))
        && contents.file_name() == Some(OsStr::new(BUNDLE_CONTENTS_DIR_NAME))
        && bundle.extension() == Some(OsStr::new(BUNDLE_EXTENSION))
        && contents.join(BUNDLE_INFO_PLIST_FILE_NAME).is_file();
    bundled.then(|| contents.to_path_buf())
}

/// Resolve the read-only root holding the loader, client module, native plugins, and shipped packages.
pub fn install_root() -> Result<PathBuf, ConfigDirError> {
    launcher_roots().map(|roots| roots.install)
}

/// Resolve the writable root for configuration, backups, logs, caches, and user packages.
pub fn state_root() -> Result<PathBuf, ConfigDirError> {
    launcher_roots().map(|roots| roots.state)
}

/// Resolve the state root and create it when it is missing.
pub fn ensure_state_root() -> Result<PathBuf, ConfigDirError> {
    launcher_roots()?.ensure_state()
}

/// Resolve the data directory and create it when it is missing, before the Wine code first
/// writes there.
pub fn ensure_data_dir() -> Result<PathBuf, ConfigDirError> {
    let dir = data_dir()?;
    create_private_dir(&dir).map_err(|source| ConfigDirError::CreateStateRoot {
        path: dir.clone(),
        source,
    })?;
    Ok(dir)
}

/// Resolve `<state-root>/config/` for callers that report `io::Error`.
pub fn state_config_dir() -> io::Result<PathBuf> {
    portable_config_dir().map_err(io::Error::other)
}

/// Resolve `<state-root>/config/`, which holds every launcher and extension settings file.
pub fn portable_config_dir() -> Result<PathBuf, ConfigDirError> {
    Ok(state_root()?.join(PORTABLE_CONFIG_DIR_NAME))
}

/// Resolve the canonical launcher configuration path.
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

/// Resolve the folder containing bounded user-created backup archives.
pub fn backups_dir() -> Result<PathBuf, ConfigDirError> {
    Ok(state_root()?.join(PORTABLE_BACKUPS_DIR_NAME))
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

/// Resolve launcher data for the managed Wine prefix and runtime; a macOS app bundle also keeps
/// its state root here.
///
/// | Platform | Location |
/// |----------|----------|
/// | Windows  | `<exe-dir>\data` |
/// | macOS and Linux | `$BAHAMUT_LAUNCHER_HOME` when it is an absolute path, otherwise `~/.bahamut-launcher` |
#[cfg(target_os = "windows")]
pub fn data_dir() -> Result<PathBuf, ConfigDirError> {
    Ok(current_exe_dir()?.join(PORTABLE_DATA_DIR_NAME))
}

#[cfg(not(target_os = "windows"))]
pub fn data_dir() -> Result<PathBuf, ConfigDirError> {
    data_dir_from(
        std::env::var_os(LAUNCHER_HOME_ENVIRONMENT).as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// Pick the data directory from `$BAHAMUT_LAUNCHER_HOME` and `$HOME`; relative or empty values
/// are ignored.
#[cfg(not(target_os = "windows"))]
pub(crate) fn data_dir_from(
    override_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Result<PathBuf, ConfigDirError> {
    if let Some(path) = override_home
        .map(Path::new)
        .filter(|path| path.is_absolute())
    {
        return Ok(path.to_path_buf());
    }
    home.map(Path::new)
        .filter(|path| path.is_absolute())
        .map(|home| home.join(LAUNCHER_HOME_DIR_NAME))
        .ok_or(ConfigDirError::NotAvailable)
}

/// Resolve the native launcher transcript path.
pub fn launcher_log_path() -> Result<PathBuf, ConfigDirError> {
    Ok(state_root()?
        .join(LAUNCHER_LOGS_DIR_NAME)
        .join(LAUNCHER_LOGS_SCOPE_NAME)
        .join(LAUNCHER_LOG_FILE_NAME))
}

/// Resolve the cache for patch scratch files.
pub(crate) fn cache_dir() -> Result<PathBuf, ConfigDirError> {
    Ok(state_root()?.join(PORTABLE_CACHE_DIR_NAME))
}

/// Resolve the WebView profile used for local storage and browser caches.
pub fn webview_data_dir() -> Result<PathBuf, ConfigDirError> {
    Ok(state_root()?.join(PORTABLE_DATA_DIR_NAME))
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
    default: impl AsRef<[u8]>,
) -> io::Result<bool> {
    let path = dir.join(file_name);
    if path.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, default)?;
    Ok(true)
}

/// Build `<parent>/Bahamut Launcher.app` with an `Info.plist` and return its executable path.
#[cfg(test)]
pub(crate) fn write_test_bundle(parent: &Path) -> PathBuf {
    let contents = parent
        .join("Bahamut Launcher.app")
        .join(BUNDLE_CONTENTS_DIR_NAME);
    std::fs::create_dir_all(contents.join(BUNDLE_EXECUTABLE_DIR_NAME)).unwrap();
    std::fs::create_dir_all(contents.join(BUNDLE_RESOURCES_DIR_NAME)).unwrap();
    std::fs::write(contents.join(BUNDLE_INFO_PLIST_FILE_NAME), "<plist/>\n").unwrap();
    let exe = contents
        .join(BUNDLE_EXECUTABLE_DIR_NAME)
        .join("bahamut-launcher");
    std::fs::write(&exe, b"").unwrap();
    exe
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_exe_dir() -> PathBuf {
        launcher_executable()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    }

    #[test]
    fn unbundled_test_binary_uses_its_directory_for_both_roots() {
        let roots = launcher_roots().expect("test host has a current_exe");
        assert_eq!(roots.install, test_exe_dir());
        assert_eq!(roots.state, test_exe_dir());
        assert_eq!(install_root().unwrap(), state_root().unwrap());
    }

    #[test]
    fn portable_config_dir_lives_next_to_the_binary() {
        let dir = portable_config_dir().expect("test host has a current_exe");
        assert_eq!(
            dir.file_name().and_then(|n| n.to_str()),
            Some(PORTABLE_CONFIG_DIR_NAME),
            "portable config dir should be the {PORTABLE_CONFIG_DIR_NAME:?} folder \
             next to the binary; got {dir:?}",
        );
        assert_eq!(dir.parent(), Some(test_exe_dir().as_path()));
        assert_eq!(state_config_dir().unwrap(), dir);
    }

    #[test]
    fn cache_dir_is_portable() {
        let cache = cache_dir().expect("resolvable");
        assert_eq!(cache.parent(), Some(test_exe_dir().as_path()));
        assert_eq!(
            cache.file_name().and_then(|name| name.to_str()),
            Some(PORTABLE_CACHE_DIR_NAME)
        );
    }

    #[test]
    fn webview_data_dir_is_portable() {
        assert_eq!(
            webview_data_dir().unwrap(),
            test_exe_dir().join(PORTABLE_DATA_DIR_NAME)
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

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn data_dir_prefers_an_absolute_launcher_home_override() {
        assert_eq!(
            data_dir_from(
                Some(OsStr::new("/srv/launcher")),
                Some(OsStr::new("/Users/me"))
            )
            .unwrap(),
            Path::new("/srv/launcher")
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn data_dir_defaults_to_a_dot_folder_in_home() {
        for override_home in [None, Some(OsStr::new("")), Some(OsStr::new("relative"))] {
            assert_eq!(
                data_dir_from(override_home, Some(OsStr::new("/Users/me"))).unwrap(),
                Path::new("/Users/me/.bahamut-launcher")
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn data_dir_is_unavailable_without_an_override_or_home() {
        for home in [None, Some(OsStr::new("")), Some(OsStr::new("relative"))] {
            assert!(matches!(
                data_dir_from(None, home),
                Err(ConfigDirError::NotAvailable)
            ));
        }
    }

    #[test]
    fn bundle_contents_dir_accepts_an_app_bundle_executable() {
        let temp = tempfile::tempdir().unwrap();
        let exe = write_test_bundle(temp.path());
        assert_eq!(
            bundle_contents_dir(&exe),
            Some(temp.path().join("Bahamut Launcher.app/Contents"))
        );
    }

    #[test]
    fn bundle_contents_dir_requires_an_info_plist_file() {
        let temp = tempfile::tempdir().unwrap();
        let exe = write_test_bundle(temp.path());
        let plist = temp.path().join("Bahamut Launcher.app/Contents/Info.plist");
        std::fs::remove_file(&plist).unwrap();
        assert_eq!(bundle_contents_dir(&exe), None);
        std::fs::create_dir(&plist).unwrap();
        assert_eq!(bundle_contents_dir(&exe), None);
    }

    #[test]
    fn bundle_contents_dir_requires_the_macos_directory() {
        let temp = tempfile::tempdir().unwrap();
        write_test_bundle(temp.path());
        let contents = temp.path().join("Bahamut Launcher.app/Contents");
        for exe in [
            contents.join("Resources/bahamut-launcher"),
            contents.join("bahamut-launcher"),
        ] {
            assert_eq!(bundle_contents_dir(&exe), None, "{}", exe.display());
        }
    }

    #[test]
    fn bundle_contents_dir_requires_the_app_suffix() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["Bahamut Launcher", "Bahamut Launcher.apps", ".app"] {
            let contents = temp.path().join(name).join("Contents");
            std::fs::create_dir_all(contents.join("MacOS")).unwrap();
            std::fs::write(contents.join("Info.plist"), "<plist/>\n").unwrap();
            let exe = contents.join("MacOS/bahamut-launcher");
            assert_eq!(bundle_contents_dir(&exe), None, "{}", exe.display());
        }
    }

    #[test]
    fn bundle_contents_dir_requires_the_contents_directory() {
        let temp = tempfile::tempdir().unwrap();
        let other = temp.path().join("Bahamut Launcher.app/Other");
        std::fs::create_dir_all(other.join("MacOS")).unwrap();
        std::fs::write(other.join("Info.plist"), "<plist/>\n").unwrap();
        assert_eq!(
            bundle_contents_dir(&other.join("MacOS/bahamut-launcher")),
            None
        );
    }

    #[test]
    fn resolve_roots_splits_a_bundled_executable() {
        let temp = tempfile::tempdir().unwrap();
        let exe = write_test_bundle(temp.path());
        let state = temp.path().join("home/.bahamut-launcher");
        let roots = resolve_roots(&exe, Some(state.clone())).unwrap();
        assert_eq!(
            roots,
            LauncherRoots {
                install: temp.path().join("Bahamut Launcher.app/Contents/Resources"),
                state,
            }
        );
    }

    #[test]
    fn resolve_roots_keeps_a_bare_executable_portable() {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("bahamut-launcher");
        std::fs::write(&exe, b"").unwrap();
        for per_user_data in [None, Some(temp.path().join("home/.bahamut-launcher"))] {
            assert_eq!(
                resolve_roots(&exe, per_user_data).unwrap(),
                LauncherRoots {
                    install: temp.path().to_path_buf(),
                    state: temp.path().to_path_buf(),
                }
            );
        }
    }

    #[test]
    fn resolve_roots_requires_a_per_user_directory_for_a_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let exe = write_test_bundle(temp.path());
        assert!(matches!(
            resolve_roots(&exe, None),
            Err(ConfigDirError::NotAvailable)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn ensure_state_creates_a_private_directory_and_keeps_an_existing_mode() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let fresh = LauncherRoots {
            install: temp.path().join("install"),
            state: temp.path().join("home/.bahamut-launcher"),
        };
        fresh.ensure_state().unwrap();
        let mode = std::fs::metadata(&fresh.state)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "a new state root is private to the user");

        let existing = temp.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755)).unwrap();
        LauncherRoots {
            install: existing.clone(),
            state: existing.clone(),
        }
        .ensure_state()
        .unwrap();
        let mode = std::fs::metadata(&existing).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "an existing directory keeps its mode");
    }

    #[test]
    fn ensure_state_creates_only_the_state_root() {
        let temp = tempfile::tempdir().unwrap();
        let exe = write_test_bundle(temp.path());
        let state = temp.path().join("home/.bahamut-launcher");
        let roots = resolve_roots(&exe, Some(state.clone())).unwrap();
        assert_eq!(roots.ensure_state().unwrap(), state);
        assert!(state.is_dir());
        assert!(std::fs::read_dir(&roots.install).unwrap().next().is_none());
        assert_eq!(roots.ensure_state().unwrap(), state);
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
            test_exe_dir().join(PORTABLE_BACKUPS_DIR_NAME)
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
        let log = launcher_log_path().expect("resolvable");
        assert_eq!(
            log,
            test_exe_dir()
                .join(LAUNCHER_LOGS_DIR_NAME)
                .join(LAUNCHER_LOGS_SCOPE_NAME)
                .join(LAUNCHER_LOG_FILE_NAME)
        );
    }
}
