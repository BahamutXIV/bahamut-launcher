//! Pause, cancel, and progress state for a running repair.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::content::http::CheckpointAction;
use crate::content::worker::control::ControlState;

const UNKNOWN_PROGRESS: u64 = u64::MAX;
const UNKNOWN_FILE_COUNT: usize = usize::MAX;
const PAUSE_POLL: Duration = Duration::from_millis(100);

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairPhase {
    Starting = 0,
    Recovering = 1,
    Verifying = 2,
    Downloading = 3,
    Staging = 4,
    Publishing = 5,
    FinalVerification = 6,
    Done = 7,
    Error = 8,
    Cancelled = 9,
}

impl RepairPhase {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Recovering,
            2 => Self::Verifying,
            3 => Self::Downloading,
            4 => Self::Staging,
            5 => Self::Publishing,
            6 => Self::FinalVerification,
            7 => Self::Done,
            8 => Self::Error,
            9 => Self::Cancelled,
            _ => Self::Starting,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Error | Self::Cancelled)
    }
}

/// Progress and controls that pause or cancel a game repair at safe checkpoints.
pub struct RepairShared {
    phase: AtomicU8,
    bytes_completed: AtomicU64,
    bytes_total: AtomicU64,
    files_completed: AtomicUsize,
    files_total: AtomicUsize,
    current_file: Mutex<Option<String>>,
    control: ControlState,
    cancel_requested: AtomicBool,
    cancel_allowed: AtomicBool,
}

