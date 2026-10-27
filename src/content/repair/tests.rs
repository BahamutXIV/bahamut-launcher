use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

use zip::write::SimpleFileOptions;

use super::archive::{create_owned_parents, repair_sources, write_staged_bytes};
use super::control::{RepairPhase, RepairShared};
use super::journal::{
    JournalPhase, RepairJournal, create_transaction, journal_read_limit, make_journal,
    maximum_repair_journal, read_json, write_journal,
};
use super::paths::{
    find_file, join_relative, resolve_cache_path, resolve_existing_directory, transaction_path,
};
use super::quote::{preflight_space, quote_for_statuses, summary};
use super::recover::{inspect_recovery_state, recover_transaction};
use super::scan::{scan_files_controlled, scan_selected, select_repair_files};
use super::types::{FileState, ManagedFileStatus, RepairQuote, RepairResult, VerificationSummary};
use super::{
    JOURNAL_FILE, OWNER_FILE, ROLLBACK_DIR, STAGED_DIR, acquire_repair_lock, canonical_game_root,
    recovery_pending, repair_all,
};
use crate::content::http::ObjectSpec;
use crate::content::installer::package_identity;
use crate::content::manifest::{ArchiveLayout, BaseArchive, BasePackage, InstallFile};
use crate::digest::sha256_hex;
use crate::version::{FFXIV_BOOT_VERSION, FFXIV_GAME_VERSION};

fn verify(
    destination: &Path,
    cache: &Path,
    package: &BasePackage,
) -> Result<VerificationSummary, String> {
    package.validate()?;
    let sources = repair_sources(package)?;
    let game = resolve_existing_directory(destination)?;
    let cache_path = resolve_cache_path(cache)?;
    let files = scan_files_controlled(&game, &package.final_files, &RepairShared::new())?;
    let transaction = transaction_path(&game)?;
    let (recovery_required, recovery_detail) = inspect_recovery_state(&transaction, package)?;
    let quote = quote_for_statuses(
        &game,
        &cache_path.path,
        &cache_path.nearest_existing,
        package,
        &sources,
        &files,
    )?;
    Ok(summary(&files, recovery_required, recovery_detail, quote))
}

#[test]
fn repair_selection_scales_to_the_full_managed_inventory() {
    const INVENTORY_SIZE: usize = 191_960;
    let mut files = Vec::with_capacity(INVENTORY_SIZE);
    let mut statuses = Vec::with_capacity(INVENTORY_SIZE);
    let mut expected_repair_indices = Vec::new();

    for index in 0..INVENTORY_SIZE {
        let path = format!("managed/file-{index:06}.dat");
        let state = if index % 30_000 == 0 {
            expected_repair_indices.push(index);
            if index % 60_000 == 0 {
                FileState::Corrupt
            } else {
                FileState::Missing
            }
        } else {
            FileState::Valid
        };
        files.push(InstallFile {
            path: path.clone(),
            length: 1,
            sha256: "0".repeat(64),
        });
        statuses.push(ManagedFileStatus {
            path,
            state,
            expected_length: 1,
            actual_length: (state != FileState::Missing).then_some(1),
            actual_sha256: None,
            detail: None,
        });
    }

    let (selected, selected_statuses) = select_repair_files(&files, &statuses).unwrap();
    assert_eq!(selected.len(), expected_repair_indices.len());
    assert_eq!(selected_statuses.len(), expected_repair_indices.len());
    for ((file, status), index) in selected
        .iter()
        .zip(&selected_statuses)
        .zip(expected_repair_indices)
    {
        assert_eq!(file.path, format!("managed/file-{index:06}.dat"));
        assert_eq!(status.path, file.path);
        assert_ne!(status.state, FileState::Valid);
    }
}

struct Fixture {
    package: BasePackage,
    game: PathBuf,
    cache: PathBuf,
    archive_bytes: Vec<u8>,
    contents: HashMap<String, Vec<u8>>,
}

