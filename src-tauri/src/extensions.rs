use std::path::{Path, PathBuf};

use bahamut_launcher::config::dirs;
use bahamut_launcher::config::extension_config::{DatsConfig, ExtensionsConfig};
use bahamut_launcher::config::preferences::ScreenshotSettings;
use bahamut_launcher::extensions::{
    CLIENT_MODULE_NAME, ExtensionLayout, LOADER_BINARY_NAME, discover_addons,
    discover_overlay_packages, select_addon_manifests, select_overlay_packages,
};
use bahamut_launcher::platform::ExtensionArtifacts;

const DISCORD_APPLICATION_ID: &str = "1546272666833522798";

const fn diagnostic_no_injection_build() -> bool {
    cfg!(feature = "diagnostic-no-injection")
}

pub(super) fn extension_layout() -> Result<ExtensionLayout, String> {
    let layout = ExtensionLayout::under_launcher_dir(
        &dirs::current_exe_dir().map_err(|error| error.to_string())?,
    );
    layout.create().map_err(|e| e.to_string())?;
    Ok(layout)
}

pub(super) fn packaged_client_paths() -> Result<(PathBuf, PathBuf), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the launcher executable: {error}"))?;
    let root = executable
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| "launcher executable has no parent directory".to_owned())?;
    Ok((root.join(LOADER_BINARY_NAME), root.join(CLIENT_MODULE_NAME)))
}

fn packaged_artifacts_present(helper: &Path, module: &Path) -> Result<(), String> {
    for path in [helper, module] {
        if !path.is_file() {
            return Err(format!("{} is not a file", path.display()));
        }
    }
    Ok(())
}

fn selected_dat_roots(dats_root: &Path, dat_config: &DatsConfig) -> Result<Vec<PathBuf>, String> {
    let packages = discover_overlay_packages(dats_root).map_err(|error| error.to_string())?;
    Ok(select_overlay_packages(&packages, &dat_config.packages).roots)
}

pub(super) fn launch_artifacts(
    extension_config: &ExtensionsConfig,
    screenshot: &ScreenshotSettings,
) -> Result<Option<ExtensionArtifacts>, String> {
    if diagnostic_no_injection_build() {
        tracing::warn!(
            "diagnostic no-injection build: using the direct patched launch without bahamut.dll"
        );
        return Ok(None);
    }
    if !cfg!(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux"
    )) {
        return Ok(None);
    }
    // Off Windows the loader and module are side-loaded, so a missing one falls back to the plain
    // launch before any layout or discovery work that could fail it.
    if !cfg!(target_os = "windows")
        && let Err(reason) = packaged_client_paths()
            .and_then(|(helper, module)| packaged_artifacts_present(&helper, &module))
    {
        tracing::warn!(
            reason,
            "client extensions are unavailable; launching without extensions"
        );
        return Ok(None);
    }
    (|| {
        let layout = extension_layout()?;
        let (helper_path, client_module_path) = packaged_client_paths()?;
        let packages = discover_addons(&layout.addons).map_err(|error| error.to_string())?;
        let dat_config = DatsConfig::load().map_err(|error| error.to_string())?;
        let dat_roots = selected_dat_roots(&layout.dats, &dat_config)?;
        let addon_manifests = select_addon_manifests(&packages, &extension_config.addons);
        let addon_catalog = packages
            .iter()
            .map(|package| package.manifest_path.clone())
            .collect();
        Ok(Some(ExtensionArtifacts {
            helper_path,
            client_module_path,
            addon_manifests,
            addon_catalog,
            addon_settings_path: layout.addon_settings,
            chat_logs_path: layout.chat_logs,
            dat_roots,
            startup_script_path: layout.scripts.join("default.txt"),
            screenshot_plugin_path: layout.screenshot_plugin_path,
            screenshots_path: layout.screenshots,
            screenshot_format: screenshot.format,
            screenshot_hide_overlays: screenshot.hide_overlays,
            screenshot_hotkey: screenshot.hotkey,
            screenshot_enabled: extension_config.plugin_enabled("screenshot"),
            discord_rpc_plugin_path: layout.discord_rpc_plugin_path,
            discord_rpc_application_id: DISCORD_APPLICATION_ID.into(),
            discord_rpc_enabled: extension_config.plugin_enabled("discord-rpc"),
            object_distance_enabled: extension_config.plugin_enabled("object-distance"),
            object_distance_percent: extension_config.object_distance_percent,
            camera_zoom_enabled: extension_config.plugin_enabled("camera-zoom"),
            camera_zoom_limit: extension_config.camera_zoom_limit,
        }))
    })()
}