impl RepairShared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            phase: AtomicU8::new(RepairPhase::Starting as u8),
            bytes_completed: AtomicU64::new(0),
            bytes_total: AtomicU64::new(UNKNOWN_PROGRESS),
            files_completed: AtomicUsize::new(0),
            files_total: AtomicUsize::new(UNKNOWN_FILE_COUNT),
            current_file: Mutex::new(None),
            control: ControlState::new(),
            cancel_requested: AtomicBool::new(false),
            cancel_allowed: AtomicBool::new(true),
        })
    }

    pub fn phase(&self) -> RepairPhase {
        RepairPhase::from_u8(self.phase.load(Ordering::Acquire))
    }

    pub fn bytes_completed(&self) -> u64 {
        self.bytes_completed.load(Ordering::Relaxed)
    }

    pub fn bytes_total(&self) -> Option<u64> {
        match self.bytes_total.load(Ordering::Relaxed) {
            UNKNOWN_PROGRESS => None,
            total => Some(total),
        }
    }

    pub fn files_completed(&self) -> usize {
        self.files_completed.load(Ordering::Relaxed)
    }

    pub fn files_total(&self) -> Option<usize> {
        match self.files_total.load(Ordering::Relaxed) {
            UNKNOWN_FILE_COUNT => None,
            total => Some(total),
        }
    }

    pub fn current_file(&self) -> Option<String> {
        self.current_file
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn error(&self) -> Option<String> {
        self.control.error()
    }

    pub fn is_paused(&self) -> bool {
        self.control.is_paused()
    }

    pub fn is_pause_requested(&self) -> bool {
        self.control.is_pause_requested()
    }

    pub fn is_cancel_requested(&self) -> bool {
        self.cancel_requested.load(Ordering::Acquire)
    }

    pub fn can_cancel(&self) -> bool {
        self.cancel_allowed.load(Ordering::Acquire) && !self.phase().is_terminal()
    }

    pub(super) fn set_failure_context(&self, destination: &Path, cache: &Path, content_root: &str) {
        self.control
            .set_failure_context(destination, cache, content_root);
    }

    pub fn request_pause(&self) {
        self.control.request_pause(|| self.phase().is_terminal());
    }

    pub fn request_resume(&self) {
        self.control.request_resume();
    }

    pub fn request_cancel(&self) {
        let guard = self.control.lock_pause();
        if self.cancel_allowed.load(Ordering::Acquire) && !self.phase().is_terminal() {
            self.cancel_requested.store(true, Ordering::Release);
            self.control.release_pause(&guard);
        }
    }

    pub(super) fn begin_phase(
        &self,
        phase: RepairPhase,
        bytes_total: Option<u64>,
        files_total: Option<usize>,
    ) {
        self.bytes_completed.store(0, Ordering::Release);
        self.bytes_total
            .store(bytes_total.unwrap_or(UNKNOWN_PROGRESS), Ordering::Release);
        self.files_completed.store(0, Ordering::Release);
        self.files_total
            .store(files_total.unwrap_or(UNKNOWN_FILE_COUNT), Ordering::Release);
        *self
            .current_file
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        self.phase.store(phase as u8, Ordering::Release);
        if phase.is_terminal() {
            self.request_resume();
        }
    }

    pub(super) fn set_current_file(&self, path: Option<&str>) {
        *self
            .current_file
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = path.map(str::to_owned);
    }

    pub(super) fn add_bytes(&self, bytes: u64) {
        self.bytes_completed.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(super) fn set_bytes(&self, bytes: u64) {
        self.bytes_completed.store(bytes, Ordering::Release);
    }

    pub(super) fn complete_file(&self) {
        self.files_completed.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn checkpoint(&self) -> Result<(), String> {
        let mut guard = self
            .control
            .lock_pause_strict()
            .map_err(|_| "Repair control state is unavailable.".to_owned())?;
        while self.control.is_pause_requested() && !self.cancel_requested.load(Ordering::Acquire) {
            self.control.set_paused(true);
            let wait = self
                .control
                .wait_timeout(guard, PAUSE_POLL)
                .map_err(|_| "Repair control state is unavailable.".to_owned())?;
            guard = wait.0;
        }
        self.control.set_paused(false);
        if self.cancel_requested.load(Ordering::Acquire) {
            Err("Game repair was cancelled.".into())
        } else {
            Ok(())
        }
    }

    pub(super) fn commit<T>(
        &self,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let mut action = Some(action);
        loop {
            self.checkpoint()?;
            let guard = self
                .control
                .lock_pause_strict()
                .map_err(|_| "Repair control state is unavailable.".to_owned())?;
            if self.cancel_requested.load(Ordering::Acquire) {
                return Err("Game repair was cancelled.".into());
            }
            if self.control.is_pause_requested() {
                drop(guard);
                continue;
            }
            let result = action.take().expect("repair commit action is available")();
            if result.is_ok() {
                self.cancel_allowed.store(false, Ordering::Release);
            }
            return result;
        }
    }

    pub(super) fn download_checkpoint(&self, durable: bool) -> CheckpointAction {
        self.control
            .download_checkpoint(self.is_cancel_requested(), durable, || {
                self.checkpoint().is_err()
            })
    }

    pub fn fail(&self, message: impl Into<String>) {
        let message = message.into();
        let context = self.control.failure_context();
        tracing::error!(
            action = "repair",
            destination = %context.destination,
            cache = %context.cache,
            content_root = %context.content_root,
            diagnostic = %message,
            "game repair failed"
        );
        self.control.set_error(message);
        self.finish_phase(RepairPhase::Error);
    }

    pub(super) fn finish_cancelled(&self) {
        self.finish_phase(RepairPhase::Cancelled);
    }

    pub(super) fn finish_phase(&self, phase: RepairPhase) {
        self.phase.store(phase as u8, Ordering::Release);
        if phase.is_terminal() {
            self.request_resume();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;

    use super::*;

    fn poison<T: Send>(mutex: &Mutex<T>) {
        std::thread::scope(|scope| {
            let _ = scope
                .spawn(|| {
                    let _guard = mutex.lock().unwrap();
                    panic!("poison the mutex");
                })
                .join();
        });
        assert!(mutex.is_poisoned());
    }

    #[test]
    fn poisoned_pause_lock_does_not_break_pause_controls() {
        let shared = RepairShared::new();
        poison(shared.control.pause_mutex());
        shared.request_pause();
        assert!(shared.is_pause_requested());
        shared.request_resume();
        assert!(!shared.is_pause_requested());
        shared.request_pause();
        shared.request_cancel();
        assert!(shared.is_cancel_requested());
        assert!(!shared.is_pause_requested());
        shared.fail("after poison");
        assert_eq!(shared.phase(), RepairPhase::Error);
    }

    #[test]
    fn poisoned_error_and_context_mutexes_do_not_break_fail() {
        let shared = RepairShared::new();
        poison(shared.control.error_mutex());
        poison(shared.control.failure_context_mutex());
        poison(&shared.current_file);
        shared.set_failure_context(Path::new("dest"), Path::new("cache"), "root");
        shared.fail("after poison");
        assert_eq!(shared.error().as_deref(), Some("after poison"));
        assert_eq!(shared.current_file(), None);
        assert_eq!(shared.phase(), RepairPhase::Error);
    }
}
