// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

use super::downloader::DownloadProgress;
use super::http::{CheckpointAction, DownloadCheckpoint};
use super::manifest::BasePackage;
use control::ControlState;

pub(crate) mod control;

/// Worker stage stored as a lock-free `u8` for UI observers.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Starting = 0,
    // Keep discriminants 1, 2, 6, and 7 unused for phase-value compatibility.
    Done = 3,
    Error = 4,
    Cancelled = 5,
    Downloading = 8,
    Installing = 9,
    ValidatingFiles = 10,
}

impl Phase {
    fn from_u8(value: u8) -> Self {
        match value {
            3 => Phase::Done,
            4 => Phase::Error,
            5 => Phase::Cancelled,
            8 => Phase::Downloading,
            9 => Phase::Installing,
            10 => Phase::ValidatingFiles,
            _ => Phase::Starting,
        }
    }
}

/// Worker/UI state: progress is public atomics; phase and error use accessors.
pub struct InstallShared {
    pub download: DownloadProgress,
    pub download_idx: AtomicUsize,
    pub previous_completed_bytes: AtomicU64,
    pub file_idx: AtomicUsize,
    phase: AtomicU8,
    control: ControlState,
    pub total_download_bytes: u64,
    pub total_files: AtomicUsize,
}

impl InstallShared {
    pub fn new() -> Arc<Self> {
        Self::with_totals(0, 0)
    }

    pub fn with_totals(download_bytes: u64, file_count: usize) -> Arc<Self> {
        Arc::new(Self {
            download: DownloadProgress::new(),
            download_idx: AtomicUsize::new(0),
            previous_completed_bytes: AtomicU64::new(0),
            file_idx: AtomicUsize::new(0),
            phase: AtomicU8::new(Phase::Starting as u8),
            control: ControlState::new(),
            total_download_bytes: download_bytes,
            total_files: AtomicUsize::new(file_count),
        })
    }

    pub fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::Acquire))
    }

    pub fn error(&self) -> Option<String> {
        self.control.error()
    }

    /// Request cancellation at the next chunk boundary.
    pub fn request_cancel(&self) {
        self.download.cancel();
        self.request_resume();
    }

    pub fn is_cancel_requested(&self) -> bool {
        self.download.is_cancelled()
    }

    /// Pause at the next safe worker boundary.
    pub fn request_pause(&self) {
        self.control.request_pause(|| self.is_terminal());
    }

    pub fn request_resume(&self) {
        self.control.request_resume();
    }

    pub fn is_paused(&self) -> bool {
        self.control.is_paused()
    }

    pub fn is_pause_requested(&self) -> bool {
        self.control.is_pause_requested()
    }

    /// Wait for Resume and report whether cancellation won while paused.
    pub(crate) fn wait_if_paused(&self) -> bool {
        self.control.wait_if_paused(|| self.is_cancel_requested())
    }

    /// Return whether the worker reached a terminal phase.
    pub fn is_terminal(&self) -> bool {
        matches!(self.phase(), Phase::Done | Phase::Error | Phase::Cancelled)
    }

    fn set_failure_context(&self, destination: &Path, cache: &Path, content_root: &str) {
        self.control
            .set_failure_context(destination, cache, content_root);
    }

    pub(crate) fn set_phase(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Release);
        if self.is_terminal() {
            self.request_resume();
        }
    }

    pub fn fail(&self, message: impl Into<String>) {
        let message = message.into();
        let context = self.control.failure_context();
        tracing::error!(
            action = "install",
            destination = %context.destination,
            cache = %context.cache,
            content_root = %context.content_root,
            diagnostic = %message,
            "client install failed"
        );
        self.control.set_error(message);
        self.set_phase(Phase::Error);
    }
}

/// One full-client install into `destination` from the content host.
pub struct InstallRequest {
    pub destination: PathBuf,
    pub content_root: String,
    pub cache_dir: PathBuf,
    pub package: BasePackage,
}

pub fn drive(shared: Arc<InstallShared>, request: InstallRequest) {
    shared.set_failure_context(
        &request.destination,
        &request.cache_dir,
        &request.content_root,
    );
    match super::installer::install(
        &shared,
        &request.destination,
        &request.content_root,
        &request.cache_dir,
        &request.package,
    ) {
        Ok(()) => shared.set_phase(Phase::Done),
        Err(_) if shared.is_cancel_requested() => shared.set_phase(Phase::Cancelled),
        Err(error) => shared.fail(error),
    }
}

