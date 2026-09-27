use std::path::PathBuf;
use std::sync::atomic::Ordering;

use bahamut_launcher::patcher::content::{self, PatchTransition};
use bahamut_launcher::patcher::{PatchSource, PatcherShared, Phase};

use crate::presentation::{
    AuthError, PatchSettingsView, PatchStatusView, patch_settings_view, phase_label,
};
use crate::shell_config::{
    load_preferences, resolve_content_root, resolve_game_dir, resolve_patch_storage_dir,
};
use crate::state::{BackupIpcState, GameIpcState, PatcherIpcState};

pub(crate) const PATCHER_BUSY_MSG: &str =
    "Install or patch work is already running. Wait for it to finish or cancel it first.";
pub(crate) const PATCHER_STATE_POISONED_MSG: &str =
    "Patcher state is unavailable because its synchronization state was poisoned.";
pub(crate) const PATCHER_CLOSING_MSG: &str = "Launcher is closing.";
const NO_INSTALL_MSG: &str = "Game install not found. Select it on Home first.";

pub(crate) fn spawn_patcher(
    state: &PatcherIpcState,
    game: &GameIpcState,
    backups: &BackupIpcState,
    game_dir: PathBuf,
    source: PatchSource,
) -> Result<(), AuthError> {
    let mut guard = state
        .patcher
        .lock()
        .map_err(|_| AuthError::server(PATCHER_STATE_POISONED_MSG))?;
    if state.is_closing() {
        return Err(AuthError::server(PATCHER_CLOSING_MSG));
    }
    clear_terminal_run(&mut guard)?;
    let reservation = game.begin_restore().ok_or_else(|| {
        AuthError::server("Close the game and wait for launch, installation, or restore to finish.")
    })?;
    let backup_reservation = backups.begin().map_err(AuthError::server)?;
    let installing = matches!(&source, PatchSource::Install { .. });
    let shared = match &source {
        PatchSource::Install { package, .. } => {
            let patch_bytes = if package.transition == PatchTransition::FullChain {
                bahamut_launcher::patcher::total_bytes()
            } else {
                0
            };
            let bytes = package
                .download_bytes()
                .map_err(AuthError::server)?
                .checked_add(patch_bytes)
                .ok_or_else(|| AuthError::server("Download size overflow."))?;
            PatcherShared::with_totals(
                bytes,
                package
                    .archives
                    .iter()
                    .map(|archive| archive.files.len())
                    .sum(),
            )
        }
        _ => PatcherShared::new(),
    };
    let worker_shared = shared.clone();
    let worker = std::thread::Builder::new().name("bahamut-content".into()).spawn(move || {
        let _reservation = reservation;
        let _backup_reservation = backup_reservation;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            bahamut_launcher::patcher::worker::drive(worker_shared.clone(), game_dir.clone(), source);
            if installing && worker_shared.phase() == Phase::Done
                && let Err(error) = super::install::persist_game_location(&game_dir) {
                    worker_shared.fail(format!("Installation finished at {} but selecting it failed: {error}. Select Existing to use it.", game_dir.display()));
            }
        }));
        if result.is_err() { worker_shared.fail("The install/update worker failed. Retry to resume verified downloads."); }
    }).map_err(|error| AuthError::server(format!("Could not start the install/update worker: {error}")))?;
    *guard = Some(crate::state::PatcherRun {
        shared,
        worker: Some(worker),
        installing,
    });
    Ok(())
}

