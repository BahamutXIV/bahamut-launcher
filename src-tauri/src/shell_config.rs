use std::path::PathBuf;

use bahamut_launcher::config::dirs;
use bahamut_launcher::config::extension_config::{
    DatsConfig, ExtensionsConfig, load_screenshot_settings,
};
use bahamut_launcher::config::launcher_ini::LauncherConfig;
use bahamut_launcher::config::preferences::{Preferences, ScreenshotSettings};
use bahamut_launcher::platform;

pub(crate) fn load_preferences() -> Result<Preferences, String> {
    Ok(load_launcher_config()?.preferences)
}

pub(crate) fn load_launcher_config() -> Result<LauncherConfig, String> {
    let _write = LauncherConfig::write_lock().map_err(|error| error.to_string())?;
    LauncherConfig::load().map_err(|error| error.to_string())
}

pub(crate) fn load_extensions_config() -> Result<ExtensionsConfig, String> {
    ExtensionsConfig::load().map_err(|error| error.to_string())
}

pub(crate) fn load_dats_config() -> Result<DatsConfig, String> {
    DatsConfig::load().map_err(|error| error.to_string())
}

pub(crate) fn load_screenshot_config() -> Result<ScreenshotSettings, String> {
    load_screenshot_settings().map_err(|error| error.to_string())
}

pub(crate) fn update_launcher_config(
    update: impl FnOnce(&mut LauncherConfig) -> Result<(), String>,
) -> Result<LauncherConfig, String> {
    let _write = LauncherConfig::write_lock().map_err(|error| error.to_string())?;
    let mut config = LauncherConfig::load().map_err(|error| error.to_string())?;
    update(&mut config)?;
    config.save().map_err(|error| error.to_string())?;
    Ok(config)
}

/// Preference `game_location` wins over registry detection; source labels are `preferences`, `registry`, and `none`.
pub(crate) fn resolve_game_dir_with_source() -> (Option<PathBuf>, &'static str) {
    if let Ok(prefs) = load_preferences()
        && let Some(path) = prefs.launcher.game_location
    {
        return (Some(path), "preferences");
    }
    match platform::detect_game_install() {
        Some(path) => (Some(path), "registry"),
        None => (None, "none"),
    }
}

pub(crate) fn resolve_game_dir_for_preferences(preferences: &Preferences) -> Option<PathBuf> {
    preferences
        .launcher
        .game_location
        .clone()
        .or_else(platform::detect_game_install)
}

pub(crate) fn resolve_game_dir() -> Option<PathBuf> {
    resolve_game_dir_with_source().0
}

/// Writable game install offered before the user selects a different path.
pub(crate) fn default_game_dir_hint() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        Some(PathBuf::from(r"C:\Games\FINAL FANTASY XIV"))
    }
    #[cfg(not(windows))]
    {
        directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().join("Games").join("FINAL FANTASY XIV"))
    }
}

pub(crate) fn resolve_content_root() -> Result<String, String> {
    let configured = load_preferences()?.launcher.content_root;
    configured
        .or(bahamut_launcher::content::manifest::shipped_manifest()?.content_root)
        .filter(|root| !root.trim().is_empty())
        .ok_or_else(|| "Game content delivery is not configured for this build.".to_owned())
}

/// `download_cache_dir` overrides the platform default (`Documents/XIVLegacy_Downloads`).
pub(crate) fn resolve_download_cache_dir() -> Result<PathBuf, String> {
    let prefs = load_preferences()?;
    match prefs.launcher.download_cache_dir {
        Some(dir) => Ok(dir),
        None => dirs::default_download_cache_dir().map_err(|e| e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::ErrorKind;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn default_install_uses_dedicated_games_directory() {
        #[cfg(windows)]
        assert_eq!(
            default_game_dir_hint(),
            Some(PathBuf::from(r"C:\Games\FINAL FANTASY XIV"))
        );
        #[cfg(not(windows))]
        assert_eq!(
            default_game_dir_hint(),
            directories::BaseDirs::new()
                .map(|dirs| dirs.home_dir().join("Games").join("FINAL FANTASY XIV"))
        );
    }

    struct ConfigRestore {
        path: PathBuf,
        original: Option<Vec<u8>>,
    }

    impl Drop for ConfigRestore {
        fn drop(&mut self) {
            if let Some(original) = &self.original {
                let _ = fs::write(&self.path, original);
            } else {
                let _ = fs::remove_file(&self.path);
            }
        }
    }

    #[test]
    fn concurrent_config_updates_preserve_disjoint_changes() {
        let path = dirs::launcher_config_path().unwrap();
        let original = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => panic!("could not preserve launcher config fixture: {error}"),
        };
        let _restore = ConfigRestore { path, original };
        LauncherConfig::defaults().save().unwrap();

        let (first_loaded_tx, first_loaded_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let (second_started_tx, second_started_rx) = mpsc::channel();
        let (second_loaded_tx, second_loaded_rx) = mpsc::channel();

        std::thread::scope(|scope| {
            let first = scope.spawn(move || {
                update_launcher_config(|config| {
                    config.preferences.launcher.download_cache_dir =
                        Some(PathBuf::from("concurrent-downloads"));
                    first_loaded_tx.send(()).unwrap();
                    release_first_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap();
            });
            first_loaded_rx.recv().unwrap();

            let second = scope.spawn(move || {
                second_started_tx.send(()).unwrap();
                update_launcher_config(|config| {
                    second_loaded_tx.send(()).unwrap();
                    config.preferences.launcher.close_on_game_start = false;
                    Ok(())
                })
                .unwrap();
            });
            second_started_rx.recv().unwrap();
            let entered_early = second_loaded_rx
                .recv_timeout(Duration::from_millis(100))
                .is_ok();
            release_first_tx.send(()).unwrap();
            assert!(
                !entered_early,
                "second writer bypassed the config transaction"
            );
            second_loaded_rx.recv().unwrap();
            first.join().unwrap();
            second.join().unwrap();
        });

        let config = load_launcher_config().unwrap();
        assert_eq!(
            config.preferences.launcher.download_cache_dir,
            Some(PathBuf::from("concurrent-downloads"))
        );
        assert!(!config.preferences.launcher.close_on_game_start);
    }
}
