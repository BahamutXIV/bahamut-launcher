//! Manifest-bound verification and recoverable repair of managed game files.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use super::http::{ObjectSpec, download_object};
use super::manifest::{
    ArchiveLayout, BasePackage, InstallFile, validate_hash, validate_relative_path,
};
use super::require_space;
use crate::diagnostics::free_disk_bytes;
use crate::version::{FFXIV_BOOT_VERSION, FFXIV_GAME_VERSION};

const IO_BUFFER_BYTES: usize = 1024 * 1024;
const TRANSACTION_SUFFIX: &str = ".bahamut-repair";
const OWNER_FILE: &str = "owner.json";
const JOURNAL_FILE: &str = "transaction.json";
const JOURNAL_NEXT_FILE: &str = "transaction.next";
const STAGED_DIR: &str = "staged";
const ROLLBACK_DIR: &str = "rollback";
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

/// Progress and safe-boundary controls for one managed-game repair.
pub struct RepairShared {
    phase: AtomicU8,
    bytes_completed: AtomicU64,
    bytes_total: AtomicU64,
    files_completed: AtomicUsize,
    files_total: AtomicUsize,
    current_file: Mutex<Option<String>>,
    error: Mutex<Option<String>>,
    failure_context: Mutex<Option<FailureContext>>,
    pause_requested: AtomicBool,
    paused: AtomicBool,
    cancel_requested: AtomicBool,
    cancel_allowed: AtomicBool,
    pause_lock: Mutex<()>,
    pause_changed: Condvar,
}

#[derive(Clone)]
struct FailureContext {
    destination: String,
    cache: String,
    content_root: String,
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
            error: Mutex::new(None),
            failure_context: Mutex::new(None),
            pause_requested: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
            cancel_allowed: AtomicBool::new(true),
            pause_lock: Mutex::new(()),
            pause_changed: Condvar::new(),
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
        self.error
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    pub fn is_pause_requested(&self) -> bool {
        self.pause_requested.load(Ordering::Acquire)
    }

    pub fn is_cancel_requested(&self) -> bool {
        self.cancel_requested.load(Ordering::Acquire)
    }

    pub fn can_cancel(&self) -> bool {
        self.cancel_allowed.load(Ordering::Acquire) && !self.phase().is_terminal()
    }

    fn set_failure_context(&self, destination: &Path, cache: &Path, content_root: &str) {
        *self
            .failure_context
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(FailureContext {
            destination: destination.display().to_string(),
            cache: cache.display().to_string(),
            content_root: content_root.to_owned(),
        });
    }

    pub fn request_pause(&self) {
        let _guard = self.pause_lock.lock().expect("repair pause mutex poisoned");
        if !self.phase().is_terminal() {
            self.pause_requested.store(true, Ordering::Release);
        }
    }

    pub fn request_resume(&self) {
        let _guard = self.pause_lock.lock().expect("repair pause mutex poisoned");
        self.pause_requested.store(false, Ordering::Release);
        self.pause_changed.notify_all();
    }

    pub fn request_cancel(&self) {
        let _guard = self.pause_lock.lock().expect("repair pause mutex poisoned");
        if self.cancel_allowed.load(Ordering::Acquire) && !self.phase().is_terminal() {
            self.cancel_requested.store(true, Ordering::Release);
            self.pause_requested.store(false, Ordering::Release);
            self.pause_changed.notify_all();
        }
    }

