// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crc32fast::Hasher as Crc32Hasher;

use super::content::{BasePackage, patch_objects};
use super::downloader::{DownloadProgress, IO_CHUNK};
use super::extract::{ExtractError, extract_manifest_patches};
use super::http::{CheckpointAction, DownloadCheckpoint, download_object};
use super::manifest::{PATCH_MANIFEST, PatchEntry, expected_sha256, leaf_of, total_bytes};
use super::process::{PatchPlan, check_game_version, write_version_files};
use crate::version::FFXIV_GAME_VERSION;
use sha2::{Digest, Sha256};

/// Where the worker sources the patch files from.
#[derive(Debug)]
pub enum PatchSource {
    /// Use a directory of patches after size, CRC32, and SHA-256 validation.
    Local {
        source_dir: PathBuf,
    },
    /// Extract manifest patches from a local archive, then apply them.
    LocalZip {
        zip_path: PathBuf,
    },
    LocalPayload {
        storage_dir: PathBuf,
    },
    Remote {
        content_root: String,
        cache_dir: PathBuf,
    },
    Install {
        content_root: String,
        cache_dir: PathBuf,
        package: BasePackage,
    },
}

/// Worker stage stored as a lock-free `u8` for UI observers.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Starting = 0,
    // Keep discriminant 1 unused for phase-value compatibility.
    Patching = 2,
    Done = 3,
    Error = 4,
    Cancelled = 5,
    Validating = 6,
    Extracting = 7,
    Downloading = 8,
    Installing = 9,
    ValidatingFiles = 10,
}

impl Phase {
    fn from_u8(value: u8) -> Self {
        match value {
            2 => Phase::Patching,
            3 => Phase::Done,
            4 => Phase::Error,
            5 => Phase::Cancelled,
            6 => Phase::Validating,
            7 => Phase::Extracting,
            8 => Phase::Downloading,
            9 => Phase::Installing,
            10 => Phase::ValidatingFiles,
            _ => Phase::Starting,
        }
    }
}

/// Worker/UI state: progress is public atomics; phase, error, and warnings use accessors.
pub struct PatcherShared {
    pub download: DownloadProgress,
    pub download_idx: AtomicUsize,
    pub previous_completed_bytes: AtomicU64,
    pub patch_idx: AtomicUsize,
    phase: AtomicU8,
    error_message: Mutex<Option<String>>,
    warnings: Mutex<Vec<String>>,
    pause_requested: AtomicBool,
    paused: AtomicBool,
    pause_lock: Mutex<()>,
    pause_changed: Condvar,
    pub total_download_bytes: u64,
    pub total_patches: AtomicUsize,
}

impl PatcherShared {
    pub fn new() -> Arc<Self> {
        Self::with_totals(total_bytes(), PATCH_MANIFEST.len())
    }

    pub fn with_totals(download_bytes: u64, file_count: usize) -> Arc<Self> {
        Arc::new(Self {
            download: DownloadProgress::new(),
            download_idx: AtomicUsize::new(0),
            previous_completed_bytes: AtomicU64::new(0),
            patch_idx: AtomicUsize::new(0),
            phase: AtomicU8::new(Phase::Starting as u8),
            error_message: Mutex::new(None),
            warnings: Mutex::new(Vec::new()),
            pause_requested: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            pause_lock: Mutex::new(()),
            pause_changed: Condvar::new(),
            total_download_bytes: download_bytes,
            total_patches: AtomicUsize::new(file_count),
        })
    }

    pub fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::Acquire))
    }

    pub fn error(&self) -> Option<String> {
        self.lock_error().clone()
    }

    pub fn warnings(&self) -> Vec<String> {
        self.lock_warnings().clone()
    }

    /// Request cancellation at the next chunk or patch boundary.
    pub fn request_cancel(&self) {
        self.download.cancel();
        self.request_resume();
    }

    pub fn is_cancel_requested(&self) -> bool {
        self.download.is_cancelled()
    }

    /// Pause at the next safe worker boundary; an in-flight patch file finishes first.
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

    fn warn(&self, message: impl Into<String>) {
        self.lock_warnings().push(message.into());
    }

    fn lock_error(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.error_message.lock().expect("error mutex poisoned")
    }

    fn lock_warnings(&self) -> std::sync::MutexGuard<'_, Vec<String>> {
        self.warnings.lock().expect("warnings mutex poisoned")
    }
}

