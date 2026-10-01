//! Native launcher diagnostics use [`tracing`]. The Tauri shell persists a
//! redacted transcript; Wine output remains a separate diagnostic source.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::MakeWriter;

use crate::config::dirs;

pub const SUPPORT_LOG_MAX_BYTES: usize = 256 * 1024;

const DEFAULT_LOG_FILTER: &str = "warn,bahamut_launcher=info,bahamut_launcher_shell=info";
const TRUNCATED_HEADER: &str = "[Log preview truncated to latest 256 KiB]\n";
const EMPTY_LOG_MESSAGE: &str = "No launcher log entries have been written yet.";
const UI_FAILURE_TARGET: &str = "bahamut_launcher_shell::ui_failure";
const LOG_TIMER_FORMAT: &str = "[%Y-%m-%d %H:%M:%S %:z]";

const SENSITIVE_KEYS: &[&str] = &[
    "authorization_header",
    "authorization-header",
    "authorization header",
    "authorizationheader",
    "credential_id",
    "credential-id",
    "credential id",
    "credentialid",
    "password_hash",
    "password-hash",
    "password hash",
    "passwordhash",
    "secret_key",
    "secret-key",
    "secret key",
    "secretkey",
    "session_key",
    "session-key",
    "session key",
    "sessionkey",
    "token_value",
    "token-value",
    "token value",
    "tokenvalue",
    "authorization",
    "credentials",
    "credential",
    "session_id",
    "session-id",
    "password",
    "api_key",
    "api-key",
    "api key",
    "apikey",
    "secret",
    "token",
    "pass",
    "session",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherLogSnapshot {
    pub content: String,
    pub log_path: PathBuf,
    pub truncated: bool,
    pub updated_unix_ms: Option<u64>,
}

struct RedactingFileWriter {
    file: Mutex<File>,
}

struct RedactingStderrWriter {
    stderr: Mutex<std::io::Stderr>,
}

struct RedactingEventWriter<'a, W: Write> {
    sink: MutexGuard<'a, W>,
    pending: Vec<u8>,
    mirror_stderr: bool,
}

impl RedactingFileWriter {
    fn new(file: File) -> Self {
        Self {
            file: Mutex::new(file),
        }
    }
}

impl RedactingStderrWriter {
    fn new() -> Self {
        Self {
            stderr: Mutex::new(std::io::stderr()),
        }
    }
}

impl<'a> MakeWriter<'a> for RedactingFileWriter {
    type Writer = RedactingEventWriter<'a, File>;

    fn make_writer(&'a self) -> Self::Writer {
        let file = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        RedactingEventWriter {
            sink: file,
            pending: Vec::new(),
            mirror_stderr: true,
        }
    }
}

impl<'a> MakeWriter<'a> for RedactingStderrWriter {
    type Writer = RedactingEventWriter<'a, std::io::Stderr>;

    fn make_writer(&'a self) -> Self::Writer {
        let stderr = self
            .stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        RedactingEventWriter {
            sink: stderr,
            pending: Vec::new(),
            mirror_stderr: false,
        }
    }
}

impl<W: Write> RedactingEventWriter<'_, W> {
    fn write_pending(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let redacted = redact_log_text(&String::from_utf8_lossy(&self.pending));
        self.sink.write_all(redacted.as_bytes())?;
        self.sink.flush()?;
        self.pending.clear();
        if self.mirror_stderr {
            std::io::stderr().write_all(redacted.as_bytes())?;
            std::io::stderr().flush()?;
        }
        Ok(())
    }
}

/// Record a frontend failure at error level so it survives the normal and coarse log filters.
/// The persistent writer redacts every rendered field before it reaches the transcript.
pub fn record_ui_failure(
    page: &str,
    action: &str,
    displayed_message: &str,
    diagnostic: &str,
    context: &str,
) {
    tracing::event!(
        target: UI_FAILURE_TARGET,
        tracing::Level::ERROR,
        version = env!("BAHAMUT_GIT_DESCRIBE"),
        platform = %format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        page = %page,
        action = %action,
        displayed_message = %displayed_message,
        diagnostic = %diagnostic,
        context = %context,
        "UI failure"
    );
}

