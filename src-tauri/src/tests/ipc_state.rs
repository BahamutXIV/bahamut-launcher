use std::sync::Arc;
use std::thread;
use std::time::Duration;

use bahamut_launcher::platform::LaunchedGame;

use crate::state::{BackupIpcState, ContentIpcState, GameIpcState};

#[test]
fn game_state_reserves_one_launch_and_releases_on_exit() {
    let state = Arc::new(GameIpcState::default());
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let attempts = (0..2)
        .map(|_| {
            let state = Arc::clone(&state);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                state.begin_launch()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let reservations = attempts
        .into_iter()
        .filter_map(|attempt| attempt.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(reservations.len(), 1);
    assert!(state.begin_restore().is_none());
    drop(reservations);
    let reservation = state.begin_launch().unwrap();
    let (game, exited) = LaunchedGame::pending(42);
    reservation.complete_launch(game);
    assert!(state.is_active());
    assert!(state.begin_restore().is_none());
    exited.send(()).unwrap();
    assert!(!state.is_active());
    assert!(state.begin_launch().is_some());
}

#[test]
fn launch_and_restore_share_an_atomic_reservation() {
    let state = Arc::new(GameIpcState::default());
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let attempts = (0..2)
        .map(|operation| {
            let state = Arc::clone(&state);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                if operation == 0 {
                    state.begin_launch()
                } else {
                    state.begin_restore()
                }
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let reservations = attempts
        .into_iter()
        .filter_map(|attempt| attempt.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(reservations.len(), 1);
    assert!(state.begin_launch().is_none());
    assert!(state.begin_restore().is_none());
    drop(reservations);
    let restore = state.begin_restore().unwrap();
    assert!(!state.is_active());
    assert!(state.begin_launch().is_none());
    drop(restore);
    assert!(state.begin_launch().is_some());
}

#[test]
fn blocking_launch_retains_ownership_when_ipc_waiter_is_dropped() {
    let state = GameIpcState::default();
    let reservation = state.begin_launch().unwrap();
    let (resume, wait) = std::sync::mpsc::channel();
    let (completed, completion) = std::sync::mpsc::channel();
    let (game, exited) = LaunchedGame::pending(42);
    let worker = tauri::async_runtime::spawn_blocking(move || {
        wait.recv().unwrap();
        reservation.complete_launch(game);
        completed.send(()).unwrap();
    });
    drop(worker);
    assert!(state.begin_restore().is_none());
    resume.send(()).unwrap();
    completion.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(state.is_active());
    assert!(state.begin_restore().is_none());
    exited.send(()).unwrap();
    assert!(state.begin_restore().is_some());
}

#[test]
fn blocking_worker_failure_releases_its_reservation() {
    for restore in [false, true] {
        let state = GameIpcState::default();
        let reservation = if restore {
            state.begin_restore()
        } else {
            state.begin_launch()
        }
        .unwrap();
        let worker = tauri::async_runtime::spawn_blocking(move || {
            let _reservation = reservation;
            panic!("injected blocking-worker failure");
        });
        assert!(tauri::async_runtime::block_on(worker).is_err());
        assert!(state.begin_launch().is_some());
        assert!(state.begin_restore().is_some());
    }
}

#[test]
fn shutdown_waits_for_launch_restore_and_backup_workers() {
    for restore in [false, true] {
        let state = Arc::new(GameIpcState::default());
        let backups = Arc::new(BackupIpcState::default());
        let reservation = if restore {
            state.begin_restore()
        } else {
            state.begin_launch()
        }
        .unwrap();
        let backup = backups.begin().unwrap();
        assert!(state.request_shutdown());
        assert!(backups.request_shutdown());
        assert!(state.begin_launch().is_none());
        assert!(state.begin_restore().is_none());
        assert!(backups.begin().is_err());
        let (finished, completion) = std::sync::mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker_backups = Arc::clone(&backups);
        let waiter = thread::spawn(move || {
            worker_state.wait_for_worker();
            worker_backups.wait_for_worker();
            finished.send(()).unwrap();
        });
        assert!(completion.recv_timeout(Duration::from_millis(30)).is_err());
        if restore {
            drop(reservation);
        } else {
            let (game, _exited) = LaunchedGame::pending(42);
            reservation.complete_launch(game);
        }
        assert!(completion.recv_timeout(Duration::from_millis(30)).is_err());
        drop(backup);
        completion.recv_timeout(Duration::from_secs(5)).unwrap();
        waiter.join().unwrap();
        assert!(!state.request_shutdown());
        assert!(!backups.request_shutdown());
    }
}

#[test]
fn backup_worker_failure_releases_ownership_and_exit_wait() {
    let backups = BackupIpcState::default();
    let reservation = backups.begin().unwrap();
    assert!(backups.begin().is_err());
    let worker = tauri::async_runtime::spawn_blocking(move || {
        let _reservation = reservation;
        panic!("injected backup failure");
    });
    assert!(tauri::async_runtime::block_on(worker).is_err());
    assert!(backups.begin().is_ok());
    assert!(!backups.request_shutdown());
    backups.wait_for_worker();
}

#[test]
fn shutdown_retains_the_exit_gate_for_non_content_work() {
    let state = ContentIpcState::default();
    let request = state.request_shutdown(true);
    assert!(request.first_request);
    assert!(request.waiting);
    assert!(request.workers.is_empty());
    assert!(state.request_shutdown(false).waiting);
    state.mark_shutdown_complete();
    assert!(!state.request_shutdown(false).waiting);
}