    fn begin_phase(
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

    fn set_current_file(&self, path: Option<&str>) {
        *self
            .current_file
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = path.map(str::to_owned);
    }

    fn add_bytes(&self, bytes: u64) {
        self.bytes_completed.fetch_add(bytes, Ordering::Relaxed);
    }

    fn set_bytes(&self, bytes: u64) {
        self.bytes_completed.store(bytes, Ordering::Release);
    }

    fn complete_file(&self) {
        self.files_completed.fetch_add(1, Ordering::Relaxed);
    }

    fn checkpoint(&self) -> Result<(), String> {
        let mut guard = self
            .pause_lock
            .lock()
            .map_err(|_| "Repair control state is unavailable.".to_owned())?;
        while self.pause_requested.load(Ordering::Acquire)
            && !self.cancel_requested.load(Ordering::Acquire)
        {
            self.paused.store(true, Ordering::Release);
            let wait = self
                .pause_changed
                .wait_timeout(guard, PAUSE_POLL)
                .map_err(|_| "Repair control state is unavailable.".to_owned())?;
            guard = wait.0;
        }
        self.paused.store(false, Ordering::Release);
        if self.cancel_requested.load(Ordering::Acquire) {
            Err("Game repair was cancelled.".into())
        } else {
            Ok(())
        }
    }

    fn commit<T>(&self, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let mut action = Some(action);
        loop {
            self.checkpoint()?;
            let guard = self
                .pause_lock
                .lock()
                .map_err(|_| "Repair control state is unavailable.".to_owned())?;
            if self.cancel_requested.load(Ordering::Acquire) {
                return Err("Game repair was cancelled.".into());
            }
            if self.pause_requested.load(Ordering::Acquire) {
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

    fn download_checkpoint(&self, durable: bool) -> super::http::CheckpointAction {
        use super::http::CheckpointAction;
        if self.is_cancel_requested() {
            return CheckpointAction::Cancel;
        }
        if self.is_pause_requested() {
            if !durable {
                return CheckpointAction::Pause;
            }
            if self.checkpoint().is_err() {
                return CheckpointAction::Cancel;
            }
        }
        CheckpointAction::Continue
    }

    pub fn fail(&self, message: impl Into<String>) {
        let message = message.into();
        let context = self
            .failure_context
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .unwrap_or_else(|| FailureContext {
                destination: "<unknown>".into(),
                cache: "<unknown>".into(),
                content_root: "<unknown>".into(),
            });
        tracing::error!(
            action = "repair",
            destination = %context.destination,
            cache = %context.cache,
            content_root = %context.content_root,
            diagnostic = %message,
            "game repair failed"
        );
        *self.error.lock().unwrap_or_else(|error| error.into_inner()) = Some(message);
        self.finish_phase(RepairPhase::Error);
    }

    fn finish_cancelled(&self) {
        self.finish_phase(RepairPhase::Cancelled);
    }

    fn finish_phase(&self, phase: RepairPhase) {
        self.phase.store(phase as u8, Ordering::Release);
        if phase.is_terminal() {
            self.request_resume();
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileState {
    Valid,
    Missing,
    Corrupt,
    Unresolved,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
pub struct ManagedFileStatus {
    pub path: String,
    pub state: FileState,
    pub expected_length: u64,
    pub actual_length: Option<u64>,
    pub actual_sha256: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
pub struct RepairQuote {
    /// Total bytes in the complete archive set, independent of cache state.
    pub full_archive_bytes: u64,
    /// Bytes that would need to be downloaded for the current selection.
    pub download_bytes: u64,
    /// Additional same-volume staging space for selected replacements.
    pub replacement_bytes: u64,
    pub available_cache_bytes: Option<u64>,
    pub available_game_bytes: Option<u64>,
    pub cache_problem: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
pub struct VerificationSummary {
    /// Non-valid files only. Counts below cover the full managed-file inventory.
    pub files: Vec<ManagedFileStatus>,
    pub valid_count: usize,
    pub missing_count: usize,
    pub corrupt_count: usize,
    pub unresolved_count: usize,
    pub recovery_required: bool,
    pub recovery_detail: Option<String>,
    pub quote: RepairQuote,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
pub struct RepairResult {
    pub repaired_files: Vec<String>,
    /// True only when every catalogued game file passed the final full hash scan.
    pub complete: bool,
    pub verification: VerificationSummary,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnerMarker {
    schema_version: u32,
    package_sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RepairJournal {
    schema_version: u32,
    package_sha256: String,
    phase: JournalPhase,
    files: Vec<JournalFile>,
    created_directories: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum JournalPhase {
    Publishing,
    Committed,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct JournalFile {
    path: String,
    original: OriginalState,
    published: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum OriginalState {
    Missing,
    Corrupt { length: u64, sha256: String },
}

#[derive(Clone)]
struct RepairSource {
    archive_index: usize,
    member_path: String,
}

struct CacheStatus {
    valid: bool,
    problem: Option<String>,
}

/// A recovery directory blocks launch even when executable and version files are intact.
/// Inspection failures also block readiness; only Repair may recover or remove this state.
pub fn recovery_pending(destination: &Path) -> bool {
    let Ok(game) = fs::canonicalize(destination) else {
        return true;
    };
    let Ok(transaction) = transaction_path(&game) else {
        return true;
    };
    !matches!(fs::symlink_metadata(transaction),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}

fn acquire_repair_lock(game: &Path) -> Result<crate::atomic_fs::ExclusiveProcessLock, String> {
    // Keep this sibling file after release: deleting it could split the lock across inodes.
    let mut lock_path = transaction_path(game)?.into_os_string();
    lock_path.push(".lock");
    crate::atomic_fs::try_lock_exclusive(Path::new(&lock_path)).map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            "Another launcher is repairing this game directory. Wait for it to finish.".into()
        } else {
            format!("Could not lock the game directory for repair: {error}")
        }
    })
}

/// Resolve a selected game directory through the same safety checks as verification and repair.
pub fn canonical_game_root(destination: &Path) -> Result<PathBuf, String> {
    resolve_existing_directory(destination)
}

/// Recover, verify, and repair every missing or corrupt managed file.
pub fn repair_all(
    destination: &Path,
    content_root: &str,
    cache: &Path,
    package: &BasePackage,
    shared: &RepairShared,
) -> Result<RepairResult, String> {
    shared.set_failure_context(destination, cache, content_root);
    shared.begin_phase(RepairPhase::Starting, None, None);
    let result = repair_all_inner(destination, content_root, cache, package, shared);
    match result {
        Ok(result) => {
            if result.complete {
                shared.finish_phase(RepairPhase::Done);
            } else {
                let diagnostic = "Repair finished, but the final scan still found managed files that need attention.";
                shared.fail(diagnostic);
            }
            Ok(result)
        }
        Err(error) if shared.is_cancel_requested() => {
            shared.finish_cancelled();
            Err(error)
        }
        Err(error) => {
            shared.fail(error.clone());
            Err(error)
        }
    }
}

fn repair_all_inner(
    destination: &Path,
    content_root: &str,
    cache: &Path,
    package: &BasePackage,
    shared: &RepairShared,
) -> Result<RepairResult, String> {
    package.validate()?;
    let sources = repair_sources(package)?;
    let game = resolve_existing_directory(destination)?;
    let _repair_lock = acquire_repair_lock(&game)?;
    let cache_path = resolve_cache_path(cache)?;
    let package_sha256 = package_identity(package);
    let transaction = transaction_path(&game)?;

    shared.begin_phase(RepairPhase::Recovering, None, None);
    shared.checkpoint()?;
    recover_transaction(&transaction, &game, package, &package_sha256)?;
    shared.checkpoint()?;

    shared.begin_phase(
        RepairPhase::Verifying,
        None,
        Some(package.final_files.len()),
    );
    let before_files = scan_files_controlled(&game, &package.final_files, shared)?;
    let (recovery_required, recovery_detail) = inspect_recovery_state(&transaction, package)?;
    if recovery_required {
        return Err(recovery_detail
            .unwrap_or_else(|| "A previous repair transaction needs recovery.".into()));
    }
    if let Some(unresolved) = before_files
        .iter()
        .find(|status| status.state == FileState::Unresolved)
    {
        return Err(format!(
            "Cannot safely repair {}: {}",
            unresolved.path,
            unresolved.detail.as_deref().unwrap_or("file access failed")
        ));
    }
    let (selected, selected_statuses) = select_repair_files(&package.final_files, &before_files)?;
    let (quote, cached_archive_indices) = quote_for_statuses_with_cache(
        &game,
        &cache_path.path,
        &cache_path.nearest_existing,
        package,
        &sources,
        &selected_statuses,
    )?;
    if selected.is_empty() {
        return Ok(RepairResult {
            repaired_files: Vec::new(),
            complete: true,
            verification: summary(&before_files, recovery_required, recovery_detail, quote),
        });
    }
    if let Some(problem) = &quote.cache_problem {
        return Err(format!("The repair cache is unavailable: {problem}"));
    }
    preflight_space(&quote, &cache_path.path, &game)?;

    let mut required_archives = required_archive_indices(&sources, &selected)
        .into_iter()
        .collect::<Vec<_>>();
    required_archives.sort_unstable();
    shared.begin_phase(
        RepairPhase::Downloading,
        Some(quote.download_bytes),
        Some(required_archives.len()),
    );
    let mut cached_archives = HashMap::new();
    let mut downloaded_before = 0_u64;
    for index in required_archives {
        shared.checkpoint()?;
        let archive = &package.archives[index];
        shared.set_current_file(Some(&archive.object.object_key));
        let archive_was_cached = cached_archive_indices.contains(&index);
        let path = download_object(
            content_root,
            &archive.object,
            &cache_path.path,
            |checkpoint| shared.download_checkpoint(checkpoint.durable),
            |bytes, _total| {
                if !archive_was_cached {
                    shared.set_bytes(downloaded_before.saturating_add(bytes));
                }
            },
        )
        .map_err(|error| {
            format!(
                "Could not obtain the verified game archive {}: {error}",
                archive.object.object_key
            )
        })?;
        if !archive_was_cached {
            downloaded_before = downloaded_before.saturating_add(archive.object.length);
            shared.set_bytes(downloaded_before);
        }
        cached_archives.insert(index, path);
        shared.complete_file();
    }

    let replacement_bytes = selected
        .iter()
        .try_fold(0_u64, |sum, file| sum.checked_add(file.length))
        .ok_or_else(|| "Repair staging size overflow.".to_string())?;
    shared.begin_phase(
        RepairPhase::Staging,
        Some(replacement_bytes),
        Some(selected.len()),
    );
    let transaction = create_transaction(&transaction, package, &package_sha256)?;
    let staged_root = transaction.join(STAGED_DIR);
    let extraction = (|| {
        for status in &selected_statuses {
            shared.checkpoint()?;
            shared.set_current_file(Some(&status.path));
            let file = find_file(package, &status.path)?;
            match status.path.to_ascii_lowercase().as_str() {
                "boot.ver" => {
                    write_staged_bytes(&staged_root, file, FFXIV_BOOT_VERSION.as_bytes())?;
                    shared.add_bytes(file.length);
                }
                "game.ver" => {
                    write_staged_bytes(&staged_root, file, FFXIV_GAME_VERSION.as_bytes())?;
                    shared.add_bytes(file.length);
                }
                _ => {
                    let source = sources
                        .get(&status.path.to_ascii_lowercase())
                        .ok_or_else(|| format!("No repair archive maps {}.", status.path))?;
                    let archive_path =
                        cached_archives.get(&source.archive_index).ok_or_else(|| {
                            format!("The repair archive for {} was not prepared.", status.path)
                        })?;
                    extract_selected_file(
                        archive_path,
                        &package.archives[source.archive_index],
                        source,
                        file,
                        &staged_root,
                        shared,
                    )?;
                }
            }
            shared.complete_file();
        }
        Ok::<(), String>(())
    })();
    if let Err(error) = extraction {
        cleanup_unjournaled_transaction(&transaction, package, &package_sha256)?;
        return Err(error);
    }

    shared.begin_phase(RepairPhase::Verifying, None, Some(selected.len()));
    let fresh = match scan_selected_controlled(&game, &selected, shared) {
        Ok(files) => files,
        Err(error) => {
            cleanup_unjournaled_transaction(&transaction, package, &package_sha256)?;
            return Err(error);
        }
    };
    if let Err(error) = ensure_snapshots_unchanged(&selected_statuses, &fresh) {
        cleanup_unjournaled_transaction(&transaction, package, &package_sha256)?;
        return Err(error);
    }
    let created_directories = match missing_parent_directories(&game, &selected) {
        Ok(directories) => directories,
        Err(error) => {
            cleanup_unjournaled_transaction(&transaction, package, &package_sha256)?;
            return Err(error);
        }
    };
    let mut journal = match make_journal(&package_sha256, &selected_statuses, created_directories) {
        Ok(journal) => journal,
        Err(error) => {
            cleanup_unjournaled_transaction(&transaction, package, &package_sha256)?;
            return Err(error);
        }
    };
    if let Err(error) = write_journal(&transaction, &journal) {
        cleanup_unjournaled_transaction(&transaction, package, &package_sha256)?;
        return Err(error);
    }

    shared.begin_phase(RepairPhase::Publishing, None, Some(selected.len()));
    if let Err(error) = publish_selected(
        &game,
        &transaction,
        &selected,
        &selected_statuses,
        &mut journal,
        shared,
    ) {
        match recover_transaction(&transaction, &game, package, &package_sha256) {
            Ok(()) => return Err(error),
            Err(recovery_error) => {
                return Err(format!(
                    "{error}. Rollback is incomplete and will be retried on the next Repair: {recovery_error}"
                ));
            }
        }
    }

    shared.begin_phase(RepairPhase::Verifying, None, Some(selected.len()));
    if let Err(error) = validate_staged_publication(&transaction, &game, &selected, shared) {
        let error = format!("Repaired file did not pass its final hash check: {error}");
        match recover_transaction(&transaction, &game, package, &package_sha256) {
            Ok(()) => return Err(error),
            Err(recovery_error) => {
                return Err(format!(
                    "{error} Rollback is incomplete and will be retried on the next Repair: {recovery_error}"
                ));
            }
        }
    }

    shared.begin_phase(
        RepairPhase::FinalVerification,
        None,
        Some(package.final_files.len()),
    );
    if let Err(error) = detach_published_stage_links(&transaction, &game, &selected, shared) {
        return Err(rollback_after_final_verification_error(
            &transaction,
            &game,
            package,
            &package_sha256,
            error,
        ));
    }
    let verification_files = match scan_files_controlled(&game, &package.final_files, shared) {
        Ok(files) => files,
        Err(error) => {
            return Err(rollback_after_final_verification_error(
                &transaction,
                &game,
                package,
                &package_sha256,
                error,
            ));
        }
    };
    let quote = match quote_for_statuses(
        &game,
        &cache_path.path,
        &cache_path.nearest_existing,
        package,
        &sources,
        &verification_files,
    ) {
        Ok(quote) => quote,
        Err(error) => {
            return Err(rollback_after_final_verification_error(
                &transaction,
                &game,
                package,
                &package_sha256,
                error,
            ));
        }
    };
    let verification = summary(&verification_files, false, None, quote);
    let complete = verification.missing_count == 0
        && verification.corrupt_count == 0
        && verification.unresolved_count == 0;
    if !complete {
        let error = format!(
            "Final verification found {} missing, {} corrupt, and {} inaccessible managed files.",
            verification.missing_count, verification.corrupt_count, verification.unresolved_count
        );
        return Err(rollback_after_final_verification_error(
            &transaction,
            &game,
            package,
            &package_sha256,
            error,
        ));
    }

    journal.phase = JournalPhase::Committed;
    if let Err(error) = shared.commit(|| write_journal(&transaction, &journal)) {
        match recover_transaction(&transaction, &game, package, &package_sha256) {
            Ok(()) => return Err(error),
            Err(recovery_error) => {
                return Err(format!(
                    "Could not record completed repair: {error}. Rollback is incomplete and will be retried on the next Repair: {recovery_error}"
                ));
            }
        }
    }
    // The full inventory was just hashed before commit; normal cleanup need not hash repaired files again.
    cleanup_committed_transaction(&transaction, package, &package_sha256)?;

    Ok(RepairResult {
        repaired_files: selected.iter().map(|file| file.path.clone()).collect(),
        complete,
        verification,
    })
}

fn rollback_after_final_verification_error(
    transaction: &Path,
    game: &Path,
    package: &BasePackage,
    package_sha256: &str,
    error: String,
) -> String {
    match recover_transaction(transaction, game, package, package_sha256) {
        Ok(()) => error,
        Err(recovery_error) => format!(
            "{error}. Rollback is incomplete and will be retried on the next Repair: {recovery_error}"
        ),
    }
}

fn summary(
    files: &[ManagedFileStatus],
    recovery_required: bool,
    recovery_detail: Option<String>,
    quote: RepairQuote,
) -> VerificationSummary {
    VerificationSummary {
        valid_count: files
            .iter()
            .filter(|file| file.state == FileState::Valid)
            .count(),
        missing_count: files
            .iter()
            .filter(|file| file.state == FileState::Missing)
            .count(),
        corrupt_count: files
            .iter()
            .filter(|file| file.state == FileState::Corrupt)
            .count(),
        unresolved_count: files
            .iter()
            .filter(|file| file.state == FileState::Unresolved)
            .count(),
        files: files
            .iter()
            .filter(|file| file.state != FileState::Valid)
            .cloned()
            .collect(),
        recovery_required,
        recovery_detail,
        quote,
    }
}

fn quote_for_statuses(
    game: &Path,
    cache: &Path,
    cache_volume_root: &Path,
    package: &BasePackage,
    sources: &HashMap<String, RepairSource>,
    statuses: &[ManagedFileStatus],
) -> Result<RepairQuote, String> {
    quote_for_statuses_with_cache(game, cache, cache_volume_root, package, sources, statuses)
        .map(|(quote, _)| quote)
}

fn quote_for_statuses_with_cache(
    game: &Path,
    cache: &Path,
    cache_volume_root: &Path,
    package: &BasePackage,
    sources: &HashMap<String, RepairSource>,
    statuses: &[ManagedFileStatus],
) -> Result<(RepairQuote, HashSet<usize>), String> {
    let full_archive_bytes = package.download_bytes()?;
    let mut replacement_bytes = 0_u64;
    let mut archive_indices = HashSet::new();
    for status in statuses {
        if !is_repairable(status.state) {
            continue;
        }
        let file = find_file(package, &status.path)?;
        replacement_bytes = replacement_bytes
            .checked_add(file.length)
            .ok_or_else(|| "Repair staging size overflow.".to_string())?;
        if let Some(source) = sources.get(&status.path.to_ascii_lowercase()) {
            archive_indices.insert(source.archive_index);
        }
    }
    let mut download_bytes = 0_u64;
    let mut cache_problem = None;
    let mut cached_archive_indices = HashSet::new();
    for index in archive_indices {
        let object = &package.archives[index].object;
        match cache_object_status(cache, object) {
            Ok(CacheStatus { valid: true, .. }) => {
                cached_archive_indices.insert(index);
            }
            Ok(CacheStatus {
                valid: false,
                problem,
            }) => {
                download_bytes = download_bytes
                    .checked_add(object.length)
                    .ok_or_else(|| "Repair download size overflow.".to_string())?;
                if cache_problem.is_none() {
                    cache_problem = problem;
                }
            }
            Err(error) => {
                download_bytes = download_bytes
                    .checked_add(object.length)
                    .ok_or_else(|| "Repair download size overflow.".to_string())?;
                if cache_problem.is_none() {
                    cache_problem = Some(error);
                }
            }
        }
    }
    Ok((
        RepairQuote {
            full_archive_bytes,
            download_bytes,
            replacement_bytes,
            available_cache_bytes: free_disk_bytes(cache_volume_root),
            available_game_bytes: free_disk_bytes(game),
            cache_problem,
        },
        cached_archive_indices,
    ))
}

fn preflight_space(quote: &RepairQuote, cache: &Path, game: &Path) -> Result<(), String> {
    if same_volume(cache, game) {
        let available = quote
            .available_cache_bytes
            .zip(quote.available_game_bytes)
            .map(|(cache, game)| cache.min(game))
            .or(quote.available_cache_bytes)
            .or(quote.available_game_bytes);
        let required = quote
            .download_bytes
            .checked_add(quote.replacement_bytes)
            .ok_or_else(|| "Repair space requirement overflow.".to_string())?;
        require_space(available, required, "download and repair staging volume")?;
    } else {
        require_space(
            quote.available_cache_bytes,
            quote.download_bytes,
            "download cache",
        )?;
        require_space(
            quote.available_game_bytes,
            quote.replacement_bytes,
            "game installation",
        )?;
    }
    Ok(())
}

fn same_volume(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows::Win32::Storage::FileSystem::GetVolumePathNameW;
        use windows::core::PCWSTR;

        let volume_path = |path: &Path| {
            let input = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            let mut output = vec![0_u16; 32768];
            unsafe { GetVolumePathNameW(PCWSTR(input.as_ptr()), &mut output) }.ok()?;
            let length = output.iter().position(|unit| *unit == 0)?;
            Some(
                OsString::from_wide(&output[..length])
                    .to_string_lossy()
                    .to_lowercase(),
            )
        };
        volume_path(left)
            .zip(volume_path(right))
            .is_some_and(|(left, right)| left == right)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(left)
            .ok()
            .zip(fs::metadata(right).ok())
            .is_some_and(|(left, right)| left.dev() == right.dev())
    }
    #[cfg(not(any(windows, unix)))]
    {
        left.components().next() == right.components().next()
    }
}

fn is_repairable(state: FileState) -> bool {
    matches!(state, FileState::Missing | FileState::Corrupt)
}

fn select_repair_files<'a>(
    files: &'a [InstallFile],
    statuses: &[ManagedFileStatus],
) -> Result<(Vec<&'a InstallFile>, Vec<ManagedFileStatus>), String> {
    if files.len() != statuses.len() {
        return Err("The verification result did not match the managed inventory.".into());
    }

    let mut selected = Vec::new();
    let mut selected_statuses = Vec::new();
    for (file, status) in files.iter().zip(statuses) {
        if !status.path.eq_ignore_ascii_case(&file.path) {
            return Err(format!("The verification result omitted {}.", file.path));
        }
        if is_repairable(status.state) {
            selected.push(file);
            selected_statuses.push(status.clone());
        }
    }
    Ok((selected, selected_statuses))
}

#[cfg(test)]
fn scan_selected(root: &Path, files: &[&InstallFile]) -> Vec<ManagedFileStatus> {
    files.iter().map(|file| inspect_file(root, file)).collect()
}

fn scan_files_controlled(
    root: &Path,
    files: &[InstallFile],
    shared: &RepairShared,
) -> Result<Vec<ManagedFileStatus>, String> {
    let mut statuses = Vec::with_capacity(files.len());
    for file in files {
        shared.checkpoint()?;
        shared.set_current_file(Some(&file.path));
        statuses.push(inspect_file_controlled(root, file, shared)?);
        shared.complete_file();
    }
    shared.set_current_file(None);
    Ok(statuses)
}

fn scan_selected_controlled(
    root: &Path,
    files: &[&InstallFile],
    shared: &RepairShared,
) -> Result<Vec<ManagedFileStatus>, String> {
    let mut statuses = Vec::with_capacity(files.len());
    for file in files {
        shared.checkpoint()?;
        shared.set_current_file(Some(&file.path));
        statuses.push(inspect_file_controlled(root, file, shared)?);
        shared.complete_file();
    }
    shared.set_current_file(None);
    Ok(statuses)
}

fn inspect_file(root: &Path, spec: &InstallFile) -> ManagedFileStatus {
    inspect_file_inner(root, spec, None).expect("uncontrolled repair scan cannot be cancelled")
}

fn inspect_file_controlled(
    root: &Path,
    spec: &InstallFile,
    shared: &RepairShared,
) -> Result<ManagedFileStatus, String> {
    inspect_file_inner(root, spec, Some(shared))
}

fn inspect_file_inner(
    root: &Path,
    spec: &InstallFile,
    shared: Option<&RepairShared>,
) -> Result<ManagedFileStatus, String> {
    let path = join_relative(root, &spec.path);
    let mut current = root.to_path_buf();
    let parts = spec.path.split('/').collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(file_status(spec, FileState::Missing, None, None, None));
            }
            Err(error) => {
                return Ok(file_status(
                    spec,
                    FileState::Unresolved,
                    None,
                    None,
                    Some(format!("Could not inspect {}: {error}", current.display())),
                ));
            }
            Ok(metadata) => {
                if is_reparse(&metadata) {
                    return Ok(file_status(
                        spec,
                        FileState::Unresolved,
                        None,
                        None,
                        Some(format!(
                            "Refusing a link or reparse point: {}",
                            current.display()
                        )),
                    ));
                }
                if index + 1 < parts.len() && !metadata.is_dir() {
                    return Ok(file_status(
                        spec,
                        FileState::Unresolved,
                        None,
                        None,
                        Some(format!(
                            "A managed parent is not a directory: {}",
                            current.display()
                        )),
                    ));
                }
                if index + 1 == parts.len() && !metadata.is_file() {
                    return Ok(file_status(
                        spec,
                        FileState::Unresolved,
                        Some(metadata.len()),
                        None,
                        Some(format!(
                            "Managed path is not a regular file: {}",
                            current.display()
                        )),
                    ));
                }
            }
        }
    }
    let hash = match shared {
        Some(shared) => hash_regular_file_controlled(&path, shared),
        None => hash_regular_file(&path),
    };
    match hash {
        Ok((length, sha256)) => {
            let state = if length == spec.length && sha256.eq_ignore_ascii_case(&spec.sha256) {
                FileState::Valid
            } else {
                FileState::Corrupt
            };
            Ok(file_status(spec, state, Some(length), Some(sha256), None))
        }
        Err(error)
            if shared.is_some_and(|shared| {
                shared.is_cancel_requested() || error == "Repair control state is unavailable."
            }) =>
        {
            Err(error)
        }
        Err(error) => Ok(file_status(
            spec,
            FileState::Unresolved,
            None,
            None,
            Some(error),
        )),
    }
}

fn file_status(
    spec: &InstallFile,
    state: FileState,
    actual_length: Option<u64>,
    actual_sha256: Option<String>,
    detail: Option<String>,
) -> ManagedFileStatus {
    ManagedFileStatus {
        path: spec.path.clone(),
        state,
        expected_length: spec.length,
        actual_length,
        actual_sha256,
        detail,
    }
}

fn hash_regular_file(path: &Path) -> Result<(u64, String), String> {
    hash_regular_file_inner(path, None)
}

fn hash_regular_file_controlled(
    path: &Path,
    shared: &RepairShared,
) -> Result<(u64, String), String> {
    hash_regular_file_inner(path, Some(shared))
}

fn hash_regular_file_inner(
    path: &Path,
    shared: Option<&RepairShared>,
) -> Result<(u64, String), String> {
    let mut options = OpenOptions::new();
    options.read(true);
    set_no_follow(&mut options);
    let mut file = options
        .open(path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    validate_regular_handle(&file, path)?;
    let length = file
        .metadata()
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?
        .len();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; IO_BUFFER_BYTES];
    loop {
        if let Some(shared) = shared {
            shared.checkpoint()?;
        }
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Could not hash {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        if let Some(shared) = shared {
            shared.add_bytes(read as u64);
        }
    }
    let actual_length = file
        .metadata()
        .map_err(|error| {
            format!(
                "Could not inspect {} after hashing: {error}",
                path.display()
            )
        })?
        .len();
    if actual_length != length {
        return Err(format!(
            "File changed while it was being hashed: {}",
            path.display()
        ));
    }
    Ok((length, format!("{:x}", hasher.finalize())))
}

fn repair_sources(package: &BasePackage) -> Result<HashMap<String, RepairSource>, String> {
    let mut sources = HashMap::new();
    let finals = package
        .final_files
        .iter()
        .map(|file| (file.path.to_ascii_lowercase(), file))
        .collect::<HashMap<_, _>>();
    for (archive_index, archive) in package.archives.iter().enumerate() {
        for file in &archive.files {
            if matches!(
                file.path.to_ascii_lowercase().as_str(),
                "boot.ver" | "game.ver"
            ) {
                return Err(format!(
                    "Archive must not contain managed version file {}.",
                    file.path
                ));
            }
            let final_file = finals.get(&file.path.to_ascii_lowercase()).ok_or_else(|| {
                format!("Archive file is not in the final inventory: {}", file.path)
            })?;
            if final_file.length != file.length
                || !final_file.sha256.eq_ignore_ascii_case(&file.sha256)
            {
                return Err(format!(
                    "Archive and final-file identities differ for {}.",
                    file.path
                ));
            }
            let key = file.path.to_ascii_lowercase();
            sources.insert(
                key,
                RepairSource {
                    archive_index,
                    member_path: archive_member_path(archive.layout, &file.path),
                },
            );
        }
    }
    for file in &package.final_files {
        match file.path.to_ascii_lowercase().as_str() {
            "boot.ver" => validate_generated_version(file, FFXIV_BOOT_VERSION)?,
            "game.ver" => validate_generated_version(file, FFXIV_GAME_VERSION)?,
            _ if !sources.contains_key(&file.path.to_ascii_lowercase()) => {
                return Err(format!("No full-archive repair source maps {}.", file.path));
            }
            _ => {}
        }
    }
    Ok(sources)
}

fn archive_member_path(layout: ArchiveLayout, path: &str) -> String {
    match layout {
        ArchiveLayout::Flat => path.to_owned(),
        ArchiveLayout::FinalFantasyXivWrapper => format!("FINAL FANTASY XIV/{path}"),
    }
}

fn validate_generated_version(file: &InstallFile, contents: &str) -> Result<(), String> {
    let digest = format!("{:x}", Sha256::digest(contents.as_bytes()));
    if file.length != contents.len() as u64 || !file.sha256.eq_ignore_ascii_case(&digest) {
        return Err(format!(
            "The final catalog has an invalid {} identity.",
            file.path
        ));
    }
    Ok(())
}

fn required_archive_indices(
    sources: &HashMap<String, RepairSource>,
    files: &[&InstallFile],
) -> HashSet<usize> {
    files
        .iter()
        .filter_map(|file| sources.get(&file.path.to_ascii_lowercase()))
        .map(|source| source.archive_index)
        .collect()
}

fn extract_selected_file(
    archive_path: &Path,
    archive_spec: &super::manifest::BaseArchive,
    source: &RepairSource,
    spec: &InstallFile,
    stage: &Path,
    shared: &RepairShared,
) -> Result<(), String> {
    let archive_file = File::open(archive_path)
        .map_err(|error| format!("Could not open the verified game archive: {error}"))?;
    let mut archive = ZipArchive::new(archive_file)
        .map_err(|error| format!("Could not read the verified game archive: {error}"))?;
    let mut matches = 0;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Could not inspect a game archive entry: {error}"))?;
        if entry.is_dir()
            || !archive_name_matches(archive_spec.layout, entry.name(), &source.member_path)
        {
            continue;
        }
        matches += 1;
        if matches > 1 {
            return Err(format!(
                "Repair archive contains duplicate member {}.",
                spec.path
            ));
        }
        validate_entry_mode(entry.unix_mode(), &spec.path)?;
        if entry.size() != spec.length {
            return Err(format!(
                "Repair archive member length differs for {}.",
                spec.path
            ));
        }
        let output_path = join_relative(stage, &spec.path);
        create_owned_parents(
            stage,
            output_path.parent().expect("managed file has a parent"),
        )?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
            .map_err(|error| format!("Could not stage {}: {error}", spec.path))?;
        let digest = copy_and_hash(&mut entry, &mut output, &spec.path, spec.length, shared)?;
        if !digest.eq_ignore_ascii_case(&spec.sha256) {
            return Err(format!(
                "Repair archive member failed SHA-256 verification: {}.",
                spec.path
            ));
        }
        output
            .sync_all()
            .map_err(|error| format!("Could not sync staged {}: {error}", spec.path))?;
    }
    if matches != 1 {
        return Err(format!("Repair archive is missing member {}.", spec.path));
    }
    Ok(())
}

fn archive_name_matches(layout: ArchiveLayout, raw_name: &str, member_path: &str) -> bool {
    let candidate = match layout {
        ArchiveLayout::Flat => raw_name,
        ArchiveLayout::FinalFantasyXivWrapper => {
            match raw_name.strip_prefix("FINAL FANTASY XIV/") {
                Some(relative) => relative,
                None => return false,
            }
        }
    };
    candidate
        == member_path
            .strip_prefix("FINAL FANTASY XIV/")
            .unwrap_or(member_path)
}

fn validate_entry_mode(mode: Option<u32>, name: &str) -> Result<(), String> {
    if let Some(mode) = mode {
        let kind = mode & 0o170000;
        if mode & 0o7000 != 0 || (kind != 0 && kind != 0o100000) {
            return Err(format!(
                "Repair archive member is not a regular file: {name}"
            ));
        }
    }
    Ok(())
}

fn copy_and_hash(
    reader: &mut impl Read,
    writer: &mut impl Write,
    label: &str,
    expected_length: u64,
    shared: &RepairShared,
) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; IO_BUFFER_BYTES];
    loop {
        shared.checkpoint()?;
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("Could not read repair member {label}: {error}"))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| format!("Repair member length overflow: {label}"))?;
        if total > expected_length {
            return Err(format!(
                "Repair archive member exceeds its catalog length: {label}"
            ));
        }
        hasher.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .map_err(|error| format!("Could not write staged repair member {label}: {error}"))?;
        shared.add_bytes(read as u64);
    }
    if total != expected_length {
        return Err(format!("Repair archive member is truncated: {label}"));
    }
    writer
        .flush()
        .map_err(|error| format!("Could not flush staged repair member {label}: {error}"))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn write_staged_bytes(stage: &Path, file: &InstallFile, bytes: &[u8]) -> Result<(), String> {
    let path = join_relative(stage, &file.path);
    create_owned_parents(stage, path.parent().expect("managed file has a parent"))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("Could not stage {}: {error}", file.path))?;
    output
        .write_all(bytes)
        .and_then(|()| output.sync_all())
        .map_err(|error| format!("Could not persist staged {}: {error}", file.path))
}

fn create_owned_parents(root: &Path, parent: &Path) -> Result<(), String> {
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| "A staged repair path escaped its owned directory.".to_string())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("Invalid component in staged repair path.".into());
        };
        current.push(name);
        match fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&current)
                    .map_err(|error| format!("Could not inspect staged directory: {error}"))?;
                if is_reparse(&metadata) || !metadata.is_dir() {
                    return Err(format!(
                        "Unsafe staged repair parent: {}",
                        current.display()
                    ));
                }
            }
            Err(error) => {
                return Err(format!("Could not create staged repair parent: {error}"));
            }
        }
    }
    Ok(())
}

