use std::thread;

use bahamut_launcher::patcher::content;
use bahamut_launcher::patcher::repair::{self, RepairPhase, RepairShared};
use serde::Serialize;

use crate::shell_config::{resolve_content_root, resolve_game_dir, resolve_patch_storage_dir};
use crate::state::{
    BackupIpcState, BackupReservation, GameIpcState, GameRepairRun, GameReservation,
    PatcherIpcState,
};

const NO_INSTALL_MSG: &str = "Game install not found. Select it on Home first.";
const REPAIR_BUSY_MSG: &str =
    "Close the game and wait for install, update, repair, or backup work to finish.";
const REPAIR_CLOSING_MSG: &str = "Launcher is closing.";
const REPAIR_STATE_POISONED_MSG: &str =
    "Game repair state is unavailable because its synchronization state was poisoned.";

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GameRepairStatusView {
    pub(crate) phase: &'static str,
    pub(crate) is_running: bool,
    pub(crate) is_terminal: bool,
    pub(crate) is_paused: bool,
    pub(crate) pause_requested: bool,
    pub(crate) cancel_requested: bool,
    pub(crate) can_cancel: bool,
    pub(crate) bytes_completed: u64,
    pub(crate) bytes_total: Option<u64>,
    pub(crate) files_completed: usize,
    pub(crate) files_total: Option<usize>,
    pub(crate) current_file: Option<String>,
    pub(crate) complete: bool,
    pub(crate) error: Option<String>,
}

impl GameRepairStatusView {
    fn idle() -> Self {
        Self {
            phase: "idle",
            is_running: false,
            is_terminal: false,
            is_paused: false,
            pause_requested: false,
            cancel_requested: false,
            can_cancel: false,
            bytes_completed: 0,
            bytes_total: None,
            files_completed: 0,
            files_total: None,
            current_file: None,
            complete: false,
            error: None,
        }
    }

    fn from_run(run: &GameRepairRun) -> Self {
        let phase = run.shared.phase();
        let worker_running = run
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished());
        Self {
            phase: repair_phase_label(phase),
            is_running: !phase.is_terminal() || worker_running,
            is_terminal: phase.is_terminal() && !worker_running,
            is_paused: run.shared.is_paused(),
            pause_requested: run.shared.is_pause_requested(),
            cancel_requested: run.shared.is_cancel_requested(),
            can_cancel: run.shared.can_cancel(),
            bytes_completed: run.shared.bytes_completed(),
            bytes_total: run.shared.bytes_total(),
            files_completed: run.shared.files_completed(),
            files_total: run.shared.files_total(),
            current_file: run.shared.current_file(),
            complete: phase == RepairPhase::Done,
            error: run.shared.error(),
        }
    }
}

fn repair_phase_label(phase: RepairPhase) -> &'static str {
    match phase {
        RepairPhase::Starting => "starting",
        RepairPhase::Recovering => "recovering",
        RepairPhase::Verifying => "verifying",
        RepairPhase::Downloading => "downloading",
        RepairPhase::Staging => "staging",
        RepairPhase::Publishing => "publishing",
        RepairPhase::FinalVerification => "final-verification",
        RepairPhase::Done => "done",
        RepairPhase::Error => "error",
        RepairPhase::Cancelled => "cancelled",
    }
}

fn begin_repair_reservations(
    game: &GameIpcState,
    backups: &BackupIpcState,
) -> Result<(GameReservation, BackupReservation), String> {
    let game_reservation = game
        .begin_restore()
        .ok_or_else(|| REPAIR_BUSY_MSG.to_owned())?;
    let backup_reservation = backups.begin()?;
    Ok((game_reservation, backup_reservation))
}

fn repair_package() -> Result<bahamut_launcher::patcher::content::BasePackage, String> {
    content::shipped_manifest()?
        .base
        .ok_or_else(|| "Game repair is not configured for this build.".to_owned())
}

fn clear_terminal_run(guard: &mut Option<GameRepairRun>) -> Result<(), String> {
    let Some(run) = guard.as_mut() else {
        return Ok(());
    };
    if let Some(worker) = &run.worker
        && !worker.is_finished()
    {
        return Err(REPAIR_BUSY_MSG.into());
    }
    if let Some(worker) = run.worker.take()
        && worker.join().is_err()
    {
        run.shared.fail("The game repair worker failed.");
    }
    *guard = None;
    Ok(())
}

