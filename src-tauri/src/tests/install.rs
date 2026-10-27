use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use bahamut_launcher::content::http::ObjectSpec;
use bahamut_launcher::content::manifest::{ArchiveLayout, BaseArchive, BasePackage, InstallFile};
use bahamut_launcher::content::worker::InstallRequest;
use bahamut_launcher::content::{InstallShared, Phase};
use bahamut_launcher::version::{FFXIV_BOOT_VERSION, FFXIV_GAME_VERSION};
use sha2::{Digest, Sha256};

use crate::commands::install::{
    CONTENT_CLOSING_MSG, CONTENT_STATE_POISONED_MSG, INSTALL_BUSY_MSG, spawn_installer,
};
use crate::state::{BackupIpcState, ContentIpcState, GameIpcState, InstallRun};

fn fixture_file(path: &str, contents: &[u8]) -> InstallFile {
    InstallFile {
        path: path.into(),
        length: contents.len() as u64,
        sha256: format!("{:x}", Sha256::digest(contents)),
    }
}

/// Minimal valid package whose archive is tiny, so the disk-space preflight passes on any host.
fn fixture_package() -> BasePackage {
    let executables = vec![
        fixture_file("ffxivboot.exe", b"boot"),
        fixture_file("ffxivgame.exe", b"game"),
    ];
    let mut final_files = executables.clone();
    final_files.push(fixture_file("boot.ver", FFXIV_BOOT_VERSION.as_bytes()));
    final_files.push(fixture_file("game.ver", FFXIV_GAME_VERSION.as_bytes()));
    let archive_bytes = b"fixture archive";
    let archive_sha256 = format!("{:x}", Sha256::digest(archive_bytes));
    let package = BasePackage {
        target_version: FFXIV_GAME_VERSION.into(),
        archives: vec![BaseArchive {
            object: ObjectSpec {
                object_key: format!("game/{archive_sha256}/fixture.zip"),
                length: archive_bytes.len() as u64,
                sha256: archive_sha256,
            },
            files: executables,
            layout: ArchiveLayout::Flat,
            excluded_files: Vec::new(),
            empty_directories: Vec::new(),
            apple_metadata_files: 0,
        }],
        final_files,
        staging_bytes: 4096,
    };
    package.validate().expect("fixture package is valid");
    package
}

fn install_request(destination: PathBuf, content_root: &str, cache_dir: PathBuf) -> InstallRequest {
    InstallRequest {
        destination,
        content_root: content_root.into(),
        cache_dir,
        package: fixture_package(),
    }
}

/// Request for tests that stop at the admission gate before the worker reads it.
fn gate_request() -> InstallRequest {
    install_request(
        PathBuf::from("game-location"),
        "http://127.0.0.1:9",
        PathBuf::from("cache"),
    )
}

#[test]
fn install_start_rejects_a_second_nonterminal_worker() {
    let state = ContentIpcState::default();
    let (release_tx, wait_rx) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || wait_rx.recv().unwrap());
    *state.install.lock().unwrap() = Some(InstallRun {
        shared: InstallShared::with_totals(0, 0),
        worker: Some(worker),
    });
    let err = match spawn_installer(
        &state,
        &GameIpcState::default(),
        &BackupIpcState::default(),
        gate_request(),
    ) {
        Ok(_) => panic!("second reservation unexpectedly succeeded"),
        Err(err) => err,
    };
    assert_eq!(err.message.as_deref(), Some(INSTALL_BUSY_MSG));
    release_tx.send(()).unwrap();
    state
        .install
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .worker
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn poisoned_install_state_returns_error() {
    let state = std::sync::Arc::new(ContentIpcState::default());
    let poisoned = std::sync::Arc::clone(&state);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = poisoned.install.lock().unwrap();
        panic!("poison install state fixture");
    }));

    let error = match spawn_installer(
        &state,
        &GameIpcState::default(),
        &BackupIpcState::default(),
        gate_request(),
    ) {
        Ok(_) => panic!("poisoned state unexpectedly accepted"),
        Err(error) => error,
    };
    assert_eq!(error.kind, "server");
    assert_eq!(error.message.as_deref(), Some(CONTENT_STATE_POISONED_MSG));
}

#[test]
fn content_worker_reserves_game_and_backups_until_download_stops() {
    use std::io::Read;
    use std::net::TcpListener;
    let temp = crate::test_support::tempdir().unwrap();
    let destination = temp.path().join("game-install");
    let cache_dir = temp.path().join("cache");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let root = format!("http://{}", listener.local_addr().unwrap());
    let (requested, request) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut buffer = [0_u8; 4096];
        assert!(stream.read(&mut buffer).unwrap() > 0);
        requested.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(10)).unwrap();
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
    });
    let content = ContentIpcState::default();
    let game = GameIpcState::default();
    let backups = BackupIpcState::default();
    spawn_installer(
        &content,
        &game,
        &backups,
        install_request(destination.clone(), &root, cache_dir.clone()),
    )
    .unwrap();
    request.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(game.begin_launch().is_none());
    assert!(game.begin_restore().is_none());
    assert!(backups.begin().is_err());
    assert!(
        spawn_installer(
            &content,
            &game,
            &backups,
            install_request(destination, &root, cache_dir)
        )
        .is_err()
    );
    release.send(()).unwrap();
    let worker = content
        .install
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .worker
        .take()
        .unwrap();
    worker.join().unwrap();
    server.join().unwrap();
    assert_eq!(
        content
            .install
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .shared
            .phase(),
        Phase::Error
    );
    assert!(game.begin_launch().is_some());
    assert!(backups.begin().is_ok());
}