fn make_journal(
    package_sha256: &str,
    statuses: &[ManagedFileStatus],
    created_directories: Vec<String>,
) -> Result<RepairJournal, String> {
    let files = statuses
        .iter()
        .map(|status| {
            let original = match status.state {
                FileState::Missing => OriginalState::Missing,
                FileState::Corrupt => OriginalState::Corrupt {
                    length: status.actual_length.ok_or_else(|| {
                        format!("Corrupt file length was unavailable: {}.", status.path)
                    })?,
                    sha256: status.actual_sha256.clone().ok_or_else(|| {
                        format!("Corrupt file hash was unavailable: {}.", status.path)
                    })?,
                },
                FileState::Valid | FileState::Unresolved => {
                    return Err(format!("{} is not eligible for repair.", status.path));
                }
            };
            Ok(JournalFile {
                path: status.path.clone(),
                original,
                published: false,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(RepairJournal {
        schema_version: 1,
        package_sha256: package_sha256.to_owned(),
        phase: JournalPhase::Publishing,
        files,
        created_directories,
    })
}

fn publish_selected(
    game: &Path,
    transaction: &Path,
    selected: &[&InstallFile],
    statuses: &[ManagedFileStatus],
    journal: &mut RepairJournal,
    shared: &RepairShared,
) -> Result<(), String> {
    let staged_root = transaction.join(STAGED_DIR);
    let rollback_root = transaction.join(ROLLBACK_DIR);
    for directory in &journal.created_directories {
        shared.checkpoint()?;
        let path = join_relative(game, directory);
        match fs::create_dir(&path) {
            Ok(()) => sync_parent(&path),
            Err(error) => {
                return Err(format!(
                    "Could not create managed parent directory {}: {error}",
                    path.display()
                ));
            }
        }
    }
    for (index, (file, snapshot)) in selected.iter().zip(statuses).enumerate() {
        shared.checkpoint()?;
        shared.set_current_file(Some(&file.path));
        let current = inspect_file_controlled(game, file, shared)?;
        if current != *snapshot {
            return Err(format!(
                "{} changed while repair was being prepared; verify again.",
                file.path
            ));
        }
        let live = join_relative(game, &file.path);
        let staged = join_relative(&staged_root, &file.path);
        let rollback = join_relative(&rollback_root, &file.path);
        create_owned_parents(
            &rollback_root,
            rollback.parent().expect("managed file has a parent"),
        )?;
        if snapshot.state == FileState::Corrupt {
            fs::rename(&live, &rollback).map_err(|error| {
                format!(
                    "Could not move {} into repair recovery; it may be in use. Repair is deferred: {error}",
                    file.path
                )
            })?;
            sync_parent(&live);
            let (length, digest) = match hash_regular_file_controlled(&rollback, shared) {
                Ok(identity) => identity,
                Err(error) => {
                    if !live.exists() {
                        let _ = fs::rename(&rollback, &live);
                        sync_parent(&live);
                    }
                    return Err(error);
                }
            };
            if Some(length) != snapshot.actual_length
                || snapshot
                    .actual_sha256
                    .as_deref()
                    .is_none_or(|expected| !digest.eq_ignore_ascii_case(expected))
            {
                if !live.exists() {
                    let _ = fs::rename(&rollback, &live);
                    sync_parent(&live);
                }
                return Err(format!(
                    "{} changed during repair publication; its latest original bytes were restored.",
                    file.path
                ));
            }
        }
        match fs::hard_link(&staged, &live) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(format!(
                    "Could not publish repaired file {}; its live path appeared during repair.",
                    file.path
                ));
            }
            Err(_) => crate::atomic_fs::rename_noreplace(&staged, &live).map_err(
                |error| {
                    format!(
                        "Could not publish repaired file {}; it may be in use. Repair is deferred: {error}",
                        file.path
                    )
                },
            )?,
        }
        sync_parent(&live);
        let journal_file = journal
            .files
            .get_mut(index)
            .filter(|entry| entry.path.eq_ignore_ascii_case(&file.path))
            .ok_or_else(|| format!("Repair journal omitted {}.", file.path))?;
        journal_file.published = true;
        write_journal(transaction, journal)?;
        if !published_file_matches_stage_or_move(&live, &staged, file)? {
            return Err(format!(
                "Published file lost its staging identity: {}.",
                file.path
            ));
        }
        let (length, digest) = hash_allowing_transaction_link_controlled(&live, shared)?;
        if length != file.length || !digest.eq_ignore_ascii_case(&file.sha256) {
            return Err(format!(
                "Published file failed verification: {}.",
                file.path
            ));
        }
        shared.complete_file();
    }
    shared.set_current_file(None);
    Ok(())
}

fn validate_staged_publication(
    transaction: &Path,
    game: &Path,
    files: &[&InstallFile],
    shared: &RepairShared,
) -> Result<(), String> {
    for file in files {
        shared.checkpoint()?;
        shared.set_current_file(Some(&file.path));
        let live = join_relative(game, &file.path);
        let staged = join_relative(&transaction.join(STAGED_DIR), &file.path);
        if !published_file_matches_stage_or_move(&live, &staged, file)? {
            return Err(format!("Published file identity changed: {}.", file.path));
        }
        let (length, digest) = hash_allowing_transaction_link_controlled(&live, shared)?;
        if length != file.length || !digest.eq_ignore_ascii_case(&file.sha256) {
            return Err(format!(
                "Published file failed verification: {}.",
                file.path
            ));
        }
        shared.complete_file();
    }
    shared.set_current_file(None);
    Ok(())
}

fn detach_published_stage_links(
    transaction: &Path,
    game: &Path,
    files: &[&InstallFile],
    shared: &RepairShared,
) -> Result<(), String> {
    let staged_root = transaction.join(STAGED_DIR);
    for file in files {
        shared.checkpoint()?;
        shared.set_current_file(Some(&file.path));
        let live = join_relative(game, &file.path);
        let staged = join_relative(&staged_root, &file.path);
        match fs::symlink_metadata(&staged) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "Could not inspect staged repair file {}: {error}",
                    staged.display()
                ));
            }
            Ok(metadata) if is_reparse(&metadata) || !metadata.is_file() => {
                return Err(format!(
                    "Refusing an unsafe staged repair file: {}",
                    staged.display()
                ));
            }
            Ok(_) => {}
        }
        if !same_two_link_file(&live, &staged)? {
            return Err(format!(
                "Published file lost its staging identity: {}.",
                file.path
            ));
        }
        fs::remove_file(&staged).map_err(|error| {
            format!(
                "Could not detach staged repair file {}: {error}",
                staged.display()
            )
        })?;
        sync_parent(&staged);
    }
    shared.set_current_file(None);
    Ok(())
}

