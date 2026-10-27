//! Backup archive creation from the retail user data or extension state.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::Ordering;

use crate::atomic_fs::is_link_or_reparse;
use zip::write::SimpleFileOptions;

use super::catalog::{prune_backups, unique_archive_path};
use super::paths::{character_data_files, ensure_no_link_ancestors, read_sorted_dir};
use super::{
    BackupError, BackupLayout, BackupTarget, EXTENSION_UNITS, TRANSACTION_COUNTER,
    USER_CONFIG_FILES, USER_DIRECTORY, io_error,
};

const COPY_BUFFER_SIZE: usize = 64 * 1024;

pub(super) fn create_backup_in(
    target: BackupTarget,
    layout: &BackupLayout,
    timestamp: &str,
) -> Result<PathBuf, BackupError> {
    let files = collect_backup_files(target, layout)?;
    if files.is_empty() {
        return Err(BackupError::NoSource(target.label()));
    }

    ensure_no_link_ancestors(&layout.launcher_root, &layout.backups_root)?;
    fs::create_dir_all(&layout.backups_root)
        .map_err(|error| io_error("creating the backup folder", &layout.backups_root, error))?;
    ensure_no_link_ancestors(&layout.launcher_root, &layout.backups_root)?;
    let (partial_path, file) = create_backup_partial(&layout.backups_root)?;
    let result = write_archive(file, &partial_path, &files)
        .and_then(|()| publish_backup(target, &layout.backups_root, timestamp, &partial_path));
    let archive_path = match result {
        Ok(path) => path,
        Err(error) => {
            // This operation exclusively created the partial file.
            let _ = fs::remove_file(&partial_path);
            return Err(error);
        }
    };
    if let Err(error) = prune_backups(target, &layout.backups_root) {
        tracing::warn!(
            path = %layout.backups_root.display(),
            error = %error,
            "backup completed but old-backup cleanup failed"
        );
    }
    Ok(archive_path)
}

fn create_backup_partial(root: &Path) -> Result<(PathBuf, File), BackupError> {
    loop {
        let counter = TRANSACTION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(
            ".bahamut-backup-{}-{counter}.partial",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io_error("creating a backup", &path, error)),
        }
    }
}

fn publish_backup(
    target: BackupTarget,
    root: &Path,
    timestamp: &str,
    partial: &Path,
) -> Result<PathBuf, BackupError> {
    loop {
        let destination = unique_archive_path(target, root, timestamp)?;
        match crate::atomic_fs::rename_noreplace(partial, &destination) {
            Ok(()) => return Ok(destination),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io_error("publishing the backup", &destination, error)),
        }
    }
}

fn collect_backup_files(
    target: BackupTarget,
    layout: &BackupLayout,
) -> Result<Vec<(PathBuf, PathBuf)>, BackupError> {
    let mut roots = Vec::new();
    match target {
        BackupTarget::UserSettings => {
            if !layout.user_settings_root.is_dir() {
                return Err(BackupError::NoSource(target.label()));
            }
            for name in USER_CONFIG_FILES {
                let source = layout.user_settings_root.join(name);
                if source.is_file() {
                    roots.push((source, PathBuf::from(name)));
                }
            }
            let user_root = layout.user_settings_root.join(USER_DIRECTORY);
            if user_root.is_dir() {
                roots.extend(character_data_files(&user_root)?);
            }
        }
        BackupTarget::Extensions => {
            for relative in EXTENSION_UNITS {
                let relative = PathBuf::from(relative);
                let source = layout.launcher_root.join(&relative);
                if source.exists() {
                    roots.push((source, relative));
                }
            }
        }
    }

    let mut files = Vec::new();
    for (source, relative) in roots {
        collect_path(&source, &relative, &mut files)?;
    }
    files.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(files)
}

fn collect_path(
    source: &Path,
    relative: &Path,
    output: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), BackupError> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| io_error("reading backup source metadata", source, error))?;
    if is_link_or_reparse(&metadata) {
        return Err(BackupError::SymbolicLink(source.to_path_buf()));
    }
    if metadata.is_file() {
        output.push((source.to_path_buf(), relative.to_path_buf()));
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in read_sorted_dir(source)? {
            collect_path(&entry.path(), &relative.join(entry.file_name()), output)?;
        }
    }
    Ok(())
}

fn write_archive(file: File, path: &Path, files: &[(PathBuf, PathBuf)]) -> Result<(), BackupError> {
    let mut archive = zip::ZipWriter::new(BufWriter::new(file));
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o600);
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];

    for (source, relative) in files {
        let name = archive_name(relative)?;
        archive.start_file(name, options)?;
        let mut input = BufReader::new(
            File::open(source)
                .map_err(|error| io_error("opening a backup source file", source, error))?,
        );
        loop {
            let read = input
                .read(&mut buffer)
                .map_err(|error| io_error("reading a backup source file", source, error))?;
            if read == 0 {
                break;
            }
            archive
                .write_all(&buffer[..read])
                .map_err(|error| io_error("writing a backup archive", path, error))?;
        }
    }
    let mut output = archive.finish()?;
    output
        .flush()
        .map_err(|error| io_error("flushing a completed backup archive", path, error))?;
    output
        .get_ref()
        .sync_all()
        .map_err(|error| io_error("synchronizing a completed backup archive", path, error))?;
    Ok(())
}

fn archive_name(path: &Path) -> Result<String, BackupError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => return Err(BackupError::InvalidPath(path.to_path_buf())),
        }
    }
    if parts.is_empty() {
        return Err(BackupError::InvalidPath(path.to_path_buf()));
    }
    Ok(parts.join("/"))
}
