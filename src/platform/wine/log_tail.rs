//! Wine log tail shared by the Linux and macOS launch backends.

use std::path::Path;

pub(super) const WINE_LOG_TAIL_LINES: usize = 12;

pub(super) fn read_log_tail(log_path: &Path, max_lines: usize) -> String {
    match std::fs::read_to_string(log_path) {
        Ok(text) => {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(max_lines);
            lines[start..].join("\n")
        }
        Err(_) => String::new(),
    }
}