fn validate_committed_publication(
    transaction: &Path,
    game: &Path,
    files: &[&InstallFile],
) -> Result<(), String> {
    for file in files {
        let live = join_relative(game, &file.path);
        let staged = join_relative(&transaction.join(STAGED_DIR), &file.path);
        if path_exists_safely(&staged)? {
            if !published_file_matches_stage_or_move(&live, &staged, file)? {
                return Err(format!("Published file identity changed: {}.", file.path));
            }
            let (length, digest) = hash_allowing_transaction_link(&live)?;
            if length != file.length || !digest.eq_ignore_ascii_case(&file.sha256) {
                return Err(format!(
                    "Published file failed verification: {}.",
                    file.path
                ));
            }
        } else {
            let status = inspect_file(game, file);
            if status.state != FileState::Valid {
                return Err(format!(
                    "Published file failed verification: {}.",
                    file.path
                ));
            }
        }
    }
    Ok(())
}

fn ensure_snapshots_unchanged(
    expected: &[ManagedFileStatus],
    current: &[ManagedFileStatus],
) -> Result<(), String> {
    if expected.len() != current.len() {
        return Err("The selected file set changed while repair was being prepared.".into());
    }
    for (expected, current) in expected.iter().zip(current) {
        if expected != current {
            return Err(format!(
                "{} changed while repair was being prepared; verify again.",
                expected.path
            ));
        }
    }
    Ok(())
}

fn missing_parent_directories(root: &Path, files: &[&InstallFile]) -> Result<Vec<String>, String> {
    let mut missing = HashSet::new();
    for file in files {
        let mut current = root.to_path_buf();
        let components = file.path.split('/').collect::<Vec<_>>();
        for part in components.iter().take(components.len().saturating_sub(1)) {
            current.push(part);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if !is_reparse(&metadata) && metadata.is_dir() => {}
                Ok(_) => return Err(format!("Unsafe parent directory for {}.", file.path)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let relative = current
                        .strip_prefix(root)
                        .map_err(|_| "A managed parent escaped the game directory.".to_string())?
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    missing.insert(relative);
                }
                Err(error) => {
                    return Err(format!(
                        "Could not inspect parent directory {}: {error}",
                        current.display()
                    ));
                }
            }
        }
    }
    let mut result = missing.into_iter().collect::<Vec<_>>();
    result.sort_by_key(|path| path.matches('/').count());
    Ok(result)
}

fn recover_transaction(
    transaction: &Path,
    game: &Path,
    package: &BasePackage,
    package_sha256: &str,
) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(transaction) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Could not inspect repair recovery directory: {error}"
            ));
        }
    };
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("The repair recovery path is not a safe directory.".into());
    }
    validate_transaction_tree(transaction, package, None)?;
    let owner = read_json::<OwnerMarker>(&transaction.join(OWNER_FILE), 4096)?;
    if owner.schema_version != 1 || owner.package_sha256 != package_sha256 {
        return Err("The repair recovery directory belongs to another package.".into());
    }
    let journal_path = transaction.join(JOURNAL_FILE);
    match fs::symlink_metadata(&journal_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure_no_rollback_data(transaction)?;
            cleanup_owned_transaction(transaction, package, None)
        }
        Err(error) => Err(format!("Could not inspect repair journal: {error}")),
        Ok(_) => {
            let journal = read_json::<RepairJournal>(
                &journal_path,
                journal_read_limit(package, package_sha256)?,
            )?;
            validate_journal(&journal, package, package_sha256)?;
            validate_transaction_tree(transaction, package, Some(&journal))?;
            match journal.phase {
                JournalPhase::Publishing => rollback_journal(transaction, game, package, &journal)?,
                JournalPhase::Committed => {
                    let selected = journal
                        .files
                        .iter()
                        .map(|entry| find_file(package, &entry.path))
                        .collect::<Result<Vec<_>, _>>()?;
                    if let Err(error) = validate_committed_publication(transaction, game, &selected)
                    {
                        return Err(format!(
                            "Committed repair recovery data remains because a repaired file no longer validates: {error}"
                        ));
                    }
                }
            }
            cleanup_owned_transaction(transaction, package, Some(&journal))
        }
    }
}

fn cleanup_committed_transaction(
    transaction: &Path,
    package: &BasePackage,
    package_sha256: &str,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(transaction)
        .map_err(|error| format!("Could not inspect repair recovery directory: {error}"))?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("The repair recovery path is not a safe directory.".into());
    }
    validate_transaction_tree(transaction, package, None)?;
    let owner = read_json::<OwnerMarker>(&transaction.join(OWNER_FILE), 4096)?;
    if owner.schema_version != 1 || owner.package_sha256 != package_sha256 {
        return Err("The repair recovery directory belongs to another package.".into());
    }
    let journal = read_json::<RepairJournal>(
        &transaction.join(JOURNAL_FILE),
        journal_read_limit(package, package_sha256)?,
    )?;
    validate_journal(&journal, package, package_sha256)?;
    if !matches!(journal.phase, JournalPhase::Committed) {
        return Err("The repair transaction was not durably committed.".into());
    }
    validate_transaction_tree(transaction, package, Some(&journal))?;
    cleanup_owned_transaction(transaction, package, Some(&journal))
}

fn inspect_recovery_state(
    transaction: &Path,
    package: &BasePackage,
) -> Result<(bool, Option<String>), String> {
    match fs::symlink_metadata(transaction) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((false, None)),
        Err(error) => Err(format!(
            "Could not inspect repair recovery directory: {error}"
        )),
        Ok(metadata) => {
            if is_reparse(&metadata) || !metadata.is_dir() {
                return Err("The repair recovery path is not a safe directory.".into());
            }
            validate_transaction_tree(transaction, package, None)?;
            let owner = read_json::<OwnerMarker>(&transaction.join(OWNER_FILE), 4096)?;
            if owner.schema_version != 1 {
                return Err("The repair recovery marker has an unsupported version.".into());
            }
            let journal_path = transaction.join(JOURNAL_FILE);
            match fs::symlink_metadata(&journal_path) {
                Ok(_) => {
                    validate_hash(&owner.package_sha256)?;
                    let journal = read_json::<RepairJournal>(
                        &journal_path,
                        journal_read_limit(package, &owner.package_sha256)?,
                    )?;
                    validate_journal(&journal, package, &owner.package_sha256)?;
                    validate_transaction_tree(transaction, package, Some(&journal))?;
                    Ok((true, Some("A previous repair was interrupted. Recover it before repairing more files.".into())))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    ensure_no_rollback_data(transaction)?;
                    Ok((
                        true,
                        Some("An unfinished repair staging directory needs recovery.".into()),
                    ))
                }
                Err(error) => Err(format!("Could not inspect repair journal: {error}")),
            }
        }
    }
}

