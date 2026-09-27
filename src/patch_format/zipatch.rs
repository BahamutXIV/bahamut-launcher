// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

//! Reader for the FFXIV 1.x "ZIPATCH" client patch format.
//!
//! ZiPatch is a publicly documented container: a 12-byte signature
//! (`91 5A 49 50 41 54 43 48 0D 0A 1A 0A`) followed by a sequence of
//! length-framed chunks, each laid out as:
//!
//! ```text
//! size : u32 big-endian   length of the body
//! tag  : 4 ASCII bytes    chunk type
//! body : size bytes       chunk payload
//! crc  : u32 big-endian   CRC32 of tag+body, written by the client and
//!                         not re-verified here
//! ```
//!
//! Only the chunks needed to materialise a 1.x install are interpreted:
//!
//! | tag  | meaning                                                     |
//! |------|-------------------------------------------------------------|
//! | FHDR | format header / version (opaque, skipped)                   |
//! | APLY | apply-stage header metadata (opaque, skipped)               |
//! | APFS | filesystem metadata seen in some patches (opaque, skipped)  |
//! | ADIR | add a directory                                             |
//! | DELD | delete a directory                                          |
//! | ETRY | a file entry: a path plus one or more body records          |
//!
//! An `ETRY` body is `pathLen:u32 BE, path, itemCount:u32 BE, items`.
//! Each item is `hashMode:u32 LE` (0x41/0x44/0x4D), a 20-byte source
//! hash, a 20-byte target hash, `compression:u32 LE` (0x4E none / 0x5A
//! zlib), then `bodySize`, `previousSize`, `newSize` as u32 BE. Only the
//! final item of an entry carries body bytes.
//!
//! The ZiPatch container is a documented format; this is an independent
//! implementation of it.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use flate2::read::ZlibDecoder;

/// Shared 12-byte FFXIV 1.x ZiPatch signature: `0x91`, `ZIPATCH`, and `\r\n\x1a\n`.
const SIGNATURE: [u8; 12] = [
    0x91, b'Z', b'I', b'P', b'A', b'T', b'C', b'H', 0x0D, 0x0A, 0x1A, 0x0A,
];

/// Retry output opens six times at one-second intervals for the Windows shell icon-cache hold.
const OPEN_ATTEMPTS: u32 = 6;
const OPEN_BACKOFF: Duration = Duration::from_secs(1);

