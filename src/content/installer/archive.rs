use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::content::INSTALL_RECEIPT_FILE as RECEIPT_NAME;
use crate::content::manifest::ArchiveLayout;
use crate::content::manifest::BaseArchive;
use crate::content::manifest::BasePackage;
use crate::content::manifest::InstallFile;
use crate::content::manifest::validate_relative_path;
use crate::content::worker::InstallShared;

use super::IO_BUFFER_BYTES;
use super::paths::{create_stage_parents, joined_content_path};
use super::space::inventory_bytes;

pub(super) fn extract_archive(
    shared: &Arc<InstallShared>,
    archive_path: &Path,
    archive_spec: &BaseArchive,
    package: &BasePackage,
    stage: &Path,
) -> Result<(), String> {
    let expected = archive_file_map(archive_spec);
    let ignored = archive_ignored_file_map(archive_spec, package)?;
    validate_archive_inventory(archive_path, archive_spec, &expected, &ignored)?;
    let file = File::open(archive_path)
        .map_err(|error| format!("Could not open the verified base archive: {error}"))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("Could not read the base archive: {error}"))?;
    let mut extracted = 0_u64;
    for index in 0..archive.len() {
        if shared.wait_if_paused() {
            return Err("Installation was cancelled.".into());
        }
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Could not read a base archive entry: {error}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_owned();
        if archive_spec.layout == ArchiveLayout::FinalFantasyXivWrapper
            && name.starts_with("__MACOSX/")
        {
            continue;
        }
        let relative = archive_payload_name(archive_spec.layout, &name)?;
        if let Some(spec) = ignored.get(&relative.to_ascii_lowercase()) {
            let digest = copy_and_hash(
                shared,
                &mut entry,
                &mut std::io::sink(),
                &spec.path,
                spec.length,
            )?;
            if !digest.eq_ignore_ascii_case(&spec.sha256) {
                return Err(format!(
                    "Excluded source file failed integrity verification: {}",
                    spec.path
                ));
            }
            continue;
        }
        let spec = expected
            .get(&relative.to_ascii_lowercase())
            .ok_or_else(|| format!("Unexpected base archive entry: {name}"))?;
        let path = joined_content_path(stage, &spec.path);
        create_stage_parents(stage, path.parent().expect("a file path has a parent"))?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("Could not create staged file {}: {error}", spec.path))?;
        let digest = copy_and_hash(shared, &mut entry, &mut output, &spec.path, spec.length)?;
        if !digest.eq_ignore_ascii_case(&spec.sha256) {
            return Err(format!(
                "Base archive file failed integrity verification: {}",
                spec.path
            ));
        }
        extracted = extracted
            .checked_add(spec.length)
            .ok_or_else(|| "Extracted base size overflow.".to_string())?;
        shared
            .file_idx
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
    let expected_bytes = inventory_bytes(&archive_spec.files)?;
    if extracted != expected_bytes {
        return Err("Extracted base archive size differs from its manifest.".into());
    }
    for directory in &archive_spec.empty_directories {
        let path = joined_content_path(stage, directory);
        create_stage_parents(stage, &path)?;
    }
    Ok(())
}

fn archive_payload_name(layout: ArchiveLayout, name: &str) -> Result<&str, String> {
    match layout {
        ArchiveLayout::Flat => Ok(name),
        ArchiveLayout::FinalFantasyXivWrapper => name
            .strip_prefix("FINAL FANTASY XIV/")
            .filter(|relative| !relative.is_empty())
            .ok_or_else(|| format!("Unexpected wrapped archive entry: {name}")),
    }
}

fn archive_ignored_file_map<'a>(
    archive: &'a BaseArchive,
    package: &'a BasePackage,
) -> Result<HashMap<String, &'a InstallFile>, String> {
    let mut ignored = HashMap::new();
    for file in &archive.excluded_files {
        let _ = ignored.insert(file.path.to_ascii_lowercase(), file);
    }
    if archive.layout == ArchiveLayout::FinalFantasyXivWrapper {
        for name in ["boot.ver", "game.ver"] {
            let file = package
                .final_files
                .iter()
                .find(|file| file.path.eq_ignore_ascii_case(name))
                .ok_or_else(|| format!("Missing managed version identity: {name}"))?;
            let _ = ignored.insert(name.to_owned(), file);
        }
    }
    Ok(ignored)
}

fn archive_file_map(archive: &BaseArchive) -> HashMap<String, &InstallFile> {
    let mut files = HashMap::new();
    for file in &archive.files {
        let _ = files.insert(file.path.to_ascii_lowercase(), file);
    }
    files
}

