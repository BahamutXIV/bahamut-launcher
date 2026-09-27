use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static SAVE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn write_atomic(path: &Path, body: &[u8]) -> io::Result<()> {
    write_staged(path, |output| output.write_all(body))
}

fn write_staged(path: &Path, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
    let temporary = temporary_path(path);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let written = (|| {
        if let Ok(metadata) = fs::metadata(path) {
            output.set_permissions(metadata.permissions())?;
        }
        write(&mut output)?;
        output.flush()?;
        output.sync_all()
    })();
    drop(output);
    let result = written.and_then(|()| replace_file(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn temporary_path(path: &Path) -> PathBuf {
    let counter = SAVE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() as u64);
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        timestamp ^ counter
    ));
    PathBuf::from(temporary)
}

#[cfg(not(target_os = "windows"))]
fn replace_file(temporary: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temporary, path)
}

#[cfg(target_os = "windows")]
fn replace_file(temporary: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let temporary_wide = temporary
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    let path_wide = path
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            windows::core::PCWSTR(temporary_wide.as_ptr()),
            windows::core::PCWSTR(path_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_staging_write_preserves_the_live_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");
        fs::write(&path, b"previous complete settings").unwrap();

        let result = write_staged(&path, |output| {
            output.write_all(b"partial replacement")?;
            assert_eq!(fs::read(&path)?, b"previous complete settings");
            Err(io::Error::other("injected write failure"))
        });

        assert_eq!(result.unwrap_err().to_string(), "injected write failure");
        assert_eq!(fs::read(&path).unwrap(), b"previous complete settings");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
