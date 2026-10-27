//! Archive member selection, extraction, and staging for repair.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path};

use sha2::{Digest, Sha256};
use zip::ZipArchive;

use super::IO_BUFFER_BYTES;
use super::control::RepairShared;
use super::paths::join_relative;
use crate::atomic_fs::is_link_or_reparse;
use crate::content::manifest::{ArchiveLayout, BaseArchive, BasePackage, InstallFile};
use crate::digest::sha256_hex;
use crate::version::{FFXIV_BOOT_VERSION, FFXIV_GAME_VERSION};

#[derive(Clone)]
pub(super) struct RepairSource {
    pub(super) archive_index: usize,
    pub(super) member_path: String,
}

pub(super) fn repair_sources(
    package: &BasePackage,
) -> Result<HashMap<String, RepairSource>, String> {
    let mut sources = HashMap::new();
    let finals = package
        .final_files
        .iter()
        .map(|file| (file.path.to_ascii_lowercase(), file))
        .collect::<HashMap<_, _>>();
    for (archive_index, archive) in package.archives.iter().enumerate() {
        for file in &archive.files {
            if matches!(
                file.path.to_ascii_lowercase().as_str(),
                "boot.ver" | "game.ver"
            ) {
                return Err(format!(
                    "Archive must not contain managed version file {}.",
                    file.path
                ));
            }
            let final_file = finals.get(&file.path.to_ascii_lowercase()).ok_or_else(|| {
                format!("Archive file is not in the final inventory: {}", file.path)
            })?;
            if final_file.length != file.length
                || !final_file.sha256.eq_ignore_ascii_case(&file.sha256)
            {
                return Err(format!(
                    "Archive and final-file identities differ for {}.",
                    file.path
                ));
            }
            let key = file.path.to_ascii_lowercase();
            sources.insert(
                key,
                RepairSource {
                    archive_index,
                    member_path: archive_member_path(archive.layout, &file.path),
                },
            );
        }
    }
    for file in &package.final_files {
        match file.path.to_ascii_lowercase().as_str() {
            "boot.ver" => validate_generated_version(file, FFXIV_BOOT_VERSION)?,
            "game.ver" => validate_generated_version(file, FFXIV_GAME_VERSION)?,
            _ if !sources.contains_key(&file.path.to_ascii_lowercase()) => {
                return Err(format!("No full-archive repair source maps {}.", file.path));
            }
            _ => {}
        }
    }
    Ok(sources)
}

pub(super) fn archive_member_path(layout: ArchiveLayout, path: &str) -> String {
    match layout {
        ArchiveLayout::Flat => path.to_owned(),
        ArchiveLayout::FinalFantasyXivWrapper => format!("FINAL FANTASY XIV/{path}"),
    }
}

pub(super) fn validate_generated_version(file: &InstallFile, contents: &str) -> Result<(), String> {
    let digest = sha256_hex(contents.as_bytes());
    if file.length != contents.len() as u64 || !file.sha256.eq_ignore_ascii_case(&digest) {
        return Err(format!(
            "The final catalog has an invalid {} identity.",
            file.path
        ));
    }
    Ok(())
}

pub(super) fn required_archive_indices(
    sources: &HashMap<String, RepairSource>,
    files: &[&InstallFile],
) -> HashSet<usize> {
    files
        .iter()
        .filter_map(|file| sources.get(&file.path.to_ascii_lowercase()))
        .map(|source| source.archive_index)
        .collect()
}