/// Non-fatal apply warnings returned for display without aborting the patch.
#[derive(Debug, Default)]
pub struct PatchApplyResult {
    pub messages: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PatchApplyError {
    #[error("{0} is not a ZIPATCH file (bad signature)")]
    BadSignature(PathBuf),

    #[error("unhandled ZIPATCH chunk tag {0:?}")]
    UnhandledChunk(String),

    #[error("ETRY entry uses an unknown hash mode 0x{0:02X}")]
    UnknownHashMode(u32),

    #[error("ETRY entry uses an unknown compression mode 0x{0:02X}")]
    UnknownCompressionMode(u32),

    #[error("ETRY entry carries body data before its final item")]
    UnexpectedItemData,

    #[error("ETRY path length {len} bytes exceeds the {max}-byte sanity cap")]
    PathTooLong { len: usize, max: usize },

    #[error("ZIPATCH path is not safely relative to the game directory: {0:?}")]
    UnsafePath(String),

    #[error("could not open {path} for writing after {attempts} attempts: {source}")]
    OpenFailed {
        path: PathBuf,
        attempts: u32,
        #[source]
        source: io::Error,
    },

    #[error(transparent)]
    Io(#[from] io::Error),
}

type Result<T> = std::result::Result<T, PatchApplyError>;

/// Apply `patch_path` under `game_root`, collecting warnings and short-circuiting on fatal errors.
pub fn apply_patch_file(patch_path: &Path, game_root: &Path) -> Result<PatchApplyResult> {
    let mut reader = BufReader::new(File::open(patch_path)?);
    apply_patch(&mut reader, patch_path, game_root)
}

fn apply_patch<R: Read>(
    reader: &mut R,
    patch_path: &Path,
    game_root: &Path,
) -> Result<PatchApplyResult> {
    let mut signature = [0u8; 12];
    reader.read_exact(&mut signature)?;
    if signature != SIGNATURE {
        return Err(PatchApplyError::BadSignature(patch_path.to_path_buf()));
    }

    let mut applier = ZiPatchApplier {
        game_root: game_root.to_path_buf(),
        report: PatchApplyResult::default(),
    };
    applier.run(reader)?;
    Ok(applier.report)
}

struct ZiPatchApplier {
    game_root: PathBuf,
    report: PatchApplyResult,
}

impl ZiPatchApplier {
    fn run<R: Read>(&mut self, reader: &mut R) -> Result<()> {
        while let Some(size) = read_optional_u32_be(reader)? {
            let mut tag = [0u8; 4];
            reader.read_exact(&mut tag)?;

            let mut body = reader.by_ref().take(u64::from(size));
            match &tag {
                b"FHDR" | b"APLY" | b"APFS" => {}
                b"ADIR" => self.add_directory(&mut body)?,
                b"DELD" => self.delete_directory(&mut body)?,
                b"ETRY" => self.apply_entry(&mut body)?,
                _ => {
                    return Err(PatchApplyError::UnhandledChunk(
                        String::from_utf8_lossy(&tag).into_owned(),
                    ));
                }
            }

            io::copy(&mut body, &mut io::sink())?;
            // Consume the client-written CRC32 without verifying it; truncation still errors.
            let mut _crc = [0u8; 4];
            reader.read_exact(&mut _crc)?;
        }
        Ok(())
    }

    fn add_directory<R: Read>(&mut self, body: &mut R) -> Result<()> {
        let target = self.resolve(&read_length_prefixed_path(body)?)?;
        if target.exists() {
            self.warn(format!(
                "Directory '{}' already exists; skipping the add request.",
                target.display()
            ));
        } else {
            fs::create_dir_all(&target)?;
        }
        Ok(())
    }

    fn delete_directory<R: Read>(&mut self, body: &mut R) -> Result<()> {
        let target = self.resolve(&read_length_prefixed_path(body)?)?;
        if target.exists() {
            fs::remove_dir_all(&target)?;
        } else {
            self.warn(format!(
                "Directory '{}' not found; skipping the delete request.",
                target.display()
            ));
        }
        Ok(())
    }

    fn apply_entry<R: Read>(&mut self, body: &mut R) -> Result<()> {
        let target = self.resolve(&read_length_prefixed_path(body)?)?;

        if let Some(parent) = target.parent()
            && !parent.exists()
        {
            self.warn(format!(
                "Parent directory '{}' is missing; creating it.",
                parent.display()
            ));
            fs::create_dir_all(parent)?;
        }
        if !target.exists() {
            self.warn(format!("File '{}' is new; creating it.", target.display()));
        }

        let item_count = read_u32_be(body)?;
        for index in 0..item_count {
            let hash_mode = read_u32_le(body)?;
            if !matches!(hash_mode, 0x41 | 0x44 | 0x4D) {
                return Err(PatchApplyError::UnknownHashMode(hash_mode));
            }
            let mut hashes = [0u8; 40];
            body.read_exact(&mut hashes)?;

            let compression = read_u32_le(body)?;
            if !matches!(compression, 0x4E | 0x5A) {
                return Err(PatchApplyError::UnknownCompressionMode(compression));
            }
            let body_size = read_u32_be(body)?;
            read_u32_be(body)?;
            read_u32_be(body)?;

            let is_final = index + 1 == item_count;
            if !is_final && body_size != 0 {
                return Err(PatchApplyError::UnexpectedItemData);
            }
            if body_size == 0 {
                continue;
            }

            let mut writer = BufWriter::new(self.create_output(&target)?);
            let mut source = body.by_ref().take(u64::from(body_size));
            match compression {
                0x4E => {
                    io::copy(&mut source, &mut writer)?;
                }
                0x5A => {
                    io::copy(&mut ZlibDecoder::new(&mut source), &mut writer)?;
                }
                _ => unreachable!("compression mode validated above"),
            }
            writer.flush()?;
        }
        Ok(())
    }

    fn create_output(&self, path: &Path) -> Result<File> {
        let mut attempt = 1;
        loop {
            match File::create(path) {
                Ok(file) => return Ok(file),
                Err(_) if attempt < OPEN_ATTEMPTS => {
                    attempt += 1;
                    sleep(OPEN_BACKOFF);
                }
                Err(source) => {
                    return Err(PatchApplyError::OpenFailed {
                        path: path.to_path_buf(),
                        attempts: OPEN_ATTEMPTS,
                        source,
                    });
                }
            }
        }
    }

    /// Resolve a Windows-separator patch-relative path without following an ancestor outside the game root.
    fn resolve(&self, relative: &str) -> Result<PathBuf> {
        let normalized = relative.replace('\\', std::path::MAIN_SEPARATOR_STR);
        let mut clean = PathBuf::new();
        for component in Path::new(&normalized).components() {
            match component {
                Component::Normal(part) => clean.push(part),
                Component::CurDir => {}
                Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                    return Err(PatchApplyError::UnsafePath(relative.to_owned()));
                }
            }
        }
        if clean.as_os_str().is_empty() {
            return Err(PatchApplyError::UnsafePath(relative.to_owned()));
        }

        let root = self.game_root.canonicalize()?;
        let target = self.game_root.join(clean);
        let existing_ancestor = target
            .ancestors()
            .find(|path| fs::symlink_metadata(path).is_ok())
            .ok_or_else(|| PatchApplyError::UnsafePath(relative.to_owned()))?;
        if !existing_ancestor.canonicalize()?.starts_with(&root) {
            return Err(PatchApplyError::UnsafePath(relative.to_owned()));
        }
        Ok(target)
    }

    fn warn(&mut self, message: String) {
        self.report.messages.push(message);
    }
}

/// Cap untrusted patch-path reads at 64 KiB to prevent oversized allocations.
const MAX_PATH_BYTES: usize = 64 * 1024;

fn read_length_prefixed_path<R: Read>(reader: &mut R) -> Result<String> {
    let len = read_u32_be(reader)? as usize;
    if len > MAX_PATH_BYTES {
        return Err(PatchApplyError::PathTooLong {
            len,
            max: MAX_PATH_BYTES,
        });
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn read_u32_be<R: Read>(reader: &mut R) -> Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_be_bytes(buf))
}

fn read_u32_le<R: Read>(reader: &mut R) -> Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_optional_u32_be<R: Read>(reader: &mut R) -> Result<Option<u32>> {
    let mut buf = [0u8; 4];
    loop {
        match reader.read(&mut buf[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    reader.read_exact(&mut buf[1..])?;
    Ok(Some(u32::from_be_bytes(buf)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Literal format fixtures stay independent of the chunk writer below.
    const EMPTY_PATCH: &[u8] = b"\x91ZIPATCH\r\n\x1a\n";
    const HEADER_CHUNK: &[u8] = b"\0\0\0\x02FHDR\x01\x02\0\0\0\0";

    #[test]
    fn length_boundary_accepts_only_empty_or_complete_chunks() {
        let tmp = tempfile::tempdir().unwrap();
        for previous in [&[][..], HEADER_CHUNK] {
            let complete = [EMPTY_PATCH, previous].concat();
            let patch = write_temp(tmp.path(), "boundary.patch", &complete);
            apply_patch_file(&patch, tmp.path()).unwrap();
            for partial in 1..=3 {
                let bytes = [complete.as_slice(), &HEADER_CHUNK[..partial]].concat();
                fs::write(&patch, bytes).unwrap();
                assert!(matches!(
                    apply_patch_file(&patch, tmp.path()),
                    Err(PatchApplyError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof
                ));
            }
        }
    }

    struct ShortReader<'a> {
        bytes: &'a [u8],
        offset: usize,
        fail_at: Option<usize>,
        interrupt_at: Option<usize>,
    }

    impl Read for ShortReader<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.interrupt_at == Some(self.offset) {
                self.interrupt_at = None;
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.fail_at == Some(self.offset) {
                return Err(io::ErrorKind::ConnectionReset.into());
            }
            let count = out.len().min(1).min(self.bytes.len() - self.offset);
            out[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
            self.offset += count;
            Ok(count)
        }
    }

    #[test]
    fn parser_handles_short_reads_and_interrupted_chunk_lengths() {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = [EMPTY_PATCH, HEADER_CHUNK].concat();
        for interrupt_at in [12, 13, 14, 15] {
            let mut reader = ShortReader {
                bytes: &bytes,
                offset: 0,
                fail_at: None,
                interrupt_at: Some(interrupt_at),
            };
            apply_patch(&mut reader, Path::new("fixture.patch"), tmp.path()).unwrap();
        }
    }

    #[test]
    fn parser_preserves_io_failure_before_and_during_chunk_lengths() {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = [EMPTY_PATCH, HEADER_CHUNK].concat();
        for fail_at in [12, 13, 14, 15, bytes.len()] {
            let mut reader = ShortReader {
                bytes: &bytes,
                offset: 0,
                fail_at: Some(fail_at),
                interrupt_at: None,
            };
            assert!(matches!(
                apply_patch(&mut reader, Path::new("fixture.patch"), tmp.path()),
                Err(PatchApplyError::Io(e)) if e.kind() == io::ErrorKind::ConnectionReset
            ));
        }
    }

    fn write_temp(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn chunk(tag: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(tag);
        out.extend_from_slice(body);
        out.extend_from_slice(&[0u8; 4]);
        out
    }

    fn patch_with(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut out = SIGNATURE.to_vec();
        for c in chunks {
            out.extend_from_slice(c);
        }
        out
    }

    fn dir_body(rel: &str) -> Vec<u8> {
        let mut body = (rel.len() as u32).to_be_bytes().to_vec();
        body.extend_from_slice(rel.as_bytes());
        body
    }

    #[test]
    fn rejects_file_without_signature() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        fs::create_dir_all(&game).unwrap();
        let patch = write_temp(tmp.path(), "bad.patch", b"not a zipatch at all");
        let err = apply_patch_file(&patch, &game).unwrap_err();
        assert!(
            matches!(err, PatchApplyError::BadSignature(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn resolve_swaps_windows_separators() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        fs::create_dir_all(&game).unwrap();
        let applier = ZiPatchApplier {
            game_root: game,
            report: PatchApplyResult::default(),
        };
        let shown = applier
            .resolve("sub\\inner\\file.dat")
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            shown.contains("sub") && shown.contains("inner") && shown.contains("file.dat"),
            "got {shown}"
        );
    }

    #[test]
    fn resolve_rejects_parent_and_rooted_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        fs::create_dir_all(&game).unwrap();
        let applier = ZiPatchApplier {
            game_root: game,
            report: PatchApplyResult::default(),
        };

        for path in ["..\\outside.dat", "/outside.dat", "."] {
            assert!(
                matches!(applier.resolve(path), Err(PatchApplyError::UnsafePath(_))),
                "accepted {path:?}"
            );
        }
    }

    #[test]
    fn adir_creates_missing_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        fs::create_dir_all(&game).unwrap();
        let patch = write_temp(
            tmp.path(),
            "mk.patch",
            &patch_with(&[chunk(b"ADIR", &dir_body("fresh"))]),
        );
        let report = apply_patch_file(&patch, &game).unwrap();
        assert!(game.join("fresh").is_dir());
        assert!(report.messages.is_empty(), "got {:?}", report.messages);
    }

    #[test]
    fn adir_warns_when_directory_present() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        fs::create_dir_all(game.join("already")).unwrap();
        let patch = write_temp(
            tmp.path(),
            "mk.patch",
            &patch_with(&[chunk(b"ADIR", &dir_body("already"))]),
        );
        let report = apply_patch_file(&patch, &game).unwrap();
        assert_eq!(report.messages.len(), 1);
        assert!(
            report.messages[0].contains("already exists"),
            "got {:?}",
            report.messages
        );
    }

    #[test]
    fn unknown_tag_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        fs::create_dir_all(&game).unwrap();
        let patch = write_temp(
            tmp.path(),
            "weird.patch",
            &patch_with(&[chunk(b"WXYZ", &[])]),
        );
        match apply_patch_file(&patch, &game).unwrap_err() {
            PatchApplyError::UnhandledChunk(tag) => assert_eq!(tag, "WXYZ"),
            other => panic!("expected UnhandledChunk, got {other:?}"),
        }
    }

    #[test]
    fn etry_writes_a_stored_file() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        fs::create_dir_all(&game).unwrap();

        let payload = b"hello zipatch";
        let rel = "data\\hello.txt";
        let mut etry = Vec::new();
        etry.extend_from_slice(&(rel.len() as u32).to_be_bytes());
        etry.extend_from_slice(rel.as_bytes());
        etry.extend_from_slice(&1u32.to_be_bytes());
        etry.extend_from_slice(&0x41u32.to_le_bytes());
        etry.extend_from_slice(&[0u8; 40]);
        etry.extend_from_slice(&0x4Eu32.to_le_bytes());
        etry.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        etry.extend_from_slice(&0u32.to_be_bytes());
        etry.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        etry.extend_from_slice(payload);

        let patch = write_temp(
            tmp.path(),
            "file.patch",
            &patch_with(&[chunk(b"ETRY", &etry)]),
        );
        let report = apply_patch_file(&patch, &game).unwrap();

        let written = fs::read(game.join("data").join("hello.txt")).unwrap();
        assert_eq!(written, payload);
        assert!(report.messages.iter().any(|m| m.contains("hello.txt")));
    }
}
