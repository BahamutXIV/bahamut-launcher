// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use super::downloader::DownloadProgress;
use super::http::{CheckpointAction, DownloadCheckpoint};
use super::manifest::BasePackage;

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
    error_message: Mutex<Option<String>>,
    pause_requested: AtomicBool,
    paused: AtomicBool,
    pause_lock: Mutex<()>,
    pause_changed: Condvar,
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
            error_message: Mutex::new(None),
            pause_requested: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            pause_lock: Mutex::new(()),
            pause_changed: Condvar::new(),
            total_download_bytes: download_bytes,
            total_files: AtomicUsize::new(file_count),
        })
    }

    pub fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::Acquire))
    }

    pub fn error(&self) -> Option<String> {
        self.lock_error().clone()
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
        let _guard = self.pause_lock.lock().expect("pause mutex poisoned");
        if !self.is_terminal() {
            self.pause_requested.store(true, Ordering::Release);
        }
    }

    pub fn request_resume(&self) {
        let _guard = self.pause_lock.lock().expect("pause mutex poisoned");
        self.pause_requested.store(false, Ordering::Release);
        self.pause_changed.notify_all();
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    pub fn is_pause_requested(&self) -> bool {
        self.pause_requested.load(Ordering::Acquire)
    }

    /// Wait for Resume and report whether cancellation won while paused.
    pub(crate) fn wait_if_paused(&self) -> bool {
        let mut guard = self.pause_lock.lock().expect("pause mutex poisoned");
        while self.is_pause_requested() && !self.is_cancel_requested() {
            self.paused.store(true, Ordering::Release);
            guard = self
                .pause_changed
                .wait(guard)
                .expect("pause mutex poisoned while waiting");
        }
        self.paused.store(false, Ordering::Release);
        self.is_cancel_requested()
    }

    /// Return whether the worker reached a terminal phase.
    pub fn is_terminal(&self) -> bool {
        matches!(self.phase(), Phase::Done | Phase::Error | Phase::Cancelled)
    }

    pub(crate) fn set_phase(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Release);
        if self.is_terminal() {
            self.request_resume();
        }
    }

    pub fn fail(&self, message: impl Into<String>) {
        *self.lock_error() = Some(message.into());
        self.set_phase(Phase::Error);
    }

    fn lock_error(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.error_message.lock().expect("error mutex poisoned")
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
    if shared.is_cancel_requested() {
        return CheckpointAction::Cancel;
    }
    if shared.is_pause_requested() {
        if !checkpoint.durable {
            return CheckpointAction::Pause;
        }
        if shared.wait_if_paused() {
            return CheckpointAction::Cancel;
        }
    }
    CheckpointAction::Continue
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
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
}
