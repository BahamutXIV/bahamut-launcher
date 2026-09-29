// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

//! Shared download progress and cancellation.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Cloneable atomic counters shared by a streaming worker and its UI observer.
#[derive(Clone)]
pub struct DownloadProgress {
    pub(crate) bytes_downloaded: Arc<AtomicU64>,
    pub(crate) cancel_flag: Arc<AtomicBool>,
}

impl DownloadProgress {
    pub fn new() -> Self {
        Self {
            bytes_downloaded: Arc::new(AtomicU64::new(0)),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Request cancellation at the next chunk boundary.
    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    pub fn bytes(&self) -> u64 {
        self.bytes_downloaded.load(Ordering::Relaxed)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::Relaxed)
    }
}

impl Default for DownloadProgress {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_flag_is_sticky() {
        let p = DownloadProgress::new();
        assert!(!p.is_cancelled());
        p.cancel();
        assert!(p.is_cancelled());
    }
}
