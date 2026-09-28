use std::path::{Path, PathBuf};

use bahamut_launcher::config::extension_config::{
    DISCORD_RPC_PLUGIN_ID, DatsConfig, ExtensionsConfig, OFFICIAL_DAT_OVERLAY_PACKAGE_ID,
    SCREENSHOT_PLUGIN_ID,
};

use crate::extensions;
use crate::presentation::{
    ExtensionCommandView, ExtensionInventoryView, ExtensionItemView, OverlayConflictView,
    OverlayPackageView,
};
use crate::shell_config::{load_dats_config, load_extensions_config};

#[tauri::command]
pub(crate) fn get_extension_inventory() -> Result<ExtensionInventoryView, String> {
    let layout = extensions::extension_layout()?;
    let packages = extensions::installed_addons(&layout)?;
    let config = load_extensions_config()?;
    let dat_packages = extensions::installed_overlay_packages(&layout)?;
    let dat_config = load_dats_config()?;
    Ok(extension_inventory_view_with_overlays(
        packages,
        &config,
        dat_packages,
        &dat_config,
    ))
}

pub(crate) fn extension_inventory_view_with_overlays(
    mut packages: Vec<bahamut_launcher::extensions::AddonPackage>,
    config: &ExtensionsConfig,
    mut dat_packages: Vec<bahamut_launcher::extensions::OverlayPackage>,
    dat_config: &DatsConfig,
) -> ExtensionInventoryView {
    packages.sort_by_key(|package| {
        config
            .addons
            .iter()
            .position(|addon| addon.id == package.id)
            .unwrap_or(usize::MAX)
    });
    let addons = packages
        .into_iter()
        .map(|package| {
            let supported = package
                .supported_client_builds
                .iter()
                .any(|build| build == bahamut_launcher::extensions::SUPPORTED_GAME_VERSION);
            let enabled = config
                .addons
                .iter()
                .find(|preference| preference.id == package.id)
                .is_some_and(|preference| preference.enabled);
            ExtensionItemView {
                id: package.id,
                name: package.name,
                author: package.author,
                version: package.version,
                description: package.description,
                homepage: package.homepage,
                commands: package
                    .commands
                    .into_iter()
                    .map(|command| ExtensionCommandView {
                        name: command.name,
                        usage: command.usage,
                        description: command.description,
                    })
                    .collect(),
                capabilities: package.capabilities,
                compatibility: if supported {
                    "Supported client build".to_owned()
                } else {
                    "Client build is not listed by this package".to_owned()
                },
                status: if enabled {
                    "Enabled".to_owned()
                } else {
                    "Disabled".to_owned()
                },
                trust: "Isolated Lua",
                enabled,
            }
        })
        .collect();
    dat_packages.sort_by_key(|package| {
        if package.id == OFFICIAL_DAT_OVERLAY_PACKAGE_ID {
            0
        } else {
            dat_config
                .packages
                .iter()
                .position(|preference| preference.id == package.id)
                .map(|position| position.saturating_add(1))
                .unwrap_or(usize::MAX)
        }
    });
    let dat_selection =
        bahamut_launcher::extensions::select_overlay_packages(&dat_packages, &dat_config.packages);
    let overlays = dat_packages
        .into_iter()
        .map(|package| {
            let enabled = package.id == OFFICIAL_DAT_OVERLAY_PACKAGE_ID
                || dat_config
                    .packages
                    .iter()
                    .find(|preference| preference.id == package.id)
                    .is_some_and(|preference| preference.enabled);
            OverlayPackageView {
                id: package.id,
                name: package.name,
                author: package.author,
                version: package.version,
                description: package.description,
                homepage: package.homepage,
                enabled,
                status: if enabled {
                    "Enabled".to_owned()
                } else {
                    "Disabled".to_owned()
                },
                trust: "Data-only overlay",
            }
        })
        .collect();
    let overlay_conflicts = dat_selection
        .conflicts
        .into_iter()
        .map(|conflict| OverlayConflictView {
            relative_path: conflict.relative_path.to_string_lossy().replace('\\', "/"),
            package_ids: conflict.package_ids,
        })
        .collect();
    ExtensionInventoryView {
        addons,
        plugins: vec![
            screenshot_plugin_view(config.plugin_enabled(SCREENSHOT_PLUGIN_ID)),
            discord_rpc_plugin_view(config.plugin_enabled(DISCORD_RPC_PLUGIN_ID)),
        ],
        overlays,
        overlay_conflicts,
    }
}

