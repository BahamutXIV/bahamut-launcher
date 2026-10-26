//! Manifest-bound verification and recoverable repair of managed game files.

use std::fs;
use std::path::{Path, PathBuf};

use self::paths::{resolve_existing_directory, transaction_path};
use self::run::repair_all_inner;
use super::manifest::BasePackage;

mod archive;
mod control;
mod journal;
mod paths;
mod publish;
mod quote;
mod recover;
mod run;
mod scan;
#[cfg(test)]
mod tests;
mod types;

pub use control::{RepairPhase, RepairShared};
pub use types::{FileState, ManagedFileStatus, RepairQuote, RepairResult, VerificationSummary};

const IO_BUFFER_BYTES: usize = 1024 * 1024;
const OWNER_FILE: &str = "owner.json";
const JOURNAL_FILE: &str = "transaction.json";
const JOURNAL_NEXT_FILE: &str = "transaction.next";
const STAGED_DIR: &str = "staged";
const ROLLBACK_DIR: &str = "rollback";

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
