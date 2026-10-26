use std::io::{Cursor, Write};
use std::thread;
use std::time::{Duration, Instant};

use super::archive::*;
use super::final_tree::*;
use super::paths::*;
use super::receipt::*;
use super::space::*;
use super::*;
use crate::content::manifest::ArchiveLayout;
use crate::content::manifest::{BaseArchive, InstallFile};
use crate::content::test_support::tempdir;
use std::fs::File;
use std::path::PathBuf;
use zip::write::{SimpleFileOptions, ZipWriter};

struct Entry<'a> {
    path: &'a str,
    contents: &'a [u8],
    mode: Option<u32>,
}

struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("simulated write failure"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn zip_bytes(entries: &[Entry<'_>]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for entry in entries {
        let options = entry
            .mode
            .map(|mode| SimpleFileOptions::default().unix_permissions(mode))
            .unwrap_or_default();
        if entry.path.ends_with('/') {
            zip.add_directory(entry.path, options).unwrap();
        } else {
            zip.start_file(entry.path, options).unwrap();
            zip.write_all(entry.contents).unwrap();
        }
    }
    zip.finish().unwrap().into_inner()
}

fn with_unix_mode(mut bytes: Vec<u8>, name: &str, mode: u32) -> Vec<u8> {
    let signature = b"PK\x01\x02";
    let mut cursor = 0;
    while let Some(offset) = bytes[cursor..]
        .windows(signature.len())
        .position(|window| window == signature)
    {
        let start = cursor + offset;
        let name_length = u16::from_le_bytes([bytes[start + 28], bytes[start + 29]]) as usize;
        let extra_length = u16::from_le_bytes([bytes[start + 30], bytes[start + 31]]) as usize;
        let comment_length = u16::from_le_bytes([bytes[start + 32], bytes[start + 33]]) as usize;
        let entry_name = &bytes[start + 46..start + 46 + name_length];
        if entry_name == name.as_bytes() {
            bytes[start + 38..start + 42].copy_from_slice(&(mode << 16).to_le_bytes());
            return bytes;
        }
        cursor = start + 46 + name_length + extra_length + comment_length;
    }
    panic!("central-directory entry {name} not found");
}

fn file(path: &str, contents: &[u8]) -> InstallFile {
    InstallFile {
        path: path.to_owned(),
        length: contents.len() as u64,
        sha256: crate::digest::sha256_hex(contents),
    }
}

fn fixture(
    root: &Path,
    entries: &[Entry<'_>],
    expected: &[(&str, &[u8])],
    final_boot_hash: Option<&str>,
) -> (BasePackage, PathBuf, PathBuf) {
    let cache = root.join("cache");
    fs::create_dir(&cache).unwrap();
    let destination = root.join("game");
    let archive_bytes = zip_bytes(entries);
    let digest = crate::digest::sha256_hex(&archive_bytes);
    let archive_path = cache.join(format!("{digest}-{}.object", archive_bytes.len()));
    fs::write(&archive_path, &archive_bytes).unwrap();

    let archive_files = expected
        .iter()
        .map(|(path, contents)| file(path, contents))
        .collect::<Vec<_>>();
    let mut final_files = archive_files.clone();
    final_files.push(file("boot.ver", FFXIV_BOOT_VERSION.as_bytes()));
    final_files.push(file("game.ver", FFXIV_GAME_VERSION.as_bytes()));
    if let Some(hash) = final_boot_hash {
        final_files
            .iter_mut()
            .find(|entry| entry.path == "ffxivboot.exe")
            .unwrap()
            .sha256 = hash.to_owned();
    }
    let total = archive_files.iter().map(|file| file.length).sum::<u64>();
    let package = BasePackage {
        target_version: FFXIV_GAME_VERSION.to_owned(),
        archives: vec![BaseArchive {
            object: super::super::http::ObjectSpec {
                object_key: "game/test/archive.zip".to_owned(),
                length: archive_bytes.len() as u64,
                sha256: digest,
            },
            files: archive_files,
            layout: ArchiveLayout::Flat,
            excluded_files: Vec::new(),
            empty_directories: Vec::new(),
            apple_metadata_files: 0,
        }],
        final_files,
        staging_bytes: total
            + FFXIV_BOOT_VERSION.len() as u64
            + FFXIV_GAME_VERSION.len() as u64
            + 64,
    };
    (package, cache, destination)
}

fn valid_fixture(root: &Path) -> (BasePackage, PathBuf, PathBuf) {
    let entries = [
        Entry {
            path: "ffxivboot.exe",
            contents: b"boot",
            mode: None,
        },
        Entry {
            path: "ffxivgame.exe",
            contents: b"game",
            mode: None,
        },
    ];
    fixture(
        root,
        &entries,
        &[("ffxivboot.exe", b"boot"), ("ffxivgame.exe", b"game")],
        None,
    )
}

#[test]
fn wrapped_final_archive_maps_stamps_metadata_and_empty_directories() {
    let temporary = tempdir().unwrap();
    let entries = [
        Entry {
            path: "FINAL FANTASY XIV/",
            contents: b"",
            mode: None,
        },
        Entry {
            path: "FINAL FANTASY XIV/ffxivboot.exe",
            contents: b"boot",
            mode: None,
        },
        Entry {
            path: "FINAL FANTASY XIV/ffxivgame.exe",
            contents: b"game",
            mode: None,
        },
        Entry {
            path: "FINAL FANTASY XIV/patch.ver",
            contents: b"1.23b",
            mode: None,
        },
        Entry {
            path: "FINAL FANTASY XIV/boot.ver",
            contents: FFXIV_BOOT_VERSION.as_bytes(),
            mode: None,
        },
        Entry {
            path: "FINAL FANTASY XIV/game.ver",
            contents: FFXIV_GAME_VERSION.as_bytes(),
            mode: None,
        },
        Entry {
            path: "FINAL FANTASY XIV/.DS_Store",
            contents: b"finder",
            mode: None,
        },
        Entry {
            path: "FINAL FANTASY XIV/empty/",
            contents: b"",
            mode: None,
        },
        Entry {
            path: "__MACOSX/FINAL FANTASY XIV/._ffxivgame.exe",
            contents: b"apple",
            mode: None,
        },
    ];
    let (mut package, cache, destination) = fixture(
        temporary.path(),
        &entries,
        &[
            ("ffxivboot.exe", b"boot"),
            ("ffxivgame.exe", b"game"),
            ("patch.ver", b"1.23b"),
        ],
        None,
    );
    let archive = &mut package.archives[0];
    archive.layout = ArchiveLayout::FinalFantasyXivWrapper;
    archive.excluded_files = vec![file(".DS_Store", b"finder")];
    archive.empty_directories = vec!["empty".to_owned()];
    archive.apple_metadata_files = 1;
    let shared =
        InstallShared::with_totals(package.download_bytes().unwrap(), package.final_files.len());
    run_install(&shared, &destination, &cache, package)
        .join()
        .unwrap();
    assert_eq!(shared.phase(), Phase::Done, "{:?}", shared.error());
    assert_eq!(fs::read(destination.join("patch.ver")).unwrap(), b"1.23b");
    assert_eq!(
        fs::read(destination.join("boot.ver")).unwrap(),
        FFXIV_BOOT_VERSION.as_bytes()
    );
    assert!(destination.join("empty").is_dir());
    assert!(!destination.join(".DS_Store").exists());
    assert!(!destination.join("__MACOSX").exists());
}

#[test]
fn wrapped_archive_rejects_unlisted_content() {
    let temporary = tempdir().unwrap();
    let entries = [
        Entry {
            path: "FINAL FANTASY XIV/ffxivboot.exe",
            contents: b"boot",
            mode: None,
        },
        Entry {
            path: "FINAL FANTASY XIV/ffxivgame.exe",
            contents: b"game",
            mode: None,
        },
        Entry {
            path: "FINAL FANTASY XIV/boot.ver",
            contents: FFXIV_BOOT_VERSION.as_bytes(),
            mode: None,
        },
        Entry {
            path: "FINAL FANTASY XIV/game.ver",
            contents: FFXIV_GAME_VERSION.as_bytes(),
            mode: None,
        },
        Entry {
            path: "FINAL FANTASY XIV/unreviewed.txt",
            contents: b"unexpected",
            mode: None,
        },
    ];
    let (mut package, cache, destination) = fixture(
        temporary.path(),
        &entries,
        &[("ffxivboot.exe", b"boot"), ("ffxivgame.exe", b"game")],
        None,
    );
    package.archives[0].layout = ArchiveLayout::FinalFantasyXivWrapper;
    let shared =
        InstallShared::with_totals(package.download_bytes().unwrap(), package.final_files.len());
    run_install(&shared, &destination, &cache, package)
        .join()
        .unwrap();
    assert_eq!(shared.phase(), Phase::Error);
    assert!(!destination.exists());
}

fn run_install(
    shared: &Arc<InstallShared>,
    destination: &Path,
    cache: &Path,
    package: BasePackage,
) -> thread::JoinHandle<()> {
    let shared = Arc::clone(shared);
    let destination = destination.to_path_buf();
    let cache = cache.to_path_buf();
    thread::spawn(move || {
        crate::content::worker::drive(
            shared,
            crate::content::worker::InstallRequest {
                destination,
                content_root: "http://127.0.0.1:9".to_owned(),
                cache_dir: cache,
                package,
            },
        )
    })
}

#[test]
fn empty_destination_reaches_ready_after_inventory_and_version_checks() {
    let temporary = tempdir().unwrap();
    let (package, cache, destination) = valid_fixture(temporary.path());
    let shared =
        InstallShared::with_totals(package.download_bytes().unwrap(), package.final_files.len());
    run_install(&shared, &destination, &cache, package)
        .join()
        .unwrap();

    assert_eq!(shared.phase(), Phase::Done);
    assert_eq!(
        fs::read(destination.join("ffxivboot.exe")).unwrap(),
        b"boot"
    );
    assert_eq!(
        fs::read(destination.join("ffxivgame.exe")).unwrap(),
        b"game"
    );
    assert_eq!(
        fs::read_to_string(destination.join("game.ver")).unwrap(),
        FFXIV_GAME_VERSION
    );
    assert!(!destination.join(RECEIPT_NAME).exists());
}

#[test]
fn matching_owned_staging_is_reset_and_recovered_after_interruption() {
    let temporary = tempdir().unwrap();
    let (package, cache, destination) = valid_fixture(temporary.path());
    let stage = stage_path_for(&destination).unwrap();
    fs::create_dir(&stage).unwrap();
    write_receipt(&stage, &package_identity(&package)).unwrap();
    fs::write(stage.join("partial.bin"), b"unfinished extraction").unwrap();

    let shared =
        InstallShared::with_totals(package.download_bytes().unwrap(), package.final_files.len());
    run_install(&shared, &destination, &cache, package)
        .join()
        .unwrap();

    assert_eq!(shared.phase(), Phase::Done);
    assert!(!stage.exists());
    assert!(destination.join("ffxivgame.exe").is_file());
}

#[test]
fn retry_reclaims_owned_staging_before_space_preflight() {
    let temporary = tempdir().unwrap();
    let (mut package, cache, destination) = valid_fixture(temporary.path());
    package.staging_bytes = u64::MAX - cache_space_requirement(&package, &cache).unwrap();
    let identity = package_identity(&package);
    let stage = stage_path_for(&destination).unwrap();
    fs::create_dir(&stage).unwrap();
    write_receipt(&stage, &identity).unwrap();
    fs::write(stage.join("partial.bin"), b"unfinished extraction").unwrap();
    let shared =
        InstallShared::with_totals(package.download_bytes().unwrap(), package.final_files.len());

    let error = install(
        &shared,
        &destination,
        "http://127.0.0.1:9",
        &cache,
        &package,
    )
    .unwrap_err();

    assert!(error.contains("Insufficient free space"), "{error}");
    assert!(!stage.join("partial.bin").exists());
    assert_eq!(read_receipt(&stage).unwrap(), identity);
    assert!(!destination.exists());
}

#[test]
fn nonempty_destination_is_rejected_without_changing_user_files() {
    let temporary = tempdir().unwrap();
    let (package, cache, destination) = valid_fixture(temporary.path());
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("notes.txt"), b"owner data").unwrap();
    let shared =
        InstallShared::with_totals(package.download_bytes().unwrap(), package.final_files.len());
    run_install(&shared, &destination, &cache, package)
        .join()
        .unwrap();

    assert_eq!(shared.phase(), Phase::Error);
    assert_eq!(
        fs::read(destination.join("notes.txt")).unwrap(),
        b"owner data"
    );
    assert!(!destination.join("ffxivgame.exe").exists());
}

#[test]
fn changed_destination_is_not_removed_at_publish() {
    let temporary = tempdir().unwrap();
    let (package, cache, destination) = valid_fixture(temporary.path());
    fs::create_dir(&destination).unwrap();
    let shared =
        InstallShared::with_totals(package.download_bytes().unwrap(), package.final_files.len());
    shared.request_pause();
    let worker = run_install(&shared, &destination, &cache, package);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !shared.is_paused() {
        assert!(
            Instant::now() < deadline,
            "download did not reach its durable pause"
        );
        thread::sleep(Duration::from_millis(1));
    }
    fs::write(destination.join("owner.txt"), b"changed after start").unwrap();
    shared.request_resume();
    worker.join().unwrap();

    assert_eq!(shared.phase(), Phase::Error);
    assert_eq!(
        fs::read(destination.join("owner.txt")).unwrap(),
        b"changed after start"
    );
    assert!(!destination.join("ffxivgame.exe").exists());
}

#[test]
fn unsafe_archive_path_is_rejected_before_publish() {
    let temporary = tempdir().unwrap();
    let entries = [
        Entry {
            path: "ffxivboot.exe",
            contents: b"boot",
            mode: None,
        },
        Entry {
            path: "ffxivgame.exe",
            contents: b"game",
            mode: None,
        },
        Entry {
            path: "../escape.txt",
            contents: b"escape",
            mode: None,
        },
    ];
    let expected = [
        ("ffxivboot.exe", b"boot".as_slice()),
        ("ffxivgame.exe", b"game".as_slice()),
    ];
    let (package, cache, destination) = fixture(temporary.path(), &entries, &expected, None);
    let error = install(
        &InstallShared::with_totals(1, 1),
        &destination,
        "http://127.0.0.1:9",
        &cache,
        &package,
    )
    .unwrap_err();

    assert!(error.contains("Unsafe content path"), "{error}");
    assert!(!destination.exists());
    assert!(!destination.join("game.ver").exists());
}

#[test]
fn archive_links_and_duplicate_destinations_are_rejected() {
    let temporary = tempdir().unwrap();
    let entries = [
        Entry {
            path: "ffxivboot.exe",
            contents: b"boot",
            mode: None,
        },
        Entry {
            path: "ffxivgame.exe",
            contents: b"game",
            mode: None,
        },
        Entry {
            path: "link",
            contents: b"ffxivgame.exe",
            mode: Some(0o120777),
        },
    ];
    let (mut package, cache, destination) = valid_fixture(temporary.path());
    let bytes = with_unix_mode(zip_bytes(&entries), "link", 0o120777);
    let digest = crate::digest::sha256_hex(&bytes);
    package.archives[0].object.sha256 = digest.clone();
    package.archives[0].object.length = bytes.len() as u64;
    let path = cache.join(format!("{digest}-{}.object", bytes.len()));
    fs::write(&path, &bytes).unwrap();
    let error = install(
        &InstallShared::with_totals(1, 1),
        &destination,
        "http://127.0.0.1:9",
        &cache,
        &package,
    )
    .unwrap_err();
    assert!(error.contains("Links or special modes"), "{error}");
    assert!(!destination.exists());

    let duplicate_root = temporary.path().join("duplicate");
    fs::create_dir(&duplicate_root).unwrap();
    let duplicate_entries = [
        Entry {
            path: "ffxivboot.exe",
            contents: b"boot",
            mode: None,
        },
        Entry {
            path: "FFXIVBOOT.EXE",
            contents: b"boot",
            mode: None,
        },
        Entry {
            path: "ffxivgame.exe",
            contents: b"game",
            mode: None,
        },
    ];
    let (package, cache, destination) = fixture(
        &duplicate_root,
        &duplicate_entries,
        &[("ffxivboot.exe", b"boot"), ("ffxivgame.exe", b"game")],
        None,
    );
    let error = install(
        &InstallShared::with_totals(1, 1),
        &destination,
        "http://127.0.0.1:9",
        &cache,
        &package,
    )
    .unwrap_err();
    assert!(
        error.contains("Duplicate normalized archive destination"),
        "{error}"
    );
    assert!(!destination.exists());
}

#[test]
fn declared_size_hash_and_missing_inventory_are_rejected() {
    let temporary = tempdir().unwrap();
    let entries = [
        Entry {
            path: "ffxivboot.exe",
            contents: b"boot",
            mode: None,
        },
        Entry {
            path: "ffxivgame.exe",
            contents: b"game",
            mode: None,
        },
    ];
    let (mut package, cache, destination) = valid_fixture(temporary.path());
    package.archives[0].files[0].length += 1;
    package.staging_bytes += 1;
    let error = install(
        &InstallShared::with_totals(1, 1),
        &destination,
        "http://127.0.0.1:9",
        &cache,
        &package,
    )
    .unwrap_err();
    assert!(error.contains("Declared length mismatch"), "{error}");
    assert!(!destination.exists());

    let hash_root = temporary.path().join("wrong-hash");
    fs::create_dir(&hash_root).unwrap();
    let wrong_hash = "0".repeat(64);
    let (package, cache, destination) = fixture(
        &hash_root,
        &entries,
        &[("ffxivboot.exe", b"boot"), ("ffxivgame.exe", b"game")],
        Some(&wrong_hash),
    );
    let mut package = package;
    package.archives[0].files[0].sha256 = wrong_hash;
    let error = install(
        &InstallShared::with_totals(1, 1),
        &destination,
        "http://127.0.0.1:9",
        &cache,
        &package,
    )
    .unwrap_err();
    assert!(
        error.contains("Base archive file failed integrity verification"),
        "{error}"
    );
    assert!(!destination.exists());

    let missing_root = temporary.path().join("missing");
    fs::create_dir(&missing_root).unwrap();
    let (mut package, cache, destination) = valid_fixture(&missing_root);
    package.archives[0]
        .files
        .push(file("sqpack/missing.bin", b"missing"));
    package
        .final_files
        .push(file("sqpack/missing.bin", b"missing"));
    package.staging_bytes += b"missing".len() as u64;
    let error = install(
        &InstallShared::with_totals(1, 1),
        &destination,
        "http://127.0.0.1:9",
        &cache,
        &package,
    )
    .unwrap_err();
    assert!(error.contains("missing inventory file"), "{error}");
    assert!(!destination.exists());
}

#[test]
fn invalid_final_identity_never_publishes_a_version_marker() {
    let temporary = tempdir().unwrap();
    let wrong_hash = "0".repeat(64);
    let (package, cache, destination) = valid_fixture(temporary.path());
    let mut package = package;
    package.final_files[0].sha256 = wrong_hash;
    let error = install(
        &InstallShared::with_totals(1, 1),
        &destination,
        "http://127.0.0.1:9",
        &cache,
        &package,
    )
    .unwrap_err();

    assert!(
        error.contains("Final install file failed validation"),
        "{error}"
    );
    assert!(!destination.exists());
    assert!(!destination.join("game.ver").exists());
}

fn install_valid_fixture(root: &Path) -> (BasePackage, PathBuf, PathBuf) {
    let (package, cache, destination) = valid_fixture(root);
    install(
        &InstallShared::with_totals(1, 1),
        &destination,
        "http://127.0.0.1:9",
        &cache,
        &package,
    )
    .unwrap();
    (package, cache, destination)
}

#[test]
fn same_length_tamper_is_rejected_by_hashing_install_paths() {
    let temporary = tempdir().unwrap();
    let (package, cache, destination) = install_valid_fixture(temporary.path());
    fs::write(destination.join("ffxivgame.exe"), b"evil").unwrap();

    write_receipt(&destination, &package_identity(&package)).unwrap();
    let error = install(
        &InstallShared::with_totals(1, 1),
        &destination,
        "http://127.0.0.1:9",
        &cache,
        &package,
    )
    .unwrap_err();
    assert!(
        error.contains("Final install file failed validation"),
        "{error}"
    );
    fs::remove_file(destination.join(RECEIPT_NAME)).unwrap();

    let error = install(
        &InstallShared::with_totals(1, 1),
        &destination,
        "http://127.0.0.1:9",
        &cache,
        &package,
    )
    .unwrap_err();
    assert!(error.contains("not empty"), "{error}");
    assert_eq!(
        fs::read(destination.join("ffxivgame.exe")).unwrap(),
        b"evil"
    );
}

#[test]
fn extraction_verified_check_skips_content_but_keeps_length() {
    let temporary = tempdir().unwrap();
    let (package, _cache, destination) = install_valid_fixture(temporary.path());
    let identity = package_identity(&package);
    write_receipt(&destination, &identity).unwrap();
    let shared = InstallShared::with_totals(1, 1);
    let check = |mode| validate_final_tree(&destination, &package, &identity, &shared, true, mode);
    check(FinalContentCheck::ExtractionVerified).unwrap();

    fs::write(destination.join("ffxivgame.exe"), b"evil").unwrap();
    check(FinalContentCheck::ExtractionVerified).unwrap();
    let error = check(FinalContentCheck::Hash).unwrap_err();
    assert!(
        error.contains("Final install file failed validation"),
        "{error}"
    );

    fs::write(destination.join("ffxivgame.exe"), b"evil!").unwrap();
    let error = check(FinalContentCheck::ExtractionVerified).unwrap_err();
    assert!(
        error.contains("Final install file failed validation: ffxivgame.exe"),
        "{error}"
    );
}

#[test]
fn insufficient_space_fails_before_download_or_staging() {
    let quote = InstallQuote {
        download_bytes: 10,
        staging_bytes: 20,
        destination_bytes: 18,
        available_cache_bytes: Some(5),
        available_destination_bytes: Some(100),
    };
    assert!(
        check_space(&quote, false, quote.download_bytes)
            .unwrap_err()
            .contains("Insufficient free space")
    );
    assert!(
        check_space(&quote, true, quote.download_bytes)
            .unwrap_err()
            .contains("Insufficient free space")
    );
}

#[test]
fn cache_space_preflight_credits_existing_allocations_without_changing_transfer_quote() {
    let temporary = tempdir().unwrap();
    let mut state = 0x7a31_4421_u32;
    let large_payload = (0..128 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect::<Vec<_>>();
    let entries = [
        Entry {
            path: "ffxivboot.exe",
            contents: b"boot",
            mode: None,
        },
        Entry {
            path: "ffxivgame.exe",
            contents: b"game",
            mode: None,
        },
        Entry {
            path: "large.dat",
            contents: &large_payload,
            mode: None,
        },
    ];
    let expected = [
        ("ffxivboot.exe", b"boot".as_slice()),
        ("ffxivgame.exe", b"game".as_slice()),
        ("large.dat", large_payload.as_slice()),
    ];
    let (package, original_cache, destination) =
        fixture(temporary.path(), &entries, &expected, None);
    let cache = temporary.path().join("resumable-cache");
    fs::create_dir(&cache).unwrap();
    let object = &package.archives[0].object;
    let identity = format!("{}-{}", object.sha256, object.length);
    let original = original_cache.join(format!("{identity}.object"));
    let bytes = fs::read(original).unwrap();
    assert!(object.length > 64 * 1024);
    let full_requirement =
        object.length.div_ceil(allocation_unit(&cache).unwrap()) * allocation_unit(&cache).unwrap();
    assert_eq!(
        cache_space_requirement(&package, &cache).unwrap(),
        full_requirement
    );

    fs::write(
        cache.join(format!("{identity}.part")),
        &bytes[..(bytes.len() / 2)],
    )
    .unwrap();
    let partial_requirement = cache_space_requirement(&package, &cache).unwrap();
    assert!(partial_requirement > 0 && partial_requirement < full_requirement);
    let display_quote = quote(&destination, &cache, &package).unwrap();
    assert_eq!(display_quote.download_bytes, object.length);
    check_space(&display_quote, false, partial_requirement).unwrap();

    fs::write(cache.join(format!("{identity}.object")), &bytes).unwrap();
    let complete_allocation = cache_file_footprint(&cache.join(format!("{identity}.object")))
        .unwrap()
        .unwrap()
        .allocated_bytes;
    assert_eq!(
        cache_space_requirement(&package, &cache).unwrap(),
        full_requirement - complete_allocation.min(full_requirement)
    );
    assert_eq!(
        quote(&destination, &cache, &package)
            .unwrap()
            .download_bytes,
        object.length
    );
}

#[test]
fn corrupt_complete_cache_with_low_allocation_fails_space_preflight() {
    let temporary = tempdir().unwrap();
    let (package, cache, destination) = valid_fixture(temporary.path());
    let object = &package.archives[0].object;
    let path = cache.join(format!("{}-{}.object", object.sha256, object.length));
    fs::remove_file(&path).unwrap();
    File::create(&path).unwrap().set_len(object.length).unwrap();

    let footprint = cache_file_footprint(&path).unwrap().unwrap();
    let unit = allocation_unit(&cache).unwrap();
    let full_allocation = object.length.div_ceil(unit) * unit;
    assert_eq!(footprint.logical_bytes, object.length);
    assert!(footprint.allocated_bytes < full_allocation);
    assert_ne!(
        crate::digest::sha256_hex(&fs::read(&path).unwrap()),
        object.sha256
    );

    let required = cache_space_requirement(&package, &cache).unwrap();
    assert_eq!(required, full_allocation - footprint.allocated_bytes);
    let mut display_quote = quote(&destination, &cache, &package).unwrap();
    assert_eq!(display_quote.download_bytes, object.length);
    display_quote.available_cache_bytes = Some(required - 1);
    display_quote.available_destination_bytes = Some(display_quote.staging_bytes);
    assert!(
        check_space(&display_quote, false, required)
            .unwrap_err()
            .contains("Insufficient free space on the download cache")
    );
}

#[test]
fn extraction_and_hashing_bound_length_and_propagate_write_errors() {
    let shared = InstallShared::with_totals(0, 0);
    let mut output = Vec::new();
    let error =
        copy_and_hash(&shared, &mut Cursor::new(b"x"), &mut output, "short.bin", 2).unwrap_err();
    assert!(error.contains("ended before"));

    output.clear();
    let error = copy_and_hash(
        &shared,
        &mut Cursor::new(b"xyz"),
        &mut output,
        "excess.bin",
        2,
    )
    .unwrap_err();
    assert!(error.contains("exceeded its manifest length"));
    assert_eq!(output, b"xy");

    let error = copy_and_hash(
        &shared,
        &mut Cursor::new(b"ok"),
        &mut FailingWriter,
        "write-error.bin",
        2,
    )
    .unwrap_err();
    assert!(error.contains("simulated write failure"));

    let temporary = tempdir().unwrap();
    let oversized = temporary.path().join("oversized.bin");
    fs::write(&oversized, b"xyz").unwrap();
    let error = hash_file(&shared, &oversized, "oversized.bin", 2).unwrap_err();
    assert!(error.contains("exceeded its manifest length"));
}

#[test]
fn quote_reports_transfer_stage_final_and_available_space() {
    let temporary = tempdir().unwrap();
    let (package, cache, destination) = valid_fixture(temporary.path());
    let quote = quote(&destination, &cache, &package).unwrap();
    assert_eq!(quote.download_bytes, package.download_bytes().unwrap());
    assert_eq!(
        quote.staging_bytes,
        staging_space_bound(&package, allocation_unit(temporary.path()).unwrap()).unwrap()
    );
    assert!(quote.staging_bytes >= package.staging_bytes);
    assert_eq!(
        quote.destination_bytes,
        inventory_bytes(&package.final_files).unwrap()
    );
    let object = &package.archives[0].object;
    let unit = allocation_unit(&cache).unwrap();
    let full_requirement = object.length.div_ceil(unit) * unit;
    let complete = cache.join(format!("{}-{}.object", object.sha256, object.length));
    let complete_allocation = cache_file_footprint(&complete)
        .unwrap()
        .unwrap()
        .allocated_bytes;
    assert_eq!(
        cache_space_requirement(&package, &cache).unwrap(),
        full_requirement - complete_allocation.min(full_requirement)
    );
    assert!(quote.available_cache_bytes.is_some() && quote.available_destination_bytes.is_some());
}

#[test]
fn full_client_staging_bound_accounts_for_filesystem_allocation_units() {
    let package = crate::content::manifest::shipped_manifest()
        .unwrap()
        .base
        .unwrap();
    assert_eq!(package.staging_bytes, 14_000_000_000);
    let four_k = staging_space_bound(&package, 4096).unwrap();
    let eight_k = staging_space_bound(&package, 8192).unwrap();
    assert!(four_k > package.staging_bytes);
    assert!(eight_k > four_k);
}

#[test]
fn install_creates_missing_games_parent_after_quote() {
    let temporary = tempdir().unwrap();
    let (package, cache, _) = valid_fixture(temporary.path());
    let destination = temporary.path().join("Games").join("FINAL FANTASY XIV");
    assert!(!destination.parent().unwrap().exists());
    quote(&destination, &cache, &package).unwrap();
    assert!(!destination.parent().unwrap().exists());
    let shared =
        InstallShared::with_totals(package.download_bytes().unwrap(), package.final_files.len());
    run_install(&shared, &destination, &cache, package)
        .join()
        .unwrap();
    assert!(destination.is_dir());
}

#[test]
fn quote_and_install_recover_a_receipt_owned_published_tree() {
    let temporary = tempdir().unwrap();
    let (package, cache, destination) = valid_fixture(temporary.path());
    let shared =
        InstallShared::with_totals(package.download_bytes().unwrap(), package.final_files.len());
    run_install(&shared, &destination, &cache, package.clone())
        .join()
        .unwrap();
    let identity = package_identity(&package);
    write_receipt(&destination, &identity).unwrap();
    let quote = quote(&destination, &cache, &package).unwrap();
    assert_eq!(
        quote.destination_bytes,
        inventory_bytes(&package.final_files).unwrap()
    );
    let second =
        InstallShared::with_totals(package.download_bytes().unwrap(), package.final_files.len());
    run_install(&second, &destination, &cache, package)
        .join()
        .unwrap();
    assert_eq!(second.phase(), Phase::Done, "{:?}", second.error());
    assert!(!destination.join(RECEIPT_NAME).exists());
}

#[test]
fn free_space_uses_additional_cache_bytes_not_the_display_transfer_total() {
    let quote = InstallQuote {
        download_bytes: 100,
        staging_bytes: 20,
        destination_bytes: 18,
        available_cache_bytes: Some(20),
        available_destination_bytes: Some(20),
    };
    assert!(check_space(&quote, false, 20).is_ok());
    assert!(check_space(&quote, false, 21).is_err());
    assert!(check_space(&quote, true, 0).is_ok());
}
