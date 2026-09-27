use bahamut_launcher::config::preferences::valid_borderless_monitor_id;
use bahamut_launcher::platform;
use bahamut_launcher::profiles::ServerProfile;

use crate::presentation::{
    BorderlessMonitorView, GameSettingsPayload, GameSettingsView, LauncherBehaviorView,
    ServerSettingsView, borderless_monitor_view, game_settings_view, replace_game_settings,
};
use crate::shell_config::{load_launcher_config, resolve_game_dir, update_launcher_config};

#[tauri::command]
pub(crate) fn launch_config_tool() -> Result<(), String> {
    let game_dir =
        resolve_game_dir().ok_or_else(|| "No valid game install selected.".to_owned())?;
    platform::launch_config_tool(&game_dir)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn config_tool_supported() -> bool {
    cfg!(target_os = "windows")
}

#[tauri::command]
pub(crate) fn get_game_settings() -> Result<GameSettingsView, String> {
    Ok(game_settings_view(load_launcher_config()?.preferences.game))
}

#[tauri::command]
pub(crate) fn set_game_settings(settings: GameSettingsPayload) -> Result<GameSettingsView, String> {
    let config = update_launcher_config(|config| replace_game_settings(config, settings))?;
    Ok(game_settings_view(config.preferences.game))
}

#[tauri::command]
pub(crate) fn get_launcher_behavior() -> Result<LauncherBehaviorView, String> {
    let config = load_launcher_config()?;
    Ok(LauncherBehaviorView {
        close_on_game_start: config.preferences.launcher.close_on_game_start,
        native_resolution_override: config.preferences.launcher.native_resolution_override,
    })
}

#[tauri::command]
pub(crate) fn set_close_on_game_start(
    close_on_game_start: bool,
) -> Result<LauncherBehaviorView, String> {
    let config = update_launcher_config(|config| {
        config.preferences.launcher.close_on_game_start = close_on_game_start;
        Ok(())
    })?;
    Ok(LauncherBehaviorView {
        close_on_game_start: config.preferences.launcher.close_on_game_start,
        native_resolution_override: config.preferences.launcher.native_resolution_override,
    })
}

#[tauri::command]
pub(crate) fn set_native_resolution_override(
    native_resolution_override: bool,
) -> Result<LauncherBehaviorView, String> {
    let config = update_launcher_config(|config| {
        config.preferences.launcher.native_resolution_override = native_resolution_override;
        Ok(())
    })?;
    Ok(LauncherBehaviorView {
        close_on_game_start: config.preferences.launcher.close_on_game_start,
        native_resolution_override: config.preferences.launcher.native_resolution_override,
    })
}

#[tauri::command]
pub(crate) fn get_borderless_monitors() -> Result<BorderlessMonitorView, String> {
    let snapshot = platform::enumerate_borderless_monitors()?;
    let config = load_launcher_config()?;
    Ok(borderless_monitor_view(
        snapshot,
        config.preferences.launcher.borderless_monitor,
    ))
}

#[tauri::command]
pub(crate) fn set_borderless_monitor(
    monitor_id: Option<String>,
) -> Result<BorderlessMonitorView, String> {
    if let Some(id) = &monitor_id
        && !valid_borderless_monitor_id(id)
    {
        return Err(
            "Monitor identity must be nonempty, at most 1024 UTF-8 bytes, and contain no control characters."
                .into(),
        );
    }
    let snapshot = platform::enumerate_borderless_monitors()?;
    let config = update_launcher_config(|config| {
        config.preferences.launcher.borderless_monitor = monitor_id.clone();
        Ok(())
    })?;
    Ok(borderless_monitor_view(
        snapshot,
        config.preferences.launcher.borderless_monitor,
    ))
}

#[tauri::command]
pub(crate) fn get_server_settings() -> Result<ServerSettingsView, String> {
    let config = load_launcher_config()?;
    Ok(ServerSettingsView {
        selected_server: config.selected_server,
        servers: config.servers,
    })
}

#[tauri::command]
pub(crate) fn set_selected_server(display_name: String) -> Result<ServerSettingsView, String> {
    let config = update_launcher_config(|config| {
        config
            .select_server(&display_name)
            .map_err(|error| error.to_string())
    })?;
    Ok(ServerSettingsView {
        selected_server: config.selected_server,
        servers: config.servers,
    })
}

#[tauri::command]
pub(crate) fn save_server_profile(
    original_display_name: String,
    profile: ServerProfile,
) -> Result<ServerSettingsView, String> {
    let config = update_launcher_config(|config| {
        config
            .replace_server(&original_display_name, profile)
            .map_err(|error| error.to_string())
    })?;
    Ok(ServerSettingsView {
        selected_server: config.selected_server,
        servers: config.servers,
    })
}