fn fixture(root: &Path) -> Fixture {
    let game = root.join("game");
    let cache = root.join("cache");
    fs::create_dir_all(&game).unwrap();
    fs::create_dir_all(&cache).unwrap();
    let contents = HashMap::from([
        ("ffxivboot.exe".to_owned(), b"boot exe".to_vec()),
        ("ffxivgame.exe".to_owned(), b"game exe".to_vec()),
        (
            "data/client.dat".to_owned(),
            b"managed client data".to_vec(),
        ),
        (
            "boot.ver".to_owned(),
            FFXIV_BOOT_VERSION.as_bytes().to_vec(),
        ),
        (
            "game.ver".to_owned(),
            FFXIV_GAME_VERSION.as_bytes().to_vec(),
        ),
    ]);
    for (relative, bytes) in &contents {
        let path = join_relative(&game, relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for relative in ["ffxivboot.exe", "ffxivgame.exe", "data/client.dat"] {
        writer
            .start_file(relative, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&contents[relative]).unwrap();
    }
    let archive_bytes = writer.finish().unwrap().into_inner();
    let archive_files = ["ffxivboot.exe", "ffxivgame.exe", "data/client.dat"]
        .into_iter()
        .map(|path| install_file(path, &contents[path]))
        .collect::<Vec<_>>();
    let final_files = contents
        .iter()
        .map(|(path, bytes)| install_file(path, bytes))
        .collect::<Vec<_>>();
    let archive_sha256 = sha256_hex(&archive_bytes);
    let archive = BaseArchive {
        object: ObjectSpec {
            object_key: "game/test/client.zip".into(),
            length: archive_bytes.len() as u64,
            sha256: archive_sha256,
        },
        files: archive_files,
        layout: ArchiveLayout::Flat,
        excluded_files: Vec::new(),
        empty_directories: Vec::new(),
        apple_metadata_files: 0,
    };
    let staging_bytes = final_files.iter().map(|file| file.length).sum();
    let package = BasePackage {
        target_version: FFXIV_GAME_VERSION.into(),
        archives: vec![archive],
        final_files,
        staging_bytes,
    };
    package.validate().unwrap();
    fs::write(
        cache.join(format!(
            "{}-{}.object",
            package.archives[0].object.sha256, package.archives[0].object.length
        )),
        &archive_bytes,
    )
    .unwrap();
    Fixture {
        package,
        game,
        cache,
        archive_bytes,
        contents,
    }
}

fn install_file(path: &str, bytes: &[u8]) -> InstallFile {
    InstallFile {
        path: path.into(),
        length: bytes.len() as u64,
        sha256: sha256_hex(bytes),
    }
}

fn local_archive_server(bytes: Vec<u8>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/zip\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .unwrap();
        stream.write_all(&bytes).unwrap();
    });
    (format!("http://{address}/"), server)
}

fn managed(root: &Path, package: &BasePackage, path: &str) -> PathBuf {
    join_relative(root, &find_file(package, path).unwrap().path)
}

fn run_repair(destination: &Path, root: &str, fixture: &Fixture) -> Result<RepairResult, String> {
    repair_all(
        destination,
        root,
        &fixture.cache,
        &fixture.package,
        &RepairShared::new(),
    )
}