pub(crate) fn download_checkpoint(
    shared: &InstallShared,
    checkpoint: DownloadCheckpoint,
) -> CheckpointAction {
    shared
        .control
        .download_checkpoint(shared.is_cancel_requested(), checkpoint.durable, || {
            shared.wait_if_paused()
        })
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, mpsc};
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn fresh_shared_is_in_starting_phase() {
        let s = InstallShared::new();
        assert_eq!(s.phase(), Phase::Starting);
        assert!(!s.is_terminal());
        assert_eq!(s.error(), None);
    }

    #[test]
    fn cancel_request_reaches_the_progress_flag() {
        let s = InstallShared::new();
        assert!(!s.is_cancel_requested());
        s.request_cancel();
        assert!(s.is_cancel_requested());
    }

    #[test]
    fn pause_resume_and_cancel_share_one_cooperative_gate() {
        let s = InstallShared::new();
        s.request_pause();
        assert!(s.is_pause_requested());
        assert!(!s.is_paused());
        s.request_resume();
        assert!(!s.is_paused());
        assert!(!s.is_pause_requested());
        s.request_pause();
        s.request_cancel();
        assert!(!s.is_paused());
        assert!(!s.is_pause_requested());
        assert!(s.is_cancel_requested());
    }

    #[test]
    fn pause_is_acknowledged_only_at_the_worker_boundary() {
        for cancel in [false, true] {
            let shared = InstallShared::new();
            shared.set_phase(Phase::Downloading);
            shared.request_pause();
            assert!(shared.is_pause_requested());
            assert!(!shared.is_paused());
            let worker_state = Arc::clone(&shared);
            let worker = thread::spawn(move || worker_state.wait_if_paused());
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !shared.is_paused() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "worker did not acknowledge pause"
                );
                thread::sleep(Duration::from_millis(1));
            }
            assert!(!worker.is_finished());
            if cancel {
                shared.request_cancel();
            } else {
                shared.request_resume();
            }
            assert_eq!(worker.join().unwrap(), cancel);
            assert!(!shared.is_paused());
            assert!(!shared.is_pause_requested());
        }
        let shared = InstallShared::new();
        shared.request_pause();
        shared.set_phase(Phase::Done);
        assert!(!shared.is_pause_requested());
        shared.request_pause();
        assert!(!shared.is_pause_requested());
    }

    #[test]
    fn cancelling_a_paused_waiter_wakes_the_worker() {
        let s = InstallShared::new();
        s.request_pause();
        let (done_tx, done_rx) = mpsc::channel();
        let worker_state = Arc::clone(&s);
        let waiter = thread::spawn(move || {
            let cancelled = worker_state.wait_if_paused();
            done_tx.send(cancelled).unwrap();
        });

        thread::yield_now();
        s.request_cancel();
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        waiter.join().unwrap();
    }

    #[test]
    fn fail_records_message_and_is_terminal() {
        let s = InstallShared::new();
        s.fail("simulated failure");
        assert_eq!(s.phase(), Phase::Error);
        assert_eq!(s.error().as_deref(), Some("simulated failure"));
        assert!(s.is_terminal());
    }

    #[test]
    fn phase_survives_a_u8_round_trip() {
        for phase in [
            Phase::Starting,
            Phase::Done,
            Phase::Error,
            Phase::Cancelled,
            Phase::Downloading,
            Phase::Installing,
            Phase::ValidatingFiles,
        ] {
            assert_eq!(Phase::from_u8(phase as u8), phase);
        }
    }

    fn poison<T: Send>(mutex: &Mutex<T>) {
        thread::scope(|scope| {
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
        let s = InstallShared::new();
        poison(s.control.pause_mutex());
        s.request_pause();
        assert!(s.is_pause_requested());
        s.request_resume();
        assert!(!s.is_pause_requested());
        assert!(!s.wait_if_paused());
        s.request_pause();
        s.request_cancel();
        assert!(s.is_cancel_requested());
        assert!(!s.is_pause_requested());
        assert!(s.wait_if_paused());
        s.set_phase(Phase::Cancelled);
    }

    #[test]
    fn poisoning_while_a_waiter_is_paused_still_wakes_it() {
        let s = InstallShared::new();
        s.request_pause();
        let (done_tx, done_rx) = mpsc::channel();
        let worker_state = Arc::clone(&s);
        let waiter = thread::spawn(move || {
            done_tx.send(worker_state.wait_if_paused()).unwrap();
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !s.is_paused() {
            assert!(std::time::Instant::now() < deadline, "waiter never paused");
            thread::sleep(Duration::from_millis(1));
        }
        poison(s.control.pause_mutex());
        s.request_cancel();
        assert!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        waiter.join().unwrap();
    }

    #[test]
    fn poisoned_error_mutex_does_not_break_fail_or_error() {
        let s = InstallShared::new();
        poison(s.control.error_mutex());
        s.fail("after poison");
        assert_eq!(s.error().as_deref(), Some("after poison"));
        assert_eq!(s.phase(), Phase::Error);
    }

    #[test]
    fn poisoned_failure_context_does_not_break_fail() {
        let s = InstallShared::new();
        poison(s.control.failure_context_mutex());
        s.set_failure_context(Path::new("dest"), Path::new("cache"), "root");
        s.fail("after poison");
        assert_eq!(s.error().as_deref(), Some("after poison"));
        assert_eq!(s.phase(), Phase::Error);
    }
}
