// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

//! Bounded, identity-checked HTTP object downloads for the R2 content lane.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use reqwest::header::{
    ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, LOCATION, RANGE,
};
use reqwest::{Client, Response, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::runtime::Builder as RuntimeBuilder;
use tokio::time::sleep;
use url::Host;

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

#[derive(Debug)]
struct CachePaths {
    partial: PathBuf,
    complete: PathBuf,
}

impl CachePaths {
    fn new(cache: &Path, sha256: &str, length: u64) -> Self {
        let identity = format!("{sha256}-{length}");
        Self {
            partial: cache.join(format!("{identity}.part")),
            complete: cache.join(format!("{identity}.object")),
        }
    }
}

async fn download_object_async<C, P>(
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

    if let Some(mut complete) = open_existing(&paths.complete, false)? {
        if file_matches(&mut complete, length, &expected_sha256, checkpoint)? {
            progress(length, length);
            return Ok(paths.complete);
        }
        drop(complete);
        fs::remove_file(&paths.complete)?;
        progress(0, length);
    }

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
        if file_matches(&mut partial, length, &expected_sha256, checkpoint)? {
            progress(length, length);
            return publish(partial, &paths.partial, &paths.complete);
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
        if file_matches(&mut partial, length, &expected_sha256, checkpoint)? {
            progress(length, length);
            return publish(partial, &paths.partial, &paths.complete);
        }
        return Err(DownloadError::HashMismatch);
    }

    let mut transient_failures = 0;
    let mut clean_restart_used = false;
    let mut force_full = false;

    loop {
        partial_length = partial.metadata()?.len();
        if partial_length == length {
            partial.sync_all()?;
            if file_matches(&mut partial, length, &expected_sha256, checkpoint)? {
                progress(length, length);
                return publish(partial, &paths.partial, &paths.complete);
            }
            partial.set_len(0)?;
            partial.sync_data()?;
            return Err(DownloadError::HashMismatch);
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

        let (response_start, response_bytes) = match status {
            StatusCode::OK => {
                if has_header(&response, &CONTENT_RANGE)? {
                    return Err(DownloadError::ContentRange);
                }
                validate_representation_headers(&response, length)?;
                if partial_length > 0 {
                    partial.set_len(0)?;
                    partial.seek(SeekFrom::Start(0))?;
                    partial.sync_data()?;
                    progress(0, length);
                }
                (0, length)
            }
            StatusCode::PARTIAL_CONTENT => {
                let range = parse_content_range(&response)?;
                if range.start != partial_length
                    || range.total != length
                    || range.end < range.start
                    || range.end >= range.total
                    || range.end != range.total - 1
                {
                    return Err(DownloadError::ContentRange);
                }
                let bytes = range.end - range.start + 1;
                validate_representation_headers(&response, bytes)?;
                (range.start, bytes)
            }
            _ => return Err(DownloadError::HttpStatus(status.as_u16())),
        };

        partial.seek(SeekFrom::Start(response_start))?;
        let outcome = consume_response(
            response,
            response_bytes,
            length,
            response_start,
            &mut partial,
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
                partial.sync_all()?;
                if !file_matches(&mut partial, length, &expected_sha256, checkpoint)? {
                    partial.set_len(0)?;
                    partial.sync_data()?;
                    return Err(DownloadError::HashMismatch);
                }
                progress(length, length);
                return publish(partial, &paths.partial, &paths.complete);
            }
            Err(error) if error.is_retryable() => {
                retry_or_return(error, &mut transient_failures, &mut partial, checkpoint).await?;
            }
            Err(error) => return Err(error),
        }
    }
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

async fn consume_response<C, P>(
    mut response: Response,
    expected_body_bytes: u64,
    total_bytes: u64,
    mut transferred: u64,
    partial: &mut File,
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

fn wait_at_durable_checkpoint<C>(checkpoint: &mut C) -> Result<(), DownloadError>
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

async fn send_with_redirects(
    client: &Client,
    initial_url: &Url,
    range_start: Option<u64>,
) -> Result<Response, DownloadError> {
    let mut url = initial_url.clone();
    let mut require_https = url.scheme() == "https";

    for redirect_count in 0..=MAX_REDIRECTS {
        let mut request = client.get(url.clone()).header(ACCEPT_ENCODING, "identity");
        if let Some(start) = range_start {
            request = request.header(RANGE, format!("bytes={start}-"));
        }
        let response = request.send().await.map_err(DownloadError::Transport)?;
        if !is_redirect_status(response.status()) {
            return Ok(response);
        }
        if redirect_count == MAX_REDIRECTS {
            return Err(DownloadError::InvalidRedirect);
        }

        let locations = response.headers().get_all(LOCATION);
        let mut values = locations.iter();
        let location = values
            .next()
            .and_then(|value| value.to_str().ok())
            .ok_or(DownloadError::InvalidRedirect)?;
        if values.next().is_some() {
            return Err(DownloadError::InvalidRedirect);
        }
        let next = url
            .join(location)
            .map_err(|_| DownloadError::InvalidRedirect)?;
        validate_url(&next, require_https).map_err(|_| DownloadError::InvalidRedirect)?;
        require_https |= next.scheme() == "https";
        url = next;
    }

    Err(DownloadError::InvalidRedirect)
}

fn object_url(root: &str, object_key: &str) -> Result<Url, DownloadError> {
    let mut url = Url::parse(root).map_err(|_| DownloadError::InvalidRoot)?;
    validate_url(&url, false)?;

    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| DownloadError::InvalidRoot)?;
        segments.pop_if_empty();
        for segment in object_key.split('/') {
            segments.push(segment);
        }
    }
    Ok(url)
}

fn validate_object_key(object_key: &str) -> Result<(), DownloadError> {
    if object_key.is_empty()
        || object_key.starts_with('/')
        || object_key.contains('\\')
        || object_key.chars().any(char::is_control)
        || object_key
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(DownloadError::InvalidObjectKey);
    }
    Ok(())
}