fn rollback_journal(
    transaction: &Path,
    game: &Path,
    package: &BasePackage,
    journal: &RepairJournal,
) -> Result<(), String> {
    let rollback_root = transaction.join(ROLLBACK_DIR);
    for item in journal.files.iter().rev() {
        let file = find_file(package, &item.path)?;
        let live = join_relative(game, &item.path);
        let backup = join_relative(&rollback_root, &item.path);
        let backup_status = path_exists_safely(&backup)?;
        match (&item.original, backup_status) {
            (OriginalState::Corrupt { length, sha256 }, true) => {
                let (backup_length, backup_hash) = hash_regular_file(&backup)?;
                if backup_length != *length || !backup_hash.eq_ignore_ascii_case(sha256) {
                    return Err(format!(
                        "Repair recovery copy failed validation: {}.",
                        item.path
                    ));
                }
                let staged = join_relative(&transaction.join(STAGED_DIR), &item.path);
                match inspect_file(game, file).state {
                    FileState::Missing => {}
                    FileState::Unresolved | FileState::Valid
                        if remove_in_progress_publication(&live, &staged, file)? => {}
                    _ => {
                        return Err(format!(
                            "Cannot safely roll back {} because its live path changed.",
                            item.path
                        ));
                    }
                }
                ensure_live_parent(game, &live)?;
                fs::rename(&backup, &live).map_err(|error| {
                    format!("Could not restore original {}: {error}", item.path)
                })?;
                sync_parent(&live);
            }
            (OriginalState::Corrupt { length, sha256 }, false) => {
                let current = inspect_file(game, file);
                if current.state != FileState::Corrupt
                    || current.actual_length != Some(*length)
                    || current
                        .actual_sha256
                        .as_deref()
                        .is_none_or(|actual| !actual.eq_ignore_ascii_case(sha256))
                {
                    return Err(format!(
                        "Cannot safely recover {} because its original copy is missing.",
                        item.path
                    ));
                }
            }
            (OriginalState::Missing, true) => {
                return Err(format!(
                    "Unexpected recovery copy exists for originally missing file {}.",
                    item.path
                ));
            }
            (OriginalState::Missing, false) => {
                let staged = join_relative(&transaction.join(STAGED_DIR), &item.path);
                match inspect_file(game, file).state {
                    FileState::Missing => {}
                    FileState::Unresolved | FileState::Valid | FileState::Corrupt => {
                        let _ = remove_in_progress_publication(&live, &staged, file)?;
                    }
                }
            }
        }
    }
    for relative in journal.created_directories.iter().rev() {
        let path = join_relative(game, relative);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Could not inspect created directory: {error}")),
            Ok(metadata) if is_reparse(&metadata) || !metadata.is_dir() => {
                return Err(format!(
                    "Created repair directory changed: {}",
                    path.display()
                ));
            }
            Ok(_) => match fs::remove_dir(&path) {
                Ok(()) => sync_parent(&path),
                Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!("Could not remove empty repair directory: {error}"));
                }
            },
        }
    }
    Ok(())
}

fn remove_in_progress_publication(
    live: &Path,
    staged: &Path,
    spec: &InstallFile,
) -> Result<bool, String> {
    match fs::symlink_metadata(staged) {
        Ok(_) if !same_two_link_file(live, staged)? => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // On filesystems without hard-link support, publication atomically moves
            // the verified staged file. Its absence records that the move completed.
        }
        Err(error) => {
            return Err(format!(
                "Could not inspect repair staging identity {}: {error}",
                staged.display()
            ));
        }
    }
    let (length, digest) = hash_allowing_transaction_link(live)?;
    if length != spec.length || !digest.eq_ignore_ascii_case(&spec.sha256) {
        return Err(format!(
            "The interrupted repair link failed catalog verification: {}.",
            spec.path
        ));
    }
    if staged.exists() && !same_two_link_file(live, staged)? {
        return Err(format!(
            "The interrupted repair publication changed during recovery: {}.",
            spec.path
        ));
    }
    fs::remove_file(live).map_err(|error| {
        format!(
            "Could not remove interrupted replacement {}: {error}",
            spec.path
        )
    })?;
    sync_parent(live);
    Ok(true)
}

fn published_file_matches_stage_or_move(
    live: &Path,
    staged: &Path,
    spec: &InstallFile,
) -> Result<bool, String> {
    match fs::symlink_metadata(staged) {
        Ok(_) => same_two_link_file(live, staged),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let (length, digest) = hash_allowing_transaction_link(live)?;
            Ok(length == spec.length && digest.eq_ignore_ascii_case(&spec.sha256))
        }
        Err(error) => Err(format!(
            "Could not inspect staged repair file {}: {error}",
            staged.display()
        )),
    }
}

fn same_two_link_file(left: &Path, right: &Path) -> Result<bool, String> {
    let left_file = open_regular_for_identity(left)?;
    let right_file = open_regular_for_identity(right)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let left_metadata = left_file
            .metadata()
            .map_err(|error| format!("Could not inspect repair file identity: {error}"))?;
        let right_metadata = right_file
            .metadata()
            .map_err(|error| format!("Could not inspect repair file identity: {error}"))?;
        Ok(left_metadata.nlink() == 2
            && right_metadata.nlink() == 2
            && left_metadata.dev() == right_metadata.dev()
            && left_metadata.ino() == right_metadata.ino())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };

        let mut left_info = BY_HANDLE_FILE_INFORMATION::default();
        let mut right_info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe {
            GetFileInformationByHandle(HANDLE(left_file.as_raw_handle()), &mut left_info).and_then(
                |()| {
                    GetFileInformationByHandle(HANDLE(right_file.as_raw_handle()), &mut right_info)
                },
            )
        }
        .map_err(|error| format!("Could not inspect repair file identity: {error}"))?;
        Ok(left_info.nNumberOfLinks == 2
            && right_info.nNumberOfLinks == 2
            && left_info.dwVolumeSerialNumber == right_info.dwVolumeSerialNumber
            && left_info.nFileIndexHigh == right_info.nFileIndexHigh
            && left_info.nFileIndexLow == right_info.nFileIndexLow)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (left_file, right_file);
        Ok(false)
    }
}

fn open_regular_for_identity(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    set_no_follow(&mut options);
    let file = options
        .open(path)
        .map_err(|error| format!("Could not inspect repair file {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Could not inspect repair file {}: {error}", path.display()))?;
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "Refusing a non-regular repair file: {}",
            path.display()
        ));
    }
    Ok(file)
}

fn hash_allowing_transaction_link(path: &Path) -> Result<(u64, String), String> {
    hash_allowing_transaction_link_inner(path, None)
}

fn hash_allowing_transaction_link_controlled(
    path: &Path,
    shared: &RepairShared,
) -> Result<(u64, String), String> {
    hash_allowing_transaction_link_inner(path, Some(shared))
}

fn hash_allowing_transaction_link_inner(
    path: &Path,
    shared: Option<&RepairShared>,
) -> Result<(u64, String), String> {
    let mut file = open_regular_for_identity(path)?;
    let length = file
        .metadata()
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?
        .len();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; IO_BUFFER_BYTES];
    loop {
        if let Some(shared) = shared {
            shared.checkpoint()?;
        }
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Could not hash {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        if let Some(shared) = shared {
            shared.add_bytes(read as u64);
        }
    }
    let after = file
        .metadata()
        .map_err(|error| {
            format!(
                "Could not inspect {} after hashing: {error}",
                path.display()
            )
        })?
        .len();
    if after != length {
        return Err(format!("File changed while hashing: {}", path.display()));
    }
    Ok((length, format!("{:x}", hasher.finalize())))
}

fn validate_journal(
    journal: &RepairJournal,
    package: &BasePackage,
    package_sha256: &str,
) -> Result<(), String> {
    if journal.schema_version != 1 || journal.package_sha256 != package_sha256 {
        return Err("The repair journal does not match this package.".into());
    }
    let mut seen = HashSet::new();
    for item in &journal.files {
        let file = find_file(package, &item.path)?;
        if matches!(journal.phase, JournalPhase::Committed) && !item.published {
            return Err(format!(
                "Committed repair journal has an unpublished file: {}.",
                item.path
            ));
        }
        if !seen.insert(item.path.to_ascii_lowercase()) {
            return Err(format!("Duplicate path in repair journal: {}.", item.path));
        }
        match &item.original {
            OriginalState::Missing => {}
            OriginalState::Corrupt { sha256, .. } => {
                if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(format!(
                        "Invalid original hash in repair journal: {}.",
                        item.path
                    ));
                }
                let _ = file;
            }
        }
    }
    let mut directories = HashSet::new();
    for directory in &journal.created_directories {
        validate_relative_path(directory)?;
        if !directories.insert(directory.to_ascii_lowercase()) {
            return Err(format!(
                "Duplicate directory in repair journal: {directory}."
            ));
        }
        if !journal.files.iter().any(|file| {
            file.path
                .to_ascii_lowercase()
                .starts_with(&format!("{}/", directory.to_ascii_lowercase()))
        }) {
            return Err(format!(
                "Unrelated directory in repair journal: {directory}."
            ));
        }
    }
    Ok(())
}

fn validate_transaction_tree(
    transaction: &Path,
    package: &BasePackage,
    journal: Option<&RepairJournal>,
) -> Result<(), String> {
    let allowed_files = journal
        .map(|journal| {
            journal
                .files
                .iter()
                .map(|file| file.path.to_ascii_lowercase())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_else(|| {
            package
                .final_files
                .iter()
                .map(|file| file.path.to_ascii_lowercase())
                .collect::<HashSet<_>>()
        });
    let allowed_dirs = package
        .final_files
        .iter()
        .flat_map(|file| parent_paths(&file.path))
        .map(|path| path.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut pending = vec![(transaction.to_path_buf(), String::new())];
    while let Some((directory, relative)) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("Could not inspect repair recovery data: {error}"))?
        {
            let entry = entry
                .map_err(|error| format!("Could not inspect repair recovery entry: {error}"))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
            if is_reparse(&metadata) {
                return Err(format!(
                    "Repair recovery contains a reparse point: {}",
                    path.display()
                ));
            }
            let leaf = entry.file_name().to_string_lossy().into_owned();
            let child_relative = if relative.is_empty() {
                leaf.clone()
            } else {
                format!("{relative}/{leaf}")
            };
            if metadata.is_dir() {
                if relative.is_empty() {
                    if !matches!(leaf.as_str(), STAGED_DIR | ROLLBACK_DIR) {
                        return Err(format!("Unexpected repair recovery directory: {leaf}"));
                    }
                } else if relative == STAGED_DIR || relative == ROLLBACK_DIR {
                    let folded = child_relative[relative.len() + 1..].to_ascii_lowercase();
                    if !allowed_dirs.contains(&folded) {
                        return Err(format!(
                            "Unexpected repair recovery directory: {child_relative}"
                        ));
                    }
                } else if relative.starts_with(&format!("{STAGED_DIR}/"))
                    || relative.starts_with(&format!("{ROLLBACK_DIR}/"))
                {
                    let prefix = if relative.starts_with(&format!("{STAGED_DIR}/")) {
                        STAGED_DIR
                    } else {
                        ROLLBACK_DIR
                    };
                    let inner = child_relative[prefix.len() + 1..].to_ascii_lowercase();
                    if !allowed_dirs.contains(&inner) {
                        return Err(format!(
                            "Unexpected repair recovery directory: {child_relative}"
                        ));
                    }
                } else {
                    return Err(format!(
                        "Unexpected repair recovery directory: {child_relative}"
                    ));
                }
                pending.push((path, child_relative));
            } else if metadata.is_file() {
                let allowed = match relative.as_str() {
                    "" => matches!(leaf.as_str(), OWNER_FILE | JOURNAL_FILE | JOURNAL_NEXT_FILE),
                    STAGED_DIR | ROLLBACK_DIR => allowed_files.contains(&leaf.to_ascii_lowercase()),
                    _ if relative.starts_with(&format!("{STAGED_DIR}/")) => allowed_files
                        .contains(&child_relative[STAGED_DIR.len() + 1..].to_ascii_lowercase()),
                    _ if relative.starts_with(&format!("{ROLLBACK_DIR}/")) => allowed_files
                        .contains(&child_relative[ROLLBACK_DIR.len() + 1..].to_ascii_lowercase()),
                    _ => false,
                };
                if !allowed {
                    return Err(format!("Unexpected repair recovery file: {child_relative}"));
                }
            } else {
                return Err(format!(
                    "Unexpected special repair recovery entry: {}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn ensure_no_rollback_data(transaction: &Path) -> Result<(), String> {
    let rollback = transaction.join(ROLLBACK_DIR);
    match fs::read_dir(&rollback) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Could not inspect repair rollback directory: {error}"
        )),
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err("Repair rollback data has no readable transaction journal.".into());
            }
            Ok(())
        }
    }
}

fn cleanup_unjournaled_transaction(
    transaction: &Path,
    package: &BasePackage,
    package_sha256: &str,
) -> Result<(), String> {
    let owner = read_json::<OwnerMarker>(&transaction.join(OWNER_FILE), 4096)?;
    if owner.schema_version != 1 || owner.package_sha256 != package_sha256 {
        return Err("Refusing to remove an unowned repair staging directory.".into());
    }
    validate_transaction_tree(transaction, package, None)?;
    ensure_no_rollback_data(transaction)?;
    cleanup_owned_transaction(transaction, package, None)
}

fn cleanup_owned_transaction(
    transaction: &Path,
    package: &BasePackage,
    journal: Option<&RepairJournal>,
) -> Result<(), String> {
    validate_transaction_tree(transaction, package, journal)?;
    fs::remove_dir_all(transaction)
        .map_err(|error| format!("Could not remove completed repair recovery data: {error}"))
}

fn create_transaction(
    transaction: &Path,
    package: &BasePackage,
    package_sha256: &str,
) -> Result<PathBuf, String> {
    fs::create_dir(transaction)
        .map_err(|error| format!("Could not create repair recovery directory: {error}"))?;
    let marker = OwnerMarker {
        schema_version: 1,
        package_sha256: package_sha256.to_owned(),
    };
    write_json_new(&transaction.join(OWNER_FILE), &marker)?;
    for child in [STAGED_DIR, ROLLBACK_DIR] {
        if let Err(error) = fs::create_dir(transaction.join(child)) {
            let _ = cleanup_unjournaled_transaction(transaction, package, package_sha256);
            return Err(format!(
                "Could not create repair {child} directory: {error}"
            ));
        }
    }
    Ok(transaction.to_path_buf())
}

fn write_journal(transaction: &Path, journal: &RepairJournal) -> Result<(), String> {
    let next = transaction.join(JOURNAL_NEXT_FILE);
    match fs::symlink_metadata(&next) {
        Ok(metadata) if !is_reparse(&metadata) && metadata.is_file() => {
            fs::remove_file(&next)
                .map_err(|error| format!("Could not reset repair journal temp file: {error}"))?;
        }
        Ok(_) => return Err("Repair journal temp path is unsafe.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "Could not inspect repair journal temp path: {error}"
            ));
        }
    }
    write_json_new(&next, journal)?;
    fs::rename(&next, transaction.join(JOURNAL_FILE))
        .map_err(|error| format!("Could not persist repair transaction journal: {error}"))?;
    sync_parent(&transaction.join(JOURNAL_FILE));
    Ok(())
}

