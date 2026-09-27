use bahamut_launcher::config::dirs;

use crate::launcher_updates::{self, LauncherUpdateResult, LauncherUpdateStatus};
use crate::state::{BackupIpcState, GameIpcState};

#[tauri::command]
pub(crate) fn get_launcher_update_status() -> LauncherUpdateStatus {
    match dirs::current_exe_dir() {
        Ok(root) => launcher_updates::get_launcher_update_status(&root),
        Err(error) => LauncherUpdateStatus {
            state: "blocked".into(),
            message: format!("Could not resolve the launcher directory: {error}"),
            installed_version: None,
            offered_version: None,
        },
    }
}

#[tauri::command]
pub(crate) async fn check_launcher_update() -> Result<LauncherUpdateStatus, String> {
    let root = dirs::current_exe_dir()
        .map_err(|error| format!("Could not resolve the launcher directory: {error}"))?;
    tauri::async_runtime::spawn_blocking(move || launcher_updates::check_launcher_update(&root))
        .await
        .map_err(|error| format!("Launcher update check worker failed: {error}"))?
}

#[tauri::command]
pub(crate) fn launcher_update_restart_available(
    game: tauri::State<'_, GameIpcState>,
    backups: tauri::State<'_, BackupIpcState>,
) -> bool {
    game.is_idle() && backups.is_idle()
}

#[tauri::command]
pub(crate) async fn apply_launcher_update(
    game: tauri::State<'_, GameIpcState>,
    backups: tauri::State<'_, BackupIpcState>,
) -> Result<LauncherUpdateResult, String> {
    let root = dirs::current_exe_dir()
        .map_err(|error| format!("Could not resolve the launcher directory: {error}"))?;
    let reservation = game.begin_restore().ok_or_else(|| {
        "A game, patch, install, launch, or restore operation is active. Wait for it to finish before updating the launcher.".to_owned()
    })?;
    let backup_operation = backups.begin()?;
    let (result, reservation, backup_operation) = tauri::async_runtime::spawn_blocking(move || {
        let result = launcher_updates::apply_launcher_update(&root);
        (result, reservation, backup_operation)
    })
    .await
    .map_err(|error| format!("Launcher update worker failed: {error}"))?;

    if result
        .as_ref()
        .is_ok_and(|result| result.state == "handoff")
    {
        // Seal both admission gates while the reservations still cover the app.
        game.request_shutdown();
        backups.request_shutdown();
    }
    drop(reservation);
    drop(backup_operation);
    result
}