/// Lowercased names collected while scanning an archive's entries.
#[derive(Default)]
struct ArchiveScan {
    seen: HashSet<String>,
    found_files: HashSet<String>,
    found_ignored: HashSet<String>,
    found_directories: HashSet<String>,
    metadata_files: Vec<String>,
    metadata_directories: HashSet<String>,
}

fn validate_archive_inventory(
    path: &Path,
    archive_spec: &BaseArchive,
    expected: &HashMap<String, &InstallFile>,
    ignored: &HashMap<String, &InstallFile>,
) -> Result<(), String> {
    let file = File::open(path).map_err(|error| format!("Could not open base archive: {error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("Could not read base archive: {error}"))?;
    let mut expected_dirs = expected_directories(
        expected
            .values()
            .chain(ignored.values())
            .map(|file| file.path.as_str()),
    );
    for directory in &archive_spec.empty_directories {
        expected_dirs.insert(directory.to_ascii_lowercase());
        expected_dirs.extend(expected_directories(std::iter::once(directory.as_str())));
    }
    let scan = scan_archive_entries(
        &mut archive,
        archive_spec,
        expected,
        ignored,
        &expected_dirs,
    )?;
    check_inventory_complete(&scan, archive_spec, expected, ignored)?;
    if archive_spec.layout == ArchiveLayout::FinalFantasyXivWrapper {
        check_apple_metadata(&scan, archive_spec)?;
    }
    Ok(())
}

fn scan_archive_entries(
    archive: &mut ZipArchive<File>,
    archive_spec: &BaseArchive,
    expected: &HashMap<String, &InstallFile>,
    ignored: &HashMap<String, &InstallFile>,
    expected_dirs: &HashSet<String>,
) -> Result<ArchiveScan, String> {
    let mut scan = ArchiveScan::default();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("Could not inspect a base archive entry: {error}"))?;
        let is_dir = entry.is_dir();
        let raw_name = entry.name();
        let name = if is_dir {
            raw_name
                .strip_suffix('/')
                .ok_or_else(|| format!("Invalid directory entry in base archive: {raw_name}"))?
        } else {
            raw_name
        };
        validate_relative_path(name)?;
        if name.eq_ignore_ascii_case(RECEIPT_NAME) {
            return Err("Base archive contains a reserved launcher receipt path.".into());
        }
        validate_entry_mode(entry.unix_mode(), is_dir, name)?;
        let folded = name.to_ascii_lowercase();
        if !scan.seen.insert(folded.clone()) {
            return Err(format!("Duplicate normalized archive destination: {name}"));
        }
        if archive_spec.layout == ArchiveLayout::FinalFantasyXivWrapper
            && (name == "__MACOSX" || name.starts_with("__MACOSX/"))
        {
            if is_dir {
                scan.metadata_directories.insert(folded);
            } else {
                scan.metadata_files.push(name.to_owned());
            }
            continue;
        }
        if archive_spec.layout == ArchiveLayout::FinalFantasyXivWrapper
            && name == "FINAL FANTASY XIV"
            && is_dir
        {
            scan.found_directories.insert(String::new());
            continue;
        }
        let relative = archive_payload_name(archive_spec.layout, name)?;
        validate_relative_path(relative)?;
        let relative_folded = relative.to_ascii_lowercase();
        if is_dir {
            if !expected_dirs.contains(&relative_folded) {
                return Err(format!("Unexpected directory in base archive: {name}"));
            }
            scan.found_directories.insert(relative_folded);
            continue;
        }
        let spec = expected
            .get(&relative_folded)
            .or_else(|| ignored.get(&relative_folded))
            .ok_or_else(|| format!("Unexpected file in base archive: {name}"))?;
        if entry.size() != spec.length {
            return Err(format!("Declared length mismatch for base file: {name}"));
        }
        if expected.contains_key(&relative_folded) {
            scan.found_files.insert(relative_folded);
        } else {
            scan.found_ignored.insert(relative_folded);
        }
    }
    Ok(scan)
}