fn write_json_new(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("Could not encode repair state: {error}"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("Could not create repair state: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Could not persist repair state: {error}"))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path, max_bytes: u64) -> Result<T, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect repair state {}: {error}", path.display()))?;
    if is_reparse(&metadata) || !metadata.is_file() || metadata.len() > max_bytes {
        return Err(format!("Invalid repair state file: {}", path.display()));
    }
    let (length, _) = hash_regular_file(path)?;
    let bytes = fs::read(path)
        .map_err(|error| format!("Could not read repair state {}: {error}", path.display()))?;
    if length != bytes.len() as u64 {
        return Err(format!(
            "Repair state changed while reading: {}",
            path.display()
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid repair state {}: {error}", path.display()))
}

fn cache_object_status(cache: &Path, object: &ObjectSpec) -> Result<CacheStatus, String> {
    let path = cache.join(format!(
        "{}-{}.object",
        object.sha256.to_ascii_lowercase(),
        object.length
    ));
    let resolved = resolve_cache_path(cache)?;
    check_no_reparse_between(&resolved.nearest_existing, &path)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(CacheStatus {
            valid: false,
            problem: None,
        }),
        Err(error) => Ok(CacheStatus {
            valid: false,
            problem: Some(format!("Could not inspect cached archive: {error}")),
        }),
        Ok(metadata) if is_reparse(&metadata) || !metadata.is_file() => Ok(CacheStatus {
            valid: false,
            problem: Some("A cached archive path is linked or is not a regular file.".into()),
        }),
        Ok(_) => match hash_regular_file(&path) {
            Ok((length, digest)) => Ok(CacheStatus {
                valid: length == object.length && digest.eq_ignore_ascii_case(&object.sha256),
                problem: None,
            }),
            Err(error) => Ok(CacheStatus {
                valid: false,
                problem: Some(error),
            }),
        },
    }
}

struct ResolvedCachePath {
    path: PathBuf,
    nearest_existing: PathBuf,
}

fn resolve_cache_path(path: &Path) -> Result<ResolvedCachePath, String> {
    if !path.is_absolute() || has_parent_alias(path) {
        return Err(format!(
            "Expected a clean absolute cache path: {}",
            path.display()
        ));
    }
    let mut probe = path.to_path_buf();
    let mut missing = Vec::new();
    let deepest = loop {
        match fs::symlink_metadata(&probe) {
            Ok(metadata) => {
                if is_reparse(&metadata) {
                    return Err(format!(
                        "Refusing a cache reparse point: {}",
                        probe.display()
                    ));
                }
                if !metadata.is_dir() {
                    return Err(format!(
                        "Cache ancestor is not a directory: {}",
                        probe.display()
                    ));
                }
                break probe;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let leaf = probe
                    .file_name()
                    .ok_or_else(|| "Could not resolve the cache path.".to_string())?;
                missing.push(leaf.to_os_string());
                if !probe.pop() {
                    return Err("Could not resolve the cache path.".into());
                }
            }
            Err(error) => {
                return Err(format!(
                    "Could not inspect cache path {}: {error}",
                    probe.display()
                ));
            }
        }
    };
    inspect_directory_ancestors(&deepest)?;
    let nearest_existing = fs::canonicalize(&deepest)
        .map_err(|error| format!("Could not resolve cache directory: {error}"))?;
    let mut resolved = nearest_existing.clone();
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(ResolvedCachePath {
        path: resolved,
        nearest_existing,
    })
}

fn resolve_existing_directory(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || has_parent_alias(path) {
        return Err(format!(
            "Expected a clean absolute game directory: {}",
            path.display()
        ));
    }
    inspect_directory_ancestors(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "Could not inspect game directory {}: {error}",
            path.display()
        )
    })?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("The selected game path is not a safe directory.".into());
    }
    fs::canonicalize(path).map_err(|error| format!("Could not resolve game directory: {error}"))
}

fn inspect_directory_ancestors(path: &Path) -> Result<(), String> {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let metadata = fs::symlink_metadata(&prefix).map_err(|error| {
            format!("Could not inspect directory {}: {error}", prefix.display())
        })?;
        if is_reparse(&metadata) {
            return Err(format!(
                "Refusing a directory reparse point: {}",
                prefix.display()
            ));
        }
        if !metadata.is_dir() {
            return Err(format!(
                "A path ancestor is not a directory: {}",
                prefix.display()
            ));
        }
    }
    Ok(())
}

fn check_no_reparse_between(root: &Path, path: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "Cache object escaped the cache directory.".to_string())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("Invalid cache object path.".into());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_reparse(&metadata) => {
                return Err(format!(
                    "Refusing a cache reparse point: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("Could not inspect cache object: {error}")),
        }
    }
    Ok(())
}

fn transaction_path(game: &Path) -> Result<PathBuf, String> {
    let parent = game
        .parent()
        .ok_or_else(|| "The game directory has no parent for repair recovery.".to_string())?;
    let leaf = game
        .file_name()
        .ok_or_else(|| "The game directory has no final path component.".to_string())?;
    let mut recovery_leaf = leaf.to_os_string();
    recovery_leaf.push(TRANSACTION_SUFFIX);
    Ok(parent.join(recovery_leaf))
}

fn path_exists_safely(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Could not inspect repair recovery file: {error}")),
        Ok(metadata) if is_reparse(&metadata) || !metadata.is_file() => Err(format!(
            "Repair recovery copy is unsafe: {}",
            path.display()
        )),
        Ok(_) => Ok(true),
    }
}

fn ensure_live_parent(game: &Path, path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "A managed file has no parent directory.".to_string())?;
    let relative = parent
        .strip_prefix(game)
        .map_err(|_| "A managed file parent escaped the game directory.".to_string())?;
    let mut current = game.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("Invalid managed parent path.".into());
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            format!(
                "Could not inspect managed parent {}: {error}",
                current.display()
            )
        })?;
        if is_reparse(&metadata) || !metadata.is_dir() {
            return Err(format!(
                "Unsafe managed parent directory: {}",
                current.display()
            ));
        }
    }
    Ok(())
}

fn find_file<'a>(package: &'a BasePackage, path: &str) -> Result<&'a InstallFile, String> {
    package
        .final_files
        .iter()
        .find(|file| file.path.eq_ignore_ascii_case(path))
        .ok_or_else(|| format!("Path is not in the managed game catalog: {path}"))
}

fn package_identity(package: &BasePackage) -> String {
    let encoded = serde_json::to_vec(package).expect("BasePackage is infallibly serializable");
    format!("{:x}", Sha256::digest(encoded))
}

fn join_relative(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |mut path, part| {
            path.push(part);
            path
        })
}

fn parent_paths(path: &str) -> Vec<String> {
    path.match_indices('/')
        .map(|(index, _)| path[..index].to_owned())
        .collect()
}

fn maximum_repair_journal(
    package: &BasePackage,
    package_sha256: &str,
) -> Result<RepairJournal, String> {
    validate_hash(package_sha256)?;
    let files = package
        .final_files
        .iter()
        .map(|file| JournalFile {
            path: file.path.clone(),
            original: OriginalState::Corrupt {
                length: u64::MAX,
                sha256: "f".repeat(64),
            },
            published: false,
        })
        .collect();
    let mut created_directories = package
        .final_files
        .iter()
        .flat_map(|file| parent_paths(&file.path))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    created_directories.sort();
    Ok(RepairJournal {
        schema_version: 1,
        package_sha256: package_sha256.to_owned(),
        phase: JournalPhase::Publishing,
        files,
        created_directories,
    })
}

fn journal_read_limit(package: &BasePackage, package_sha256: &str) -> Result<u64, String> {
    let maximum = maximum_repair_journal(package, package_sha256)?;
    let bytes = serde_json::to_vec(&maximum)
        .map_err(|error| format!("Could not size repair journal: {error}"))?;
    u64::try_from(bytes.len()).map_err(|_| "Repair journal size overflow.".into())
}

fn has_parent_alias(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    false
}

fn set_no_follow(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
}

