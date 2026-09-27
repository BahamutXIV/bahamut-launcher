// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::manifest::{PATCH_MANIFEST, leaf_of};
use super::summarize_names;
use crate::version::{FFXIV_BOOT_VERSION, FFXIV_GAME_VERSION};

/// Patch files in worker application order.
#[derive(Debug)]
pub struct PatchPlan {
    pub patches_in_order: Vec<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum PatchPlanError {
    #[error("patch source is not a directory: {0}")]
    NotADirectory(PathBuf),

    #[error("{missing_count} patch file(s) not found under {dir}: {summary}")]
    MissingPatches {
        missing_count: usize,
        dir: PathBuf,
        summary: String,
    },

    #[error("i/o error while {context}: {source}")]
    Io {
        context: &'static str,
        #[source]
        source: io::Error,
    },
}

impl PatchPlanError {
    fn io(context: &'static str, source: io::Error) -> Self {
        Self::Io { context, source }
    }
}

impl PatchPlan {
    /// Resolve a user-supplied patch cache by manifest leaf; the worker verifies size and CRC32.
    pub fn from_local_source(source_dir: &Path) -> Result<Self, PatchPlanError> {
        let found = collect_patch_files(source_dir)?;
        let mut patches = Vec::with_capacity(PATCH_MANIFEST.len());
        let mut missing = Vec::new();
        for entry in PATCH_MANIFEST {
            let leaf = leaf_of(entry.path);
            match found.get(leaf) {
                Some(path) => patches.push(path.clone()),
                None => missing.push(leaf.to_owned()),
            }
        }
        if !missing.is_empty() {
            return Err(PatchPlanError::MissingPatches {
                missing_count: missing.len(),
                dir: source_dir.to_path_buf(),
                summary: summarize_names(&missing),
            });
        }
        sort_by_leaf(&mut patches);
        Ok(Self {
            patches_in_order: patches,
        })
    }
}

/// Sort patch paths by leaf filename, which defines patch chronology.
fn sort_by_leaf(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
}

/// Index recursive `*.patch` files by leaf; the first duplicate encountered wins.
fn collect_patch_files(root: &Path) -> Result<HashMap<String, PathBuf>, PatchPlanError> {
    if !root.is_dir() {
        return Err(PatchPlanError::NotADirectory(root.to_path_buf()));
    }
    let mut found = HashMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let listing =
            fs::read_dir(&dir).map_err(|e| PatchPlanError::io("listing a directory", e))?;
        for item in listing {
            let item = item.map_err(|e| PatchPlanError::io("reading a directory entry", e))?;
            let kind = item
                .file_type()
                .map_err(|e| PatchPlanError::io("inspecting an entry type", e))?;
            let path = item.path();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file()
                && path.extension().and_then(|e| e.to_str()) == Some("patch")
                && let Some(leaf) = path.file_name().and_then(|n| n.to_str())
            {
                found.entry(leaf.to_owned()).or_insert(path);
            }
        }
    }
    Ok(found)
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
pub fn write_version_files(game_location: &Path) -> Result<(), PatchPlanError> {
    fs::write(game_location.join("boot.ver"), FFXIV_BOOT_VERSION)
        .map_err(|e| PatchPlanError::io("writing boot.ver", e))?;
    fs::write(game_location.join("game.ver"), FFXIV_GAME_VERSION)
        .map_err(|e| PatchPlanError::io("writing game.ver", e))?;
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

    #[test]
    fn local_plan_finds_files_in_nested_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        for entry in PATCH_MANIFEST {
            fs::write(nested.join(leaf_of(entry.path)), b"").unwrap();
        }
        let plan = PatchPlan::from_local_source(tmp.path()).unwrap();
        assert_eq!(plan.patches_in_order.len(), PATCH_MANIFEST.len());
    }

    #[test]
    fn local_plan_reports_missing_files() {
        let tmp = tempfile::tempdir().unwrap();
        match PatchPlan::from_local_source(tmp.path()).unwrap_err() {
            PatchPlanError::MissingPatches { missing_count, .. } => {
                assert_eq!(missing_count, PATCH_MANIFEST.len());
            }
            other => panic!("expected MissingPatches, got {other:?}"),
        }
    }

    #[test]
    fn local_plan_rejects_a_file_source() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("not-a-dir.txt");
        fs::write(&file, b"x").unwrap();
        assert!(matches!(
            PatchPlan::from_local_source(&file).unwrap_err(),
            PatchPlanError::NotADirectory(_)
        ));
    }
}
