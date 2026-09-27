use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use bahamut_launcher::patcher::PatcherShared;
use bahamut_launcher::patcher::repair::RepairShared;
use bahamut_launcher::platform::LaunchedGame;

pub(crate) struct PatcherRun {
    pub(crate) shared: Arc<PatcherShared>,
    pub(crate) worker: Option<JoinHandle<()>>,
    pub(crate) installing: bool,
}

pub(crate) struct GameRepairRun {
    pub(crate) shared: Arc<RepairShared>,
    pub(crate) worker: Option<JoinHandle<()>>,
}

/// Patcher state and the close gate shared by IPC and native window events.
#[derive(Default)]
pub(crate) struct PatcherIpcState {
    pub(crate) patcher: Mutex<Option<PatcherRun>>,
    pub(crate) repair: Mutex<Option<GameRepairRun>>,
    closing: AtomicBool,
    shutdown_pending: AtomicBool,
}

pub(crate) struct ShutdownRequest {
    pub(crate) first_request: bool,
    pub(crate) workers: Vec<JoinHandle<()>>,
    pub(crate) waiting: bool,
}

impl ShutdownRequest {
    pub(crate) fn join_worker(self) -> bool {
        let mut panicked = false;
        for worker in self.workers {
            panicked |= worker.join().is_err();
        }
        panicked
    }
}

impl PatcherIpcState {
    pub(crate) fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }

    /// Requests cancellation and takes the worker for an off-thread join.
    pub(crate) fn request_shutdown(&self, other_work_pending: bool) -> ShutdownRequest {
        let first_request = self
            .closing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();

        if !first_request {
            return ShutdownRequest {
                first_request: false,
                workers: Vec::new(),
                waiting: self.shutdown_pending.load(Ordering::Acquire),
            };
        }

        self.shutdown_pending.store(true, Ordering::Release);
        let mut guard = self
            .patcher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let patch_worker = guard.as_mut().and_then(|run| {
            run.shared.request_cancel();
            run.worker.take()
        });
        drop(guard);
        let mut repair_guard = self
            .repair
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let repair_worker = repair_guard.as_mut().and_then(|run| {
            run.shared.request_cancel();
            run.worker.take()
        });
        let workers = [patch_worker, repair_worker]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let waiting = !workers.is_empty() || other_work_pending;
        self.shutdown_pending.store(waiting, Ordering::Release);
        ShutdownRequest {
            first_request,
            workers,
            waiting,
        }
    }

    pub(crate) fn mark_shutdown_complete(&self) {
        self.shutdown_pending.store(false, Ordering::Release);
    }
}

enum GameRun {
    Idle,
    Starting,
    Restoring,
    Running(LaunchedGame),
}

/// Owns the one game process started by this launcher instance.
pub(crate) struct GameIpcState {
    run: Arc<Mutex<GameRun>>,
    completed: Arc<Condvar>,
    closing: AtomicBool,
}

impl Default for GameIpcState {
    fn default() -> Self {
        Self {
            run: Arc::new(Mutex::new(GameRun::Idle)),
            completed: Arc::new(Condvar::new()),
            closing: AtomicBool::new(false),
        }
    }
}

impl GameIpcState {
    pub(crate) fn begin_launch(&self) -> Option<GameReservation> {
        self.reserve(GameRun::Starting)
    }

    pub(crate) fn begin_restore(&self) -> Option<GameReservation> {
        self.reserve(GameRun::Restoring)
    }

