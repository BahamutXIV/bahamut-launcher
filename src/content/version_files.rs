// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

use std::fs;
use std::io;
use std::path::Path;

use crate::version::{FFXIV_BOOT_VERSION, FFXIV_GAME_VERSION};

#[derive(Debug, thiserror::Error)]
pub enum VersionFileError {
    #[error("i/o error while {context}: {source}")]
    Io {
        context: &'static str,
        #[source]
        source: io::Error,
    },
}

impl VersionFileError {
    fn io(context: &'static str, source: io::Error) -> Self {
        Self::Io { context, source }
    }
}

/// Return whether `<game_location>/game.ver` records the target build.
pub fn check_game_version(game_location: &Path) -> bool {
    if super::installation_in_progress(game_location)
        || super::repair::recovery_pending(game_location)
    {
        return false;
    }
    match fs::read_to_string(game_location.join("game.ver")) {
        Ok(text) => text.trim() == FFXIV_GAME_VERSION,
        Err(_) => false,
    }
}

/// Stamp `game.ver` and `boot.ver` for the target 1.23b build.
pub fn write_version_files(game_location: &Path) -> Result<(), VersionFileError> {
    fs::write(game_location.join("boot.ver"), FFXIV_BOOT_VERSION)
        .map_err(|e| VersionFileError::io("writing boot.ver", e))?;
    fs::write(game_location.join("game.ver"), FFXIV_GAME_VERSION)
        .map_err(|e| VersionFileError::io("writing game.ver", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_game_ver_reads_as_out_of_date() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!check_game_version(tmp.path()));
    }

    #[test]
    fn matching_game_ver_reads_as_current() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("game.ver"), FFXIV_GAME_VERSION).unwrap();
        assert!(check_game_version(tmp.path()));
    }

    #[test]
    fn writing_version_files_creates_both() {
        let tmp = tempfile::tempdir().unwrap();
        write_version_files(tmp.path()).unwrap();
        assert!(tmp.path().join("game.ver").exists());
        assert!(tmp.path().join("boot.ver").exists());
    }

    #[test]
    fn failed_boot_stamp_cannot_mark_game_current() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("boot.ver")).unwrap();
        assert!(write_version_files(tmp.path()).is_err());
        assert!(!check_game_version(tmp.path()));
    }
}