pub(super) fn extract_selected_file(
    archive_path: &Path,
    archive_spec: &BaseArchive,
    source: &RepairSource,
    spec: &InstallFile,
    stage: &Path,
    shared: &RepairShared,
) -> Result<(), String> {
    let archive_file = File::open(archive_path)
        .map_err(|error| format!("Could not open the verified game archive: {error}"))?;
    let mut archive = ZipArchive::new(archive_file)
        .map_err(|error| format!("Could not read the verified game archive: {error}"))?;
    let mut matches = 0;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Could not inspect a game archive entry: {error}"))?;
        if entry.is_dir()
            || !archive_name_matches(archive_spec.layout, entry.name(), &source.member_path)
        {
            continue;
        }
        matches += 1;
        if matches > 1 {
            return Err(format!(
                "Repair archive contains duplicate member {}.",
                spec.path
            ));
        }
        validate_entry_mode(entry.unix_mode(), &spec.path)?;
        if entry.size() != spec.length {
            return Err(format!(
                "Repair archive member length differs for {}.",
                spec.path
            ));
        }
        let output_path = join_relative(stage, &spec.path);
        create_owned_parents(
            stage,
            output_path.parent().expect("managed file has a parent"),
        )?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
            .map_err(|error| format!("Could not stage {}: {error}", spec.path))?;
        let digest = copy_and_hash(&mut entry, &mut output, &spec.path, spec.length, shared)?;
        if !digest.eq_ignore_ascii_case(&spec.sha256) {
            return Err(format!(
                "Repair archive member failed SHA-256 verification: {}.",
                spec.path
            ));
        }
        output
            .sync_all()
            .map_err(|error| format!("Could not sync staged {}: {error}", spec.path))?;
    }
    if matches != 1 {
        return Err(format!("Repair archive is missing member {}.", spec.path));
    }
    Ok(())
}

pub(super) fn archive_name_matches(
    layout: ArchiveLayout,
    raw_name: &str,
    member_path: &str,
) -> bool {
    let candidate = match layout {
        ArchiveLayout::Flat => raw_name,
        ArchiveLayout::FinalFantasyXivWrapper => {
            match raw_name.strip_prefix("FINAL FANTASY XIV/") {
                Some(relative) => relative,
                None => return false,
            }
        }
    };
    candidate
        == member_path
            .strip_prefix("FINAL FANTASY XIV/")
            .unwrap_or(member_path)
}

pub(super) fn validate_entry_mode(mode: Option<u32>, name: &str) -> Result<(), String> {
    if let Some(mode) = mode {
        let kind = mode & 0o170000;
        if mode & 0o7000 != 0 || (kind != 0 && kind != 0o100000) {
            return Err(format!(
                "Repair archive member is not a regular file: {name}"
            ));
        }
    }
    Ok(())
}

pub(super) fn copy_and_hash(
    reader: &mut impl Read,
    writer: &mut impl Write,
    label: &str,
    expected_length: u64,
    shared: &RepairShared,
) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; IO_BUFFER_BYTES];
    loop {
        shared.checkpoint()?;
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("Could not read repair member {label}: {error}"))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| format!("Repair member length overflow: {label}"))?;
        if total > expected_length {
            return Err(format!(
                "Repair archive member exceeds its catalog length: {label}"
            ));
        }
        hasher.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .map_err(|error| format!("Could not write staged repair member {label}: {error}"))?;
        shared.add_bytes(read as u64);
    }
    if total != expected_length {
        return Err(format!("Repair archive member is truncated: {label}"));
    }
    writer
        .flush()
        .map_err(|error| format!("Could not flush staged repair member {label}: {error}"))?;
    Ok(format!("{:x}", hasher.finalize()))
}

pub(super) fn write_staged_bytes(
    stage: &Path,
    file: &InstallFile,
    bytes: &[u8],
) -> Result<(), String> {
    let path = join_relative(stage, &file.path);
    create_owned_parents(stage, path.parent().expect("managed file has a parent"))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("Could not stage {}: {error}", file.path))?;
    output
        .write_all(bytes)
        .and_then(|()| output.sync_all())
        .map_err(|error| format!("Could not persist staged {}: {error}", file.path))
}

pub(super) fn create_owned_parents(root: &Path, parent: &Path) -> Result<(), String> {
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| "A staged repair path escaped its owned directory.".to_string())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("Invalid component in staged repair path.".into());
        };
        current.push(name);
        match fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&current)
                    .map_err(|error| format!("Could not inspect staged directory: {error}"))?;
                if is_link_or_reparse(&metadata) || !metadata.is_dir() {
                    return Err(format!(
                        "Unsafe staged repair parent: {}",
                        current.display()
                    ));
                }
            }
            Err(error) => {
                return Err(format!("Could not create staged repair parent: {error}"));
            }
        }
    }
    Ok(())
}
