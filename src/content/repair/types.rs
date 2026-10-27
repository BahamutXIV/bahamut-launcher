//! Result types returned to the UI by verification and repair.

use serde::{Deserialize, Serialize};

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
    /// Files that failed validation. Counts below cover all managed files.
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