impl<W: Write> Write for RedactingEventWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.write_pending()?;
        self.sink.flush()
    }
}

impl<W: Write> Drop for RedactingEventWriter<'_, W> {
    fn drop(&mut self) {
        let _ = self.write_pending();
    }
}

/// Install the global tracing subscriber; repeated calls are no-ops and never panic if one is already installed.
pub fn init() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
            LOG_TIMER_FORMAT.to_string(),
        ))
        .with_target(false)
        .with_writer(RedactingStderrWriter::new())
        .try_init();
}

/// Persist redacted native launcher events and mirror them to stderr.
pub fn init_persistent() -> io::Result<PathBuf> {
    let log_path = dirs::launcher_log_path().map_err(io::Error::other)?;
    let parent = log_path
        .parent()
        .ok_or_else(|| io::Error::other("launcher log path has no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    start_new_log_session(&log_path)?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
            LOG_TIMER_FORMAT.to_string(),
        ))
        .with_target(false)
        .with_ansi(false)
        .with_writer(RedactingFileWriter::new(file))
        .try_init();
    Ok(log_path)
}

fn start_new_log_session(path: &Path) -> io::Result<()> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(io::Error::other("launcher log path is not a file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("bahamut-launcher");
    let backup = path.with_file_name(format!("{stem}.previous.log"));
    if backup.exists() {
        std::fs::remove_file(&backup)?;
    }
    std::fs::rename(path, backup)
}

fn log_filter() -> EnvFilter {
    log_filter_with_env(std::env::var("RUST_LOG").ok().as_deref())
}

fn log_filter_with_env(value: Option<&str>) -> EnvFilter {
    let mut filter = EnvFilter::new(DEFAULT_LOG_FILTER);
    for directive in value
        .into_iter()
        .flat_map(|value| value.split(','))
        .filter_map(|directive| directive.trim().parse().ok())
    {
        filter = filter.add_directive(directive);
    }
    filter
}

/// Read the newest bounded native transcript and redact again at the IPC edge.
pub fn launcher_log_snapshot() -> io::Result<LauncherLogSnapshot> {
    let path = dirs::launcher_log_path().map_err(io::Error::other)?;
    read_log_snapshot(&path, SUPPORT_LOG_MAX_BYTES)
}

fn read_log_snapshot(path: &Path, max_bytes: usize) -> io::Result<LauncherLogSnapshot> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(LauncherLogSnapshot {
                content: EMPTY_LOG_MESSAGE.to_string(),
                log_path: path.to_path_buf(),
                truncated: false,
                updated_unix_ms: None,
            });
        }
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    let file_len = metadata.len();
    let mut truncated = file_len > max_bytes as u64;
    let payload_limit = if truncated {
        max_bytes.saturating_sub(TRUNCATED_HEADER.len())
    } else {
        max_bytes
    };
    let start = file_len.saturating_sub(payload_limit as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(payload_limit);
    file.take(payload_limit as u64).read_to_end(&mut bytes)?;
    if truncated {
        match bytes.iter().position(|byte| *byte == b'\n') {
            Some(newline) => {
                bytes.drain(..=newline);
            }
            None => bytes.clear(),
        }
    }
    let mut content = redact_log_text(&String::from_utf8_lossy(&bytes));
    let output_limit = if truncated {
        max_bytes.saturating_sub(TRUNCATED_HEADER.len())
    } else {
        max_bytes
    };
    if content.len() > output_limit {
        truncated = true;
        let output_limit = max_bytes.saturating_sub(TRUNCATED_HEADER.len());
        let mut start = content.len().saturating_sub(output_limit);
        while !content.is_char_boundary(start) {
            start += 1;
        }
        content = content[start..].to_string();
    }
    let content = if truncated {
        format!("{TRUNCATED_HEADER}{content}")
    } else if content.is_empty() {
        EMPTY_LOG_MESSAGE.to_string()
    } else {
        content
    };
    let updated_unix_ms = metadata.modified().ok().and_then(|updated| {
        updated
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_millis() as u64)
    });
    Ok(LauncherLogSnapshot {
        content,
        log_path: path.to_path_buf(),
        truncated,
        updated_unix_ms,
    })
}

