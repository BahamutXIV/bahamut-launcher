//! Tauri shell and IPC orchestration; reusable launcher domains stay in the Tauri-free core.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod commands;
mod diagnostics;
mod extensions;
mod launcher_updates;
mod prerequisites;
mod presentation;
mod shell_config;
mod state;

use std::time::Instant;

use bahamut_launcher::config::dirs;
use bahamut_launcher::config::launcher_ini::LauncherConfig;
use bahamut_launcher::logging;
use bahamut_launcher::news;
use commands::{
    apply_launcher_update, cancel_game_repair, cancel_patch, check_launcher_update,
    config_tool_supported, control_window, create_backup, detect_game_install_command,
    fit_window_to_work_area, game_repair_status, game_status, get_borderless_monitors,
    get_camera_zoom_selection, get_extension_inventory, get_game_settings, get_home_status,
    get_launcher_behavior, get_launcher_log, get_launcher_update_status,
    get_object_distance_selection, get_patch_settings, get_server_settings, install_game,
    install_quote, launch_config_tool, launch_game, launcher_update_restart_available,
    launcher_version, list_news, login, logout, open_extension_folder, open_external,
    open_launcher_log, patch_status, pause_game_repair, pause_patch, pick_directory,
    pick_install_dir, register, reorder_dat_package, reset_patch, restore_backup,
    resume_game_repair, resume_patch, save_server_profile, set_addon_enabled,
    set_borderless_monitor, set_camera_zoom_selection, set_close_on_game_start,
    set_dat_package_enabled, set_discord_rpc_enabled, set_game_settings,
    set_native_resolution_override, set_object_distance_selection, set_screenshot_enabled,
    set_selected_server, start_game_repair, start_local_patch, start_patch_download,
    validate_session,
};
use shell_config::{load_dats_config, load_extensions_config, load_screenshot_config};
use state::{BackupIpcState, GameIpcState, PatcherIpcState};
use tauri::{Emitter, Manager};

fn continue_shutdown(app: tauri::AppHandle, state: &PatcherIpcState, exit_code: i32) -> bool {
    let game_pending = app.state::<GameIpcState>().request_shutdown();
    let backup_pending = app.state::<BackupIpcState>().request_shutdown();
    let request = state.request_shutdown(game_pending || backup_pending);
    if !request.waiting {
        return false;
    }

    if request.first_request {
        if let Err(error) = app.emit("launcher-closing", ()) {
            tracing::warn!(%error, "could not notify the launcher that close is waiting");
        }
        let app_for_waiter = app.clone();
        std::thread::spawn(move || {
            if request.join_worker() {
                tracing::error!("patcher worker panicked while launcher was closing");
            }
            app_for_waiter.state::<GameIpcState>().wait_for_worker();
            app_for_waiter.state::<BackupIpcState>().wait_for_worker();
            app_for_waiter
                .state::<PatcherIpcState>()
                .mark_shutdown_complete();
            app_for_waiter.exit(exit_code);
        });
    }
    true
}

/// Update recovery runs before anything else writes, so it creates the state root first.
fn request_pending_update_recovery(force_rollback: bool) -> Result<bool, String> {
    let roots = dirs::launcher_roots()
        .map_err(|error| format!("Could not resolve the launcher directories: {error}"))?;
    request_pending_update_recovery_for(&roots, force_rollback)
}

fn request_pending_update_recovery_for(
    roots: &dirs::LauncherRoots,
    force_rollback: bool,
) -> Result<bool, String> {
    let root = roots
        .ensure_state()
        .map_err(|error| format!("Could not prepare the launcher state directory: {error}"))?;
    launcher_updates::recover_failed_launcher_update(&root, force_rollback)
}

