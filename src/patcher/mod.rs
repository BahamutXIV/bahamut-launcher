// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

pub mod content;
pub mod downloader;
pub mod extract;
pub mod http;
pub mod installer;
pub mod manifest;
pub mod process;
pub mod repair;
pub mod worker;

pub use extract::{PatchPayload, find_patch_payload};
pub use manifest::{PATCH_MANIFEST, total_bytes};
pub use process::check_game_version;
pub use worker::{PatchSource, PatcherShared, Phase};

pub const INSTALL_RECEIPT_FILE: &str = ".bahamut-install.json";

/// Receipt removal is the final readiness boundary for a base installation.
pub fn installation_in_progress(directory: &std::path::Path) -> bool {
    !matches!(std::fs::symlink_metadata(directory.join(INSTALL_RECEIPT_FILE)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}

const MAX_NAMES_IN_ERROR: usize = 4;

fn summarize_names(names: &[String]) -> String {
    if names.len() <= MAX_NAMES_IN_ERROR {
        return names.join(", ");
    }
    let shown = names[..MAX_NAMES_IN_ERROR].join(", ");
    format!("{shown}, and {} more", names.len() - MAX_NAMES_IN_ERROR)
}

#[cfg(test)]
pub(crate) mod test_support {
    /// Temporary directory whose ancestors are real directories: macOS places
    /// `env::temp_dir()` under `/var`, a symlink the installer's path checks refuse.
    /// Windows keeps the plain path because `canonicalize` yields a verbatim prefix.
    pub(crate) fn tempdir() -> std::io::Result<tempfile::TempDir> {
        #[cfg(windows)]
        let base = std::env::temp_dir();
        #[cfg(not(windows))]
        let base = std::env::temp_dir().canonicalize()?;
        tempfile::Builder::new().tempdir_in(base)
    }
}
