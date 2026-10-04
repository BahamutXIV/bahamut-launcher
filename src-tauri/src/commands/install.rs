use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use bahamut_launcher::content::manifest;
use bahamut_launcher::content::worker::InstallRequest;
use bahamut_launcher::content::{InstallShared, Phase};

use crate::presentation::{AuthError, GameInstallInfo, InstallStatusView, phase_label};
use crate::shell_config::{
    resolve_content_root, resolve_download_cache_dir, resolve_game_dir_with_source,
    update_launcher_config,
};
use crate::state::{BackupIpcState, ContentIpcState, GameIpcState, InstallRun};

pub(crate) const INSTALL_BUSY_MSG: &str =
    "An install or repair is already running. Wait for it to finish or cancel it first.";
pub(crate) const CONTENT_STATE_POISONED_MSG: &str =
    "Install state is unavailable because its synchronization state was poisoned.";
pub(crate) const CONTENT_CLOSING_MSG: &str = "Launcher is closing.";
const NO_BASE_PACKAGE_MSG: &str = "Game installation is not configured for this build. Choose an existing game folder in Settings > Misc > Install Location.";

#[tauri::command]
pub(crate) fn detect_game_install_command() -> GameInstallInfo {
    let (path, source) = resolve_game_dir_with_source();
    GameInstallInfo {
        detected: path.map(|p| p.to_string_lossy().into_owned()),
        source: source.into(),
    }
}

/// Async so the callback result can be awaited off the runtime; blocking_pick_folder deadlocks here.
#[tauri::command]
pub(crate) async fn pick_install_dir(
    app: tauri::AppHandle,
    state: tauri::State<'_, GameIpcState>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |picked| {
        let _ = tx.send(picked);
    });

    let picked = tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .map_err(|error| format!("Folder selection failed: {error}"))?;
    let Some(picked) = picked else {
        return Ok(None);
    };

    let path = picked.into_path().map_err(|error| error.to_string())?;

    let _reservation = state.begin_restore().ok_or("Wait for game, install, launch, or restore work to finish before selecting another folder.")?;

    persist_game_location(&path)?;

    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Async for the same main-thread dialog constraint as [`pick_install_dir`]; it does not persist `game_location`.
#[tauri::command]
pub(crate) async fn pick_directory(app: tauri::AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |picked| {
        let _ = tx.send(picked);
    });

    let picked = tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .ok()
        .flatten()?;

    let path = picked.into_path().ok()?;
    Some(path.to_string_lossy().into_owned())
}

/// Persists `game_location` without replacing other preference fields.
pub(crate) fn persist_game_location(path: &Path) -> Result<(), String> {
    update_launcher_config(|config| {
        config.preferences.launcher.game_location = Some(path.to_path_buf());
        Ok(())
    })
    .map(|_| ())
}

pub(crate) fn spawn_installer(
    state: &ContentIpcState,
    game: &GameIpcState,
    backups: &BackupIpcState,
    request: InstallRequest,
) -> Result<(), AuthError> {
    let mut guard = state
        .install
        .lock()
        .map_err(|_| AuthError::server(CONTENT_STATE_POISONED_MSG))?;
    if state.is_closing() {
        return Err(AuthError::server(CONTENT_CLOSING_MSG));
    }
    clear_terminal_run(&mut guard)?;
    let reservation = game.begin_restore().ok_or_else(|| {
        AuthError::server("Close the game and wait for launch, installation, or restore to finish.")
    })?;
    let backup_reservation = backups.begin().map_err(AuthError::server)?;
    let shared = InstallShared::with_totals(
        request
            .package
            .download_bytes()
            .map_err(AuthError::server)?,
        request
            .package
            .archives
            .iter()
            .map(|archive| archive.files.len())
            .sum(),
    );
    let worker_shared = shared.clone();
    let worker = std::thread::Builder::new()
        .name("bahamut-content".into())
        .spawn(move || {
            let _reservation = reservation;
            let _backup_reservation = backup_reservation;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let destination = request.destination.clone();
                bahamut_launcher::content::worker::drive(worker_shared.clone(), request);
                if worker_shared.phase() == Phase::Done
                    && let Err(error) = persist_game_location(&destination)
                {
                    worker_shared.fail(format!(
                        "Installation finished at {} but selecting it failed: {error}. Choose that folder in Settings > Misc > Install Location.",
                        destination.display()
                    ));
                }
            }));
            if result.is_err() {
                worker_shared.fail("The install worker failed. Retry to resume verified downloads.");
            }
        })
        .map_err(|error| AuthError::server(format!("Could not start the install worker: {error}")))?;
    *guard = Some(InstallRun {
        shared,
        worker: Some(worker),
    });
    Ok(())
}