fn main() {
    if launcher_updates::has_recovered_update_argument() {
        let root = match dirs::state_root() {
            Ok(root) => root,
            Err(error) => {
                prerequisites::show_startup_error_dialog(&format!(
                    "Could not resolve the launcher state directory: {error}"
                ));
                return;
            }
        };
        if let Err(error) = launcher_updates::finalize_recovered_launcher_update(&root) {
            prerequisites::show_startup_error_dialog(&error);
            return;
        }
    } else if !launcher_updates::has_update_transaction_argument() {
        match request_pending_update_recovery(false) {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                prerequisites::show_startup_error_dialog(&error);
                return;
            }
        }
    }

    if let Err(error) = prerequisites::ensure_webview2_for_startup() {
        if launcher_updates::has_update_transaction_argument() {
            match request_pending_update_recovery(true) {
                Ok(true) => return,
                Ok(false) => {}
                Err(recovery_error) => {
                    eprintln!("Could not recover the updated launcher: {recovery_error}")
                }
            }
        }
        prerequisites::show_startup_error_dialog(&error.to_string());
        return;
    }

    let startup_started = Instant::now();
    let launcher_log_path = match logging::init_persistent() {
        Ok(path) => Some(path),
        Err(error) => {
            logging::init();
            eprintln!("could not initialize persistent launcher logs: {error}");
            None
        }
    };
    tracing::info!("------------------------------------------------------------");
    tracing::info!(
        "BahamutXIV Launcher v{}",
        bahamut_launcher::version::LAUNCHER_VERSION
    );
    match dirs::launcher_roots() {
        Ok(roots) => {
            tracing::info!("Install root = {}", roots.install.display());
            tracing::info!("State root = {}", roots.state.display());
        }
        Err(error) => tracing::warn!(%error, "launcher roots are unavailable"),
    }
    // Created here so the Wine code never creates it with a wider mode.
    if let Err(error) = dirs::ensure_data_dir() {
        tracing::warn!(%error, "launcher data directory is unavailable");
    }
    if let Some(log_path) = launcher_log_path.as_deref() {
        tracing::info!("Launcher log = {}", log_path.display());
    }
    tracing::info!("");
    tracing::info!("Startup: loading settings and launcher config.");

    match LauncherConfig::load() {
        Ok(config) => tracing::info!(
            path = %dirs::launcher_config_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "<unavailable>".into()),
            selected_server = %config.selected_server,
            "launcher configuration loaded"
        ),
        Err(err) => tracing::warn!(
            error = %err,
            "launcher configuration failed; repair <state-root>/config/bahamut.ini"
        ),
    }

    for (name, result) in [
        ("extensions.ini", load_extensions_config().map(|_| ())),
        ("dats.ini", load_dats_config().map(|_| ())),
        (
            "plugins/screenshot/settings.ini",
            load_screenshot_config().map(|_| ()),
        ),
    ] {
        match result {
            Ok(()) => tracing::info!(file = name, "extension configuration loaded"),
            Err(error) => tracing::warn!(
                file = name,
                error = %error,
                "extension configuration failed; repair the file under <state-root>/config"
            ),
        }
    }

    match news::bootstrap_default_if_missing() {
        Ok(true) => tracing::info!("wrote default news.toml on first run"),
        Ok(false) => {}
        Err(err) => tracing::warn!(
            error = %err,
            "news bootstrap failed; falling back to embedded default at runtime"
        ),
    }

    tracing::info!(
        "Startup: settings and launcher config loaded in {} ms.",
        startup_started.elapsed().as_millis()
    );
    tracing::info!("Startup: validating install layout.");
    diagnostics::log_support_diagnostics(launcher_log_path.as_deref());
    tracing::info!("Startup validation complete; creating main window.");
    tracing::info!("");

    let app = match tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let main_window_config = app
                .config()
                .app
                .windows
                .iter()
                .find(|window| window.label == "main")
                .cloned()
                .ok_or_else(|| std::io::Error::other("main window configuration is missing"))?;
            let webview_data_dir = dirs::webview_data_dir()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            tauri::WebviewWindowBuilder::from_config(app.handle(), &main_window_config)?
                .data_directory(webview_data_dir)
                .build()?;
            diagnostics::log_display_diagnostics(app.handle());
            tracing::info!(
                "Startup complete; main window created in {} ms.",
                startup_started.elapsed().as_millis()
            );
            Ok(())
        })
        .manage(PatcherIpcState::default())
        .manage(GameIpcState::default())
        .manage(BackupIpcState::default())
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle().clone();
                let state = app.state::<PatcherIpcState>();
                if continue_shutdown(app.clone(), &state, 0) {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            control_window,
            fit_window_to_work_area,
            get_launcher_update_status,
            check_launcher_update,
            launcher_update_restart_available,
            apply_launcher_update,
            get_home_status,
            get_extension_inventory,
            get_object_distance_selection,
            get_camera_zoom_selection,
            start_game_repair,
            game_repair_status,
            pause_game_repair,
            resume_game_repair,
            cancel_game_repair,
            set_addon_enabled,
            set_dat_package_enabled,
            reorder_dat_package,
            set_screenshot_enabled,
            set_discord_rpc_enabled,
            set_object_distance_selection,
            set_camera_zoom_selection,
            get_game_settings,
            set_game_settings,
            get_borderless_monitors,
            set_borderless_monitor,
            get_launcher_behavior,
            set_close_on_game_start,
            set_native_resolution_override,
            launch_config_tool,
            config_tool_supported,
            get_server_settings,
            set_selected_server,
            save_server_profile,
            launcher_version,
            detect_game_install_command,
            game_status,
            launch_game,
            login,
            register,
            validate_session,
            pick_install_dir,
            pick_directory,
            logout,
            list_news,
            open_external,
            open_launcher_log,
            create_backup,
            restore_backup,
            open_extension_folder,
            get_launcher_log,
            start_local_patch,
            start_patch_download,
            install_game,
            install_quote,
            cancel_patch,
            pause_patch,
            resume_patch,
            reset_patch,
            patch_status,
            get_patch_settings,
        ])
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(error) => {
            tracing::error!(%error, "error while building tauri application");
            if launcher_updates::has_update_transaction_argument() {
                match request_pending_update_recovery(true) {
                    Ok(true) => return,
                    Ok(false) => tracing::error!(
                        "updated launcher failed before startup and no recoverable transaction remains"
                    ),
                    Err(recovery_error) => tracing::error!(
                        %recovery_error,
                        "could not start recovery after updated launcher startup failed"
                    ),
                }
            }
            std::process::exit(1);
        }
    };
    if launcher_updates::has_update_transaction_argument() {
        let confirmation = dirs::state_root()
            .map_err(|error| error.to_string())
            .and_then(|root| launcher_updates::confirm_pending_launcher_update(&root));
        if let Err(error) = confirmation {
            tracing::error!(%error, "updated launcher could not confirm its signed installation");
            match request_pending_update_recovery(true) {
                Ok(true) => return,
                Ok(false) => tracing::error!(
                    "update confirmation failed and no recoverable transaction remains"
                ),
                Err(recovery_error) => tracing::error!(
                    %recovery_error,
                    "could not start recovery after update confirmation failed"
                ),
            }
            std::process::exit(1);
        }
    }
    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
            let state = app.state::<PatcherIpcState>();
            if continue_shutdown(app.clone(), &state, code.unwrap_or(0)) {
                api.prevent_exit();
            }
        }
    });
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests;