#[cfg(test)]
mod launch_overlay_tests {
    use super::*;
    use bahamut_launcher::config::extension_config::OFFICIAL_DAT_OVERLAY_PACKAGE_ID;
    use std::fs;

    #[test]
    fn missing_or_malformed_official_overlay_keeps_valid_custom_roots_for_launch() {
        let dat_config =
            DatsConfig::from_ini_str("[dats]\n\n[dat.1]\nid = custom\nenabled = true\n").unwrap();
        for malformed_official in [false, true] {
            let fixture = tempfile::tempdir().unwrap();
            let dats_root = fixture.path();
            let custom_root = dats_root.join("custom");
            fs::create_dir_all(&custom_root).unwrap();
            fs::write(
                custom_root.join("overlay.toml"),
                "manifest_schema_version = 1\nid = \"custom\"\nname = \"Custom\"\nauthor = \"Tester\"\nversion = \"1\"\ndescription = \"Valid\"\n",
            )
            .unwrap();
            if malformed_official {
                let official_root = dats_root.join(OFFICIAL_DAT_OVERLAY_PACKAGE_ID);
                fs::create_dir_all(&official_root).unwrap();
                fs::write(official_root.join("overlay.toml"), "invalid manifest").unwrap();
            }

            assert_eq!(
                selected_dat_roots(dats_root, &dat_config).unwrap(),
                vec![fs::canonicalize(&custom_root).unwrap()]
            );
        }
    }
}

#[cfg(test)]
mod packaged_artifact_tests {
    use super::*;
    use std::fs;

    #[test]
    fn packaged_artifacts_require_both_files() {
        let fixture = tempfile::tempdir().unwrap();
        let helper = fixture.path().join(LOADER_BINARY_NAME);
        let module = fixture.path().join(CLIENT_MODULE_NAME);

        let missing = packaged_artifacts_present(&helper, &module).unwrap_err();
        assert!(missing.contains(LOADER_BINARY_NAME), "{missing}");

        fs::write(&helper, b"MZ").unwrap();
        fs::create_dir(&module).unwrap();
        let directory = packaged_artifacts_present(&helper, &module).unwrap_err();
        assert!(directory.contains(CLIENT_MODULE_NAME), "{directory}");

        fs::remove_dir(&module).unwrap();
        fs::write(&module, b"MZ").unwrap();
        assert_eq!(packaged_artifacts_present(&helper, &module), Ok(()));
    }

    #[cfg(all(
        any(target_os = "linux", target_os = "macos"),
        not(feature = "diagnostic-no-injection")
    ))]
    #[test]
    fn wine_host_without_packaged_artifacts_keeps_the_plain_launch() {
        let (helper, module) = packaged_client_paths().unwrap();
        assert!(
            !helper.is_file() || !module.is_file(),
            "the test binary directory must not hold packaged extension artifacts"
        );
        let artifacts =
            launch_artifacts(&ExtensionsConfig::default(), &ScreenshotSettings::default()).unwrap();
        assert_eq!(artifacts, None);
    }
}

#[cfg(all(test, feature = "diagnostic-no-injection"))]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_build_bypasses_all_packaged_artifacts() {
        let artifacts =
            launch_artifacts(&ExtensionsConfig::default(), &ScreenshotSettings::default()).unwrap();
        assert_eq!(artifacts, None);
    }
}