#[test]
fn active_game_or_backup_rejects_content_before_worker_dispatch() {
    let content = ContentIpcState::default();
    let game = GameIpcState::default();
    let backups = BackupIpcState::default();
    let launch = game.begin_launch().unwrap();
    assert!(spawn_installer(&content, &game, &backups, gate_request()).is_err());
    drop(launch);
    let backup = backups.begin().unwrap();
    assert!(spawn_installer(&content, &game, &backups, gate_request()).is_err());
    assert!(content.install.lock().unwrap().is_none());
    assert!(game.begin_launch().is_some());
    drop(backup);
}

#[test]
fn shutdown_joins_slow_writer_before_allowing_a_new_start() {
    let state = ContentIpcState::default();
    let shared = InstallShared::with_totals(0, 0);
    let temp = crate::test_support::tempdir().unwrap();
    let output = temp.path().join("install-output.tmp");
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_gate = Arc::clone(&gate);
    let worker_output = output.clone();
    let worker = thread::spawn(move || {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(worker_output)
            .unwrap();
        file.write_all(b"safe prefix").unwrap();
        ready_tx.send(()).unwrap();
        let (lock, changed) = &*worker_gate;
        let mut released = lock.lock().unwrap();
        while !*released {
            released = changed.wait(released).unwrap();
        }
        file.write_all(b" and completed suffix").unwrap();
        file.sync_all().unwrap();
    });
    *state.install.lock().unwrap() = Some(InstallRun {
        shared,
        worker: Some(worker),
    });

    ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let request = state.request_shutdown(false);
    assert!(request.first_request);
    assert!(request.waiting);
    let error = match spawn_installer(
        &state,
        &GameIpcState::default(),
        &BackupIpcState::default(),
        gate_request(),
    ) {
        Ok(_) => panic!("reservation unexpectedly succeeded during shutdown"),
        Err(error) => error,
    };
    assert_eq!(error.message.as_deref(), Some(CONTENT_CLOSING_MSG));
    let repeated = state.request_shutdown(false);
    assert!(!repeated.first_request);
    assert!(repeated.waiting);

    let (joined_tx, joined_rx) = std::sync::mpsc::channel();
    let joiner = thread::spawn(move || {
        assert!(!request.join_worker());
        joined_tx.send(()).unwrap();
    });
    assert!(joined_rx.recv_timeout(Duration::from_millis(50)).is_err());
    {
        let (lock, changed) = &*gate;
        *lock.lock().unwrap() = true;
        changed.notify_all();
    }
    joined_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    joiner.join().unwrap();
    state.mark_shutdown_complete();

    assert_eq!(
        std::fs::read(&output).unwrap(),
        b"safe prefix and completed suffix"
    );
    assert!(!state.request_shutdown(false).waiting);
}

#[test]
fn idle_shutdown_is_immediate_and_paused_shutdown_cancels() {
    let idle = ContentIpcState::default();
    let request = idle.request_shutdown(false);
    assert!(request.first_request);
    assert!(!request.waiting);
    assert!(!idle.request_shutdown(false).waiting);

    let paused = ContentIpcState::default();
    let shared = InstallShared::with_totals(0, 0);
    shared.request_pause();
    let observed = Arc::clone(&shared);
    *paused.install.lock().unwrap() = Some(InstallRun {
        shared,
        worker: None,
    });
    let request = paused.request_shutdown(false);
    assert!(request.first_request);
    assert!(!request.waiting);
    assert!(observed.is_cancel_requested());
    assert!(!observed.is_paused());
}

#[test]
fn shutdown_cancels_and_joins_a_real_paused_install_worker() {
    let state = ContentIpcState::default();
    let temp = crate::test_support::tempdir().unwrap();
    let shared = InstallShared::with_totals(0, 0);
    shared.request_pause();
    let worker_shared = Arc::clone(&shared);
    let request = install_request(
        temp.path().join("game"),
        "http://127.0.0.1:9",
        temp.path().join("cache"),
    );
    let worker = std::thread::spawn(move || {
        bahamut_launcher::content::worker::drive(worker_shared, request)
    });
    *state.install.lock().unwrap() = Some(InstallRun {
        shared: Arc::clone(&shared),
        worker: Some(worker),
    });

    let request = state.request_shutdown(false);
    assert!(request.first_request);
    assert!(request.waiting);
    assert!(shared.is_cancel_requested());
    assert!(!request.join_worker());
    // The worker clears its pause acknowledgment on its own thread, so the flag
    // is settled only after the join.
    assert!(!shared.is_paused());
    assert_eq!(shared.phase(), Phase::Cancelled);
}
