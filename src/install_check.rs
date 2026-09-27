// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

//! Login and launch require [`InstallState::Ready`]: the client and final version
//! must exist, and a base-install receipt must no longer be present.

use std::path::{Path, PathBuf};

use crate::patcher::check_game_version;

/// Present from the base 1.x install; absence means the path is not an FFXIV install.
const BOOT_EXE: &str = "ffxivboot.exe";

/// Created by the patch chain and required to launch.
const CLIENT_EXE: &str = "ffxivgame.exe";

/// Install state relative to the 1.23b launch requirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallState {
    /// No base install exists, or its owned staging receipt is still present.
    NotFound,
    /// A real install that is not at the target 1.23b build.
    FoundNeedsPatch {
        /// Trimmed `game.ver` contents for the UI.
        game_version: Option<String>,
    },
    /// `game.ver` records the 1.23b build and `ffxivgame.exe` exists.
    Ready,
}

/// Gate verdict and the directory it was computed against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallStatus {
    pub state: InstallState,
    pub game_dir: Option<PathBuf>,
}

/// Evaluate the 1.23b install gate for an optional game directory.
pub fn check_install(game_dir: Option<&Path>) -> InstallStatus {
    let Some(dir) = game_dir else {
        return InstallStatus {
            state: InstallState::NotFound,
            game_dir: None,
        };
    };
    if !dir.join(BOOT_EXE).is_file() || crate::patcher::installation_in_progress(dir) {
        return InstallStatus {
            state: InstallState::NotFound,
            game_dir: Some(dir.to_path_buf()),
        };
    }
    let state = if check_game_version(dir) && dir.join(CLIENT_EXE).is_file() {
        InstallState::Ready
    } else {
        InstallState::FoundNeedsPatch {
            game_version: read_game_version(dir),
        }
    };
    InstallStatus {
        state,
        game_dir: Some(dir.to_path_buf()),
    }
}

fn read_game_version(game_dir: &Path) -> Option<String> {
    std::fs::read_to_string(game_dir.join("game.ver"))
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::FFXIV_GAME_VERSION;
    use std::fs;

    #[test]
    fn no_dir_is_not_found() {
        let status = check_install(None);
        assert_eq!(status.state, InstallState::NotFound);
        assert_eq!(status.game_dir, None);
    }

    #[test]
    fn missing_dir_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("nope");
        let status = check_install(Some(&gone));
        assert_eq!(status.state, InstallState::NotFound);
    }

    #[test]
    fn dir_without_boot_exe_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let status = check_install(Some(tmp.path()));
        assert_eq!(status.state, InstallState::NotFound);
        assert_eq!(status.game_dir.as_deref(), Some(tmp.path()));
    }

    #[test]
    fn base_install_needs_patch_and_reports_its_version() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("ffxivboot.exe"), b"x").unwrap();
        fs::write(tmp.path().join("game.ver"), "2010.07.10.0000\n").unwrap();
        let status = check_install(Some(tmp.path()));
        assert_eq!(
            status.state,
            InstallState::FoundNeedsPatch {
                game_version: Some("2010.07.10.0000".into())
            }
        );
    }

    #[test]
    fn boot_exe_without_game_ver_needs_patch() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("ffxivboot.exe"), b"x").unwrap();
        let status = check_install(Some(tmp.path()));
        assert_eq!(
            status.state,
            InstallState::FoundNeedsPatch { game_version: None }
        );
    }

    #[test]
    fn target_game_ver_without_client_exe_still_needs_patch() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("ffxivboot.exe"), b"x").unwrap();
        fs::write(tmp.path().join("game.ver"), FFXIV_GAME_VERSION).unwrap();
        let status = check_install(Some(tmp.path()));
        assert!(matches!(status.state, InstallState::FoundNeedsPatch { .. }));
    }

    #[test]
    fn patched_install_is_ready() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("ffxivboot.exe"), b"x").unwrap();
        fs::write(tmp.path().join("ffxivgame.exe"), b"x").unwrap();
        fs::write(tmp.path().join("game.ver"), FFXIV_GAME_VERSION).unwrap();
        let status = check_install(Some(tmp.path()));
        assert_eq!(status.state, InstallState::Ready);
    }

    #[test]
    fn install_receipt_blocks_readiness_even_with_final_version_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("ffxivboot.exe"), b"x").unwrap();
        fs::write(tmp.path().join("ffxivgame.exe"), b"x").unwrap();
        fs::write(tmp.path().join("game.ver"), FFXIV_GAME_VERSION).unwrap();
        let receipt = tmp.path().join(crate::patcher::INSTALL_RECEIPT_FILE);
        fs::write(&receipt, b"pending verification").unwrap();
        assert_eq!(
            check_install(Some(tmp.path())).state,
            InstallState::NotFound
        );
        assert!(!check_game_version(tmp.path()));
        fs::remove_file(receipt).unwrap();
        assert_eq!(check_install(Some(tmp.path())).state, InstallState::Ready);
    }
}