/// Start one recover, verify, and repair pass over the complete managed-game inventory.
#[tauri::command]
pub(crate) fn start_game_repair(
    state: tauri::State<'_, PatcherIpcState>,
    game: tauri::State<'_, GameIpcState>,
    backups: tauri::State<'_, BackupIpcState>,
) -> Result<(), String> {
    if state.is_closing() {
        return Err(REPAIR_CLOSING_MSG.into());
    }
    let mut guard = state
        .repair
        .lock()
        .map_err(|_| REPAIR_STATE_POISONED_MSG.to_owned())?;
    clear_terminal_run(&mut guard)?;

    let (game_reservation, backup_reservation) = begin_repair_reservations(&game, &backups)?;
    let game_dir = resolve_game_dir().ok_or_else(|| NO_INSTALL_MSG.to_owned())?;
    let game_root = repair::canonical_game_root(&game_dir)?;
    let content_root = resolve_content_root()?;
    let cache = resolve_patch_storage_dir()?;
    let package = repair_package()?;
    let shared = RepairShared::new();
    let worker_shared = shared.clone();
    let worker = thread::Builder::new()
        .name("bahamut-game-repair".into())
        .spawn(move || {
            let _game_reservation = game_reservation;
            let _backup_reservation = backup_reservation;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                repair::repair_all(&game_root, &content_root, &cache, &package, &worker_shared)
            }));
            if result.is_err() {
                worker_shared.fail("The game repair worker failed.");
            }
        })
        .map_err(|error| format!("Could not start the game repair worker: {error}"))?;

    *guard = Some(GameRepairRun {
        shared,
        worker: Some(worker),
    });
    Ok(())
}

#[tauri::command]
pub(crate) fn game_repair_status(
    state: tauri::State<'_, PatcherIpcState>,
) -> Result<GameRepairStatusView, String> {
    let guard = state
        .repair
        .lock()
        .map_err(|_| REPAIR_STATE_POISONED_MSG.to_owned())?;
    Ok(guard
        .as_ref()
        .map(GameRepairStatusView::from_run)
        .unwrap_or_else(GameRepairStatusView::idle))
}

#[tauri::command]
pub(crate) fn pause_game_repair(state: tauri::State<'_, PatcherIpcState>) -> Result<(), String> {
    let guard = state
        .repair
        .lock()
        .map_err(|_| REPAIR_STATE_POISONED_MSG.to_owned())?;
    if let Some(run) = guard.as_ref() {
        run.shared.request_pause();
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn resume_game_repair(state: tauri::State<'_, PatcherIpcState>) -> Result<(), String> {
    if state.is_closing() {
        return Err(REPAIR_CLOSING_MSG.into());
    }
    let guard = state
        .repair
        .lock()
        .map_err(|_| REPAIR_STATE_POISONED_MSG.to_owned())?;
    if let Some(run) = guard.as_ref() {
        run.shared.request_resume();
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn cancel_game_repair(state: tauri::State<'_, PatcherIpcState>) -> Result<(), String> {
    let guard = state
        .repair
        .lock()
        .map_err(|_| REPAIR_STATE_POISONED_MSG.to_owned())?;
    if let Some(run) = guard.as_ref() {
        run.shared.request_cancel();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_reservation_blocks_repair_during_launch_or_install() {
        let game = GameIpcState::default();
        let backups = BackupIpcState::default();
        let _launch = game.begin_launch().unwrap();
        assert_eq!(
            begin_repair_reservations(&game, &backups).err().as_deref(),
            Some(REPAIR_BUSY_MSG)
        );
    }

    #[test]
    fn backup_reservation_blocks_repair() {
        let game = GameIpcState::default();
        let backups = BackupIpcState::default();
        let _backup = backups.begin().unwrap();
        assert!(begin_repair_reservations(&game, &backups).is_err());
        assert!(game.begin_launch().is_some());
    }

    #[test]
    fn repair_status_starts_idle_and_reports_a_terminal_run() {
        let idle = GameRepairStatusView::idle();
        assert_eq!(idle.phase, "idle");
        assert!(!idle.is_running);

        let shared = RepairShared::new();
        shared.fail("failed fixture repair");
        let run = GameRepairRun {
            shared,
            worker: None,
        };
        let status = GameRepairStatusView::from_run(&run);
        assert_eq!(status.phase, "error");
        assert!(status.is_terminal);
        assert_eq!(status.error.as_deref(), Some("failed fixture repair"));
    }
}
