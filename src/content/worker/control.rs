//! Pause, failure-report, and checkpoint state shared by the install and
//! repair workers. Each worker keeps its own phase and cancellation rules.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, LockResult, Mutex, MutexGuard, PoisonError, WaitTimeoutResult};
use std::time::Duration;

use crate::content::http::CheckpointAction;

/// Paths reported with a worker failure.
#[derive(Clone)]
pub(crate) struct FailureContext {
    pub(crate) destination: String,
    pub(crate) cache: String,
    pub(crate) content_root: String,
}

impl FailureContext {
    fn unknown() -> Self {
        Self {
            destination: "<unknown>".into(),
            cache: "<unknown>".into(),
            content_root: "<unknown>".into(),
        }
    }
}

/// Every lock below tolerates poisoning except `lock_pause_strict`, which lets
/// repair report an unusable control state instead.
pub(crate) struct ControlState {
    error: Mutex<Option<String>>,
    failure_context: Mutex<Option<FailureContext>>,
    pause_requested: AtomicBool,
    paused: AtomicBool,
    pause_lock: Mutex<()>,
    pause_changed: Condvar,
}

impl ControlState {
    pub(crate) fn new() -> Self {
        Self {
            error: Mutex::new(None),
            failure_context: Mutex::new(None),
            pause_requested: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            pause_lock: Mutex::new(()),
            pause_changed: Condvar::new(),
        }
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn set_error(&self, message: String) {
        *self.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(message);
    }

    pub(crate) fn set_failure_context(&self, destination: &Path, cache: &Path, content_root: &str) {
        *self
            .failure_context
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(FailureContext {
            destination: destination.display().to_string(),
            cache: cache.display().to_string(),
            content_root: content_root.to_owned(),
        });
    }

    /// The recorded context, or placeholders before the worker starts.
    pub(crate) fn failure_context(&self) -> FailureContext {
        self.failure_context
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(FailureContext::unknown)
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
    }

    pub(crate) fn is_pause_requested(&self) -> bool {
        self.pause_requested.load(Ordering::Acquire)
    }

    pub(crate) fn lock_pause(&self) -> MutexGuard<'_, ()> {
        self.pause_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn lock_pause_strict(&self) -> LockResult<MutexGuard<'_, ()>> {
        self.pause_lock.lock()
    }

    pub(crate) fn wait_timeout<'a>(
        &self,
        guard: MutexGuard<'a, ()>,
        timeout: Duration,
    ) -> LockResult<(MutexGuard<'a, ()>, WaitTimeoutResult)> {
        self.pause_changed.wait_timeout(guard, timeout)
    }

    /// Request a pause unless `is_terminal`, which is evaluated under the lock.
    pub(crate) fn request_pause(&self, is_terminal: impl FnOnce() -> bool) {
        let _guard = self.lock_pause();
        if !is_terminal() {
            self.pause_requested.store(true, Ordering::Release);
        }
    }

    pub(crate) fn request_resume(&self) {
        let guard = self.lock_pause();
        self.release_pause(&guard);
    }

    /// Clear the pause request and wake waiters while the caller holds the lock.
    pub(crate) fn release_pause(&self, _guard: &MutexGuard<'_, ()>) {
        self.pause_requested.store(false, Ordering::Release);
        self.pause_changed.notify_all();
    }

    /// Block while a pause is requested and `cancelled` is false; report
    /// whether cancellation is set on return.
    pub(crate) fn wait_if_paused(&self, cancelled: impl Fn() -> bool) -> bool {
        let mut guard = self.lock_pause();
        while self.is_pause_requested() && !cancelled() {
            self.set_paused(true);
            guard = self
                .pause_changed
                .wait(guard)
                .unwrap_or_else(PoisonError::into_inner);
        }
        self.set_paused(false);
        cancelled()
    }

    /// Map a download-chunk boundary to an action. `wait_cancelled` blocks
    /// through a durable pause and reports whether the worker should cancel.
    pub(crate) fn download_checkpoint(
        &self,
        cancelled: bool,
        durable: bool,
        wait_cancelled: impl FnOnce() -> bool,
    ) -> CheckpointAction {
        if cancelled {
            return CheckpointAction::Cancel;
        }
        if self.is_pause_requested() {
            if !durable {
                return CheckpointAction::Pause;
            }
            if wait_cancelled() {
                return CheckpointAction::Cancel;
            }
        }
        CheckpointAction::Continue
    }

    #[cfg(test)]
    pub(crate) fn error_mutex(&self) -> &Mutex<Option<String>> {
        &self.error
    }

    #[cfg(test)]
    pub(crate) fn failure_context_mutex(&self) -> &Mutex<Option<FailureContext>> {
        &self.failure_context
    }

    #[cfg(test)]
    pub(crate) fn pause_mutex(&self) -> &Mutex<()> {
        &self.pause_lock
    }
}