fn discord_rpc_plugin_view(enabled: bool) -> ExtensionItemView {
    ExtensionItemView {
        id: "discord-rpc".into(),
        name: "DiscordRPC".into(),
        author: "Aeshur".into(),
        version: "1.0".into(),
        description: "Shows your character name, location, and level in Discord as rich presence."
            .into(),
        homepage: None,
        commands: Vec::new(),
        capabilities: Vec::new(),
        compatibility: "Supported client build".into(),
        status: if enabled {
            "Enabled".into()
        } else {
            "Disabled".into()
        },
        trust: "First-party native",
        enabled,
    }
}

fn screenshot_plugin_view(enabled: bool) -> ExtensionItemView {
    ExtensionItemView {
        id: "screenshot".into(),
        name: "Screenshot".into(),
        author: "Aeshur".into(),
        version: "1.0".into(),
        description: "Captures the game to the portable screenshots folder.".into(),
        homepage: None,
        commands: vec![ExtensionCommandView {
            name: "screenshot".into(),
            usage: "/screenshot".into(),
            description: "Captures a frame when invoked by a configured binding.".into(),
        }],
        capabilities: Vec::new(),
        compatibility: "Supported client build".into(),
        status: if enabled {
            "Enabled".into()
        } else {
            "Disabled".into()
        },
        trust: "First-party native",
        enabled,
    }
}

#[tauri::command]
pub(crate) fn set_addon_enabled(
    id: String,
    enabled: bool,
) -> Result<ExtensionInventoryView, String> {
    let layout = extensions::extension_layout()?;
    let packages = extensions::installed_addons(&layout)?;
    if !packages.iter().any(|package| package.id == id) {
        return Err(format!("addon {id:?} is not installed"));
    }
    let mut config = load_extensions_config()?;
    config
        .set_addon_enabled(&id, enabled)
        .map_err(|error| error.to_string())?;
    let dat_packages = extensions::installed_overlay_packages(&layout)?;
    let dat_config = load_dats_config()?;
    config.save().map_err(|error| error.to_string())?;
    Ok(extension_inventory_view_with_overlays(
        packages,
        &config,
        dat_packages,
        &dat_config,
    ))
}

#[tauri::command]
pub(crate) fn set_screenshot_enabled(enabled: bool) -> Result<ExtensionInventoryView, String> {
    set_plugin_enabled(SCREENSHOT_PLUGIN_ID, enabled)
}

#[tauri::command]
pub(crate) fn set_discord_rpc_enabled(enabled: bool) -> Result<ExtensionInventoryView, String> {
    set_plugin_enabled(DISCORD_RPC_PLUGIN_ID, enabled)
}

#[tauri::command]
pub(crate) fn get_object_distance_selection() -> Result<Option<u16>, String> {
    Ok(load_extensions_config()?.object_distance_selection())
}

#[tauri::command]
pub(crate) fn set_object_distance_selection(percent: Option<u16>) -> Result<Option<u16>, String> {
    let mut config = load_extensions_config()?;
    config
        .set_object_distance_selection(percent)
        .map_err(|error| error.to_string())?;
    config.save().map_err(|error| error.to_string())?;
    Ok(config.object_distance_selection())
}

#[tauri::command]
pub(crate) fn get_camera_zoom_selection() -> Result<Option<u8>, String> {
    Ok(load_extensions_config()?.camera_zoom_selection())
}

#[tauri::command]
pub(crate) fn set_camera_zoom_selection(limit: Option<u8>) -> Result<Option<u8>, String> {
    let mut config = load_extensions_config()?;
    config
        .set_camera_zoom_selection(limit)
        .map_err(|error| error.to_string())?;
    config.save().map_err(|error| error.to_string())?;
    Ok(config.camera_zoom_selection())
}

fn set_plugin_enabled(id: &str, enabled: bool) -> Result<ExtensionInventoryView, String> {
    let layout = extensions::extension_layout()?;
    let packages = extensions::installed_addons(&layout)?;
    let mut config = load_extensions_config()?;
    config
        .set_plugin_enabled(id, enabled)
        .map_err(|error| error.to_string())?;
    let dat_packages = extensions::installed_overlay_packages(&layout)?;
    let dat_config = load_dats_config()?;
    config.save().map_err(|error| error.to_string())?;
    Ok(extension_inventory_view_with_overlays(
        packages,
        &config,
        dat_packages,
        &dat_config,
    ))
}

