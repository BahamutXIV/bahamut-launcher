use reqwest::header::CONTENT_RANGE;
use reqwest::{Client, Response, StatusCode, Url};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tokio::time::sleep;

use super::files::{file_matches, open_existing, open_new_regular, publish};
use super::policy::{
    has_header, is_transient_status, parse_content_range, send_with_redirects,
    validate_representation_headers,
};
use super::{
    CONNECT_TIMEOUT, CheckpointAction, DownloadCheckpoint, DownloadError, HTML_SNIFF_BYTES,
    IO_CHUNK, MAX_RETRIES, PAUSE_POLL, READ_TIMEOUT, RETRY_BACKOFF,
};

#[derive(Debug)]
pub(super) struct CachePaths {
    pub(super) partial: PathBuf,
    pub(super) complete: PathBuf,
}

impl CachePaths {
    pub(super) fn new(cache: &Path, sha256: &str, length: u64) -> Self {
        let identity = format!("{sha256}-{length}");
        Self {
            partial: cache.join(format!("{identity}.part")),
            complete: cache.join(format!("{identity}.object")),
        }
    }
}

pub(super) async fn download_object_async<C, P>(
    url: Url,
    length: u64,
    expected_sha256: String,
    paths: CachePaths,
    checkpoint: &mut C,
    progress: &mut P,
) -> Result<PathBuf, DownloadError>
where
    C: FnMut(DownloadCheckpoint) -> CheckpointAction,
    P: FnMut(u64, u64),
{
    let client = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(DownloadError::Transport)?;

    if reuse_complete_object(&paths, length, &expected_sha256, checkpoint, progress)? {
        return Ok(paths.complete);
    }
    let mut partial = match open_partial(&paths, length, &expected_sha256, checkpoint, progress)? {
        PartialFile::Published(path) => return Ok(path),
        PartialFile::Open(file) => file,
    };

    let mut transient_failures = 0;
    let mut clean_restart_used = false;
    let mut force_full = false;

    loop {
        let mut partial_length = partial.metadata()?.len();
        if partial_length == length {
            return finish_full_partial(
                partial,
                &paths,
                length,
                &expected_sha256,
                checkpoint,
                progress,
            );
        }
        if partial_length > length {
            partial.set_len(0)?;
            partial_length = 0;
            progress(0, length);
        }
        partial.seek(SeekFrom::Start(partial_length))?;
        partial.sync_data()?;
        wait_at_durable_checkpoint(checkpoint)?;

        let send_range = partial_length > 0 && !force_full;
        let requested_offset = if send_range { partial_length } else { 0 };
        force_full = false;

        let response = match send_with_redirects(
            &client,
            &url,
            send_range.then_some(requested_offset),
        )
        .await
        {
            Ok(response) => response,
            Err(error) if error.is_retryable() => {
                retry_or_return(error, &mut transient_failures, &mut partial, checkpoint).await?;
                continue;
            }
            Err(error) => return Err(error),
        };

        let status = response.status();
        if status == StatusCode::RANGE_NOT_SATISFIABLE && send_range {
            if !clean_restart_used {
                partial.set_len(0)?;
                partial.sync_data()?;
                progress(0, length);
                clean_restart_used = true;
                force_full = true;
                continue;
            }
            return Err(DownloadError::HttpStatus(status.as_u16()));
        }

        if is_transient_status(status) {
            let error = DownloadError::HttpStatus(status.as_u16());
            drop(response);
            retry_or_return(error, &mut transient_failures, &mut partial, checkpoint).await?;
            continue;
        }

        let (response_start, response_bytes) = accept_response(
            &response,
            status,
            &mut partial,
            partial_length,
            length,
            progress,
        )?;

        partial.seek(SeekFrom::Start(response_start))?;
        // Only an uninterrupted stream from byte 0 is verified in memory.
        let mut digest = (response_start == 0).then(Sha256::new);
        let outcome = consume_response(
            response,
            response_bytes,
            length,
            response_start,
            &mut partial,
            digest.as_mut(),
            checkpoint,
            progress,
        )
        .await;

        match outcome {
            Ok(StreamOutcome::Paused) => {
                wait_at_durable_checkpoint(checkpoint)?;
                continue;
            }
            Ok(StreamOutcome::Complete) => {
                return finish_streamed_download(
                    partial,
                    digest,
                    &paths,
                    length,
                    &expected_sha256,
                    checkpoint,
                    progress,
                );
            }
            Err(error) if error.is_retryable() => {
                retry_or_return(error, &mut transient_failures, &mut partial, checkpoint).await?;
            }
            Err(error) => return Err(error),
        }
    }
}

/// The partial download file, or the published object when the partial already held it.
enum PartialFile {
    Published(PathBuf),
    Open(File),
}