pub fn drive(shared: Arc<PatcherShared>, game_dir: PathBuf, source: PatchSource) {
    let source = if let PatchSource::LocalPayload { storage_dir } = source {
        match super::extract::find_patch_payload(&storage_dir) {
            Some(super::extract::PatchPayload::Zip(zip_path)) => PatchSource::LocalZip { zip_path },
            Some(super::extract::PatchPayload::PatchDir(source_dir)) => {
                PatchSource::Local { source_dir }
            }
            None => {
                shared.fail("No local patch files or supported patch archive were found in the selected folder.");
                return;
            }
        }
    } else {
        source
    };
    if let PatchSource::Install {
        content_root,
        cache_dir,
        package,
    } = source
    {
        match super::installer::install(&shared, &game_dir, &content_root, &cache_dir, &package) {
            Ok(()) => shared.set_phase(Phase::Done),
            Err(_) if shared.is_cancel_requested() => shared.set_phase(Phase::Cancelled),
            Err(error) => shared.fail(error),
        }
        return;
    }
    if super::installation_in_progress(&game_dir) {
        shared.fail("This folder contains an unfinished installation. Retry Install for that destination before applying patches.");
        return;
    }
    // Short-circuit only when target game.ver and ffxivgame.exe both exist; delete game.ver to force a rerun.
    if check_game_version(&game_dir) && game_dir.join("ffxivgame.exe").is_file() {
        shared.warn(format!(
            "Game is already at version {FFXIV_GAME_VERSION}; no patches applied. \
             Delete <game>/game.ver to force a re-apply."
        ));
        shared.set_phase(Phase::Done);
        return;
    }

    let mut staging_cleanup = StagingCleanup {
        shared: shared.clone(),
        staging_dir: None,
    };

    if shared.wait_if_paused() {
        shared.set_phase(Phase::Cancelled);
        return;
    }

    let plan = match source {
        PatchSource::Local { source_dir } => match verify_local(&shared, &source_dir) {
            Some(plan) => plan,
            None => return,
        },
        PatchSource::LocalZip { zip_path } => {
            match extract_and_verify(&shared, &zip_path, &mut staging_cleanup) {
                Some(plan) => plan,
                None => return,
            }
        }
        PatchSource::Remote {
            content_root,
            cache_dir,
        } => match download_patches(&shared, &content_root, &cache_dir) {
            Ok(plan) => plan,
            Err(_) if shared.is_cancel_requested() => {
                shared.set_phase(Phase::Cancelled);
                return;
            }
            Err(error) => {
                shared.fail(error);
                return;
            }
        },
        PatchSource::Install { .. } => unreachable!("install handled above"),
        PatchSource::LocalPayload { .. } => unreachable!("local discovery handled above"),
    };

    if let Err(error) = apply_plan(&shared, &game_dir, &plan) {
        if shared.is_cancel_requested() {
            shared.set_phase(Phase::Cancelled);
        } else {
            shared.fail(error);
        }
        return;
    }
    shared.set_phase(Phase::Done);
}

