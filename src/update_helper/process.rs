//! Parent-process waiting and launcher restart.

use std::io;
use std::path::Path;
use std::process::{Child, Command};

use super::Result;
#[cfg(not(unix))]
use super::invalid;

pub(super) fn wait_for_process(pid: u32) -> Result<()> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };
        let process = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
            Ok(process) => process,
            // The launcher may exit between starting this helper and opening its handle.
            Err(error) if error.code().0 == 0x8007_0057_u32 as i32 => return Ok(()),
            Err(error) => {
                return Err(invalid(&format!(
                    "Could not wait for launcher process {pid}: {error}"
                )));
            }
        };
        let result = unsafe { WaitForSingleObject(process, u32::MAX) };
        let _ = unsafe { CloseHandle(process) };
        if result != WAIT_OBJECT_0 {
            return Err(invalid("Waiting for the launcher process failed."));
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        use std::thread;
        use std::time::Duration;
        loop {
            let result = unsafe { libc::kill(pid as i32, 0) };
            if result != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    return Ok(());
                }
                if error.raw_os_error() != Some(libc::EPERM) {
                    return Err(error.into());
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        Err(invalid(
            "Waiting for a parent process is unsupported on this platform.",
        ))
    }
}

/// Start the portable launcher from `target` with a transaction hand-off flag.
pub(super) fn spawn_launcher(target: &Path, flag: &str, transaction: &Path) -> io::Result<Child> {
    Command::new(target.join("bahamut-launcher.exe"))
        .current_dir(target)
        .arg(flag)
        .arg(transaction)
        .spawn()
}