#[tauri::command]
pub(crate) fn set_dat_package_enabled(
    id: String,
    enabled: bool,
) -> Result<ExtensionInventoryView, String> {
    let layout = extensions::extension_layout()?;
    let packages = extensions::installed_addons(&layout)?;
    let dat_packages = extensions::installed_overlay_packages(&layout)?;
    if !dat_packages.iter().any(|package| package.id == id) {
        return Err(format!("DAT package {id:?} is not installed"));
    }
    let mut dat_config = load_dats_config()?;
    dat_config
        .set_package_enabled(&id, enabled)
        .map_err(|error| error.to_string())?;
    let config = load_extensions_config()?;
    dat_config.save().map_err(|error| error.to_string())?;
    Ok(extension_inventory_view_with_overlays(
        packages,
        &config,
        dat_packages,
        &dat_config,
    ))
}

#[tauri::command]
pub(crate) fn reorder_dat_package(
    id: String,
    position: usize,
) -> Result<ExtensionInventoryView, String> {
    let layout = extensions::extension_layout()?;
    let packages = extensions::installed_addons(&layout)?;
    let dat_packages = extensions::installed_overlay_packages(&layout)?;
    if !dat_packages.iter().any(|package| package.id == id) {
        return Err(format!("DAT package {id:?} is not installed"));
    }
    let mut dat_config = load_dats_config()?;
    let installed_ids = dat_packages
        .iter()
        .map(|package| package.id.clone())
        .collect::<Vec<_>>();
    dat_config
        .reorder_package_for_installed(&id, position, &installed_ids)
        .map_err(|error| error.to_string())?;
    let config = load_extensions_config()?;
    dat_config.save().map_err(|error| error.to_string())?;
    Ok(extension_inventory_view_with_overlays(
        packages,
        &config,
        dat_packages,
        &dat_config,
    ))
}

/// Map an Open Folder target to a writable directory; shipped install-root content is never opened.
pub(crate) fn extension_folder_path(
    layout: &bahamut_launcher::extensions::ExtensionLayout,
    target: &str,
) -> Result<PathBuf, String> {
    match target {
        "addons" => Ok(layout.addons.clone()),
        "plugins" => Ok(layout.plugins.clone()),
        "logs" => Ok(layout.logs.clone()),
        "screenshots" => Ok(layout.screenshots.clone()),
        "backups" => {
            bahamut_launcher::config::dirs::backups_dir().map_err(|error| error.to_string())
        }
        "install" => Ok(layout.state_root.clone()),
        _ => Err(format!("unknown extension folder target: {target}")),
    }
}

#[tauri::command]
pub(crate) fn open_extension_folder(target: String) -> Result<(), String> {
    let layout = extensions::extension_layout()?;
    let path = extension_folder_path(&layout, &target)?;
    std::fs::create_dir_all(&path)
        .map_err(|error| format!("could not create {}: {error}", path.display()))?;
    open_directory(&path).map_err(|error| format!("could not open {}: {error}", path.display()))
}

#[cfg(windows)]
fn open_directory(path: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("explorer.exe")
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn open_directory(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg(path)
        .spawn()
        .map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_directory(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
}

#[cfg(not(any(windows, unix)))]
fn open_directory(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "opening a directory is unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use bahamut_launcher::config::extension_config::ExtensionPreference;
    use bahamut_launcher::extensions::OverlayPackage;

    #[test]
    fn official_overlay_is_reported_enabled_in_the_first_slot() {
        let package = |id: &str| OverlayPackage {
            manifest_path: PathBuf::from(format!("{id}/overlay.toml")),
            root_path: PathBuf::from(id),
            id: id.into(),
            name: id.into(),
            author: "test".into(),
            version: "1".into(),
            description: String::new(),
            homepage: None,
            payload_files: Vec::new(),
        };
        let config = DatsConfig {
            packages: vec![
                ExtensionPreference {
                    id: "custom".into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: OFFICIAL_DAT_OVERLAY_PACKAGE_ID.into(),
                    enabled: false,
                },
            ],
        };
        let view = extension_inventory_view_with_overlays(
            Vec::new(),
            &ExtensionsConfig::default(),
            vec![package("custom"), package(OFFICIAL_DAT_OVERLAY_PACKAGE_ID)],
            &config,
        );

        assert_eq!(view.overlays[0].id, OFFICIAL_DAT_OVERLAY_PACKAGE_ID);
        assert!(view.overlays[0].enabled);
        assert_eq!(view.overlays[0].status, "Enabled");
        assert_eq!(view.overlays[1].id, "custom");
    }
}
