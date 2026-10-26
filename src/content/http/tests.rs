use super::files::*;
use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;
use crate::atomic_fs::is_link_or_reparse;
use crate::content::test_support::tempdir;
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
    crate::digest::sha256_hex(bytes)
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
fn a_clean_stream_is_verified_without_a_reread() {
    let bytes: Vec<u8> = (0..IO_CHUNK * 3 + 7).map(|i| (i % 251) as u8).collect();
    let spec = object_spec(&bytes);
    let temp = tempdir().unwrap();
    let server = TestServer::start(vec![ok_response(&bytes)]);
    let progress = Cell::new((0, 0));
    let mut durable_after_last_byte = 0;
    let path = download_object(
        &server.root,
        &spec,
        temp.path(),
        |state| {
            let (transferred, total) = progress.get();
            if state.durable && transferred == total && total > 0 {
                durable_after_last_byte += 1;
            }
            CheckpointAction::Continue
        },
        |transferred, total| progress.set((transferred, total)),
    )
    .unwrap();
    assert_eq!(server.finish().len(), 1);
    assert_eq!(durable_after_last_byte, 0);
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn a_range_resume_keeps_the_full_reread() {
    let bytes: Vec<u8> = (0..IO_CHUNK * 3 + 7).map(|i| (i % 251) as u8).collect();
    let spec = object_spec(&bytes);
    let temp = tempdir().unwrap();
    let (partial, _) = cache_paths(temp.path(), &spec);
    let prefix = IO_CHUNK + 11;
    fs::write(&partial, &bytes[..prefix]).unwrap();
    let server = TestServer::start(vec![partial_response(
        prefix,
        &bytes[prefix..],
        bytes.len(),
    )]);
    let progress = Cell::new((0, 0));
    let mut durable_after_last_byte = 0;
    let path = download_object(
        &server.root,
        &spec,
        temp.path(),
        |state| {
            let (transferred, total) = progress.get();
            if state.durable && transferred == total && total > 0 {
                durable_after_last_byte += 1;
            }
            CheckpointAction::Continue
        },
        |transferred, total| progress.set((transferred, total)),
    )
    .unwrap();
    let requests = server.finish();
    assert!(has_request_header(
        &requests[0],
        "Range",
        &format!("bytes={prefix}-")
    ));
    assert!(durable_after_last_byte >= 1);
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
