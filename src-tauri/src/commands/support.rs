use std::path::Path;

use bahamut_launcher::{backups, config::dirs, logging};

use crate::presentation::LauncherLogView;
use crate::state::{BackupIpcState, GameIpcState};

pub(crate) const RESTORE_BUSY_ERROR: &str = "Close the launcher-owned game and wait for any launch or restore to finish before restoring a backup.";

/// Keyed allowlist: only these identifiers cross IPC, so the WebView cannot request an arbitrary URL.
pub(crate) const EXTERNAL_LINKS: &[(&str, &str)] = &[
    ("github", "https://github.com/BahamutXIV/bahamut"),
    ("discord", "https://discord.gg/PxK5RJYQjm"),
    ("wiki", "https://bahamut.miraheze.org/wiki/Main_Page"),
    ("youtube", "https://www.youtube.com/@Aeshur"),
];

/// Rejects unknown keys rather than passing them to the shell.
#[tauri::command]
pub(crate) fn open_external(target: String) -> Result<(), String> {
    let url = EXTERNAL_LINKS
        .iter()
        .find(|(key, _)| *key == target)
        .map(|(_, url)| *url)
        .ok_or_else(|| format!("unknown link target: {target}"))?;
    open_in_browser(url).map_err(|err| format!("could not open {url}: {err}"))
}

/// Returns only the fixed native launcher transcript; Wine/runtime logs remain separate.
#[tauri::command]
pub(crate) fn get_launcher_log() -> Result<LauncherLogView, String> {
    let snapshot = logging::launcher_log_snapshot()
        .map_err(|error| format!("could not read launcher logs: {error}"))?;
    Ok(LauncherLogView {
        content: snapshot.content,
        log_path: snapshot.log_path.to_string_lossy().into_owned(),
        truncated: snapshot.truncated,
        updated_at: snapshot.updated_unix_ms,
    })
}

/// Persist a frontend failure with the displayed text and diagnostic context.
#[tauri::command]
pub(crate) fn record_ui_failure(
    page: String,
    action: String,
    message: String,
    diagnostic: String,
    context: String,
) -> Result<(), String> {
    logging::record_ui_failure(&page, &action, &message, &diagnostic, &context);
    Ok(())
}

#[tauri::command]
pub(crate) fn open_launcher_log() -> Result<(), String> {
    let path = dirs::launcher_log_path().map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("could not create {}: {error}", path.display()))?;
    open_path(&path).map_err(|error| format!("could not open {}: {error}", path.display()))
}

#[tauri::command]
pub(crate) async fn create_backup(
    backups: tauri::State<'_, BackupIpcState>,
    target: String,
) -> Result<String, String> {
    let target = backups::BackupTarget::parse(&target).map_err(|error| error.to_string())?;
    let operation = backups.begin()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        let archive_path = backups::create_backup(target).map_err(|error| {
            tracing::error!(target = target.label(), error = %error, "manual backup failed");
            error.to_string()
        })?;
        tracing::info!(
            target = target.label(),
            path = %archive_path.display(),
            "manual backup created"
        );
        Ok(format!("{} backup created.", target.label()))
    })
    .await
    .map_err(|error| format!("Backup worker failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn restore_backup(
    state: tauri::State<'_, GameIpcState>,
    backups: tauri::State<'_, BackupIpcState>,
    target: String,
) -> Result<String, String> {
    let target = backups::BackupTarget::parse(&target).map_err(|error| error.to_string())?;
    let reservation = state
        .begin_restore()
        .ok_or_else(|| RESTORE_BUSY_ERROR.to_string())?;
    let operation = backups.begin()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _reservation = reservation;
        let _operation = operation;
        let archive_path = backups::restore_latest_backup(target).map_err(|error| {
            tracing::error!(target = target.label(), error = %error, "manual restore failed");
            error.to_string()
        })?;
        tracing::info!(
            target = target.label(),
            path = %archive_path.display(),
            "latest manual backup restored"
        );
        Ok(format!("{} restored.", target.label()))
    })
    .await
    .map_err(|error| format!("Restore worker failed: {error}"))?
}

#[cfg(windows)]
fn open_path(path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{PCWSTR, w};

    let path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize <= 32 {
        return Err(std::io::Error::other(format!(
            "ShellExecuteW failed with code {}",
            result.0 as isize
        )));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_path(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg(path)
        .spawn()
        .map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_path(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
}

#[cfg(not(any(windows, unix)))]
fn open_path(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "opening a file is unsupported on this platform",
    ))
}

#[cfg(windows)]
fn open_in_browser(url: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn open_in_browser(url: &str) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_in_browser(url: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
}

#[cfg(not(any(windows, unix)))]
fn open_in_browser(_url: &str) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "opening a browser is unsupported on this platform",
    ))
}