pub fn redact_log_text(text: &str) -> String {
    let mut redacted = redact_bearer_values(text);
    redacted = redact_assignments(&redacted);
    redacted = redact_url_userinfo(&redacted);
    redact_session_ids(&redacted)
}

fn redact_assignments(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let lower_bytes = lower.as_bytes();
    let mut ranges = Vec::new();

    for key in SENSITIVE_KEYS {
        let key_bytes = key.as_bytes();
        let mut offset = 0;
        while offset + key_bytes.len() <= lower_bytes.len() {
            let Some(found) = lower[offset..].find(key) else {
                break;
            };
            let start = offset + found;
            let after = start + key_bytes.len();
            offset = after;
            let cli = start >= 2 && &lower_bytes[start - 2..start] == b"--";
            let after_ok = after == lower_bytes.len()
                || !matches!(lower_bytes[after], b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-');
            if !after_ok {
                continue;
            }

            let mut cursor = after;
            let (quote_start, quote) = quote_after_backslashes(bytes, cursor);
            if quote.is_some() {
                cursor = quote_start + 1;
            }
            let whitespace_start = cursor;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            let had_whitespace = cursor > whitespace_start;
            if cursor < bytes.len() && matches!(bytes[cursor], b'=' | b':') {
                cursor += 1;
                while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                    cursor += 1;
                }
            } else if !(cli && had_whitespace) {
                continue;
            }
            if cursor >= bytes.len() {
                continue;
            }
            let value_start = cursor;
            let value_end = structured_value_end(bytes, cursor);
            if value_end > value_start {
                ranges.push((value_start, value_end));
            }
        }
    }

    replace_ranges(text, ranges)
}

fn quote_after_backslashes(bytes: &[u8], start: usize) -> (usize, Option<u8>) {
    let mut cursor = start;
    while bytes.get(cursor) == Some(&b'\\') {
        cursor += 1;
    }
    let quote = bytes
        .get(cursor)
        .copied()
        .filter(|byte| matches!(byte, b'\'' | b'"'));
    (cursor, quote)
}

fn structured_value_end(bytes: &[u8], start: usize) -> usize {
    let mut cursor = start;
    let mut stack = Vec::new();
    let mut quote = None;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if matches!(byte, b'\\' | b'\'' | b'"') {
            let (quote_start, delimiter) = quote_after_backslashes(bytes, cursor);
            let backslashes = quote_start - cursor;
            if let Some((active_quote, encoding)) = quote {
                // Debug/JSON encoding escapes delimiters too. Pairs of encoded
                // data backslashes may precede a closing quote; an interior quote is escaped.
                if delimiter == Some(active_quote) && backslashes % (2 * (encoding + 1)) == encoding
                {
                    quote = None;
                    if stack.is_empty() {
                        return quote_start + 1;
                    }
                }
            } else if let Some(delimiter) = delimiter {
                if stack.is_empty() && cursor != start {
                    return cursor;
                }
                quote = Some((delimiter, backslashes));
            }
            cursor = quote_start + usize::from(delimiter.is_some());
            continue;
        }
        if quote.is_some() {
            cursor += 1;
            continue;
        }

        match byte {
            b'[' | b'{' | b'(' => stack.push(byte),
            b']' | b'}' | b')' => {
                if stack.is_empty() {
                    return cursor;
                }
                stack.pop();
                if stack.is_empty() {
                    return cursor + 1;
                }
            }
            b',' | b';' | b'&' if stack.is_empty() => return cursor,
            byte if byte.is_ascii_whitespace() && stack.is_empty() => return cursor,
            _ => {}
        }
        cursor += 1;
    }
    cursor
}

