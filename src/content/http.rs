// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

//! Bounded HTTP downloads verified against pinned content identities.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::runtime::Builder as RuntimeBuilder;

mod download;
mod files;
mod policy;
#[cfg(test)]
mod tests;

use download::{CachePaths, download_object_async};
use files::prepare_cache_dir;
use policy::{object_url, validate_spec};

const IO_CHUNK: usize = 64 * 1024;
const HTML_SNIFF_BYTES: usize = 64;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;
const MAX_RETRIES: usize = 3;
const RETRY_BACKOFF: [Duration; MAX_RETRIES] = [
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(500),
];
const PAUSE_POLL: Duration = Duration::from_millis(50);

/// Immutable identity supplied by the launcher's checked-in content manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ObjectSpec {
    pub object_key: String,
    pub length: u64,
    pub sha256: String,
}

/// State presented at a cooperative download checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadCheckpoint {
    /// `true` means the partial file was synced before this callback.
    pub durable: bool,
}

/// The worker's choice at a transfer checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckpointAction {
    Continue,
    Pause,
    Cancel,
}

/// Errors returned by a verified object transfer.
#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("object specification is invalid")]
    InvalidSpec,
    #[error("download URL must use HTTPS, or loopback HTTP for fixtures")]
    InvalidRoot,
    #[error("object key must be a root-relative path")]
    InvalidObjectKey,
    #[error("download cache contains an unsafe link or non-regular file")]
    UnsafeCachePath,
    #[error("download cache path is not a directory")]
    CacheNotDirectory,
    #[error("download was cancelled")]
    Cancelled,
    #[error("HTTP redirect is invalid or exceeds the limit")]
    InvalidRedirect,
    #[error("HTTP response used an unsupported content encoding")]
    ContentEncoding,
    #[error("HTTP response used an HTML content type")]
    HtmlResponse,
    #[error("HTTP response has an invalid Content-Range header")]
    ContentRange,
    #[error("HTTP response length does not match the requested object")]
    ResponseLength,
    #[error("HTTP response ended before the requested bytes arrived")]
    IncompleteResponse,
    #[error("HTTP response exceeded the expected object length")]
    ExcessBytes,
    #[error("downloaded content does not match the expected SHA-256")]
    HashMismatch,
    #[error("HTTP server returned status {0}")]
    HttpStatus(u16),
    #[error("could not start the download runtime: {0}")]
    Runtime(#[source] io::Error),
    #[error("download cache I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("HTTP transport failed")]
    Transport(#[source] reqwest::Error),
}

impl DownloadError {
    fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::IncompleteResponse | Self::Transport(_) | Self::HttpStatus(408 | 429 | 500..=599)
        )
    }
}

/// Download one immutable object into `cache`, reusing only bytes bound to its
/// SHA-256 and length. The callback is polled after each bounded read. A pause
/// request syncs the partial file, closes the response, then polls again with
/// `durable: true` until resumed or cancelled.
pub fn download_object<C, P>(
    root: &str,
    spec: &ObjectSpec,
    cache: &Path,
    mut checkpoint: C,
    mut progress: P,
) -> Result<PathBuf, DownloadError>
where
    C: FnMut(DownloadCheckpoint) -> CheckpointAction,
    P: FnMut(u64, u64),
{
    let expected_sha256 = validate_spec(spec)?;
    let object_url = object_url(root, &spec.object_key)?;
    let cache_root = prepare_cache_dir(cache)?;
    let paths = CachePaths::new(&cache_root, &expected_sha256, spec.length);
    let runtime = RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .map_err(DownloadError::Runtime)?;

    runtime.block_on(download_object_async(
        object_url,
        spec.length,
        expected_sha256,
        paths,
        &mut checkpoint,
        &mut progress,
    ))
}
