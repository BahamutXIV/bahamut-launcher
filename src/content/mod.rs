// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

pub mod downloader;
pub mod http;
pub mod installer;
pub mod manifest;
pub mod repair;
pub mod version_files;
pub mod worker;

pub use version_files::{check_game_version, write_version_files};
pub use worker::{InstallShared, Phase};

pub const INSTALL_RECEIPT_FILE: &str = ".bahamut-install.json";

/// Receipt removal is the final readiness boundary for a base installation.
pub fn installation_in_progress(directory: &std::path::Path) -> bool {
    !matches!(std::fs::symlink_metadata(directory.join(INSTALL_RECEIPT_FILE)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound)
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