fn redact_url_userinfo(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut offset = 0;
    while let Some(found) = text[offset..].find("://") {
        let authority_start = offset + found + 3;
        let mut authority_end = authority_start;
        while authority_end < bytes.len()
            && !bytes[authority_end].is_ascii_whitespace()
            && !matches!(bytes[authority_end], b'/' | b'?' | b'#')
        {
            authority_end += 1;
        }
        let authority = &bytes[authority_start..authority_end];
        if let Some(at) = authority.iter().rposition(|byte| *byte == b'@') {
            let userinfo = &authority[..at];
            if let Some(colon) = userinfo.iter().position(|byte| *byte == b':') {
                let password_start = authority_start + colon + 1;
                let password_end = authority_start + at;
                if password_start < password_end {
                    ranges.push((password_start, password_end));
                }
            }
        }
        offset = authority_end;
    }
    replace_ranges(text, ranges)
}

fn redact_bearer_values(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut offset = 0;
    while let Some(found) = lower[offset..].find("bearer") {
        let start = offset + found;
        let after = start + "bearer".len();
        offset = after;
        if start > 0 && lower.as_bytes()[start - 1].is_ascii_alphanumeric() {
            continue;
        }
        let mut cursor = after;
        let whitespace_start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == whitespace_start {
            continue;
        }
        let value_start = cursor;
        while cursor < bytes.len()
            && !bytes[cursor].is_ascii_whitespace()
            && !matches!(bytes[cursor], b',' | b';')
        {
            cursor += 1;
        }
        if cursor > value_start {
            ranges.push((value_start, cursor));
        }
    }
    replace_ranges(text, ranges)
}

fn redact_session_ids(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if !bytes[cursor].is_ascii_hexdigit() {
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_hexdigit() {
            cursor += 1;
        }
        if cursor - start == 56 {
            ranges.push((start, cursor));
        }
    }
    replace_ranges(text, ranges)
}

