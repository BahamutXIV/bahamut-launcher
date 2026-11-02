use std::backtrace::Backtrace;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic::{self, PanicHookInfo};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::logging::redact_log_text;
use crate::version::LAUNCHER_VERSION;

const CRASH_LOG_HEADER: &str = "Bahamut launcher crash log";

struct CrashReport<'a> {
    time: DateTime<Utc>,
    version: &'a str,
    os: &'a str,
    arch: &'a str,
    thread: &'a str,
    message: &'a str,
    location: &'a str,
    backtrace: &'a str,
}

fn format_crash_log(report: &CrashReport<'_>) -> String {
    format!(
        "{CRASH_LOG_HEADER}\nTime (UTC): {}\nLauncher version: {}\nPlatform: {} {}\nThread: {}\nPanic message: {}\nPanic location: {}\n\nBacktrace:\n{}\n",
        report.time.format("%Y-%m-%dT%H:%M:%SZ"),
        report.version,
        report.os,
        report.arch,
        report.thread,
        redact_log_text(report.message),
        report.location,
        report.backtrace,
    )
}

/// Write panics to `directory`, then run the previous hook.
pub fn install_panic_hook(directory: PathBuf) {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // Write before tracing: its subscriber may be gone, and a nested panic aborts.
        let (message, location) = record_panic(&directory, info);
        previous(info);
        tracing::error!(message = %message, location = %location, "launcher panicked");
    }));
}

/// Ignore I/O failures; return the redacted message and location.
fn record_panic(directory: &Path, info: &PanicHookInfo<'_>) -> (String, String) {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("Box<dyn Any>");
    let location = info
        .location()
        .map(|at| format!("{}:{}:{}", at.file(), at.line(), at.column()))
        .unwrap_or_else(|| "<unknown>".into());
    let current = std::thread::current();
    let thread = current.name().unwrap_or("<unnamed>");
    let time = Utc::now();
    let backtrace = Backtrace::force_capture().to_string();
    let report = CrashReport {
        time,
        version: LAUNCHER_VERSION,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        thread,
        message,
        location: &location,
        backtrace: &backtrace,
    };
    let _ = write_crash_log(directory, &report, std::process::id());
    (redact_log_text(message), location)
}

fn write_crash_log(directory: &Path, report: &CrashReport<'_>, pid: u32) -> std::io::Result<()> {
    fs::create_dir_all(directory)?;
    let name = format!(
        "launcher-{}-{pid}.log",
        report.time.format("%Y%m%dT%H%M%SZ")
    );
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(name))?;
    file.write_all(format_crash_log(report).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 4, 5, 6, 7).unwrap()
    }

    #[test]
    fn log_lists_facts_and_redacts_the_message() {
        let report = CrashReport {
            time: time(),
            version: "v1.2.3",
            os: "windows",
            arch: "x86_64",
            thread: "main",
            message: "bad login password=hunter2 for user",
            location: "src/main.rs:10:5",
            backtrace: "   0: frame",
        };
        let text = format_crash_log(&report);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Bahamut launcher crash log");
        assert_eq!(lines[1], "Time (UTC): 2026-03-04T05:06:07Z");
        assert_eq!(lines[2], "Launcher version: v1.2.3");
        assert_eq!(lines[3], "Platform: windows x86_64");
        assert_eq!(lines[4], "Thread: main");
        assert!(
            lines[5].starts_with("Panic message: bad login "),
            "{}",
            lines[5]
        );
        assert!(!text.contains("hunter2"));
        assert_eq!(lines[6], "Panic location: src/main.rs:10:5");
        assert_eq!(lines[8], "Backtrace:");
        assert_eq!(lines[9], "   0: frame");
    }

    #[test]
    fn write_creates_directory_and_never_replaces_files() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("logs").join("crashes");
        let report = CrashReport {
            time: time(),
            version: "v1",
            os: "linux",
            arch: "x86_64",
            thread: "t",
            message: "boom",
            location: "a.rs:1:1",
            backtrace: "",
        };
        write_crash_log(&directory, &report, 7).unwrap();
        let path = directory.join("launcher-20260304T050607Z-7.log");
        let first = fs::read_to_string(&path).unwrap();
        assert!(first.starts_with("Bahamut launcher crash log\n"));
        write_crash_log(&directory, &report, 7).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap().len(), first.len() * 2);
    }
}