fn validate_regular_handle(file: &File, path: &Path) -> Result<(), String> {
    let metadata = file
        .metadata()
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err(format!("Refusing a non-regular file: {}", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(format!(
                "Refusing a hard-linked managed file: {}",
                path.display()
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{
            FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandleEx,
        };
        let mut info = FILE_STANDARD_INFO::default();
        unsafe {
            GetFileInformationByHandleEx(
                HANDLE(file.as_raw_handle()),
                FileStandardInfo,
                &mut info as *mut _ as _,
                std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
            )
        }
        .map_err(|error| format!("Could not inspect {} links: {error}", path.display()))?;
        if info.NumberOfLinks != 1 {
            return Err(format!(
                "Refusing a hard-linked managed file: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn sync_parent(path: &Path) {
    #[cfg(unix)]
    {
        if let Some(parent) = path.parent()
            && let Ok(directory) = File::open(parent)
        {
            let _ = directory.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Write};
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::content::manifest::{BaseArchive, InstallFile};

    fn verify(
        destination: &Path,
        cache: &Path,
        package: &BasePackage,
    ) -> Result<VerificationSummary, String> {
        package.validate()?;
        let sources = repair_sources(package)?;
        let game = resolve_existing_directory(destination)?;
        let cache_path = resolve_cache_path(cache)?;
        let files = scan_files_controlled(&game, &package.final_files, &RepairShared::new())?;
        let transaction = transaction_path(&game)?;
        let (recovery_required, recovery_detail) = inspect_recovery_state(&transaction, package)?;
        let quote = quote_for_statuses(
            &game,
            &cache_path.path,
            &cache_path.nearest_existing,
            package,
            &sources,
            &files,
        )?;
        Ok(summary(&files, recovery_required, recovery_detail, quote))
    }

    #[test]
    fn repair_selection_scales_to_the_full_managed_inventory() {
        const INVENTORY_SIZE: usize = 191_960;
        let mut files = Vec::with_capacity(INVENTORY_SIZE);
        let mut statuses = Vec::with_capacity(INVENTORY_SIZE);
        let mut expected_repair_indices = Vec::new();

        for index in 0..INVENTORY_SIZE {
            let path = format!("managed/file-{index:06}.dat");
            let state = if index % 30_000 == 0 {
                expected_repair_indices.push(index);
                if index % 60_000 == 0 {
                    FileState::Corrupt
                } else {
                    FileState::Missing
                }
            } else {
                FileState::Valid
            };
            files.push(InstallFile {
                path: path.clone(),
                length: 1,
                sha256: "0".repeat(64),
            });
            statuses.push(ManagedFileStatus {
                path,
                state,
                expected_length: 1,
                actual_length: (state != FileState::Missing).then_some(1),
                actual_sha256: None,
                detail: None,
            });
        }

        let (selected, selected_statuses) = select_repair_files(&files, &statuses).unwrap();
        assert_eq!(selected.len(), expected_repair_indices.len());
        assert_eq!(selected_statuses.len(), expected_repair_indices.len());
        for ((file, status), index) in selected
            .iter()
            .zip(&selected_statuses)
            .zip(expected_repair_indices)
        {
            assert_eq!(file.path, format!("managed/file-{index:06}.dat"));
            assert_eq!(status.path, file.path);
            assert_ne!(status.state, FileState::Valid);
        }
    }

    struct Fixture {
        package: BasePackage,
        game: PathBuf,
        cache: PathBuf,
        archive_bytes: Vec<u8>,
        contents: HashMap<String, Vec<u8>>,
    }

    fn fixture(root: &Path) -> Fixture {
        let game = root.join("game");
        let cache = root.join("cache");
        fs::create_dir_all(&game).unwrap();
        fs::create_dir_all(&cache).unwrap();
        let contents = HashMap::from([
            ("ffxivboot.exe".to_owned(), b"boot exe".to_vec()),
            ("ffxivgame.exe".to_owned(), b"game exe".to_vec()),
            (
                "data/client.dat".to_owned(),
                b"managed client data".to_vec(),
            ),
            (
                "boot.ver".to_owned(),
                FFXIV_BOOT_VERSION.as_bytes().to_vec(),
            ),
            (
                "game.ver".to_owned(),
                FFXIV_GAME_VERSION.as_bytes().to_vec(),
            ),
        ]);
        for (relative, bytes) in &contents {
            let path = join_relative(&game, relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }

        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for relative in ["ffxivboot.exe", "ffxivgame.exe", "data/client.dat"] {
            writer
                .start_file(relative, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&contents[relative]).unwrap();
        }
        let archive_bytes = writer.finish().unwrap().into_inner();
        let archive_files = ["ffxivboot.exe", "ffxivgame.exe", "data/client.dat"]
            .into_iter()
            .map(|path| install_file(path, &contents[path]))
            .collect::<Vec<_>>();
        let final_files = contents
            .iter()
            .map(|(path, bytes)| install_file(path, bytes))
            .collect::<Vec<_>>();
        let archive_sha256 = format!("{:x}", Sha256::digest(&archive_bytes));
        let archive = BaseArchive {
            object: ObjectSpec {
                object_key: "game/test/client.zip".into(),
                length: archive_bytes.len() as u64,
                sha256: archive_sha256,
            },
            files: archive_files,
            layout: ArchiveLayout::Flat,
            excluded_files: Vec::new(),
            empty_directories: Vec::new(),
            apple_metadata_files: 0,
        };
        let staging_bytes = final_files.iter().map(|file| file.length).sum();
        let package = BasePackage {
            target_version: FFXIV_GAME_VERSION.into(),
            archives: vec![archive],
            final_files,
            staging_bytes,
        };
        package.validate().unwrap();
        fs::write(
            cache.join(format!(
                "{}-{}.object",
                package.archives[0].object.sha256, package.archives[0].object.length
            )),
            &archive_bytes,
        )
        .unwrap();
        Fixture {
            package,
            game,
            cache,
            archive_bytes,
            contents,
        }
    }

    fn install_file(path: &str, bytes: &[u8]) -> InstallFile {
        InstallFile {
            path: path.into(),
            length: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        }
    }

    fn local_archive_server(bytes: Vec<u8>) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/zip\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
        });
        (format!("http://{address}/"), server)
    }

    fn managed(root: &Path, package: &BasePackage, path: &str) -> PathBuf {
        join_relative(root, &find_file(package, path).unwrap().path)
    }

    fn run_repair(
        destination: &Path,
        root: &str,
        fixture: &Fixture,
    ) -> Result<RepairResult, String> {
        repair_all(
            destination,
            root,
            &fixture.cache,
            &fixture.package,
            &RepairShared::new(),
        )
    }

    fn add_large_managed_file(fixture: &mut Fixture, path: &str, length: usize) {
        let bytes = vec![0x5a; length];
        fixture.contents.insert(path.to_owned(), bytes.clone());
        let file = install_file(path, &bytes);
        fixture.package.final_files.push(file.clone());
        fixture
            .package
            .final_files
            .sort_by(|left, right| left.path.cmp(&right.path));
        let archive_files = &mut fixture.package.archives[0].files;
        archive_files.push(file);
        archive_files.sort_by(|left, right| left.path.cmp(&right.path));

        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for archived_file in archive_files.iter() {
            writer
                .start_file(&archived_file.path, SimpleFileOptions::default())
                .unwrap();
            writer
                .write_all(&fixture.contents[&archived_file.path])
                .unwrap();
        }
        fixture.archive_bytes = writer.finish().unwrap().into_inner();
        let old_object = fixture.package.archives[0].object.clone();
        fixture.package.archives[0].object.length = fixture.archive_bytes.len() as u64;
        fixture.package.archives[0].object.sha256 =
            format!("{:x}", Sha256::digest(&fixture.archive_bytes));
        fixture.package.staging_bytes = fixture
            .package
            .final_files
            .iter()
            .map(|file| file.length)
            .sum();
        fixture.package.validate().unwrap();

        fs::remove_file(fixture.cache.join(format!(
            "{}-{}.object",
            old_object.sha256, old_object.length
        )))
        .unwrap();
        fs::write(
            fixture.cache.join(format!(
                "{}-{}.object",
                fixture.package.archives[0].object.sha256,
                fixture.package.archives[0].object.length
            )),
            &fixture.archive_bytes,
        )
        .unwrap();
        let large_path = join_relative(&fixture.game, path);
        fs::write(large_path, bytes).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn dangling_recovery_link_blocks_readiness_without_being_removed() {
        let temporary = crate::content::test_support::tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let transaction = transaction_path(&fixture.game).unwrap();
        std::os::unix::fs::symlink(temporary.path().join("missing"), &transaction).unwrap();
        assert!(recovery_pending(&fixture.game));
        assert!(!crate::content::check_game_version(&fixture.game));
        assert!(
            fs::symlink_metadata(transaction)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn second_process_cannot_recover_active_repair() {
        const CHILD_ROOT: &str = "BAHAMUT_TEST_REPAIR_LOCK_ROOT";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let root = PathBuf::from(root);
            let package: BasePackage =
                serde_json::from_slice(&fs::read(root.join("package.json")).unwrap()).unwrap();
            let error = repair_all(
                &root.join("game"),
                "https://unused.invalid/",
                &root.join("cache"),
                &package,
                &RepairShared::new(),
            )
            .unwrap_err();
            assert!(error.contains("Another launcher is repairing"), "{error}");
            return;
        }

        let temporary = crate::content::test_support::tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let game = canonical_game_root(&fixture.game).unwrap();
        let lock = acquire_repair_lock(&game).unwrap();
        let file = find_file(&fixture.package, "data/client.dat").unwrap();
        let live = join_relative(&game, &file.path);
        fs::write(&live, b"corrupt original").unwrap();
        let before = scan_selected(&game, &[file]);
        let identity = package_identity(&fixture.package);
        let transaction = transaction_path(&game).unwrap();
        create_transaction(&transaction, &fixture.package, &identity).unwrap();
        let owner_bytes = fs::read(transaction.join(OWNER_FILE)).unwrap();
        fs::write(
            temporary.path().join("package.json"),
            serde_json::to_vec(&fixture.package).unwrap(),
        )
        .unwrap();

        for journaled in [false, true] {
            if journaled {
                write_staged_bytes(
                    &transaction.join(STAGED_DIR),
                    file,
                    &fixture.contents["data/client.dat"],
                )
                .unwrap();
                let journal = make_journal(&identity, &before, Vec::new()).unwrap();
                write_journal(&transaction, &journal).unwrap();
            }
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "content::repair::tests::second_process_cannot_recover_active_repair",
                    "--nocapture",
                ])
                .env(CHILD_ROOT, temporary.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "child failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                "child test was not discovered: {}",
                String::from_utf8_lossy(&output.stdout)
            );
            assert_eq!(fs::read(transaction.join(OWNER_FILE)).unwrap(), owner_bytes);
            assert_eq!(fs::read(&live).unwrap(), b"corrupt original");
            assert_eq!(transaction.join(JOURNAL_FILE).is_file(), journaled);
            if journaled {
                assert_eq!(
                    fs::read(join_relative(&transaction.join(STAGED_DIR), &file.path)).unwrap(),
                    fixture.contents["data/client.dat"]
                );
            }
        }
        drop(lock);
        assert!(
            run_repair(&game, "https://unused.invalid/", &fixture)
                .unwrap()
                .complete
        );
        assert!(!transaction.exists());
        assert_eq!(fs::read(live).unwrap(), fixture.contents["data/client.dat"]);
        // The stable lock remains reusable after the completed repair.
        assert!(acquire_repair_lock(&game).is_ok());
    }

    #[test]
    fn verification_omits_valid_rows_but_counts_the_full_inventory() {
        let temporary = crate::content::test_support::tempdir().unwrap();
        let fixture = fixture(temporary.path());

        let verified = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();

        assert!(verified.files.is_empty());
        assert_eq!(verified.valid_count, fixture.package.final_files.len());
        assert_eq!(verified.missing_count, 0);
        assert_eq!(verified.corrupt_count, 0);
        assert_eq!(verified.unresolved_count, 0);
    }

    #[test]
    fn verification_hashes_equal_length_corruption_and_repair_preserves_user_state() {
        let temporary = crate::content::test_support::tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let user_state = fixture.game.join("user");
        fs::create_dir_all(&user_state).unwrap();
        fs::write(user_state.join("custom-package.dat"), b"player-owned").unwrap();
        fs::write(fixture.game.join("settings.ini"), b"player settings").unwrap();
        fs::write(
            managed(&fixture.game, &fixture.package, "data/client.dat"),
            b"X".repeat(fixture.contents["data/client.dat"].len()),
        )
        .unwrap();
        fs::remove_file(managed(&fixture.game, &fixture.package, "game.ver")).unwrap();

        let verified = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
        let corrupt = verified
            .files
            .iter()
            .find(|file| file.path == "data/client.dat")
            .unwrap();
        assert_eq!(corrupt.state, FileState::Corrupt);
        assert!(
            verified
                .files
                .iter()
                .all(|file| file.state != FileState::Valid)
        );
        assert_eq!(verified.valid_count, fixture.package.final_files.len() - 2);
        assert_eq!(verified.missing_count, 1);
        assert_eq!(corrupt.actual_length, Some(corrupt.expected_length));
        assert_ne!(
            corrupt.actual_sha256.as_deref(),
            Some(
                find_file(&fixture.package, "data/client.dat")
                    .unwrap()
                    .sha256
                    .as_str()
            )
        );
        assert_eq!(verified.quote.download_bytes, 0);

        let result = run_repair(&fixture.game, "https://unused.invalid/", &fixture).unwrap();
        assert!(result.complete);
        assert_eq!(result.repaired_files.len(), 2);
        assert_eq!(
            fs::read(managed(&fixture.game, &fixture.package, "data/client.dat")).unwrap(),
            fixture.contents["data/client.dat"]
        );
        assert_eq!(
            fs::read(user_state.join("custom-package.dat")).unwrap(),
            b"player-owned"
        );
        assert_eq!(
            fs::read(fixture.game.join("settings.ini")).unwrap(),
            b"player settings"
        );
        assert!(!transaction_path(&fixture.game).unwrap().exists());
    }

    #[test]
    fn missing_file_is_quoted_and_restored_from_the_verified_archive_cache() {
        let temporary = crate::content::test_support::tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let missing = managed(&fixture.game, &fixture.package, "data/client.dat");
        fs::remove_file(&missing).unwrap();
        let verified = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
        assert_eq!(verified.missing_count, 1);
        assert_eq!(
            verified
                .files
                .iter()
                .find(|file| file.path == "data/client.dat")
                .unwrap()
                .state,
            FileState::Missing
        );
        assert_eq!(verified.quote.download_bytes, 0);

        let result = run_repair(&fixture.game, "https://unused.invalid/", &fixture).unwrap();
        assert!(result.complete);
        assert_eq!(
            fs::read(missing).unwrap(),
            fixture.contents["data/client.dat"]
        );
    }

    #[test]
    fn unresolved_managed_path_is_never_overwritten() {
        let temporary = crate::content::test_support::tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let managed_path = managed(&fixture.game, &fixture.package, "data/client.dat");
        fs::remove_file(&managed_path).unwrap();
        fs::create_dir(&managed_path).unwrap();
        fs::write(managed_path.join("user-data"), b"do not remove").unwrap();
        let verified = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
        assert_eq!(verified.unresolved_count, 1);
        let error = run_repair(&fixture.game, "https://unused.invalid/", &fixture).unwrap_err();
        assert!(error.contains("Cannot safely repair"));
        assert_eq!(
            fs::read(managed_path.join("user-data")).unwrap(),
            b"do not remove"
        );
    }

    #[cfg(windows)]
    #[test]
    fn unreadable_managed_file_blocks_repair() {
        use std::os::windows::fs::OpenOptionsExt;

        let temporary = crate::content::test_support::tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let path = managed(&fixture.game, &fixture.package, "data/client.dat");
        let _exclusive_read = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();

        let status = inspect_file(
            &fixture.game,
            find_file(&fixture.package, "data/client.dat").unwrap(),
        );
        assert_eq!(status.state, FileState::Unresolved);
        let error = run_repair(&fixture.game, "https://unused.invalid/", &fixture).unwrap_err();
        assert!(error.contains("Cannot safely repair"));
    }

    #[test]
    fn invalid_cached_zip_is_quoted_as_a_full_transfer_and_revalidated_before_repair() {
        let temporary = crate::content::test_support::tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let expected_cache = fixture.cache.join(format!(
            "{}-{}.object",
            fixture.package.archives[0].object.sha256, fixture.package.archives[0].object.length
        ));
        fs::write(&expected_cache, b"invalid cached archive").unwrap();
        fs::write(
            managed(&fixture.game, &fixture.package, "data/client.dat"),
            b"X".repeat(fixture.contents["data/client.dat"].len()),
        )
        .unwrap();

        let verified = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
        assert_eq!(
            verified.quote.download_bytes,
            fixture.package.archives[0].object.length
        );
        assert_eq!(
            verified.quote.full_archive_bytes,
            fixture.archive_bytes.len() as u64
        );

        let (root, server) = local_archive_server(fixture.archive_bytes.clone());
        let result = run_repair(&fixture.game, &root, &fixture).unwrap();
        server.join().unwrap();
        assert!(result.complete);
        assert_eq!(fs::read(&expected_cache).unwrap(), fixture.archive_bytes);
        assert_eq!(
            fs::read(managed(&fixture.game, &fixture.package, "data/client.dat")).unwrap(),
            fixture.contents["data/client.dat"]
        );
    }

    #[test]
    fn insufficient_cache_or_game_space_blocks_before_creating_repair_state() {
        let temporary = crate::content::test_support::tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let quote = RepairQuote {
            full_archive_bytes: 100,
            download_bytes: 100,
            replacement_bytes: 20,
            available_cache_bytes: Some(110),
            available_game_bytes: Some(110),
            cache_problem: None,
        };
        let error = preflight_space(&quote, &fixture.cache, &fixture.game).unwrap_err();
        assert!(error.contains("Insufficient free space"));
        assert!(!transaction_path(&fixture.game).unwrap().exists());
    }

    #[test]
    fn interrupted_publication_can_be_recovered_without_file_selection() {
        let temporary = crate::content::test_support::tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let file = find_file(&fixture.package, "data/client.dat").unwrap();
        let live = join_relative(&fixture.game, &file.path);
        fs::write(&live, b"old bad client data").unwrap();
        let before = scan_selected(&fixture.game, &[file]);
        assert_eq!(before[0].state, FileState::Corrupt);
        let package_sha256 = package_identity(&fixture.package);
        let transaction = transaction_path(&fixture.game).unwrap();
        create_transaction(&transaction, &fixture.package, &package_sha256).unwrap();
        let staged_root = transaction.join(STAGED_DIR);
        let rollback_root = transaction.join(ROLLBACK_DIR);
        write_staged_bytes(&staged_root, file, &fixture.contents["data/client.dat"]).unwrap();
        create_owned_parents(
            &rollback_root,
            join_relative(&rollback_root, &file.path).parent().unwrap(),
        )
        .unwrap();
        let journal = make_journal(&package_sha256, &before, Vec::new()).unwrap();
        write_journal(&transaction, &journal).unwrap();
        let rollback = join_relative(&rollback_root, &file.path);
        fs::rename(&live, &rollback).unwrap();
        assert!(!crate::content::check_game_version(&fixture.game));
        assert_ne!(
            crate::install_check::check_install(Some(&fixture.game)).state,
            crate::install_check::InstallState::Ready
        );
        crate::atomic_fs::rename_noreplace(&join_relative(&staged_root, &file.path), &live)
            .unwrap();
        assert!(recovery_pending(&fixture.game));
        let mut journal = journal;
        journal.files[0].published = true;
        write_journal(&transaction, &journal).unwrap();

        let interrupted = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
        assert!(interrupted.recovery_required);
        let game_root = canonical_game_root(&fixture.game).unwrap();
        let result = run_repair(&game_root, "https://unused.invalid/", &fixture).unwrap();
        assert!(result.complete);
        assert!(!transaction.exists());
        assert_eq!(fs::read(live).unwrap(), fixture.contents["data/client.dat"]);
        assert!(!recovery_pending(&fixture.game));
        assert_eq!(
            crate::install_check::check_install(Some(&fixture.game)).state,
            crate::install_check::InstallState::Ready
        );
    }

    #[test]
    fn recovery_accepts_a_legitimate_journal_larger_than_one_megabyte() {
        let temporary = crate::content::test_support::tempdir().unwrap();
        let mut fixture = fixture(temporary.path());
        let padding = "a".repeat(128);
        let extras = (0..6_000)
            .map(|index| InstallFile {
                path: format!("data/{padding}-{index:04}.dat"),
                length: 1,
                sha256: format!("{:x}", Sha256::digest(b"x")),
            })
            .collect::<Vec<_>>();
        fixture.package.final_files.extend(extras.iter().cloned());
        fixture.package.archives[0]
            .files
            .extend(extras.iter().cloned());
        fixture.package.staging_bytes = fixture
            .package
            .final_files
            .iter()
            .map(|file| file.length)
            .sum();
        fixture.package.validate().unwrap();

        let missing = extras
            .iter()
            .map(|file| ManagedFileStatus {
                path: file.path.clone(),
                state: FileState::Missing,
                expected_length: file.length,
                actual_length: None,
                actual_sha256: None,
                detail: None,
            })
            .collect::<Vec<_>>();
        let package_sha256 = package_identity(&fixture.package);
        let maximum = maximum_repair_journal(&fixture.package, &package_sha256).unwrap();
        let maximum_bytes = serde_json::to_vec(&maximum).unwrap();
        assert!(maximum_bytes.len() > 1024 * 1024);
        assert_eq!(
            journal_read_limit(&fixture.package, &package_sha256).unwrap(),
            maximum_bytes.len() as u64
        );
        let transaction = transaction_path(&fixture.game).unwrap();
        create_transaction(&transaction, &fixture.package, &package_sha256).unwrap();
        let journal = make_journal(&package_sha256, &missing, Vec::new()).unwrap();
        assert!(serde_json::to_vec(&journal).unwrap().len() > 1024 * 1024);
        write_journal(&transaction, &journal).unwrap();

        recover_transaction(
            &transaction,
            &fixture.game,
            &fixture.package,
            &package_sha256,
        )
        .unwrap();
        let recovered = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
        assert!(!recovered.recovery_required);
        assert_eq!(recovered.missing_count, extras.len());
        assert!(!transaction.exists());
    }

    #[test]
    fn interrupted_move_publication_before_journal_update_recovers_and_retries() {
        let temporary = crate::content::test_support::tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let file = find_file(&fixture.package, "data/client.dat").unwrap();
        let live = join_relative(&fixture.game, &file.path);
        fs::remove_file(&live).unwrap();
        let before = scan_selected(&fixture.game, &[file]);
        assert_eq!(before[0].state, FileState::Missing);
        let package_sha256 = package_identity(&fixture.package);
        let transaction = transaction_path(&fixture.game).unwrap();
        create_transaction(&transaction, &fixture.package, &package_sha256).unwrap();
        let staged_root = transaction.join(STAGED_DIR);
        let staged = join_relative(&staged_root, &file.path);
        write_staged_bytes(&staged_root, file, &fixture.contents["data/client.dat"]).unwrap();
        let journal = make_journal(&package_sha256, &before, Vec::new()).unwrap();
        write_journal(&transaction, &journal).unwrap();

        crate::atomic_fs::rename_noreplace(&staged, &live).unwrap();
        let interrupted = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
        assert!(interrupted.recovery_required);

        let result = run_repair(&fixture.game, "https://unused.invalid/", &fixture).unwrap();
        assert!(result.complete);
        assert!(!transaction.exists());
        assert_eq!(fs::read(live).unwrap(), fixture.contents["data/client.dat"]);
    }

    #[test]
    fn repair_is_idempotent_when_every_managed_file_is_valid() {
        let temporary = crate::content::test_support::tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let result = run_repair(&fixture.game, "https://unused.invalid/", &fixture).unwrap();
        assert!(result.complete);
        assert!(result.repaired_files.is_empty());
    }

    #[test]
    fn cancel_while_paused_during_verification_leaves_managed_files_untouched() {
        let temporary = crate::content::test_support::tempdir().unwrap();
        let mut fixture = fixture(temporary.path());
        add_large_managed_file(&mut fixture, "zz-large.dat", 64 * 1024 * 1024);
        let corrupt_path = managed(&fixture.game, &fixture.package, "data/client.dat");
        let corrupt_bytes = b"X".repeat(fixture.contents["data/client.dat"].len());
        fs::write(&corrupt_path, &corrupt_bytes).unwrap();

        let shared = RepairShared::new();
        let worker_shared = shared.clone();
        let game = fixture.game.clone();
        let cache = fixture.cache.clone();
        let package = fixture.package.clone();
        let worker = std::thread::spawn(move || {
            repair_all(
                &game,
                "https://unused.invalid/",
                &cache,
                &package,
                &worker_shared,
            )
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "repair did not reach the large managed file"
            );
            if shared.phase() == RepairPhase::Verifying
                && shared.current_file().as_deref() == Some("zz-large.dat")
            {
                shared.request_pause();
                break;
            }
            assert_ne!(shared.phase(), RepairPhase::Error, "repair failed early");
            std::thread::sleep(Duration::from_millis(1));
        }
        while !shared.is_paused() {
            assert!(
                std::time::Instant::now() < deadline,
                "repair did not stop at the requested pause checkpoint"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        shared.request_cancel();

        let error = worker.join().unwrap().unwrap_err();
        assert!(error.contains("cancelled"));
        assert_eq!(shared.phase(), RepairPhase::Cancelled);
        assert_eq!(fs::read(corrupt_path).unwrap(), corrupt_bytes);
        assert!(!transaction_path(&fixture.game).unwrap().exists());
    }

    #[test]
    fn cancel_during_final_verification_rolls_back_published_repairs() {
        let temporary = crate::content::test_support::tempdir().unwrap();
        let mut fixture = fixture(temporary.path());
        add_large_managed_file(&mut fixture, "zz-large.dat", 64 * 1024 * 1024);
        let repaired_path = managed(&fixture.game, &fixture.package, "data/client.dat");
        let original_corrupt_bytes = b"X".repeat(fixture.contents["data/client.dat"].len());
        fs::write(&repaired_path, &original_corrupt_bytes).unwrap();

        let shared = RepairShared::new();
        let worker_shared = shared.clone();
        let game = fixture.game.clone();
        let cache = fixture.cache.clone();
        let package = fixture.package.clone();
        let worker = std::thread::spawn(move || {
            repair_all(
                &game,
                "https://unused.invalid/",
                &cache,
                &package,
                &worker_shared,
            )
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "repair did not reach the full final verification scan"
            );
            if shared.phase() == RepairPhase::FinalVerification
                && shared.current_file().as_deref() == Some("zz-large.dat")
            {
                shared.request_pause();
                break;
            }
            assert_ne!(shared.phase(), RepairPhase::Error, "repair failed early");
            std::thread::sleep(Duration::from_millis(1));
        }
        while !shared.is_paused() {
            assert!(
                std::time::Instant::now() < deadline,
                "repair did not stop at the requested final verification checkpoint"
            );
            std::thread::sleep(Duration::from_millis(1));
        }

        let transaction = transaction_path(&fixture.game).unwrap();
        let package_sha256 = package_identity(&fixture.package);
        let journal = read_json::<RepairJournal>(
            &transaction.join(JOURNAL_FILE),
            journal_read_limit(&fixture.package, &package_sha256).unwrap(),
        )
        .unwrap();
        assert!(matches!(journal.phase, JournalPhase::Publishing));
        assert_eq!(
            fs::read(&repaired_path).unwrap(),
            fixture.contents["data/client.dat"]
        );
        assert!(
            !join_relative(&transaction.join(STAGED_DIR), "data/client.dat").exists(),
            "final verification should not scan a hard-linked managed file"
        );

        shared.request_cancel();
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.contains("cancelled"));
        assert_eq!(shared.phase(), RepairPhase::Cancelled);
        assert_eq!(fs::read(repaired_path).unwrap(), original_corrupt_bytes);
        assert!(!transaction.exists());
    }
}
