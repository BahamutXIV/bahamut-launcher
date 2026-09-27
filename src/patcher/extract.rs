// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use super::downloader::IO_CHUNK;
use super::manifest::{PATCH_MANIFEST, PatchEntry, leaf_of};
use super::summarize_names;
use super::worker::PatcherShared;

/// Maximum directory depth searched for the patch zip.
const MAX_SEARCH_DEPTH: u32 = 4;

/// Where [`find_patch_payload`] located the patch data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchPayload {
    /// A zip archive that must be extracted before application.
    Zip(PathBuf),
    /// A directory containing all manifest patches as loose `*.patch` files.
    PatchDir(PathBuf),
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("patch archive is not a file: {0}")]
    NotAZip(PathBuf),

    #[error("i/o error while {context}: {source}")]
    Io {
        context: &'static str,
        #[source]
        source: io::Error,
    },

    #[error("could not read the patch archive: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("{missing_count} patch file(s) not found in the archive: {summary}")]
    MissingPatches {
        missing_count: usize,
        summary: String,
    },

    #[error(
        "patch entry {name} did not match its expected size of {expected} bytes (observed {actual})"
    )]
    WrongSize {
        name: String,
        expected: u64,
        actual: u64,
    },

    #[error("extraction was cancelled")]
    Cancelled,
}

impl ExtractError {
    fn io(context: &'static str, source: io::Error) -> Self {
        Self::Io { context, source }
    }
}

/// Find loose manifest patches or a patch zip under `storage_dir`; return `None` when neither shape exists.
pub fn find_patch_payload(storage_dir: &Path) -> Option<PatchPayload> {
    if !storage_dir.is_dir() {
        return None;
    }
    if has_every_manifest_patch(storage_dir) {
        return Some(PatchPayload::PatchDir(storage_dir.to_path_buf()));
    }
    find_patch_zip(storage_dir).map(PatchPayload::Zip)
}