fn check_inventory_complete(
    scan: &ArchiveScan,
    archive_spec: &BaseArchive,
    expected: &HashMap<String, &InstallFile>,
    ignored: &HashMap<String, &InstallFile>,
) -> Result<(), String> {
    if scan.found_files.len() != expected.len() {
        let missing = expected
            .keys()
            .find(|name| !scan.found_files.contains(*name))
            .cloned()
            .unwrap_or_else(|| "unknown file".into());
        return Err(format!("Base archive is missing inventory file: {missing}"));
    }
    if scan.found_ignored.len() != ignored.len() {
        let missing = ignored
            .keys()
            .find(|name| !scan.found_ignored.contains(*name))
            .cloned()
            .unwrap_or_else(|| "unknown file".into());
        return Err(format!(
            "Base archive is missing excluded or managed source file: {missing}"
        ));
    }
    for directory in &archive_spec.empty_directories {
        if !scan
            .found_directories
            .contains(&directory.to_ascii_lowercase())
        {
            return Err(format!(
                "Base archive is missing empty directory: {directory}"
            ));
        }
    }
    Ok(())
}

fn check_apple_metadata(scan: &ArchiveScan, archive_spec: &BaseArchive) -> Result<(), String> {
    if scan.metadata_files.len() as u64 != archive_spec.apple_metadata_files {
        return Err("Apple metadata entry count differs from the manifest.".into());
    }
    let expected_metadata_dirs =
        expected_directories(scan.metadata_files.iter().map(String::as_str));
    for metadata in &scan.metadata_files {
        let counterpart = apple_metadata_counterpart(metadata)?;
        if !scan.seen.contains(&counterpart.to_ascii_lowercase()) {
            return Err(format!(
                "Apple metadata has no source counterpart: {metadata}"
            ));
        }
    }
    if !scan.metadata_directories.is_subset(&expected_metadata_dirs) {
        return Err("Apple metadata contains an unexpected directory.".into());
    }
    Ok(())
}

fn apple_metadata_counterpart(name: &str) -> Result<String, String> {
    let relative = name
        .strip_prefix("__MACOSX/")
        .ok_or_else(|| format!("Unexpected Apple metadata path: {name}"))?;
    let (parent, leaf) = relative.rsplit_once('/').unwrap_or(("", relative));
    let counterpart_leaf = leaf
        .strip_prefix("._")
        .filter(|leaf| !leaf.is_empty())
        .ok_or_else(|| format!("Unexpected Apple metadata path: {name}"))?;
    let counterpart = if parent.is_empty() {
        counterpart_leaf.to_owned()
    } else {
        format!("{parent}/{counterpart_leaf}")
    };
    if counterpart != "FINAL FANTASY XIV" && !counterpart.starts_with("FINAL FANTASY XIV/") {
        return Err(format!(
            "Apple metadata is outside the selected source: {name}"
        ));
    }
    Ok(counterpart)
}

fn validate_entry_mode(mode: Option<u32>, is_dir: bool, name: &str) -> Result<(), String> {
    if let Some(mode) = mode {
        let kind = mode & 0o170000;
        if mode & 0o7000 != 0
            || (is_dir && kind != 0 && kind != 0o040000)
            || (!is_dir && kind != 0 && kind != 0o100000)
        {
            return Err(format!(
                "Links or special modes are not allowed in base archives: {name}"
            ));
        }
    }
    Ok(())
}

pub(super) fn expected_directories<'a>(paths: impl Iterator<Item = &'a str>) -> HashSet<String> {
    let mut directories = HashSet::new();
    for path in paths {
        for (index, _) in path.match_indices('/') {
            directories.insert(path[..index].to_ascii_lowercase());
        }
    }
    directories
}

pub(super) fn copy_and_hash(
    shared: &InstallShared,
    reader: &mut impl Read,
    writer: &mut impl Write,
    label: &str,
    expected_length: u64,
) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; IO_BUFFER_BYTES];
    while total < expected_length {
        if shared.wait_if_paused() {
            return Err("Installation was cancelled.".into());
        }
        let remaining = expected_length - total;
        let limit = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = reader
            .read(&mut buffer[..limit])
            .map_err(|error| format!("Could not read staged file {label}: {error}"))?;
        if read == 0 {
            return Err(format!(
                "Staged file ended before its manifest length: {label}"
            ));
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .map_err(|error| format!("Could not write staged file {label}: {error}"))?;
    }
    let mut excess = [0_u8; 1];
    if reader
        .read(&mut excess)
        .map_err(|error| format!("Could not check staged file length {label}: {error}"))?
        != 0
    {
        return Err(format!("Staged file exceeded its manifest length: {label}"));
    }
    writer
        .flush()
        .map_err(|error| format!("Could not flush staged file {label}: {error}"))?;
    Ok(format!("{:x}", hasher.finalize()))
}
