// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

//! Support for the FFXIV 1.x ZiPatch archive format.

pub mod zipatch;

pub use zipatch::{PatchApplyError, PatchApplyResult, apply_patch_file};