pub(crate) fn apply_plan(
    shared: &Arc<PatcherShared>,
    game_dir: &Path,
    plan: &PatchPlan,
) -> Result<(), String> {
    shared
        .total_patches
        .store(plan.patches_in_order.len(), Ordering::Release);
    shared.patch_idx.store(0, Ordering::Release);
    shared.set_phase(Phase::Patching);
    match fs::remove_file(game_dir.join("game.ver")) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Could not invalidate the game version: {error}")),
    }
    for (idx, patch) in plan.patches_in_order.iter().enumerate() {
        if shared.wait_if_paused() {
            return Err("Patching was cancelled.".into());
        }
        shared.patch_idx.store(idx, Ordering::Release);

        match crate::patch_format::apply_patch_file(patch, game_dir) {
            Ok(report) => {
                for message in report.messages {
                    shared.warn(message);
                }
            }
            Err(e) => {
                return Err(format!(
                    "Applying patch {} failed: {e}",
                    display_leaf(patch)
                ));
            }
        }
    }

    write_version_files(game_dir).map_err(|e| format!("Could not stamp the version files: {e}"))
}

pub(crate) fn download_checkpoint(
    shared: &PatcherShared,
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

pub(crate) fn download_patches(
    shared: &Arc<PatcherShared>,
    root: &str,
    cache: &Path,
) -> Result<PatchPlan, String> {
    shared.set_phase(Phase::Downloading);
    let mut patches = Vec::new();
    for (idx, (entry, object)) in PATCH_MANIFEST.iter().zip(patch_objects()).enumerate() {
        shared.set_phase(Phase::Downloading);
        shared.download_idx.store(idx, Ordering::Release);
        let path = download_object(
            root,
            &object,
            cache,
            |checkpoint| download_checkpoint(shared, checkpoint),
            |bytes, _total| {
                shared
                    .download
                    .bytes_downloaded
                    .store(bytes, Ordering::Release)
            },
        )
        .map_err(|error| error.to_string())?;
        shared.set_phase(Phase::Validating);
        shared.download.bytes_downloaded.store(0, Ordering::Release);
        match verify_one(&path, entry, shared) {
            VerifyOutcome::Ok => {}
            VerifyOutcome::Cancelled => return Err("Patching was cancelled.".into()),
            _ => {
                return Err(format!(
                    "Downloaded patch {} failed integrity verification.",
                    entry.path
                ));
            }
        }
        patches.push((leaf_of(entry.path), path));
        shared
            .previous_completed_bytes
            .fetch_add(entry.size, Ordering::Release);
        shared.download.bytes_downloaded.store(0, Ordering::Release);
    }
    patches.sort_by_key(|(leaf, _)| *leaf);
    Ok(PatchPlan {
        patches_in_order: patches.into_iter().map(|(_, path)| path).collect(),
    })
}

/// Best-effort cleanup guard for LocalZip staging on every terminal path.
struct StagingCleanup {
    shared: Arc<PatcherShared>,
    staging_dir: Option<PathBuf>,
}

impl Drop for StagingCleanup {
    fn drop(&mut self) {
        let Some(dir) = self.staging_dir.take() else {
            return;
        };
        if let Err(e) = fs::remove_dir_all(&dir)
            && e.kind() != io::ErrorKind::NotFound
        {
            self.shared
                .warn(format!("Could not remove the patch staging directory: {e}"));
        }
    }
}

/// Extract and validate a LocalZip source, recording staging for cleanup; cancellation or failure sets a terminal phase.
fn extract_and_verify(
    shared: &Arc<PatcherShared>,
    zip_path: &Path,
    cleanup: &mut StagingCleanup,
) -> Option<PatchPlan> {
    shared.set_phase(Phase::Extracting);

    let staging = match crate::config::dirs::patch_staging_dir() {
        Ok(dir) => dir,
        Err(e) => {
            shared.fail(format!(
                "Could not resolve the patch staging directory: {e}"
            ));
            return None;
        }
    };
    cleanup.staging_dir = Some(staging.clone());

    if let Err(e) = extract_manifest_patches(zip_path, &staging, shared) {
        match e {
            ExtractError::Cancelled => shared.set_phase(Phase::Cancelled),
            other => shared.fail(format!("Could not unpack the patch archive: {other}")),
        }
        return None;
    }

    shared.previous_completed_bytes.store(0, Ordering::Release);
    shared.download.bytes_downloaded.store(0, Ordering::SeqCst);

    verify_local(shared, &staging)
}

/// Resolve and verify a local patch directory while updating shared progress.
fn verify_local(shared: &Arc<PatcherShared>, source_dir: &Path) -> Option<PatchPlan> {
    shared.set_phase(Phase::Validating);

    let plan = match PatchPlan::from_local_source(source_dir) {
        Ok(plan) => plan,
        Err(e) => {
            shared.fail(format!("Could not scan the local patch directory: {e}"));
            return None;
        }
    };

    for (idx, (entry, path)) in resolve_against_manifest(&plan.patches_in_order)
        .iter()
        .enumerate()
    {
        if shared.is_cancel_requested() {
            shared.set_phase(Phase::Cancelled);
            return None;
        }
        shared.download_idx.store(idx, Ordering::Release);
        shared.download.bytes_downloaded.store(0, Ordering::SeqCst);

        match verify_one(path, entry, shared) {
            VerifyOutcome::Ok => {
                shared
                    .previous_completed_bytes
                    .fetch_add(entry.size, Ordering::Release);
            }
            VerifyOutcome::Cancelled => {
                shared.set_phase(Phase::Cancelled);
                return None;
            }
            VerifyOutcome::WrongSize { actual } => {
                shared.fail(format!(
                    "Local patch {} failed its size check ({actual} bytes observed; expected {})",
                    path.display(),
                    entry.size
                ));
                return None;
            }
            VerifyOutcome::WrongChecksum => {
                shared.fail(format!(
                    "Local patch {} failed its CRC32 check",
                    path.display()
                ));
                return None;
            }
            VerifyOutcome::WrongSha256 => {
                shared.fail(format!(
                    "Local patch {} failed its SHA-256 identity check",
                    path.display()
                ));
                return None;
            }
            VerifyOutcome::Io(e) => {
                shared.fail(format!(
                    "Could not read local patch {}: {e}",
                    path.display()
                ));
                return None;
            }
        }
    }

    Some(plan)
}

/// Resolve patch paths in manifest order.
fn resolve_against_manifest(paths: &[PathBuf]) -> Vec<(&'static PatchEntry, PathBuf)> {
    let mut pairs = Vec::with_capacity(PATCH_MANIFEST.len());
    for entry in PATCH_MANIFEST {
        let leaf = leaf_of(entry.path);
        if let Some(path) = paths
            .iter()
            .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(leaf))
        {
            pairs.push((entry, path.clone()));
        }
    }
    pairs
}

