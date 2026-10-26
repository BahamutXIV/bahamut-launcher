//! Out-of-process portable launcher replacement.
//!
//! The helper accepts only a local Ed25519 public key and exact signed
//! metadata/artifact files. Its scope is fixed to the Windows launcher. The
//! helper does not fetch keys or release data and does not manage user files.

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::release::ReleaseError;

const JOURNAL_FILE: &str = "transaction.json";
const NEW_METADATA_FILE: &str = "new-metadata.json";
const NEW_SIGNATURE_FILE: &str = "new-signature.bin";
const OLD_METADATA_FILE: &str = "old-metadata.json";
const OLD_SIGNATURE_FILE: &str = "old-signature.bin";
const SNAPSHOT_FILE: &str = "snapshot.json";
// The shell reads and locks these install-relative paths too.
pub const POINTER_FILE: &str = "config/launcher-update-state.json";
pub const PENDING_FILE: &str = "config/launcher-update-pending.json";
pub const HELPER_LOCK_FILE: &str = "config/launcher-update-helper.lock";
pub const OFFICIAL_OVERLAY_MANIFEST: &str = "plugins/dats/bahamut-dats-overlay/overlay.toml";
const PAYLOAD_DIR: &str = "payload";
const BACKUP_DIR: &str = "backup";
const JOURNAL_SCHEMA: u32 = 1;
const MAX_JOURNAL_BYTES: usize = 16 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;

mod apply;
mod cli;
mod confirm;
mod fsutil;
mod hooks;
mod inventory;
mod overlay;
mod payload;
mod pointer;
mod process;
mod recover;
mod replace;
mod rollback;
mod snapshot;
mod transaction;
mod verify;

#[cfg(test)]
mod tests;

pub use cli::run_cli;
pub use recover::recovery_release;

type Result<T> = std::result::Result<T, UpdateError>;

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("{0}")]
    Invalid(String),
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("release authorization failed: {0}")]
    Release(String),
    #[error("JSON encoding or decoding failed: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<ReleaseError> for UpdateError {
    fn from(value: ReleaseError) -> Self {
        Self::Release(value.to_string())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    Applying,
    AwaitingValidation,
    Committed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    phase: Phase,
    mode: TransactionMode,
    target: PathBuf,
    new_metadata_sha256: String,
    old_metadata_sha256: String,
    trusted_key_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    schema_version: u32,
    present_old_managed: Vec<String>,
    previous_managed_files: Vec<FileIdentity>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct FileIdentity {
    path: String,
    length: u64,
    sha256: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum TransactionMode {
    Update,
    Repair,
}

impl TransactionMode {
    fn transaction_name(self) -> &'static str {
        match self {
            Self::Update => "bahamut-update",
            Self::Repair => "bahamut-repair",
        }
    }
}

fn invalid(message: &str) -> UpdateError {
    UpdateError::Invalid(message.to_owned())
}