fn validate_url(url: &Url, require_https: bool) -> Result<(), DownloadError> {
    if has_user_info(url)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(DownloadError::InvalidRoot);
    }

    match url.scheme() {
        "https" => Ok(()),
        "http" if !require_https && is_loopback_host(url) => Ok(()),
        _ => Err(DownloadError::InvalidRoot),
    }
}

fn has_user_info(url: &Url) -> bool {
    url.as_str()
        .split_once("://")
        .and_then(|(_, remainder)| remainder.split('/').next())
        .is_some_and(|authority| authority.contains('@'))
}

fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

fn validate_spec(spec: &ObjectSpec) -> Result<String, DownloadError> {
    validate_object_key(&spec.object_key)?;
    if spec.sha256.len() != 64 || !spec.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DownloadError::InvalidSpec);
    }
    Ok(spec.sha256.to_ascii_lowercase())
}

fn prepare_cache_dir(path: &Path) -> Result<PathBuf, DownloadError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut current = PathBuf::new();

    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                current.pop();
            }
            Component::Normal(part) => {
                current.push(part);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) => validate_cache_directory(&metadata)?,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        match fs::create_dir(&current) {
                            Ok(()) => {}
                            Err(create_error)
                                if create_error.kind() == io::ErrorKind::AlreadyExists => {}
                            Err(create_error) => return Err(create_error.into()),
                        }
                        let metadata = fs::symlink_metadata(&current)?;
                        validate_cache_directory(&metadata)?;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }

    Ok(fs::canonicalize(current)?)
}

fn validate_cache_directory(metadata: &fs::Metadata) -> Result<(), DownloadError> {
    if is_link_or_reparse(metadata) {
        return Err(DownloadError::UnsafeCachePath);
    }
    if !metadata.is_dir() {
        return Err(DownloadError::CacheNotDirectory);
    }
    Ok(())
}

fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    false
}

fn open_existing(path: &Path, write: bool) -> Result<Option<File>, DownloadError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_file() {
                return Err(DownloadError::UnsafeCachePath);
            }
            let mut options = OpenOptions::new();
            options.read(true).write(write);
            set_no_follow(&mut options);
            let file = options.open(path)?;
            validate_open_file(&file)?;
            Ok(Some(file))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn open_new_regular(path: &Path) -> Result<File, DownloadError> {
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    set_no_follow(&mut options);
    let file = options.open(path)?;
    validate_open_file(&file)?;
    Ok(file)
}

fn set_no_follow(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
}

fn validate_open_file(file: &File) -> Result<(), DownloadError> {
    let metadata = file.metadata()?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(DownloadError::UnsafeCachePath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(DownloadError::UnsafeCachePath);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{
            FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandleEx,
        };

        let mut info = FILE_STANDARD_INFO::default();
        unsafe {
            GetFileInformationByHandleEx(
                HANDLE(file.as_raw_handle()),
                FileStandardInfo,
                &mut info as *mut _ as _,
                std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
            )
        }
        .map_err(io::Error::other)?;
        if info.NumberOfLinks != 1 {
            return Err(DownloadError::UnsafeCachePath);
        }
    }
    Ok(())
}