fn replace_ranges(text: &str, mut ranges: Vec<(usize, usize)>) -> String {
    if ranges.is_empty() {
        return text.to_string();
    }
    ranges.sort_unstable();
    let mut output = String::with_capacity(text.len());
    let mut copied = 0;
    for (start, end) in ranges {
        if start < copied {
            continue;
        }
        output.push_str(&text[copied..start]);
        output.push_str("[REDACTED]");
        copied = end;
    }
    output.push_str(&text[copied..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_masks_assignments_headers_and_exact_session_ids() {
        let session = "0123456789abcdef0123456789abcdef0123456789abcdef01234567";
        let input = format!(
            "password=hunter2 pass: swordfish token='alpha' session_id=\"{session}\" \
             secret=beta authorization: Bearer gamma api-key=delta credential=epsilon \
             --password quoted-value access_token=zeta user_password=eta sessionToken=theta \
             secret_key=iota password_hash=kappa authorization_header=lambda \
             credential_id=mu sessionKey=nu tokenValue=xi password_len=8 \
             hash=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        );
        let output = redact_log_text(&input);
        for secret in [
            "hunter2",
            "swordfish",
            "alpha",
            session,
            "beta",
            "gamma",
            "delta",
            "epsilon",
            "quoted-value",
            "zeta",
            "eta",
            "theta",
            "iota",
            "kappa",
            "lambda",
            "mu",
            "nu",
            "xi",
        ] {
            assert!(!output.contains(secret), "secret survived: {secret}");
        }
        assert!(
            output.contains("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
        );
        assert!(output.contains("password_len=8"));
    }

    #[test]
    fn redaction_masks_nested_values_escaped_quotes_and_url_userinfo() {
        let input = r#"diagnostic={"token":["first-secret","second-secret"],"password":"prefix\"suffix-secret"} context=https://user:secret-value@downloads.example/"#;
        let output = redact_log_text(input);

        for secret in [
            "first-secret",
            "second-secret",
            "suffix-secret",
            "secret-value",
        ] {
            assert!(!output.contains(secret), "secret survived: {secret}");
        }
        assert!(output.contains("[REDACTED]"));
        assert!(output.contains("https://user:[REDACTED]@downloads.example/"));
    }

    #[test]
    fn redaction_masks_debug_error_chain_values() {
        let input = r#"Transport(reqwest::Error { url: Some("https://user:network-secret@downloads.example"), source: Some(hyper::Error { password: "prefix\"suffix-secret" }) })"#;
        let output = redact_log_text(input);

        for secret in ["network-secret", "suffix-secret"] {
            assert!(!output.contains(secret), "secret survived: {secret}");
        }
        assert!(output.contains("Transport(reqwest::Error"));
        assert!(output.contains("https://user:[REDACTED]@downloads.example"));
    }

    #[test]
    fn redaction_masks_colon_rich_userinfo_and_preserves_url_query_diagnostics() {
        let input = r#"Transport(reqwest::Error { url: "https://user:alpha-value:beta-value@downloads.example/", source: ConnectError("connection refused") }) comma=https://user:alpha,beta@downloads.example/ semicolon=https://user:alpha;beta@downloads.example/ bracket=https://user:alpha]beta@downloads.example/ brace=https://user:alpha}beta@downloads.example/ next=https://example.test/?token=query-secret&keep=this"#;
        let output = redact_log_text(input);

        for secret in [
            "alpha-value",
            "beta-value",
            "alpha,beta",
            "alpha;beta",
            "alpha]beta",
            "alpha}beta",
            "query-secret",
        ] {
            assert!(!output.contains(secret), "secret survived: {secret}");
        }
        assert!(output.contains("https://user:[REDACTED]@downloads.example/"));
        assert!(output.contains("comma=https://user:[REDACTED]@downloads.example/"));
        assert!(output.contains("semicolon=https://user:[REDACTED]@downloads.example/"));
        assert!(output.contains("bracket=https://user:[REDACTED]@downloads.example/"));
        assert!(output.contains("brace=https://user:[REDACTED]@downloads.example/"));
        assert!(output.contains("https://example.test/?token=[REDACTED]&keep=this"));
        assert!(output.contains("source: ConnectError(\"connection refused\")"));
    }

    #[test]
    fn persistent_writer_redacts_before_writing_to_disk() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("launcher.log");
        let sink = RedactingFileWriter::new(File::create(&path).unwrap());
        {
            let mut writer = sink.make_writer();
            writeln!(writer, "login password=hunter2 token=alpha").unwrap();
        }
        drop(sink);

        let persisted = std::fs::read_to_string(path).unwrap();
        assert!(!persisted.contains("hunter2"));
        assert!(!persisted.contains("alpha"));
        assert_eq!(persisted, "login password=[REDACTED] token=[REDACTED]\n");
    }

    #[test]
    fn persistent_writer_buffers_fragmented_multiline_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("launcher.log");
        let sink = RedactingFileWriter::new(File::create(&path).unwrap());
        {
            let mut writer = sink.make_writer();
            writer
                .write_all(br#"diagnostic={"token":["first-"#)
                .unwrap();
            writer
                .write_all(b"\nsecret\",\"second-secret\"],\"password\":\"prefix\\")
                .unwrap();
            writer.write_all(b"\"suffix-secret\"}\n").unwrap();
        }

        let persisted = std::fs::read_to_string(path).unwrap();
        for secret in ["first-secret", "second-secret", "suffix-secret"] {
            assert!(!persisted.contains(secret), "secret survived: {secret}");
        }
        assert!(persisted.contains("[REDACTED]"));
    }

    #[test]
    fn persistent_writer_preserves_debug_diagnostics_while_redacting_urls_and_queries() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("launcher.log");
        let sink = RedactingFileWriter::new(File::create(&path).unwrap());
        {
            let mut writer = sink.make_writer();
            writeln!(
                writer,
                r#"Transport(reqwest::Error {{ url: "https://user:alpha-value:beta-value@downloads.example/", source: ConnectError("connection refused") }}) comma=https://user:alpha,beta@downloads.example/ semicolon=https://user:alpha;beta@downloads.example/ bracket=https://user:alpha]beta@downloads.example/ brace=https://user:alpha}}beta@downloads.example/ next=https://example.test/?token=query-secret&keep=this"#
            )
            .unwrap();
        }

        let persisted = std::fs::read_to_string(path).unwrap();
        for secret in [
            "alpha-value",
            "beta-value",
            "alpha,beta",
            "alpha;beta",
            "alpha]beta",
            "alpha}beta",
            "query-secret",
        ] {
            assert!(!persisted.contains(secret), "secret survived: {secret}");
        }
        assert!(persisted.contains("https://user:[REDACTED]@downloads.example/"));
        assert!(persisted.contains("comma=https://user:[REDACTED]@downloads.example/"));
        assert!(persisted.contains("semicolon=https://user:[REDACTED]@downloads.example/"));
        assert!(persisted.contains("bracket=https://user:[REDACTED]@downloads.example/"));
        assert!(persisted.contains("brace=https://user:[REDACTED]@downloads.example/"));
        assert!(persisted.contains("https://example.test/?token=[REDACTED]&keep=this"));
        assert!(persisted.contains("source: ConnectError(\"connection refused\")"));
    }

    #[test]
    fn ui_failure_persists_context_and_redacts_sensitive_values() {
        let temp = tempfile::tempdir().unwrap();
        for (name, environment) in [("default", None), ("coarse", Some("warn"))] {
            let path = temp.path().join(format!("{name}.log"));
            let sink = RedactingFileWriter::new(File::create(&path).unwrap());
            let subscriber = tracing_subscriber::fmt()
                .with_env_filter(log_filter_with_env(environment))
                .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
                    LOG_TIMER_FORMAT.to_string(),
                ))
                .with_target(false)
                .with_ansi(false)
                .with_writer(sink)
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                record_ui_failure(
                    "home",
                    "launch",
                    "Could not launch",
                    r#"{"token":["first-secret","second-secret"],"password":"prefix\"suffix-secret"}"#,
                    "download=https://user:secret-value@downloads.example/",
                );
            });

            let persisted = std::fs::read_to_string(path).unwrap();
            let timestamp = persisted.split(']').next().unwrap();
            assert!(timestamp.contains(" +") || timestamp.contains(" -"));
            assert!(persisted.contains("page=home"));
            assert!(persisted.contains("action=launch"));
            assert!(persisted.contains("displayed_message=Could not launch"));
            assert!(
                persisted.contains("context=download=https://user:[REDACTED]@downloads.example/")
            );
            assert!(persisted.contains("version="));
            assert!(persisted.contains("platform="));
            for secret in [
                "first-secret",
                "second-secret",
                "suffix-secret",
                "secret-value",
            ] {
                assert!(!persisted.contains(secret), "secret survived: {secret}");
            }
        }
    }

    #[test]
    fn escaped_auth_diagnostics_are_redacted_before_persistence_and_support_copy() {
        let temp = tempfile::tempdir().unwrap();
        let body = r#"{"password":"prefix\"suffix-secret","token":["first-secret","second-secret"],"note":"diagnostic kept"}"#;
        let error = crate::auth::client::AuthClientError::Malformed(format!(
            "missing session_id; body excerpt: {body}"
        ));
        let frontend = serde_json::json!({ "kind": "server", "message": error.to_string() });
        for (name, diagnostic) in [
            ("native", format!("{error:?}")),
            ("frontend", frontend.to_string()),
        ] {
            let path = temp.path().join(format!("{name}.log"));
            let subscriber = tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(RedactingFileWriter::new(File::create(&path).unwrap()))
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                if name == "native" {
                    tracing::error!(diagnostic = ?error, "auth failure translated");
                } else {
                    record_ui_failure("home", "log in", "Could not log in", &diagnostic, "");
                }
            });
            let persisted = std::fs::read_to_string(&path).unwrap();
            let support = read_log_snapshot(&path, SUPPORT_LOG_MAX_BYTES).unwrap();
            for text in [&persisted, &support.content] {
                for secret in ["prefix", "suffix-secret", "first-secret", "second-secret"] {
                    assert!(!text.contains(secret), "{name} retained {secret}: {text}");
                }
                assert!(text.contains("[REDACTED]"));
                assert!(text.contains("missing session_id"));
                assert!(text.contains("diagnostic kept"));
            }
        }
    }

    #[test]
    fn persistent_log_starts_fresh_and_retains_one_previous_session() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bahamut-launcher.log");
        std::fs::write(&path, "first session\n").unwrap();

        start_new_log_session(&path).unwrap();
        assert!(!path.exists());
        let previous = temp.path().join("bahamut-launcher.previous.log");
        assert_eq!(
            std::fs::read_to_string(&previous).unwrap(),
            "first session\n"
        );

        std::fs::write(&path, "second session\n").unwrap();
        start_new_log_session(&path).unwrap();
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(previous).unwrap(),
            "second session\n"
        );
    }

    #[test]
    fn first_log_session_needs_no_previous_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bahamut-launcher.log");

        start_new_log_session(&path).unwrap();

        assert!(!path.exists());
        assert!(!temp.path().join("bahamut-launcher.previous.log").exists());
    }

    #[test]
    fn snapshot_is_bounded_and_discards_a_partial_boundary_line() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("launcher.log");
        let secret = "boundary-secret";
        let prefix = format!("password={secret}");
        let padding = "x".repeat(SUPPORT_LOG_MAX_BYTES);
        std::fs::write(&path, format!("{prefix}{padding}\nnewest safe line\n")).unwrap();

        let snapshot = read_log_snapshot(&path, SUPPORT_LOG_MAX_BYTES).unwrap();
        assert!(snapshot.truncated);
        assert!(snapshot.content.len() <= SUPPORT_LOG_MAX_BYTES);
        assert!(!snapshot.content.contains(secret));
        assert!(snapshot.content.contains("newest safe line"));
    }

    #[test]
    fn snapshot_remains_bounded_when_redaction_expands_values() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("launcher.log");
        std::fs::write(&path, "token=x\n".repeat(14)).unwrap();

        let snapshot = read_log_snapshot(&path, 128).unwrap();
        assert!(snapshot.truncated);
        assert!(snapshot.content.len() <= 128);
        assert!(!snapshot.content.contains("token=x"));
        assert!(snapshot.content.contains("[REDACTED]"));
    }

    #[test]
    fn absent_snapshot_is_a_stable_non_secret_message() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing.log");
        let snapshot = read_log_snapshot(&path, SUPPORT_LOG_MAX_BYTES).unwrap();
        assert_eq!(snapshot.content, EMPTY_LOG_MESSAGE);
        assert!(!snapshot.truncated);
        assert_eq!(snapshot.updated_unix_ms, None);
    }

    #[test]
    fn empty_snapshot_has_the_same_stable_message() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("empty.log");
        File::create(&path).unwrap();

        let snapshot = read_log_snapshot(&path, SUPPORT_LOG_MAX_BYTES).unwrap();
        assert_eq!(snapshot.content, EMPTY_LOG_MESSAGE);
        assert!(!snapshot.truncated);
        assert!(snapshot.updated_unix_ms.is_some());
    }

    #[test]
    fn coarse_environment_filter_keeps_launcher_info_baseline() {
        let filter = log_filter_with_env(Some("warn"));
        let rendered = filter.to_string();
        assert!(rendered.contains("bahamut_launcher=info"));
        assert!(rendered.contains("bahamut_launcher_shell=info"));
        assert!(rendered.contains("warn"));
    }
}
