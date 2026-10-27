//! Trusted update helper staging from the verified launcher ZIP.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use bahamut_launcher::atomic_fs::is_link_or_reparse;
use bahamut_launcher::release::{FileOwnership, ReleaseInventory, ReleaseMetadata};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use super::fs_io::{ensure_existing_directory, hash_file};
use super::{UNIQUE_COUNTER, UPDATE_HELPER_FILE};

const IO_CHUNK: usize = 64 * 1024;

pub(super) fn staged_helper_path(
    stage: &Path,
    metadata: &ReleaseMetadata,
) -> Result<PathBuf, String> {
    ensure_existing_directory(stage, "launcher update stage")?;
    let ReleaseInventory::Launcher { files } = &metadata.inventory else {
        return Err("Signed launcher helper inventory is missing.".into());
    };
    let expected = files
        .iter()
        .find(|file| file.path == UPDATE_HELPER_FILE && file.ownership == FileOwnership::Managed)
        .ok_or_else(|| "Signed launcher release does not manage its update helper.".to_owned())?;
    let path = stage.join(UPDATE_HELPER_FILE);
    let details = fs::symlink_metadata(&path)
        .map_err(|_| "The staged bahamut-update-helper.exe is missing.".to_owned())?;
    if !details.is_file() || is_link_or_reparse(&details) {
        return Err("The staged bahamut-update-helper.exe is not a regular file.".into());
    }
    let (length, hash) = hash_file(&path)?;
    if length != expected.length || hash != expected.sha256 {
        return Err(
            "The staged bahamut-update-helper.exe failed signed inventory verification.".into(),
        );
    }
    Ok(path)
}

pub(super) fn stage_trusted_helper_from_artifact(
    artifact: &Path,
    stage: &Path,
    metadata: &ReleaseMetadata,
) -> Result<PathBuf, String> {
    ensure_existing_directory(stage, "launcher update stage")?;
    let ReleaseInventory::Launcher { files } = &metadata.inventory else {
        return Err("Signed launcher helper inventory is missing.".into());
    };
    let expected = files
        .iter()
        .find(|file| file.path == UPDATE_HELPER_FILE && file.ownership == FileOwnership::Managed)
        .ok_or_else(|| "Signed launcher release does not manage its update helper.".to_owned())?;
    let input = File::open(artifact)
        .map_err(|error| format!("Could not open the verified launcher ZIP: {error}"))?;
    let mut archive = ZipArchive::new(input)
        .map_err(|error| format!("Could not read the verified launcher ZIP: {error}"))?;
    let mut entry = archive
        .by_name(UPDATE_HELPER_FILE)
        .map_err(|_| "Verified launcher ZIP is missing bahamut-update-helper.exe.".to_owned())?;
    if entry.is_dir() || entry.size() != expected.length {
        return Err("Verified launcher helper entry changed after validation.".into());
    }
    let path = stage.join(UPDATE_HELPER_FILE);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("Could not stage bahamut-update-helper.exe: {error}"))?;
    let result = (|| {
        let mut digest = Sha256::new();
        let mut length = 0_u64;
        let mut buffer = [0_u8; IO_CHUNK];
        loop {
            let read = entry
                .read(&mut buffer)
                .map_err(|error| format!("Could not read the launcher helper entry: {error}"))?;
            if read == 0 {
                break;
            }
            length = length
                .checked_add(read as u64)
                .ok_or_else(|| "Launcher helper entry length overflowed.".to_owned())?;
            if length > expected.length {
                return Err("Launcher helper entry exceeded its signed length.".into());
            }
            output
                .write_all(&buffer[..read])
                .map_err(|error| format!("Could not write staged launcher helper: {error}"))?;
            digest.update(&buffer[..read]);
        }
        output
            .flush()
            .and_then(|()| output.sync_all())
            .map_err(|error| format!("Could not flush staged launcher helper: {error}"))?;
        if length != expected.length || format!("{:x}", digest.finalize()) != expected.sha256 {
            return Err("Staged launcher helper failed signed inventory verification.".into());
        }
        Ok(())
    })();
    drop(output);
    if let Err(error) = result {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

pub(super) fn create_sibling_stage(target: &Path) -> Result<PathBuf, String> {
    let parent = target
        .parent()
        .ok_or_else(|| "Launcher install directory has no parent.".to_owned())?;
    ensure_existing_directory(parent, "launcher install parent")?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| format!("Could not resolve launcher install parent: {error}"))?;
    for _ in 0..64 {
        let serial = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let stage = canonical_parent.join(format!(
            ".bahamut-launcher-update-{}-{serial}",
            std::process::id()
        ));
        match fs::create_dir(&stage) {
            Ok(()) => {
                ensure_existing_directory(&stage, "launcher update stage")?;
                return Ok(stage);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Could not create same-volume launcher update stage: {error}"
                ));
            }
        }
    }
    Err("Could not allocate a unique launcher update stage.".into())
}

pub(super) fn process_matches_executable(pid: u32, expected_path: &Path) -> Result<bool, String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        };
        use windows::core::PWSTR;

        let process = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
            Ok(process) => process,
            Err(error)
                if error.code().0 == 0x8007_0057_u32 as i32
                    || error.code().0 == 0x8007_0005_u32 as i32 =>
            {
                return Ok(false);
            }
            Err(error) => {
                return Err(format!(
                    "Could not inspect launcher update helper process {pid}: {error}"
                ));
            }
        };
        let mut buffer = vec![0_u16; 32768];
        let mut length = buffer.len() as u32;
        let queried = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        };
        let _ = unsafe { CloseHandle(process) };
        queried.map_err(|error| {
            format!("Could not read launcher update helper process path: {error}")
        })?;
        let actual_path = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]));
        let comparable = |path: &Path| {
            let value = path.to_string_lossy();
            if let Some(rest) = value.strip_prefix("\\\\?\\UNC\\") {
                format!("\\\\{rest}")
            } else if let Some(rest) = value.strip_prefix("\\\\?\\") {
                rest.to_owned()
            } else {
                value.into_owned()
            }
        };
        Ok(comparable(&actual_path).eq_ignore_ascii_case(&comparable(expected_path)))
    }
    #[cfg(not(windows))]
    {
        let _ = (pid, expected_path);
        // Managed launcher updates are currently scoped to Windows x86_64.
        Ok(false)
    }
}