/// Accept a cached complete object if it matches; otherwise discard it.
fn reuse_complete_object<C, P>(
    paths: &CachePaths,
    length: u64,
    expected_sha256: &str,
    checkpoint: &mut C,
    progress: &mut P,
) -> Result<bool, DownloadError>
where
    C: FnMut(DownloadCheckpoint) -> CheckpointAction,
    P: FnMut(u64, u64),
{
    if let Some(mut complete) = open_existing(&paths.complete, false)? {
        if file_matches(&mut complete, length, expected_sha256, checkpoint)? {
            progress(length, length);
            return Ok(true);
        }
        drop(complete);
        fs::remove_file(&paths.complete)?;
        progress(0, length);
    }
    Ok(false)
}

/// Open the partial file and normalize its length before any request is sent.
fn open_partial<C, P>(
    paths: &CachePaths,
    length: u64,
    expected_sha256: &str,
    checkpoint: &mut C,
    progress: &mut P,
) -> Result<PartialFile, DownloadError>
where
    C: FnMut(DownloadCheckpoint) -> CheckpointAction,
    P: FnMut(u64, u64),
{
    let mut partial = match open_existing(&paths.partial, true)? {
        Some(file) => file,
        None => open_new_regular(&paths.partial)?,
    };
    let mut partial_length = partial.metadata()?.len();
    if partial_length > length {
        partial.set_len(0)?;
        partial.sync_data()?;
        partial_length = 0;
    } else if partial_length == length {
        partial.sync_data()?;
        if file_matches(&mut partial, length, expected_sha256, checkpoint)? {
            progress(length, length);
            return publish(partial, &paths.partial, &paths.complete).map(PartialFile::Published);
        }
        partial.set_len(0)?;
        partial.sync_data()?;
        partial_length = 0;
        progress(0, length);
    }

    progress(partial_length, length);
    partial.sync_data()?;
    wait_at_durable_checkpoint(checkpoint)?;

    if length == 0 {
        if file_matches(&mut partial, length, expected_sha256, checkpoint)? {
            progress(length, length);
            return publish(partial, &paths.partial, &paths.complete).map(PartialFile::Published);
        }
        return Err(DownloadError::HashMismatch);
    }
    Ok(PartialFile::Open(partial))
}

/// Publish a partial file that already has the full length, or discard it on a hash mismatch.
fn finish_full_partial<C, P>(
    mut partial: File,
    paths: &CachePaths,
    length: u64,
    expected_sha256: &str,
    checkpoint: &mut C,
    progress: &mut P,
) -> Result<PathBuf, DownloadError>
where
    C: FnMut(DownloadCheckpoint) -> CheckpointAction,
    P: FnMut(u64, u64),
{
    partial.sync_all()?;
    if file_matches(&mut partial, length, expected_sha256, checkpoint)? {
        progress(length, length);
        return publish(partial, &paths.partial, &paths.complete);
    }
    partial.set_len(0)?;
    partial.sync_data()?;
    Err(DownloadError::HashMismatch)
}

/// Validate the response status and headers against the requested range and return the
/// first byte offset and the body length it carries.
fn accept_response<P>(
    response: &Response,
    status: StatusCode,
    partial: &mut File,
    partial_length: u64,
    length: u64,
    progress: &mut P,
) -> Result<(u64, u64), DownloadError>
where
    P: FnMut(u64, u64),
{
    match status {
        StatusCode::OK => {
            if has_header(response, &CONTENT_RANGE)? {
                return Err(DownloadError::ContentRange);
            }
            validate_representation_headers(response, length)?;
            if partial_length > 0 {
                partial.set_len(0)?;
                partial.seek(SeekFrom::Start(0))?;
                partial.sync_data()?;
                progress(0, length);
            }
            Ok((0, length))
        }
        StatusCode::PARTIAL_CONTENT => {
            let range = parse_content_range(response)?;
            if range.start != partial_length
                || range.total != length
                || range.end < range.start
                || range.end >= range.total
                || range.end != range.total - 1
            {
                return Err(DownloadError::ContentRange);
            }
            let bytes = range.end - range.start + 1;
            validate_representation_headers(response, bytes)?;
            Ok((range.start, bytes))
        }
        _ => Err(DownloadError::HttpStatus(status.as_u16())),
    }
}

/// Verify a fully streamed body, from the in-memory digest when the stream began at byte 0,
/// and publish it.
fn finish_streamed_download<C, P>(
    mut partial: File,
    digest: Option<Sha256>,
    paths: &CachePaths,
    length: u64,
    expected_sha256: &str,
    checkpoint: &mut C,
    progress: &mut P,
) -> Result<PathBuf, DownloadError>
where
    C: FnMut(DownloadCheckpoint) -> CheckpointAction,
    P: FnMut(u64, u64),
{
    partial.sync_all()?;
    let matches = match digest {
        Some(digest) if partial.metadata()?.len() == length => {
            format!("{:x}", digest.finalize()) == expected_sha256
        }
        _ => file_matches(&mut partial, length, expected_sha256, checkpoint)?,
    };
    if !matches {
        partial.set_len(0)?;
        partial.sync_data()?;
        return Err(DownloadError::HashMismatch);
    }
    progress(length, length);
    publish(partial, &paths.partial, &paths.complete)
}

