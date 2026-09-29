// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

//! [`FFXIV_BOOT_VERSION`] and [`FFXIV_GAME_VERSION`] are the last-known-good 1.23b stamps the installer writes to `boot.ver` and `game.ver`.
//! Do not advance them past 1.23b without re-validating PE-patch RVAs; encryption-time and lobby-host slots move between client builds.

/// Final-build value for `<game>/boot.ver`: 1.x boot baseline `2010.09.18.0000`.
pub const FFXIV_BOOT_VERSION: &str = "2010.09.18.0000";

/// Final-build value for `<game>/game.ver`: final 1.23b build `2012.09.19.0001`.
pub const FFXIV_GAME_VERSION: &str = "2012.09.19.0001";

/// The selected release tag, local `git describe` identity, or "unknown" outside Git.
pub const LAUNCHER_VERSION: &str = env!("BAHAMUT_GIT_DESCRIBE");

#[cfg(test)]
mod tests {
    use super::LAUNCHER_VERSION;

    // Reject unexpanded template or Cargo fallback stamps.
    #[test]
    fn launcher_version_is_stamped() {
        assert!(!LAUNCHER_VERSION.is_empty());
        assert!(
            !LAUNCHER_VERSION.contains('@'),
            "version looks like an unexpanded template: {LAUNCHER_VERSION}"
        );
        assert!(
            !LAUNCHER_VERSION.contains("CARGO"),
            "version looks miswired to a cargo variable: {LAUNCHER_VERSION}"
        );
    }
}