/// Check every manifest leaf within the bounded search, skipping hidden directories.
fn has_every_manifest_patch(dir: &Path) -> bool {
    let mut missing: HashSet<&str> = PATCH_MANIFEST.iter().map(|e| leaf_of(e.path)).collect();
    let mut pending = vec![(dir.to_path_buf(), 0u32)];
    while let Some((current, depth)) = pending.pop() {
        if depth > MAX_SEARCH_DEPTH {
            continue;
        }
        let Ok(listing) = fs::read_dir(&current) else {
            continue;
        };
        for item in listing.flatten() {
            let path = item.path();
            if path.is_dir() {
                if !is_hidden(&path) {
                    pending.push((path, depth + 1));
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("patch")
                && let Some(leaf) = path.file_name().and_then(|n| n.to_str())
            {
                missing.remove(leaf);
            }
        }
    }
    missing.is_empty()
}

pub(crate) const PATCH_ZIP_NAME: &str = "FFXIV_1.23b_Patches.zip";
const LEGACY_PATCH_ZIP_NAME: &str = "ffxiv_patches.zip";

/// Prefer the user-facing canonical archive name while retaining the hosted legacy payload.
fn find_patch_zip(dir: &Path) -> Option<PathBuf> {
    let zips = collect_zip_files(dir, 0);
    zips.iter()
        .find(|path| has_zip_name(path, PATCH_ZIP_NAME))
        .or_else(|| {
            zips.iter()
                .find(|path| has_zip_name(path, LEGACY_PATCH_ZIP_NAME))
        })
        .cloned()
}

fn has_zip_name(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

/// Collect `*.zip` files within [`MAX_SEARCH_DEPTH`], skipping hidden directories.
fn collect_zip_files(dir: &Path, depth: u32) -> Vec<PathBuf> {
    if depth > MAX_SEARCH_DEPTH {
        return Vec::new();
    }
    let Ok(listing) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for item in listing.flatten() {
        let path = item.path();
        if path.is_dir() {
            if is_hidden(&path) {
                continue;
            }
            found.extend(collect_zip_files(&path, depth + 1));
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
        {
            found.push(path);
        }
    }
    found
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

/// Extract all manifest patches into fresh staging, updating progress and honoring cancellation.
pub fn extract_manifest_patches(
    zip_path: &Path,
    staging_dir: &Path,
    shared: &PatcherShared,
) -> Result<(), ExtractError> {
    extract_patches(zip_path, staging_dir, shared, PATCH_MANIFEST)
}

fn extract_patches(
    zip_path: &Path,
    staging_dir: &Path,
    shared: &PatcherShared,
    patch_manifest: &[PatchEntry],
) -> Result<(), ExtractError> {
    if !zip_path.is_file() {
        return Err(ExtractError::NotAZip(zip_path.to_path_buf()));
    }

    match fs::remove_dir_all(staging_dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(ExtractError::io("clearing the staging directory", e)),
    }
    fs::create_dir_all(staging_dir)
        .map_err(|e| ExtractError::io("creating the staging directory", e))?;

    let file =
        File::open(zip_path).map_err(|e| ExtractError::io("opening the patch archive", e))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))?;

    let mut index_by_leaf: HashMap<String, usize> = HashMap::with_capacity(archive.len());
    for (i, name) in archive.file_names().enumerate() {
        if let Some(leaf) = Path::new(name).file_name().and_then(|n| n.to_str()) {
            index_by_leaf.entry(leaf.to_owned()).or_insert(i);
        }
    }

    let mut missing = Vec::new();
    for entry in patch_manifest {
        let leaf = leaf_of(entry.path);
        if !index_by_leaf.contains_key(leaf) {
            missing.push(leaf.to_owned());
        }
    }
    if !missing.is_empty() {
        return Err(ExtractError::MissingPatches {
            missing_count: missing.len(),
            summary: summarize_names(&missing),
        });
    }

    for (idx, entry) in patch_manifest.iter().enumerate() {
        if shared.is_cancel_requested() {
            return Err(ExtractError::Cancelled);
        }
        shared.download_idx.store(idx, Ordering::Release);
        shared.download.bytes_downloaded.store(0, Ordering::SeqCst);

        let leaf = leaf_of(entry.path);
        let zip_index = index_by_leaf[leaf];
        let dst_path = staging_dir.join(leaf);
        extract_one(&mut archive, zip_index, &dst_path, entry.size, shared)?;

        shared
            .previous_completed_bytes
            .fetch_add(entry.size, Ordering::Release);
    }

    Ok(())
}

/// Stream one zip entry with progress updates and cancellation checks.
fn extract_one<R: Read + io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    zip_index: usize,
    dst_path: &Path,
    expected_size: u64,
    shared: &PatcherShared,
) -> Result<(), ExtractError> {
    let mut entry = archive.by_index(zip_index)?;
    let name = entry.name().to_owned();
    if entry.size() != expected_size {
        return Err(ExtractError::WrongSize {
            name,
            expected: expected_size,
            actual: entry.size(),
        });
    }
    let mut sink =
        File::create(dst_path).map_err(|e| ExtractError::io("creating a staged patch file", e))?;

    let result = copy_entry_bounded(&mut entry, &mut sink, &name, expected_size, shared);
    if result.is_err() {
        drop(sink);
        let _ = fs::remove_file(dst_path);
    }
    result
}

fn copy_entry_bounded<R: Read, W: Write>(
    entry: &mut R,
    sink: &mut W,
    name: &str,
    expected_size: u64,
    shared: &PatcherShared,
) -> Result<(), ExtractError> {
    let mut bytes_written = 0u64;
    let mut buf = [0u8; IO_CHUNK];
    loop {
        if shared.is_cancel_requested() {
            return Err(ExtractError::Cancelled);
        }

        if bytes_written == expected_size {
            let mut extra = [0u8; 1];
            let read = entry
                .read(&mut extra)
                .map_err(|e| ExtractError::io("reading a patch entry from the archive", e))?;
            if read == 0 {
                return Ok(());
            }
            return Err(ExtractError::WrongSize {
                name: name.to_owned(),
                expected: expected_size,
                actual: expected_size.saturating_add(1),
            });
        }

        let remaining = expected_size - bytes_written;
        let limit = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let read = entry
            .read(&mut buf[..limit])
            .map_err(|e| ExtractError::io("reading a patch entry from the archive", e))?;
        if read == 0 {
            return Err(ExtractError::WrongSize {
                name: name.to_owned(),
                expected: expected_size,
                actual: bytes_written,
            });
        }
        sink.write_all(&buf[..read])
            .map_err(|e| ExtractError::io("writing a staged patch file", e))?;
        bytes_written += read as u64;
        shared
            .download
            .bytes_downloaded
            .fetch_add(read as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    const TEST_PATCHES: &[PatchEntry] = &[
        PatchEntry {
            path: "test-a/patch/a.patch",
            size: 1,
            crc32: 0,
        },
        PatchEntry {
            path: "test-b/patch/b.patch",
            size: 2,
            crc32: 0,
        },
    ];

    fn build_fixture_zip(dst: &Path, entries: &[PatchEntry], skip: &[usize]) {
        let file = File::create(dst).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (i, entry) in entries.iter().enumerate() {
            if skip.contains(&i) {
                continue;
            }
            zip.start_file(entry.path, options).unwrap();
            zip.write_all(&vec![b'x'; entry.size as usize]).unwrap();
        }
        zip.start_file("readme.txt", options).unwrap();
        zip.write_all(b"not a patch").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn full_extraction_stages_every_manifest_leaf() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join(PATCH_ZIP_NAME);
        build_fixture_zip(&zip_path, TEST_PATCHES, &[]);
        let staging = tmp.path().join("staging");
        let shared = PatcherShared::new();

        extract_patches(&zip_path, &staging, &shared, TEST_PATCHES).unwrap();

        for entry in TEST_PATCHES {
            assert!(
                staging.join(leaf_of(entry.path)).exists(),
                "missing {} after extraction",
                leaf_of(entry.path)
            );
        }
    }

    #[test]
    fn a_zip_missing_leaves_reports_the_count() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join(PATCH_ZIP_NAME);
        build_fixture_zip(&zip_path, TEST_PATCHES, &[0]);
        let staging = tmp.path().join("staging");
        let shared = PatcherShared::new();

        match extract_patches(&zip_path, &staging, &shared, TEST_PATCHES).unwrap_err() {
            ExtractError::MissingPatches { missing_count, .. } => {
                assert_eq!(missing_count, 1);
            }
            other => panic!("expected MissingPatches, got {other:?}"),
        }
    }

    #[test]
    fn a_preset_cancel_flag_stops_extraction() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join(PATCH_ZIP_NAME);
        build_fixture_zip(&zip_path, TEST_PATCHES, &[]);
        let staging = tmp.path().join("staging");
        let shared = PatcherShared::new();
        shared.request_cancel();

        assert!(matches!(
            extract_patches(&zip_path, &staging, &shared, TEST_PATCHES).unwrap_err(),
            ExtractError::Cancelled
        ));
    }

    #[test]
    fn zip_metadata_size_must_match_the_patch_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join(PATCH_ZIP_NAME);
        build_fixture_zip(&zip_path, TEST_PATCHES, &[]);
        let staging = tmp.path().join("staging");
        let shared = PatcherShared::new();
        let mut wrong_size = TEST_PATCHES.to_vec();
        wrong_size[0].size += 1;

        assert!(matches!(
            extract_patches(&zip_path, &staging, &shared, &wrong_size).unwrap_err(),
            ExtractError::WrongSize {
                expected: 2,
                actual: 1,
                ..
            }
        ));
        assert!(!staging.join("a.patch").exists());
    }

    #[test]
    fn zip_stream_rejects_output_beyond_the_manifest_length() {
        let mut stream = Cursor::new(b"four".as_slice());
        let mut output = Vec::new();

        assert!(matches!(
            copy_entry_bounded(
                &mut stream,
                &mut output,
                "fixture.patch",
                3,
                &PatcherShared::new(),
            ),
            Err(ExtractError::WrongSize {
                expected: 3,
                actual: 4,
                ..
            })
        ));
        assert_eq!(output, b"fou");
    }

    #[test]
    fn zip_stream_rejects_output_shorter_than_the_manifest_length() {
        let mut stream = Cursor::new(b"three".as_slice());
        let mut output = Vec::new();

        assert!(matches!(
            copy_entry_bounded(
                &mut stream,
                &mut output,
                "fixture.patch",
                6,
                &PatcherShared::new(),
            ),
            Err(ExtractError::WrongSize {
                expected: 6,
                actual: 5,
                ..
            })
        ));
        assert_eq!(output, b"three");
    }

    #[test]
    fn find_patch_payload_prefers_a_dir_with_every_loose_patch() {
        let tmp = tempfile::tempdir().unwrap();
        for entry in PATCH_MANIFEST {
            fs::write(tmp.path().join(leaf_of(entry.path)), b"x").unwrap();
        }
        assert_eq!(
            find_patch_payload(tmp.path()),
            Some(PatchPayload::PatchDir(tmp.path().to_path_buf()))
        );
    }

    #[test]
    fn find_patch_payload_finds_the_named_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join(PATCH_ZIP_NAME);
        build_fixture_zip(&zip_path, &[], &[]);
        assert_eq!(
            find_patch_payload(tmp.path()),
            Some(PatchPayload::Zip(zip_path))
        );
    }

    #[test]
    fn find_patch_payload_accepts_the_hosted_legacy_archive_name() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join(LEGACY_PATCH_ZIP_NAME);
        build_fixture_zip(&zip_path, &[], &[]);
        assert_eq!(
            find_patch_payload(tmp.path()),
            Some(PatchPayload::Zip(zip_path))
        );
    }

    #[test]
    fn find_patch_payload_rejects_an_arbitrarily_named_zip() {
        let tmp = tempfile::tempdir().unwrap();
        build_fixture_zip(&tmp.path().join("patches-mirror.zip"), &[], &[]);
        assert_eq!(find_patch_payload(tmp.path()), None);
    }

    #[test]
    fn find_patch_payload_is_none_for_an_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(find_patch_payload(tmp.path()), None);
    }
}