fn add_large_managed_file(fixture: &mut Fixture, path: &str, length: usize) {
    let bytes = vec![0x5a; length];
    fixture.contents.insert(path.to_owned(), bytes.clone());
    let file = install_file(path, &bytes);
    fixture.package.final_files.push(file.clone());
    fixture
        .package
        .final_files
        .sort_by(|left, right| left.path.cmp(&right.path));
    let archive_files = &mut fixture.package.archives[0].files;
    archive_files.push(file);
    archive_files.sort_by(|left, right| left.path.cmp(&right.path));

    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for archived_file in archive_files.iter() {
        writer
            .start_file(&archived_file.path, SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(&fixture.contents[&archived_file.path])
            .unwrap();
    }
    fixture.archive_bytes = writer.finish().unwrap().into_inner();
    let old_object = fixture.package.archives[0].object.clone();
    fixture.package.archives[0].object.length = fixture.archive_bytes.len() as u64;
    fixture.package.archives[0].object.sha256 = sha256_hex(&fixture.archive_bytes);
    fixture.package.staging_bytes = fixture
        .package
        .final_files
        .iter()
        .map(|file| file.length)
        .sum();
    fixture.package.validate().unwrap();

    fs::remove_file(fixture.cache.join(format!(
        "{}-{}.object",
        old_object.sha256, old_object.length
    )))
    .unwrap();
    fs::write(
        fixture.cache.join(format!(
            "{}-{}.object",
            fixture.package.archives[0].object.sha256, fixture.package.archives[0].object.length
        )),
        &fixture.archive_bytes,
    )
    .unwrap();
    let large_path = join_relative(&fixture.game, path);
    fs::write(large_path, bytes).unwrap();
}

#[cfg(unix)]
#[test]
fn dangling_recovery_link_blocks_readiness_without_being_removed() {
    let temporary = crate::content::test_support::tempdir().unwrap();
    let fixture = fixture(temporary.path());
    let transaction = transaction_path(&fixture.game).unwrap();
    std::os::unix::fs::symlink(temporary.path().join("missing"), &transaction).unwrap();
    assert!(recovery_pending(&fixture.game));
    assert!(!crate::content::check_game_version(&fixture.game));
    assert!(
        fs::symlink_metadata(transaction)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn second_process_cannot_recover_active_repair() {
    const CHILD_ROOT: &str = "BAHAMUT_TEST_REPAIR_LOCK_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let package: BasePackage =
            serde_json::from_slice(&fs::read(root.join("package.json")).unwrap()).unwrap();
        let error = repair_all(
            &root.join("game"),
            "https://unused.invalid/",
            &root.join("cache"),
            &package,
            &RepairShared::new(),
        )
        .unwrap_err();
        assert!(error.contains("Another launcher is repairing"), "{error}");
        return;
    }

    let temporary = crate::content::test_support::tempdir().unwrap();
    let fixture = fixture(temporary.path());
    let game = canonical_game_root(&fixture.game).unwrap();
    let lock = acquire_repair_lock(&game).unwrap();
    let file = find_file(&fixture.package, "data/client.dat").unwrap();
    let live = join_relative(&game, &file.path);
    fs::write(&live, b"corrupt original").unwrap();
    let before = scan_selected(&game, &[file]);
    let identity = package_identity(&fixture.package);
    let transaction = transaction_path(&game).unwrap();
    create_transaction(&transaction, &fixture.package, &identity).unwrap();
    let owner_bytes = fs::read(transaction.join(OWNER_FILE)).unwrap();
    fs::write(
        temporary.path().join("package.json"),
        serde_json::to_vec(&fixture.package).unwrap(),
    )
    .unwrap();

    for journaled in [false, true] {
        if journaled {
            write_staged_bytes(
                &transaction.join(STAGED_DIR),
                file,
                &fixture.contents["data/client.dat"],
            )
            .unwrap();
            let journal = make_journal(&identity, &before, Vec::new()).unwrap();
            write_journal(&transaction, &journal).unwrap();
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "content::repair::tests::second_process_cannot_recover_active_repair",
                "--nocapture",
            ])
            .env(CHILD_ROOT, temporary.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "child test was not discovered: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(fs::read(transaction.join(OWNER_FILE)).unwrap(), owner_bytes);
        assert_eq!(fs::read(&live).unwrap(), b"corrupt original");
        assert_eq!(transaction.join(JOURNAL_FILE).is_file(), journaled);
        if journaled {
            assert_eq!(
                fs::read(join_relative(&transaction.join(STAGED_DIR), &file.path)).unwrap(),
                fixture.contents["data/client.dat"]
            );
        }
    }
    drop(lock);
    assert!(
        run_repair(&game, "https://unused.invalid/", &fixture)
            .unwrap()
            .complete
    );
    assert!(!transaction.exists());
    assert_eq!(fs::read(live).unwrap(), fixture.contents["data/client.dat"]);
    // The stable lock remains reusable after the completed repair.
    assert!(acquire_repair_lock(&game).is_ok());
}

#[test]
fn verification_omits_valid_rows_but_counts_the_full_inventory() {
    let temporary = crate::content::test_support::tempdir().unwrap();
    let fixture = fixture(temporary.path());

    let verified = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();

    assert!(verified.files.is_empty());
    assert_eq!(verified.valid_count, fixture.package.final_files.len());
    assert_eq!(verified.missing_count, 0);
    assert_eq!(verified.corrupt_count, 0);
    assert_eq!(verified.unresolved_count, 0);
}

#[test]
fn verification_hashes_equal_length_corruption_and_repair_preserves_user_state() {
    let temporary = crate::content::test_support::tempdir().unwrap();
    let fixture = fixture(temporary.path());
    let user_state = fixture.game.join("user");
    fs::create_dir_all(&user_state).unwrap();
    fs::write(user_state.join("custom-package.dat"), b"player-owned").unwrap();
    fs::write(fixture.game.join("settings.ini"), b"player settings").unwrap();
    fs::write(
        managed(&fixture.game, &fixture.package, "data/client.dat"),
        b"X".repeat(fixture.contents["data/client.dat"].len()),
    )
    .unwrap();
    fs::remove_file(managed(&fixture.game, &fixture.package, "game.ver")).unwrap();

    let verified = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
    let corrupt = verified
        .files
        .iter()
        .find(|file| file.path == "data/client.dat")
        .unwrap();
    assert_eq!(corrupt.state, FileState::Corrupt);
    assert!(
        verified
            .files
            .iter()
            .all(|file| file.state != FileState::Valid)
    );
    assert_eq!(verified.valid_count, fixture.package.final_files.len() - 2);
    assert_eq!(verified.missing_count, 1);
    assert_eq!(corrupt.actual_length, Some(corrupt.expected_length));
    assert_ne!(
        corrupt.actual_sha256.as_deref(),
        Some(
            find_file(&fixture.package, "data/client.dat")
                .unwrap()
                .sha256
                .as_str()
        )
    );
    assert_eq!(verified.quote.download_bytes, 0);

    let result = run_repair(&fixture.game, "https://unused.invalid/", &fixture).unwrap();
    assert!(result.complete);
    assert_eq!(result.repaired_files.len(), 2);
    assert_eq!(
        fs::read(managed(&fixture.game, &fixture.package, "data/client.dat")).unwrap(),
        fixture.contents["data/client.dat"]
    );
    assert_eq!(
        fs::read(user_state.join("custom-package.dat")).unwrap(),
        b"player-owned"
    );
    assert_eq!(
        fs::read(fixture.game.join("settings.ini")).unwrap(),
        b"player settings"
    );
    assert!(!transaction_path(&fixture.game).unwrap().exists());
}

#[test]
fn missing_file_is_quoted_and_restored_from_the_verified_archive_cache() {
    let temporary = crate::content::test_support::tempdir().unwrap();
    let fixture = fixture(temporary.path());
    let missing = managed(&fixture.game, &fixture.package, "data/client.dat");
    fs::remove_file(&missing).unwrap();
    let verified = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
    assert_eq!(verified.missing_count, 1);
    assert_eq!(
        verified
            .files
            .iter()
            .find(|file| file.path == "data/client.dat")
            .unwrap()
            .state,
        FileState::Missing
    );
    assert_eq!(verified.quote.download_bytes, 0);

    let result = run_repair(&fixture.game, "https://unused.invalid/", &fixture).unwrap();
    assert!(result.complete);
    assert_eq!(
        fs::read(missing).unwrap(),
        fixture.contents["data/client.dat"]
    );
}

#[test]
fn unresolved_managed_path_is_never_overwritten() {
    let temporary = crate::content::test_support::tempdir().unwrap();
    let fixture = fixture(temporary.path());
    let managed_path = managed(&fixture.game, &fixture.package, "data/client.dat");
    fs::remove_file(&managed_path).unwrap();
    fs::create_dir(&managed_path).unwrap();
    fs::write(managed_path.join("user-data"), b"do not remove").unwrap();
    let verified = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
    assert_eq!(verified.unresolved_count, 1);
    let error = run_repair(&fixture.game, "https://unused.invalid/", &fixture).unwrap_err();
    assert!(error.contains("Cannot safely repair"));
    assert_eq!(
        fs::read(managed_path.join("user-data")).unwrap(),
        b"do not remove"
    );
}

#[cfg(windows)]
#[test]
fn unreadable_managed_file_blocks_repair() {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    use super::scan::inspect_file;

    let temporary = crate::content::test_support::tempdir().unwrap();
    let fixture = fixture(temporary.path());
    let path = managed(&fixture.game, &fixture.package, "data/client.dat");
    let _exclusive_read = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();

    let status = inspect_file(
        &fixture.game,
        find_file(&fixture.package, "data/client.dat").unwrap(),
    );
    assert_eq!(status.state, FileState::Unresolved);
    let error = run_repair(&fixture.game, "https://unused.invalid/", &fixture).unwrap_err();
    assert!(error.contains("Cannot safely repair"));
}

#[test]
fn invalid_cached_zip_is_quoted_as_a_full_transfer_and_revalidated_before_repair() {
    let temporary = crate::content::test_support::tempdir().unwrap();
    let fixture = fixture(temporary.path());
    let expected_cache = fixture.cache.join(format!(
        "{}-{}.object",
        fixture.package.archives[0].object.sha256, fixture.package.archives[0].object.length
    ));
    fs::write(&expected_cache, b"invalid cached archive").unwrap();
    fs::write(
        managed(&fixture.game, &fixture.package, "data/client.dat"),
        b"X".repeat(fixture.contents["data/client.dat"].len()),
    )
    .unwrap();

    let verified = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
    assert_eq!(
        verified.quote.download_bytes,
        fixture.package.archives[0].object.length
    );
    assert_eq!(
        verified.quote.full_archive_bytes,
        fixture.archive_bytes.len() as u64
    );

    let (root, server) = local_archive_server(fixture.archive_bytes.clone());
    let result = run_repair(&fixture.game, &root, &fixture).unwrap();
    server.join().unwrap();
    assert!(result.complete);
    assert_eq!(fs::read(&expected_cache).unwrap(), fixture.archive_bytes);
    assert_eq!(
        fs::read(managed(&fixture.game, &fixture.package, "data/client.dat")).unwrap(),
        fixture.contents["data/client.dat"]
    );
}

#[test]
fn insufficient_cache_or_game_space_blocks_before_creating_repair_state() {
    let temporary = crate::content::test_support::tempdir().unwrap();
    let fixture = fixture(temporary.path());
    let quote = RepairQuote {
        full_archive_bytes: 100,
        download_bytes: 100,
        replacement_bytes: 20,
        available_cache_bytes: Some(110),
        available_game_bytes: Some(110),
        cache_problem: None,
    };
    let error = preflight_space(&quote, &fixture.cache, &fixture.game).unwrap_err();
    assert!(error.contains("Insufficient free space"));
    assert!(!transaction_path(&fixture.game).unwrap().exists());
}

#[test]
fn interrupted_publication_can_be_recovered_without_file_selection() {
    let temporary = crate::content::test_support::tempdir().unwrap();
    let fixture = fixture(temporary.path());
    let file = find_file(&fixture.package, "data/client.dat").unwrap();
    let live = join_relative(&fixture.game, &file.path);
    fs::write(&live, b"old bad client data").unwrap();
    let before = scan_selected(&fixture.game, &[file]);
    assert_eq!(before[0].state, FileState::Corrupt);
    let package_sha256 = package_identity(&fixture.package);
    let transaction = transaction_path(&fixture.game).unwrap();
    create_transaction(&transaction, &fixture.package, &package_sha256).unwrap();
    let staged_root = transaction.join(STAGED_DIR);
    let rollback_root = transaction.join(ROLLBACK_DIR);
    write_staged_bytes(&staged_root, file, &fixture.contents["data/client.dat"]).unwrap();
    create_owned_parents(
        &rollback_root,
        join_relative(&rollback_root, &file.path).parent().unwrap(),
    )
    .unwrap();
    let journal = make_journal(&package_sha256, &before, Vec::new()).unwrap();
    write_journal(&transaction, &journal).unwrap();
    let rollback = join_relative(&rollback_root, &file.path);
    fs::rename(&live, &rollback).unwrap();
    assert!(!crate::content::check_game_version(&fixture.game));
    assert_ne!(
        crate::install_check::check_install(Some(&fixture.game)).state,
        crate::install_check::InstallState::Ready
    );
    crate::atomic_fs::rename_noreplace(&join_relative(&staged_root, &file.path), &live).unwrap();
    assert!(recovery_pending(&fixture.game));
    let mut journal = journal;
    journal.files[0].published = true;
    write_journal(&transaction, &journal).unwrap();

    let interrupted = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
    assert!(interrupted.recovery_required);
    let game_root = canonical_game_root(&fixture.game).unwrap();
    let result = run_repair(&game_root, "https://unused.invalid/", &fixture).unwrap();
    assert!(result.complete);
    assert!(!transaction.exists());
    assert_eq!(fs::read(live).unwrap(), fixture.contents["data/client.dat"]);
    assert!(!recovery_pending(&fixture.game));
    assert_eq!(
        crate::install_check::check_install(Some(&fixture.game)).state,
        crate::install_check::InstallState::Ready
    );
}

#[test]
fn recovery_accepts_a_legitimate_journal_larger_than_one_megabyte() {
    let temporary = crate::content::test_support::tempdir().unwrap();
    let mut fixture = fixture(temporary.path());
    let padding = "a".repeat(128);
    let extras = (0..6_000)
        .map(|index| InstallFile {
            path: format!("data/{padding}-{index:04}.dat"),
            length: 1,
            sha256: sha256_hex(b"x"),
        })
        .collect::<Vec<_>>();
    fixture.package.final_files.extend(extras.iter().cloned());
    fixture.package.archives[0]
        .files
        .extend(extras.iter().cloned());
    fixture.package.staging_bytes = fixture
        .package
        .final_files
        .iter()
        .map(|file| file.length)
        .sum();
    fixture.package.validate().unwrap();

    let missing = extras
        .iter()
        .map(|file| ManagedFileStatus {
            path: file.path.clone(),
            state: FileState::Missing,
            expected_length: file.length,
            actual_length: None,
            actual_sha256: None,
            detail: None,
        })
        .collect::<Vec<_>>();
    let package_sha256 = package_identity(&fixture.package);
    let maximum = maximum_repair_journal(&fixture.package, &package_sha256).unwrap();
    let maximum_bytes = serde_json::to_vec(&maximum).unwrap();
    assert!(maximum_bytes.len() > 1024 * 1024);
    assert_eq!(
        journal_read_limit(&fixture.package, &package_sha256).unwrap(),
        maximum_bytes.len() as u64
    );
    let transaction = transaction_path(&fixture.game).unwrap();
    create_transaction(&transaction, &fixture.package, &package_sha256).unwrap();
    let journal = make_journal(&package_sha256, &missing, Vec::new()).unwrap();
    assert!(serde_json::to_vec(&journal).unwrap().len() > 1024 * 1024);
    write_journal(&transaction, &journal).unwrap();

    recover_transaction(
        &transaction,
        &fixture.game,
        &fixture.package,
        &package_sha256,
    )
    .unwrap();
    let recovered = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
    assert!(!recovered.recovery_required);
    assert_eq!(recovered.missing_count, extras.len());
    assert!(!transaction.exists());
}

#[test]
fn interrupted_move_publication_before_journal_update_recovers_and_retries() {
    let temporary = crate::content::test_support::tempdir().unwrap();
    let fixture = fixture(temporary.path());
    let file = find_file(&fixture.package, "data/client.dat").unwrap();
    let live = join_relative(&fixture.game, &file.path);
    fs::remove_file(&live).unwrap();
    let before = scan_selected(&fixture.game, &[file]);
    assert_eq!(before[0].state, FileState::Missing);
    let package_sha256 = package_identity(&fixture.package);
    let transaction = transaction_path(&fixture.game).unwrap();
    create_transaction(&transaction, &fixture.package, &package_sha256).unwrap();
    let staged_root = transaction.join(STAGED_DIR);
    let staged = join_relative(&staged_root, &file.path);
    write_staged_bytes(&staged_root, file, &fixture.contents["data/client.dat"]).unwrap();
    let journal = make_journal(&package_sha256, &before, Vec::new()).unwrap();
    write_journal(&transaction, &journal).unwrap();

    crate::atomic_fs::rename_noreplace(&staged, &live).unwrap();
    let interrupted = verify(&fixture.game, &fixture.cache, &fixture.package).unwrap();
    assert!(interrupted.recovery_required);

    let result = run_repair(&fixture.game, "https://unused.invalid/", &fixture).unwrap();
    assert!(result.complete);
    assert!(!transaction.exists());
    assert_eq!(fs::read(live).unwrap(), fixture.contents["data/client.dat"]);
}

#[test]
fn repair_is_idempotent_when_every_managed_file_is_valid() {
    let temporary = crate::content::test_support::tempdir().unwrap();
    let fixture = fixture(temporary.path());
    let result = run_repair(&fixture.game, "https://unused.invalid/", &fixture).unwrap();
    assert!(result.complete);
    assert!(result.repaired_files.is_empty());
}

#[test]
fn cancel_while_paused_during_verification_leaves_managed_files_untouched() {
    let temporary = crate::content::test_support::tempdir().unwrap();
    let mut fixture = fixture(temporary.path());
    add_large_managed_file(&mut fixture, "zz-large.dat", 64 * 1024 * 1024);
    let corrupt_path = managed(&fixture.game, &fixture.package, "data/client.dat");
    let corrupt_bytes = b"X".repeat(fixture.contents["data/client.dat"].len());
    fs::write(&corrupt_path, &corrupt_bytes).unwrap();

    let shared = RepairShared::new();
    let worker_shared = shared.clone();
    let game = fixture.game.clone();
    let cache = fixture.cache.clone();
    let package = fixture.package.clone();
    let worker = std::thread::spawn(move || {
        repair_all(
            &game,
            "https://unused.invalid/",
            &cache,
            &package,
            &worker_shared,
        )
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "repair did not reach the large managed file"
        );
        if shared.phase() == RepairPhase::Verifying
            && shared.current_file().as_deref() == Some("zz-large.dat")
        {
            shared.request_pause();
            break;
        }
        assert_ne!(shared.phase(), RepairPhase::Error, "repair failed early");
        std::thread::sleep(Duration::from_millis(1));
    }
    while !shared.is_paused() {
        assert!(
            std::time::Instant::now() < deadline,
            "repair did not stop at the requested pause checkpoint"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    shared.request_cancel();

    let error = worker.join().unwrap().unwrap_err();
    assert!(error.contains("cancelled"));
    assert_eq!(shared.phase(), RepairPhase::Cancelled);
    assert_eq!(fs::read(corrupt_path).unwrap(), corrupt_bytes);
    assert!(!transaction_path(&fixture.game).unwrap().exists());
}

#[test]
fn cancel_during_final_verification_rolls_back_published_repairs() {
    let temporary = crate::content::test_support::tempdir().unwrap();
    let mut fixture = fixture(temporary.path());
    add_large_managed_file(&mut fixture, "zz-large.dat", 64 * 1024 * 1024);
    let repaired_path = managed(&fixture.game, &fixture.package, "data/client.dat");
    let original_corrupt_bytes = b"X".repeat(fixture.contents["data/client.dat"].len());
    fs::write(&repaired_path, &original_corrupt_bytes).unwrap();

    let shared = RepairShared::new();
    let worker_shared = shared.clone();
    let game = fixture.game.clone();
    let cache = fixture.cache.clone();
    let package = fixture.package.clone();
    let worker = std::thread::spawn(move || {
        repair_all(
            &game,
            "https://unused.invalid/",
            &cache,
            &package,
            &worker_shared,
        )
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "repair did not reach the full final verification scan"
        );
        if shared.phase() == RepairPhase::FinalVerification
            && shared.current_file().as_deref() == Some("zz-large.dat")
        {
            shared.request_pause();
            break;
        }
        assert_ne!(shared.phase(), RepairPhase::Error, "repair failed early");
        std::thread::sleep(Duration::from_millis(1));
    }
    while !shared.is_paused() {
        assert!(
            std::time::Instant::now() < deadline,
            "repair did not stop at the requested final verification checkpoint"
        );
        std::thread::sleep(Duration::from_millis(1));
    }

    let transaction = transaction_path(&fixture.game).unwrap();
    let package_sha256 = package_identity(&fixture.package);
    let journal = read_json::<RepairJournal>(
        &transaction.join(JOURNAL_FILE),
        journal_read_limit(&fixture.package, &package_sha256).unwrap(),
    )
    .unwrap();
    assert!(matches!(journal.phase, JournalPhase::Publishing));
    assert_eq!(
        fs::read(&repaired_path).unwrap(),
        fixture.contents["data/client.dat"]
    );
    assert!(
        !join_relative(&transaction.join(STAGED_DIR), "data/client.dat").exists(),
        "final verification should not scan a hard-linked managed file"
    );

    shared.request_cancel();
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.contains("cancelled"));
    assert_eq!(shared.phase(), RepairPhase::Cancelled);
    assert_eq!(fs::read(repaired_path).unwrap(), original_corrupt_bytes);
    assert!(!transaction.exists());
}