fn parse_content_range(response: &Response) -> Result<ByteRange, DownloadError> {
    let values = response.headers().get_all(CONTENT_RANGE);
    let mut headers = values.iter();
    let value = headers
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or(DownloadError::ContentRange)?;
    if headers.next().is_some() {
        return Err(DownloadError::ContentRange);
    }
    let value = value
        .strip_prefix("bytes ")
        .ok_or(DownloadError::ContentRange)?;
    let (range, total) = value.split_once('/').ok_or(DownloadError::ContentRange)?;
    let (start, end) = range.split_once('-').ok_or(DownloadError::ContentRange)?;
    let start = parse_decimal(start).ok_or(DownloadError::ContentRange)?;
    let end = parse_decimal(end).ok_or(DownloadError::ContentRange)?;
    let total = parse_decimal(total).ok_or(DownloadError::ContentRange)?;
    Ok(ByteRange { start, end, total })
}

#[derive(Clone, Copy, Debug)]
struct ByteRange {
    start: u64,
    end: u64,
    total: u64,
}

fn parse_decimal(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn validate_representation_headers(
    response: &Response,
    expected_body_bytes: u64,
) -> Result<(), DownloadError> {
    let encodings = response.headers().get_all(CONTENT_ENCODING);
    let mut values = encodings.iter();
    if let Some(value) = values.next()
        && (values.next().is_some()
            || !value
                .to_str()
                .map(|value| value.eq_ignore_ascii_case("identity"))
                .unwrap_or(false))
    {
        return Err(DownloadError::ContentEncoding);
    }

    let content_types = response.headers().get_all(CONTENT_TYPE);
    let mut values = content_types.iter();
    if let Some(value) = values.next() {
        if values.next().is_some() {
            return Err(DownloadError::HtmlResponse);
        }
        let media_type = value
            .to_str()
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        if media_type.eq_ignore_ascii_case("text/html")
            || media_type.eq_ignore_ascii_case("application/xhtml+xml")
        {
            return Err(DownloadError::HtmlResponse);
        }
    }

    let lengths = response.headers().get_all(CONTENT_LENGTH);
    let mut values = lengths.iter();
    if let Some(value) = values.next() {
        if values.next().is_some() {
            return Err(DownloadError::ResponseLength);
        }
        let value = value
            .to_str()
            .ok()
            .and_then(parse_decimal)
            .ok_or(DownloadError::ResponseLength)?;
        if value != expected_body_bytes {
            return Err(DownloadError::ResponseLength);
        }
    }
    Ok(())
}

fn has_header(
    response: &Response,
    name: &reqwest::header::HeaderName,
) -> Result<bool, DownloadError> {
    let values = response.headers().get_all(name);
    let mut headers = values.iter();
    let has_one = headers.next().is_some();
    if headers.next().is_some() {
        return Err(DownloadError::ResponseLength);
    }
    Ok(has_one)
}

fn is_redirect_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn is_transient_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

fn file_matches<C>(
    file: &mut File,
    expected_length: u64,
    expected_sha256: &str,
    checkpoint: &mut C,
) -> Result<bool, DownloadError>
where
    C: FnMut(DownloadCheckpoint) -> CheckpointAction,
{
    if file.metadata()?.len() != expected_length {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut transferred = 0u64;
    let mut buffer = [0u8; IO_CHUNK];

    while transferred < expected_length {
        wait_at_durable_checkpoint(checkpoint)?;

        let remaining = expected_length - transferred;
        let limit = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = file.read(&mut buffer[..limit])?;
        if read == 0 {
            return Ok(false);
        }
        transferred += read as u64;
        digest.update(&buffer[..read]);
    }

    let mut extra = [0u8; 1];
    if file.read(&mut extra)? != 0 || file.metadata()?.len() != expected_length {
        return Ok(false);
    }
    file.seek(SeekFrom::End(0))?;
    Ok(format!("{:x}", digest.finalize()) == expected_sha256)
}

fn publish(
    partial: File,
    partial_path: &Path,
    complete_path: &Path,
) -> Result<PathBuf, DownloadError> {
    partial.sync_all()?;
    drop(partial);
    let partial_metadata = fs::symlink_metadata(partial_path)?;
    if is_link_or_reparse(&partial_metadata) || !partial_metadata.is_file() {
        return Err(DownloadError::UnsafeCachePath);
    }

    match fs::rename(partial_path, complete_path) {
        Ok(()) => {
            open_existing(complete_path, false)?.ok_or(DownloadError::UnsafeCachePath)?;
            Ok(complete_path.to_path_buf())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(complete_path)?;
            if is_link_or_reparse(&metadata) || !metadata.is_file() {
                return Err(DownloadError::UnsafeCachePath);
            }
            fs::remove_file(complete_path)?;
            fs::rename(partial_path, complete_path)?;
            open_existing(complete_path, false)?.ok_or(DownloadError::UnsafeCachePath)?;
            Ok(complete_path.to_path_buf())
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patcher::test_support::tempdir;
    use std::cell::Cell;
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    struct RawResponse {
        status: &'static str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl RawResponse {
        fn new(status: &'static str, body: &[u8]) -> Self {
            Self {
                status,
                headers: vec![("Content-Length".into(), body.len().to_string())],
                body: body.to_vec(),
            }
        }

        fn with_header(mut self, name: &str, value: &str) -> Self {
            self.headers.push((name.into(), value.into()));
            self
        }

        fn raw_bytes(&self) -> Vec<u8> {
            let mut response = format!("HTTP/1.1 {}\r\n", self.status).into_bytes();
            for (name, value) in &self.headers {
                response.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
            }
            response.extend_from_slice(b"Connection: close\r\n\r\n");
            response.extend_from_slice(&self.body);
            response
        }
    }

    struct TestServer {
        root: String,
        requests: Arc<Mutex<Vec<String>>>,
        join: Option<JoinHandle<()>>,
    }

    impl TestServer {
        fn start(responses: Vec<RawResponse>) -> Self {
            let response_count = responses.len();
            let mut responses = responses.into_iter();
            Self::start_with_handler(response_count, move |_| {
                responses.next().expect("one response per request")
            })
        }

        fn start_with_handler<F>(response_count: usize, mut handler: F) -> Self
        where
            F: FnMut(&str) -> RawResponse + Send + 'static,
        {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let received = Arc::clone(&requests);
            let join = thread::spawn(move || {
                let mut next_response = 0;
                let deadline = Instant::now() + Duration::from_secs(5);
                while next_response < response_count && Instant::now() < deadline {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            // Accepted sockets inherit the listener's non-blocking flag
                            // except on Linux; the handler blocks.
                            stream.set_nonblocking(false).unwrap();
                            stream
                                .set_read_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            stream
                                .set_write_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            let request = read_request(&mut stream);
                            if request.is_empty() {
                                continue;
                            }
                            received.lock().unwrap().push(request.clone());
                            let response = handler(&request).raw_bytes();
                            next_response += 1;
                            let _ = stream.write_all(&response);
                            let _ = stream.flush();
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                root: format!("http://{address}/"),
                requests,
                join: Some(join),
            }
        }

        fn finish(mut self) -> Vec<String> {
            if let Some(join) = self.join.take() {
                join.join().unwrap();
            }
            self.requests.lock().unwrap().clone()
        }
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut byte = [0u8; 1];
        while bytes.len() < 16 * 1024 {
            match stream.read_exact(&mut byte) {
                Ok(()) => {
                    bytes.push(byte[0]);
                    if bytes.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn object_spec(bytes: &[u8]) -> ObjectSpec {
        ObjectSpec {
            object_key: "game/base/object.bin".into(),
            length: bytes.len() as u64,
            sha256: sha256(bytes),
        }
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn cache_paths(cache: &Path, spec: &ObjectSpec) -> (PathBuf, PathBuf) {
        let identity = format!("{}-{}", spec.sha256.to_ascii_lowercase(), spec.length);
        (
            cache.join(format!("{identity}.part")),
            cache.join(format!("{identity}.object")),
        )
    }

    fn continue_download(_: DownloadCheckpoint) -> CheckpointAction {
        CheckpointAction::Continue
    }

    fn has_request_header(request: &str, name: &str, value: &str) -> bool {
        let expected = format!(
            "{}: {}",
            name.to_ascii_lowercase(),
            value.to_ascii_lowercase()
        );
        request
            .lines()
            .any(|line| line.to_ascii_lowercase() == expected)
    }

    fn request_range_start(request: &str) -> Option<u64> {
        request.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("range") {
                value
                    .trim()
                    .strip_prefix("bytes=")?
                    .split_once('-')?
                    .0
                    .parse()
                    .ok()
            } else {
                None
            }
        })
    }

    fn download(root: &str, spec: &ObjectSpec, cache: &Path) -> Result<PathBuf, DownloadError> {
        download_object(root, spec, cache, continue_download, |_, _| {})
    }

    fn ok_response(bytes: &[u8]) -> RawResponse {
        RawResponse::new("200 OK", bytes).with_header("Content-Type", "application/octet-stream")
    }

    #[test]
    fn fixture_does_not_consume_a_response_for_an_empty_connection() {
        let bytes = b"verified object payload";
        let spec = object_spec(bytes);
        let temp = tempdir().unwrap();
        let server = TestServer::start(vec![ok_response(bytes)]);
        drop(
            TcpStream::connect(
                server
                    .root
                    .strip_prefix("http://")
                    .unwrap()
                    .strip_suffix('/')
                    .unwrap(),
            )
            .unwrap(),
        );
        let path = download(&server.root, &spec, temp.path()).unwrap();
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert_eq!(server.finish().len(), 1);
    }

    fn partial_response(start: usize, bytes: &[u8], total: usize) -> RawResponse {
        RawResponse::new("206 Partial Content", bytes)
            .with_header(
                "Content-Range",
                &format!("bytes {start}-{}/{total}", total - 1),
            )
            .with_header("Content-Type", "application/octet-stream")
    }

    #[test]
    fn downloads_verifies_and_reuses_the_identity_cache() {
        let bytes = b"verified object payload";
        let spec = object_spec(bytes);
        let temp = tempdir().unwrap();
        let server = TestServer::start(vec![ok_response(bytes)]);
        let root = server.root.clone();
        let path = download(&root, &spec, temp.path()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(server.finish().len(), 1);

        let cached = download(&root, &spec, temp.path()).unwrap();
        assert_eq!(cached, path);
    }

    #[test]
    fn a_corrupt_completed_cache_file_is_replaced() {
        let bytes = b"expected immutable bytes";
        let spec = object_spec(bytes);
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path()).unwrap();
        let (_, complete) = cache_paths(temp.path(), &spec);
        fs::write(&complete, vec![b'x'; bytes.len()]).unwrap();

        let server = TestServer::start(vec![ok_response(bytes)]);
        let path = download(&server.root, &spec, temp.path()).unwrap();
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn cancellation_preserves_a_durable_prefix_and_resume_requests_its_range() {
        let bytes = vec![0x5a; IO_CHUNK * 2 + 23];
        let spec = object_spec(&bytes);
        let temp = tempdir().unwrap();
        let first = TestServer::start(vec![ok_response(&bytes)]);
        let mut cancelled = false;
        let progress_bytes = Cell::new(0);
        let error = download_object(
            &first.root,
            &spec,
            temp.path(),
            |state| {
                if !state.durable && progress_bytes.get() >= IO_CHUNK as u64 && !cancelled {
                    cancelled = true;
                    CheckpointAction::Cancel
                } else {
                    CheckpointAction::Continue
                }
            },
            |transferred, _| progress_bytes.set(transferred),
        )
        .unwrap_err();
        assert!(matches!(error, DownloadError::Cancelled));
        assert_eq!(first.finish().len(), 1);

        let (partial, complete) = cache_paths(temp.path(), &spec);
        let partial_length = fs::metadata(&partial).unwrap().len();
        assert!(partial_length >= IO_CHUNK as u64);
        assert!(partial_length < bytes.len() as u64);
        assert!(!complete.exists());

        let suffix = &bytes[partial_length as usize..];
        let resumed = TestServer::start(vec![partial_response(
            partial_length as usize,
            suffix,
            bytes.len(),
        )]);
        let path = download(&resumed.root, &spec, temp.path()).unwrap();
        let requests = resumed.finish();
        assert!(has_request_header(
            &requests[0],
            "Range",
            &format!("bytes={partial_length}-")
        ));
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn a_pause_syncs_and_closes_the_response_before_range_resume() {
        let bytes = vec![0x33; IO_CHUNK * 2 + 13];
        let spec = object_spec(&bytes);
        let temp = tempdir().unwrap();
        let response_bytes = bytes.clone();
        let first = TestServer::start_with_handler(2, move |request| {
            if let Some(start) = request_range_start(request) {
                partial_response(
                    start as usize,
                    &response_bytes[start as usize..],
                    response_bytes.len(),
                )
            } else {
                ok_response(&response_bytes)
            }
        });
        let mut pause_requested = false;
        let mut durable_ack = false;
        let progress_bytes = Cell::new(0);
        let path = download_object(
            &first.root,
            &spec,
            temp.path(),
            |state| {
                if !pause_requested && progress_bytes.get() >= IO_CHUNK as u64 && !state.durable {
                    pause_requested = true;
                    return CheckpointAction::Pause;
                }
                if pause_requested && state.durable {
                    durable_ack = true;
                }
                CheckpointAction::Continue
            },
            |transferred, _| progress_bytes.set(transferred),
        )
        .unwrap();
        let requests = first.finish();
        assert!(pause_requested && durable_ack);
        assert_eq!(requests.len(), 2);
        let resumed_at = request_range_start(&requests[1]).unwrap();
        assert!(resumed_at >= IO_CHUNK as u64);
        assert!(resumed_at < bytes.len() as u64);
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn user_pauses_do_not_consume_the_network_retry_budget() {
        let bytes = vec![0x33; IO_CHUNK * 10];
        let spec = object_spec(&bytes);
        let temp = tempdir().unwrap();
        let response_bytes = bytes.clone();
        let server = TestServer::start_with_handler(7, move |request| {
            if let Some(start) = request_range_start(request) {
                partial_response(
                    start as usize,
                    &response_bytes[start as usize..],
                    response_bytes.len(),
                )
            } else {
                ok_response(&response_bytes)
            }
        });
        let mut pauses = 0;
        let path = download_object(
            &server.root,
            &spec,
            temp.path(),
            |state| {
                if !state.durable && pauses < 6 {
                    pauses += 1;
                    CheckpointAction::Pause
                } else {
                    CheckpointAction::Continue
                }
            },
            |_, _| {},
        )
        .unwrap();
        assert_eq!(pauses, 6);
        assert_eq!(server.finish().len(), 7);
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn pause_after_the_last_byte_publishes_without_redownloading() {
        let bytes = b"a completed final checkpoint";
        let spec = object_spec(bytes);
        let temp = tempdir().unwrap();
        let server = TestServer::start(vec![ok_response(bytes)]);
        let mut paused = false;
        let progress = Cell::new((0, 0));
        let path = download_object(
            &server.root,
            &spec,
            temp.path(),
            |state| {
                if !state.durable && progress.get().0 == progress.get().1 {
                    paused = true;
                    CheckpointAction::Pause
                } else {
                    CheckpointAction::Continue
                }
            },
            |transferred, total| progress.set((transferred, total)),
        )
        .unwrap();
        assert!(paused);
        assert_eq!(server.finish().len(), 1);
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn cache_hashing_waits_at_the_paused_offset() {
        let bytes = b"cached bytes";
        let temp = tempdir().unwrap();
        let path = temp.path().join("object");
        fs::write(&path, bytes).unwrap();
        let mut file = File::open(path).unwrap();
        let mut observer = file.try_clone().unwrap();
        let mut calls = 0;
        assert!(
            file_matches(
                &mut file,
                bytes.len() as u64,
                &object_spec(bytes).sha256,
                &mut |state| {
                    calls += 1;
                    assert!(state.durable);
                    assert_eq!(observer.stream_position().unwrap(), 0);
                    if calls < 3 {
                        CheckpointAction::Pause
                    } else {
                        CheckpointAction::Continue
                    }
                }
            )
            .unwrap()
        );
        assert_eq!(calls, 3);
    }

    #[test]
    fn a_full_200_response_restarts_an_existing_partial_file() {
        let bytes = b"server ignored range and sent full body";
        let spec = object_spec(bytes);
        let temp = tempdir().unwrap();
        let (partial, _) = cache_paths(temp.path(), &spec);
        fs::write(&partial, &bytes[..5]).unwrap();
        let server = TestServer::start(vec![ok_response(bytes)]);

        let path = download(&server.root, &spec, temp.path()).unwrap();
        let requests = server.finish();
        assert!(has_request_header(&requests[0], "Range", "bytes=5-"));
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn a_416_restarts_once_without_range() {
        let bytes = b"the authoritative object";
        let spec = object_spec(bytes);
        let temp = tempdir().unwrap();
        let (partial, _) = cache_paths(temp.path(), &spec);
        fs::write(&partial, b"old").unwrap();
        let unsatisfied = RawResponse::new("416 Range Not Satisfiable", b"")
            .with_header("Content-Range", "bytes */3");
        let server = TestServer::start(vec![unsatisfied, ok_response(bytes)]);

        let path = download(&server.root, &spec, temp.path()).unwrap();
        let requests = server.finish();
        assert!(
            has_request_header(&requests[0], "Range", "bytes=3-"),
            "first request: {:?}",
            requests[0]
        );
        assert!(!requests[1].to_ascii_lowercase().contains("range:"));
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn malformed_content_range_is_rejected_before_append() {
        let bytes = b"expected content";
        let spec = object_spec(bytes);
        let temp = tempdir().unwrap();
        let (partial, complete) = cache_paths(temp.path(), &spec);
        fs::write(&partial, b"abc").unwrap();
        let response = RawResponse::new("206 Partial Content", &bytes[3..])
            .with_header("Content-Range", "bytes 2-15/16");
        let server = TestServer::start(vec![response]);

        let error = download(&server.root, &spec, temp.path()).unwrap_err();
        assert!(matches!(error, DownloadError::ContentRange));
        assert_eq!(fs::read(partial).unwrap(), b"abc");
        assert!(!complete.exists());
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn an_early_eof_retries_from_the_durable_byte_offset() {
        let bytes = b"a body that is retried after early eof";
        let spec = object_spec(bytes);
        let temp = tempdir().unwrap();
        let short = &bytes[..10];
        let mut early = RawResponse::new("200 OK", short);
        early.headers[0].1 = bytes.len().to_string();
        let server = TestServer::start(vec![
            early,
            partial_response(short.len(), &bytes[10..], bytes.len()),
        ]);

        let path = download(&server.root, &spec, temp.path()).unwrap();
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(has_request_header(&requests[1], "Range", "bytes=10-"));
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn excess_bytes_and_wrong_hash_never_publish_an_object() {
        let bytes = b"exact bytes";
        let temp = tempdir().unwrap();
        let mut excess_spec = object_spec(bytes);
        let mut excess_response = ok_response(b"exact bytes!");
        excess_response
            .headers
            .retain(|(name, _)| !name.eq_ignore_ascii_case("Content-Length"));
        let excess = TestServer::start(vec![excess_response]);
        let error = download(&excess.root, &excess_spec, temp.path()).unwrap_err();
        assert!(matches!(error, DownloadError::ExcessBytes));
        let (_, excess_final) = cache_paths(temp.path(), &excess_spec);
        assert!(!excess_final.exists());
        excess.finish();

        let wrong = TestServer::start(vec![ok_response(bytes)]);
        excess_spec.sha256 = "0".repeat(64);
        let error = download(&wrong.root, &excess_spec, temp.path()).unwrap_err();
        assert!(matches!(error, DownloadError::HashMismatch), "{error:?}");
        let (wrong_partial, wrong_final) = cache_paths(temp.path(), &excess_spec);
        assert_eq!(fs::metadata(wrong_partial).unwrap().len(), 0);
        assert!(!wrong_final.exists());
        wrong.finish();
    }

    #[test]
    fn an_untyped_html_error_page_is_not_retained_as_partial_content() {
        let html = b"<!DOCTYPE html><html><body>not the object</body></html>";
        let spec = object_spec(html);
        let temp = tempdir().unwrap();
        let server = TestServer::start(vec![RawResponse::new("200 OK", html)]);

        let error = download(&server.root, &spec, temp.path()).unwrap_err();
        assert!(matches!(error, DownloadError::HtmlResponse));
        let (partial, complete) = cache_paths(temp.path(), &spec);
        assert_eq!(fs::metadata(partial).unwrap().len(), 0);
        assert!(!complete.exists());
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn transient_status_retries_stop_at_the_fixed_limit() {
        let bytes = b"object not served";
        let spec = object_spec(bytes);
        let temp = tempdir().unwrap();
        let responses = (0..=MAX_RETRIES)
            .map(|_| RawResponse::new("503 Service Unavailable", b"temporary"))
            .collect();
        let server = TestServer::start(responses);

        let error = download(&server.root, &spec, temp.path()).unwrap_err();
        assert!(matches!(error, DownloadError::HttpStatus(503)), "{error:?}");
        assert_eq!(server.finish().len(), MAX_RETRIES + 1);
    }

    #[test]
    fn cache_path_errors_are_returned_without_panicking() {
        let bytes = b"payload";
        let spec = object_spec(bytes);
        let temp = tempdir().unwrap();
        let cache_file = temp.path().join("not-a-directory");
        fs::write(&cache_file, b"file").unwrap();

        let error = download("https://downloads.example/", &spec, &cache_file).unwrap_err();
        assert!(matches!(error, DownloadError::CacheNotDirectory));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn cache_hard_links_are_rejected_before_modifying_other_files() {
        let spec = object_spec(b"payload");
        for completed in [false, true] {
            let temp = tempdir().unwrap();
            let cache = temp.path().join("cache");
            fs::create_dir(&cache).unwrap();
            let outside = temp.path().join("owner-data");
            let owner_bytes = b"owner bytes longer than expected payload";
            fs::write(&outside, owner_bytes).unwrap();
            let (partial, object) = cache_paths(&cache, &spec);
            fs::hard_link(&outside, if completed { &object } else { &partial }).unwrap();

            let result = download_object(
                "https://downloads.example/",
                &spec,
                &cache,
                |_| CheckpointAction::Cancel,
                |_, _| {},
            );

            assert!(matches!(result, Err(DownloadError::UnsafeCachePath)));
            assert_eq!(fs::read(&outside).unwrap(), owner_bytes);
        }
    }

    #[test]
    fn redirects_and_requests_do_not_forward_credentials_or_accept_remote_http() {
        let bytes = b"redirected bytes";
        let spec = object_spec(bytes);
        let temp = tempdir().unwrap();
        let target = TestServer::start(vec![ok_response(bytes)]);
        let redirect = RawResponse::new("302 Found", b"").with_header("Location", &target.root);
        let source = TestServer::start(vec![redirect]);

        let path = download(&source.root, &spec, temp.path()).unwrap();
        let source_requests = source.finish();
        let target_requests = target.finish();
        assert!(
            !source_requests[0]
                .to_ascii_lowercase()
                .contains("authorization:")
        );
        assert!(!source_requests[0].to_ascii_lowercase().contains("cookie:"));
        assert!(
            !target_requests[0]
                .to_ascii_lowercase()
                .contains("authorization:")
        );
        assert!(!target_requests[0].to_ascii_lowercase().contains("cookie:"));
        assert_eq!(fs::read(path).unwrap(), bytes);

        let external_redirect =
            RawResponse::new("302 Found", b"").with_header("Location", "http://example.com/object");
        let rejected = TestServer::start(vec![external_redirect]);
        let rejected_cache = tempdir().unwrap();
        let error = download(&rejected.root, &spec, rejected_cache.path()).unwrap_err();
        assert!(matches!(error, DownloadError::InvalidRedirect));
        assert_eq!(rejected.finish().len(), 1);

        let remote_http = download("http://example.com/", &spec, temp.path());
        assert!(matches!(remote_http, Err(DownloadError::InvalidRoot)));
        let userinfo = download("https://user:secret@downloads.example/", &spec, temp.path());
        assert!(matches!(userinfo, Err(DownloadError::InvalidRoot)));
    }

    #[cfg(windows)]
    #[test]
    fn cache_junction_is_rejected_without_touching_its_target() {
        use std::os::windows::process::CommandExt;

        let temp = tempdir().unwrap();
        let outside = temp.path().join("outside");
        let alias = temp.path().join("alias");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel"), b"preserve").unwrap();
        let result = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&alias)
            .arg(&outside)
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(result.status.success(), "could not create fixture junction");
        assert!(is_link_or_reparse(&fs::symlink_metadata(&alias).unwrap()));
        let result = download(
            "https://downloads.example/",
            &object_spec(b"payload"),
            &alias,
        );
        // Remove the verified link itself before the temporary tree is dropped.
        fs::remove_dir(&alias).unwrap();
        assert!(matches!(result, Err(DownloadError::UnsafeCachePath)));
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"preserve");
        assert_eq!(fs::read_dir(outside).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn cache_directory_symlinks_are_rejected_without_touching_the_target() {
        use std::os::unix::fs::symlink;

        let bytes = b"payload";
        let spec = object_spec(bytes);
        let temp = tempdir().unwrap();
        let outside = temp.path().join("outside");
        let alias = temp.path().join("alias");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, &alias).unwrap();

        let error = download("https://downloads.example/", &spec, &alias).unwrap_err();
        assert!(matches!(error, DownloadError::UnsafeCachePath));
        assert_eq!(fs::read_dir(outside).unwrap().count(), 0);

        let cache = temp.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let external_file = temp.path().join("external.part");
        fs::write(&external_file, b"outside bytes").unwrap();
        let (partial, _) = cache_paths(&cache, &spec);
        symlink(&external_file, &partial).unwrap();
        let error = download("https://downloads.example/", &spec, &cache).unwrap_err();
        assert!(matches!(error, DownloadError::UnsafeCachePath));
        assert_eq!(fs::read(external_file).unwrap(), b"outside bytes");
    }

    #[test]
    fn object_keys_reject_absolute_and_parent_segments() {
        let temporary = tempfile::tempdir().unwrap();
        for key in ["/rooted", "../escape", "a/../b", "a\\b", "a//b"] {
            let mut spec = object_spec(b"x");
            spec.object_key = key.to_owned();
            assert!(matches!(
                download("https://downloads.example/", &spec, temporary.path()),
                Err(DownloadError::InvalidObjectKey)
            ));
        }
    }
}