fn clear_terminal_run(guard: &mut Option<crate::state::PatcherRun>) -> Result<(), AuthError> {
    let Some(run) = guard.as_mut() else {
        return Ok(());
    };
    if let Some(worker) = &run.worker
        && !worker.is_finished()
    {
        return Err(AuthError::server(PATCHER_BUSY_MSG));
    }
    if let Some(worker) = run.worker.take()
        && worker.join().is_err()
    {
        run.shared.fail("The install/update worker failed.");
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn start_patch_download(
    state: tauri::State<'_, PatcherIpcState>,
    game: tauri::State<'_, GameIpcState>,
    backups: tauri::State<'_, BackupIpcState>,
) -> Result<(), AuthError> {
    if !content::hosted_patches().map_err(AuthError::server)? {
        return Err(AuthError::server(
            "This content host has no retail patch objects. Install the final client into a new folder.",
        ));
    }
    let game_dir = resolve_game_dir().ok_or_else(|| AuthError::server(NO_INSTALL_MSG))?;
    let source = PatchSource::Remote {
        content_root: resolve_content_root().map_err(AuthError::server)?,
        cache_dir: resolve_patch_storage_dir().map_err(AuthError::server)?,
    };
    spawn_patcher(&state, &game, &backups, game_dir, source)
}

#[tauri::command]
pub(crate) fn start_local_patch(
    state: tauri::State<'_, PatcherIpcState>,
    game: tauri::State<'_, GameIpcState>,
    backups: tauri::State<'_, BackupIpcState>,
) -> Result<(), AuthError> {
    let game_dir = resolve_game_dir().ok_or_else(|| AuthError::server(NO_INSTALL_MSG))?;
    let storage = resolve_patch_storage_dir().map_err(AuthError::server)?;
    let source = PatchSource::LocalPayload {
        storage_dir: storage,
    };
    spawn_patcher(&state, &game, &backups, game_dir, source)
}

#[tauri::command]
pub(crate) async fn install_quote(
    destination: String,
) -> Result<bahamut_launcher::patcher::installer::InstallQuote, String> {
    let package = content::shipped_manifest()?.base.ok_or("Base-game installation is not configured for this build. Select Existing to use your client.")?;
    let cache = resolve_patch_storage_dir()?;
    resolve_content_root()?;
    tauri::async_runtime::spawn_blocking(move || {
        bahamut_launcher::patcher::installer::quote(&PathBuf::from(destination), &cache, &package)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) fn install_game(
    destination: String,
    state: tauri::State<'_, PatcherIpcState>,
    game: tauri::State<'_, GameIpcState>,
    backups: tauri::State<'_, BackupIpcState>,
) -> Result<(), AuthError> {
    let package = content::shipped_manifest().map_err(AuthError::server)?.base.ok_or_else(|| AuthError::server("Base-game installation is not configured for this build. Select Existing to use your client."))?;
    let source = PatchSource::Install {
        content_root: resolve_content_root().map_err(AuthError::server)?,
        cache_dir: resolve_patch_storage_dir().map_err(AuthError::server)?,
        package,
    };
    spawn_patcher(&state, &game, &backups, PathBuf::from(destination), source)
}

#[tauri::command]
pub(crate) async fn cancel_patch(state: tauri::State<'_, PatcherIpcState>) -> Result<(), String> {
    let guard = state
        .patcher
        .lock()
        .map_err(|_| PATCHER_STATE_POISONED_MSG.to_owned())?;
    if let Some(run) = &*guard {
        run.shared.request_cancel();
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn pause_patch(state: tauri::State<'_, PatcherIpcState>) -> Result<(), String> {
    let guard = state
        .patcher
        .lock()
        .map_err(|_| PATCHER_STATE_POISONED_MSG.to_owned())?;
    if let Some(run) = &*guard {
        run.shared.request_pause();
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn resume_patch(state: tauri::State<'_, PatcherIpcState>) -> Result<(), String> {
    if state.is_closing() {
        return Err(PATCHER_CLOSING_MSG.into());
    }
    let guard = state
        .patcher
        .lock()
        .map_err(|_| PATCHER_STATE_POISONED_MSG.to_owned())?;
    if let Some(run) = &*guard {
        run.shared.request_resume();
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn reset_patch(state: tauri::State<'_, PatcherIpcState>) -> Result<(), String> {
    let mut guard = state
        .patcher
        .lock()
        .map_err(|_| PATCHER_STATE_POISONED_MSG.to_owned())?;
    if state.is_closing() {
        return Err(PATCHER_CLOSING_MSG.into());
    }
    clear_terminal_run(&mut guard)
        .map_err(|error| error.message.unwrap_or_else(|| PATCHER_BUSY_MSG.into()))?;
    *guard = None;
    Ok(())
}

#[tauri::command]
pub(crate) async fn patch_status(
    state: tauri::State<'_, PatcherIpcState>,
) -> Result<PatchStatusView, String> {
    let guard = state
        .patcher
        .lock()
        .map_err(|_| PATCHER_STATE_POISONED_MSG.to_owned())?;
    Ok(match guard.as_ref() {
        Some(run) => {
            let shared = &run.shared;
            PatchStatusView {
                phase: phase_label(shared.phase()),
                is_install: run.installing,
                download_idx: shared.download_idx.load(Ordering::Acquire),
                patch_idx: shared.patch_idx.load(Ordering::Acquire),
                total_patches: shared.total_patches.load(Ordering::Acquire),
                bytes_downloaded: shared.download.bytes(),
                previous_completed_bytes: shared.previous_completed_bytes.load(Ordering::Acquire),
                total_download_bytes: shared.total_download_bytes,
                error: shared.error(),
                warnings: shared.warnings(),
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
        None => PatchStatusView::idle(),
    })
}

#[tauri::command]
pub(crate) async fn get_patch_settings() -> Result<PatchSettingsView, String> {
    patch_settings_view(&load_preferences()?)
}
