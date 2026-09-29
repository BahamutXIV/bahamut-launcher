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

/// Fail when a known free-space figure is below the requirement; an unknown figure passes.
pub(crate) fn require_space(
    available: Option<u64>,
    required: u64,
    volume: &str,
) -> Result<(), String> {
    use crate::diagnostics::format_bytes;

    if let Some(available) = available.filter(|available| *available < required) {
        return Err(format!(
            "Insufficient free space on the {volume}: need {}, {} available ({} short).",
            format_bytes(required),
            format_bytes(available),
            format_bytes(required - available),
        ));
    }
    Ok(())
}

/// Receipt removal is the final readiness boundary for a base installation.
pub fn installation_in_progress(directory: &std::path::Path) -> bool {
    !matches!(std::fs::symlink_metadata(directory.join(INSTALL_RECEIPT_FILE)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}

#[cfg(test)]
mod tests {
    use super::require_space;

    #[test]
    fn insufficient_space_is_reported_in_readable_units() {
        assert_eq!(
            require_space(Some(3_502_796_800), 7_764_447_232, "download cache").unwrap_err(),
            "Insufficient free space on the download cache: need 7.23 GB, 3.26 GB available \
             (3.97 GB short)."
        );
        // Figures that round to the same text stay distinguishable through the shortfall.
        assert_eq!(
            require_space(Some(7_764_447_232 - 4096), 7_764_447_232, "download cache").unwrap_err(),
            "Insufficient free space on the download cache: need 7.23 GB, 7.23 GB available \
             (4.00 KB short)."
        );
    }

    #[test]
    fn sufficient_or_unknown_space_passes() {
        assert!(require_space(Some(10), 10, "download cache").is_ok());
        assert!(require_space(Some(11), 10, "download cache").is_ok());
        assert!(require_space(None, 10, "download cache").is_ok());
    }
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