enum VerifyOutcome {
    Ok,
    Cancelled,
    WrongSize { actual: u64 },
    WrongChecksum,
    WrongSha256,
    Io(std::io::Error),
}

/// Verify one file's length, CRC32, and manifest SHA-256 while honoring cancellation.
fn verify_one(path: &Path, entry: &PatchEntry, shared: &Arc<PatcherShared>) -> VerifyOutcome {
    let expected_sha256 = expected_sha256(entry.path)
        .expect("every embedded patch manifest entry has a SHA-256 identity");
    verify_one_with_sha256(path, entry.size, entry.crc32, expected_sha256, shared)
}

fn verify_one_with_sha256(
    path: &Path,
    expected_size: u64,
    expected_crc32: u32,
    expected_sha256: &str,
    shared: &Arc<PatcherShared>,
) -> VerifyOutcome {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => return VerifyOutcome::Io(e),
    };
    match file.metadata() {
        Ok(meta) if meta.len() != expected_size => {
            return VerifyOutcome::WrongSize { actual: meta.len() };
        }
        Ok(_) => {}
        Err(e) => return VerifyOutcome::Io(e),
    }

    let mut reader = BufReader::new(file);
    let mut crc32 = Crc32Hasher::new();
    let mut sha256 = Sha256::new();
    let mut bytes_read = 0u64;
    let mut buf = [0u8; IO_CHUNK];
    loop {
        if shared.is_cancel_requested() {
            return VerifyOutcome::Cancelled;
        }

        let remaining = expected_size - bytes_read;
        let limit = if remaining == 0 {
            1
        } else {
            usize::try_from(remaining)
                .unwrap_or(usize::MAX)
                .min(buf.len())
        };
        let read = match reader.read(&mut buf[..limit]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return VerifyOutcome::Io(e),
        };
        if read as u64 > remaining {
            return VerifyOutcome::WrongSize {
                actual: expected_size.saturating_add(1),
            };
        }
        bytes_read += read as u64;
        crc32.update(&buf[..read]);
        sha256.update(&buf[..read]);
        shared
            .download
            .bytes_downloaded
            .fetch_add(read as u64, Ordering::Relaxed);
    }

    if bytes_read != expected_size {
        return VerifyOutcome::WrongSize { actual: bytes_read };
    }
    if crc32.finalize() != expected_crc32 {
        VerifyOutcome::WrongChecksum
    } else if format!("{:x}", sha256.finalize()) != expected_sha256 {
        VerifyOutcome::WrongSha256
    } else {
        VerifyOutcome::Ok
    }
}

