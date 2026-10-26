use crate::atomic_fs::is_link_or_reparse;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use super::download::wait_at_durable_checkpoint;
use super::{CheckpointAction, DownloadCheckpoint, DownloadError, IO_CHUNK};

pub(super) fn prepare_cache_dir(path: &Path) -> Result<PathBuf, DownloadError> {
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

pub(super) fn open_existing(path: &Path, write: bool) -> Result<Option<File>, DownloadError> {
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

pub(super) fn open_new_regular(path: &Path) -> Result<File, DownloadError> {
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

pub(super) fn file_matches<C>(
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

pub(super) fn publish(
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