    fn reserve(&self, operation: GameRun) -> Option<GameReservation> {
        let mut run = self
            .run
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.closing.load(Ordering::Acquire) {
            return None;
        }
        if matches!(&*run, GameRun::Running(game) if game.has_exited()) {
            *run = GameRun::Idle;
        }
        if matches!(&*run, GameRun::Idle) {
            *run = operation;
            Some(GameReservation {
                run: Arc::clone(&self.run),
                completed: Arc::clone(&self.completed),
                published: false,
            })
        } else {
            None
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        let mut run = self
            .run
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(&*run, GameRun::Running(game) if game.has_exited()) {
            *run = GameRun::Idle;
        }
        matches!(&*run, GameRun::Starting | GameRun::Running(_))
    }

    pub(crate) fn is_idle(&self) -> bool {
        let mut run = self
            .run
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(&*run, GameRun::Running(game) if game.has_exited()) {
            *run = GameRun::Idle;
        }
        !self.closing.load(Ordering::Acquire) && matches!(&*run, GameRun::Idle)
    }

    /// Seal admission before deciding whether native close must wait.
    pub(crate) fn request_shutdown(&self) -> bool {
        let run = self.run.lock().unwrap_or_else(|error| error.into_inner());
        self.closing.store(true, Ordering::Release);
        matches!(&*run, GameRun::Starting | GameRun::Restoring)
    }

    pub(crate) fn wait_for_worker(&self) {
        let mut run = self.run.lock().unwrap_or_else(|error| error.into_inner());
        while matches!(&*run, GameRun::Starting | GameRun::Restoring) {
            run = self
                .completed
                .wait(run)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

/// The blocking worker owns the reservation even if its IPC future is dropped.
pub(crate) struct GameReservation {
    run: Arc<Mutex<GameRun>>,
    completed: Arc<Condvar>,
    published: bool,
}

impl GameReservation {
    pub(crate) fn complete_launch(mut self, game: LaunchedGame) {
        let mut run = self
            .run
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(&*run, GameRun::Starting) {
            *run = GameRun::Running(game);
            // An immediately exited game can admit a new reservation after this lock drops.
            self.published = true;
        }
    }
}

impl Drop for GameReservation {
    fn drop(&mut self) {
        let mut run = self
            .run
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.published && matches!(&*run, GameRun::Starting | GameRun::Restoring) {
            *run = GameRun::Idle;
        }
        self.completed.notify_all();
    }
}

#[derive(Default)]
struct BackupOperation {
    active: bool,
    closing: bool,
}

/// Archive creation, restore, and HUD reset share paths and must finish before exit.
#[derive(Default)]
pub(crate) struct BackupIpcState {
    operation: Arc<Mutex<BackupOperation>>,
    completed: Arc<Condvar>,
}

impl BackupIpcState {
    pub(crate) fn begin(&self) -> Result<BackupReservation, String> {
        let mut operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if operation.closing {
            return Err("The launcher is closing. Wait for it to finish.".to_owned());
        }
        if operation.active {
            return Err("A backup or HUD reset is in progress. Wait for it to finish.".to_owned());
        }
        operation.active = true;
        Ok(BackupReservation {
            operation: Arc::clone(&self.operation),
            completed: Arc::clone(&self.completed),
        })
    }

    pub(crate) fn is_idle(&self) -> bool {
        let operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        !operation.closing && !operation.active
    }

    pub(crate) fn request_shutdown(&self) -> bool {
        let mut operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        operation.closing = true;
        operation.active
    }

    pub(crate) fn wait_for_worker(&self) {
        let mut operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while operation.active {
            operation = self
                .completed
                .wait(operation)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

pub(crate) struct BackupReservation {
    operation: Arc<Mutex<BackupOperation>>,
    completed: Arc<Condvar>,
}

impl Drop for BackupReservation {
    fn drop(&mut self) {
        let mut operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        operation.active = false;
        self.completed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_is_available_only_when_game_and_backup_operations_are_idle() {
        let game = GameIpcState::default();
        let backups = BackupIpcState::default();
        assert!(game.is_idle());
        assert!(backups.is_idle());

        let game_reservation = game.begin_restore().unwrap();
        assert!(!game.is_idle());
        drop(game_reservation);

        let backup_reservation = backups.begin().unwrap();
        assert!(!backups.is_idle());
        drop(backup_reservation);
        assert!(game.is_idle());
        assert!(backups.is_idle());

        game.request_shutdown();
        assert!(!game.is_idle());
    }

    #[test]
    fn completed_launch_guard_cannot_release_a_successor() {
        for restore in [false, true] {
            let state = GameIpcState::default();
            let (game, exited) = LaunchedGame::pending(42);
            *state.run.lock().unwrap() = GameRun::Running(game);
            // Model completion after publication but before the old guard's destructor.
            let old = GameReservation {
                run: Arc::clone(&state.run),
                completed: Arc::clone(&state.completed),
                published: true,
            };
            exited.send(()).unwrap();
            let successor = if restore {
                state.begin_restore()
            } else {
                state.begin_launch()
            }
            .unwrap();
            drop(old);
            assert!(state.begin_launch().is_none());
            assert!(state.begin_restore().is_none());
            drop(successor);
            assert!(state.begin_launch().is_some());
        }
    }
}