fn display_leaf(path: &Path) -> String {
    path.file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn fresh_shared_is_in_starting_phase() {
        let s = PatcherShared::new();
        assert_eq!(s.phase(), Phase::Starting);
        assert!(!s.is_terminal());
        assert_eq!(s.error(), None);
        assert!(s.warnings().is_empty());
    }

    #[test]
    fn cancel_request_reaches_the_progress_flag() {
        let s = PatcherShared::new();
        assert!(!s.is_cancel_requested());
        s.request_cancel();
        assert!(s.is_cancel_requested());
    }

    #[test]
    fn pause_resume_and_cancel_share_one_cooperative_gate() {
        let s = PatcherShared::new();
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
            let shared = PatcherShared::new();
            shared.set_phase(Phase::Patching);
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
        let shared = PatcherShared::new();
        shared.request_pause();
        shared.set_phase(Phase::Done);
        assert!(!shared.is_pause_requested());
        shared.request_pause();
        assert!(!shared.is_pause_requested());
    }

    #[test]
    fn cancelling_a_paused_waiter_wakes_the_worker() {
        let s = PatcherShared::new();
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
        let s = PatcherShared::new();
        s.fail("simulated failure");
        assert_eq!(s.phase(), Phase::Error);
        assert_eq!(s.error().as_deref(), Some("simulated failure"));
        assert!(s.is_terminal());
    }

    #[test]
    fn warnings_keep_their_order() {
        let s = PatcherShared::new();
        s.warn("first");
        s.warn("second");
        assert_eq!(
            s.warnings(),
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn phase_survives_a_u8_round_trip() {
        for phase in [
            Phase::Starting,
            Phase::Patching,
            Phase::Done,
            Phase::Error,
            Phase::Cancelled,
            Phase::Validating,
            Phase::Extracting,
            Phase::Downloading,
            Phase::Installing,
            Phase::ValidatingFiles,
        ] {
            assert_eq!(Phase::from_u8(phase as u8), phase);
        }
    }

    #[test]
    fn patch_with_the_expected_content_identity_is_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("patch.patch");
        fs::write(&path, b"abc").unwrap();
        assert!(matches!(
            verify_one_with_sha256(
                &path,
                3,
                0x352441C2,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
                &PatcherShared::new(),
            ),
            VerifyOutcome::Ok
        ));
    }

    #[test]
    fn same_size_patch_with_matching_crc_but_wrong_sha256_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("patch.patch");
        let contents = b"same-sized content with the wrong identity";
        fs::write(&path, contents).unwrap();
        let mut crc32 = Crc32Hasher::new();
        crc32.update(contents);

        assert!(matches!(
            verify_one_with_sha256(
                &path,
                contents.len() as u64,
                crc32.finalize(),
                &"0".repeat(64),
                &PatcherShared::new(),
            ),
            VerifyOutcome::WrongSha256
        ));
    }
}