async fn retry_or_return<C>(
    error: DownloadError,
    transient_failures: &mut usize,
    partial: &mut File,
    checkpoint: &mut C,
) -> Result<(), DownloadError>
where
    C: FnMut(DownloadCheckpoint) -> CheckpointAction,
{
    if *transient_failures >= MAX_RETRIES {
        partial.sync_data()?;
        return Err(error);
    }
    partial.sync_data()?;
    wait_at_durable_checkpoint(checkpoint)?;
    let delay = RETRY_BACKOFF[*transient_failures];
    *transient_failures += 1;

    let mut remaining = delay;
    while !remaining.is_zero() {
        let wait = remaining.min(PAUSE_POLL);
        sleep(wait).await;
        remaining = remaining.saturating_sub(wait);
        wait_at_durable_checkpoint(checkpoint)?;
    }
    Ok(())
}

enum StreamOutcome {
    Complete,
    Paused,
}

#[allow(clippy::too_many_arguments)]
async fn consume_response<C, P>(
    mut response: Response,
    expected_body_bytes: u64,
    total_bytes: u64,
    mut transferred: u64,
    partial: &mut File,
    mut digest: Option<&mut Sha256>,
    checkpoint: &mut C,
    progress: &mut P,
) -> Result<StreamOutcome, DownloadError>
where
    C: FnMut(DownloadCheckpoint) -> CheckpointAction,
    P: FnMut(u64, u64),
{
    let mut body_bytes = 0u64;
    let mut empty_chunks = 0usize;
    let mut prefix = Vec::with_capacity(HTML_SNIFF_BYTES);
    let mut sniff_complete = false;

    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) if body_bytes == expected_body_bytes => return Ok(StreamOutcome::Complete),
            Ok(None) => return Err(DownloadError::IncompleteResponse),
            Err(error) => return Err(DownloadError::Transport(error)),
        };

        if chunk.is_empty() {
            empty_chunks += 1;
            if empty_chunks > 4 {
                return Err(DownloadError::IncompleteResponse);
            }
            continue;
        }
        empty_chunks = 0;

        let chunk_len = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        if chunk_len > expected_body_bytes.saturating_sub(body_bytes) {
            partial.set_len(0)?;
            partial.sync_data()?;
            return Err(DownloadError::ExcessBytes);
        }

        if !sniff_complete {
            let prefix_bytes = (HTML_SNIFF_BYTES - prefix.len()).min(chunk.len());
            prefix.extend_from_slice(&chunk[..prefix_bytes]);
            if looks_like_html(&prefix) {
                partial.set_len(0)?;
                partial.sync_data()?;
                return Err(DownloadError::HtmlResponse);
            }
            sniff_complete = prefix.len() == HTML_SNIFF_BYTES
                || body_bytes.saturating_add(chunk_len) == expected_body_bytes;
        }

        for piece in chunk.chunks(IO_CHUNK) {
            partial.write_all(piece)?;
            if let Some(digest) = digest.as_mut() {
                digest.update(piece);
            }
            let bytes = piece.len() as u64;
            body_bytes += bytes;
            transferred += bytes;
            progress(transferred, total_bytes);

            match checkpoint(DownloadCheckpoint { durable: false }) {
                CheckpointAction::Continue => {}
                CheckpointAction::Pause => {
                    partial.sync_data()?;
                    return Ok(StreamOutcome::Paused);
                }
                CheckpointAction::Cancel => {
                    partial.sync_data()?;
                    return Err(DownloadError::Cancelled);
                }
            }
        }
    }
}

fn looks_like_html(prefix: &[u8]) -> bool {
    let prefix = prefix.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(prefix);
    let text = String::from_utf8_lossy(prefix).to_ascii_lowercase();
    let text = text.trim_start();
    text.starts_with("<!doctype html") || text.starts_with("<html")
}

pub(super) fn wait_at_durable_checkpoint<C>(checkpoint: &mut C) -> Result<(), DownloadError>
where
    C: FnMut(DownloadCheckpoint) -> CheckpointAction,
{
    let state = DownloadCheckpoint { durable: true };
    loop {
        match checkpoint(state) {
            CheckpointAction::Continue => return Ok(()),
            CheckpointAction::Cancel => return Err(DownloadError::Cancelled),
            CheckpointAction::Pause => std::thread::sleep(PAUSE_POLL),
        }
    }
}