fn clear_terminal_run(guard: &mut Option<InstallRun>) -> Result<(), AuthError> {
    let Some(run) = guard.as_mut() else {
        return Ok(());
    };
    if let Some(worker) = &run.worker
        && !worker.is_finished()
    {
        return Err(AuthError::server(INSTALL_BUSY_MSG));
    }
    if let Some(worker) = run.worker.take()
        && worker.join().is_err()
    {
        run.shared.fail("The install worker failed.");
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn install_quote(
    destination: String,
) -> Result<bahamut_launcher::content::installer::InstallQuote, String> {
    let package = manifest::shipped_manifest()?
        .base
        .ok_or(NO_BASE_PACKAGE_MSG)?;
    let cache = resolve_download_cache_dir()?;
    resolve_content_root()?;
    tauri::async_runtime::spawn_blocking(move || {
        bahamut_launcher::content::installer::quote(&PathBuf::from(destination), &cache, &package)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) fn install_game(
    destination: String,
    state: tauri::State<'_, ContentIpcState>,
    game: tauri::State<'_, GameIpcState>,
    backups: tauri::State<'_, BackupIpcState>,
) -> Result<(), AuthError> {
    let package = manifest::shipped_manifest()
        .map_err(AuthError::server)?
        .base
        .ok_or_else(|| AuthError::server(NO_BASE_PACKAGE_MSG))?;
    let request = InstallRequest {
        destination: PathBuf::from(destination),
        content_root: resolve_content_root().map_err(AuthError::server)?,
        cache_dir: resolve_download_cache_dir().map_err(AuthError::server)?,
        package,
    };
    spawn_installer(&state, &game, &backups, request)
}

#[tauri::command]
pub(crate) async fn cancel_install(state: tauri::State<'_, ContentIpcState>) -> Result<(), String> {
    let guard = state
        .install
        .lock()
        .map_err(|_| CONTENT_STATE_POISONED_MSG.to_owned())?;
    if let Some(run) = &*guard {
        run.shared.request_cancel();
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn pause_install(state: tauri::State<'_, ContentIpcState>) -> Result<(), String> {
    let guard = state
        .install
        .lock()
        .map_err(|_| CONTENT_STATE_POISONED_MSG.to_owned())?;
    if let Some(run) = &*guard {
        run.shared.request_pause();
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn resume_install(state: tauri::State<'_, ContentIpcState>) -> Result<(), String> {
    if state.is_closing() {
        return Err(CONTENT_CLOSING_MSG.into());
    }
    let guard = state
        .install
        .lock()
        .map_err(|_| CONTENT_STATE_POISONED_MSG.to_owned())?;
    if let Some(run) = &*guard {
        run.shared.request_resume();
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn reset_install(state: tauri::State<'_, ContentIpcState>) -> Result<(), String> {
    let mut guard = state
        .install
        .lock()
        .map_err(|_| CONTENT_STATE_POISONED_MSG.to_owned())?;
    if state.is_closing() {
        return Err(CONTENT_CLOSING_MSG.into());
    }
    clear_terminal_run(&mut guard)
        .map_err(|error| error.message.unwrap_or_else(|| INSTALL_BUSY_MSG.into()))?;
    *guard = None;
    Ok(())
}

#[tauri::command]
pub(crate) async fn install_status(
    state: tauri::State<'_, ContentIpcState>,
) -> Result<InstallStatusView, String> {
    let guard = state
        .install
        .lock()
        .map_err(|_| CONTENT_STATE_POISONED_MSG.to_owned())?;
    Ok(match guard.as_ref() {
        Some(run) => {
            let shared = &run.shared;
            InstallStatusView {
                phase: phase_label(shared.phase()),
                download_idx: shared.download_idx.load(Ordering::Acquire),
                file_idx: shared.file_idx.load(Ordering::Acquire),
                total_files: shared.total_files.load(Ordering::Acquire),
                bytes_downloaded: shared.download.bytes(),
                previous_completed_bytes: shared.previous_completed_bytes.load(Ordering::Acquire),
                total_download_bytes: shared.total_download_bytes,
                error: shared.error(),
                is_running: !shared.is_terminal()
                    || run
                        .worker
                        .as_ref()
                        .is_some_and(|worker| !worker.is_finished()),
                is_paused: shared.is_paused(),
                pause_requested: shared.is_pause_requested(),
                is_terminal: shared.is_terminal()
                    && run
                        .worker
                        .as_ref()
                        .is_none_or(|worker| worker.is_finished()),
            }
        }
        None => InstallStatusView::idle(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_package_message_points_to_install_location() {
        assert_eq!(
            NO_BASE_PACKAGE_MSG,
            "Game installation is not configured for this build. Choose an existing game folder in Settings > Misc > Install Location."
        );
    }
}
