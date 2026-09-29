use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use bahamut_launcher::auth::client::AuthClientError;
use bahamut_launcher::auth::types::ErrorCode;
use bahamut_launcher::config::extension_config::ExtensionsConfig;
use bahamut_launcher::config::launcher_ini::LauncherConfig;
use bahamut_launcher::config::preferences::{
    DisplayMode, GameSettings, Multisampling, SUPPORTED_RESOLUTIONS,
};
use bahamut_launcher::content::http::ObjectSpec;
use bahamut_launcher::content::manifest::{ArchiveLayout, BaseArchive, BasePackage, InstallFile};
use bahamut_launcher::content::worker::InstallRequest;
use bahamut_launcher::content::{InstallShared, Phase};
use bahamut_launcher::install_check::InstallState;
use bahamut_launcher::platform::LaunchedGame;
use bahamut_launcher::version::{FFXIV_BOOT_VERSION, FFXIV_GAME_VERSION};
use sha2::{Digest, Sha256};

use crate::commands::auth::translate_client_error;
use crate::commands::extensions::{extension_folder_path, extension_inventory_view_with_overlays};
use crate::commands::home::home_state_diagnostics;
use crate::commands::install::{
    CONTENT_CLOSING_MSG, CONTENT_STATE_POISONED_MSG, INSTALL_BUSY_MSG, spawn_installer,
};
use crate::commands::support::{EXTERNAL_LINKS, open_external};
use crate::commands::window::{
    WindowControlAction, WindowControlTarget, apply_window_control, window_size_for_work_area,
};
use crate::presentation::{
    AUTH_KIND_INVALID_CREDENTIALS, AUTH_KIND_RATE_LIMITED, AuthOp, GameSettingsPayload,
    GraphicsSettingsPayload, HomeLifecycleState, game_settings_view, home_presentation,
    replace_game_settings, resolve_home_lifecycle,
};
use crate::state::{BackupIpcState, ContentIpcState, GameIpcState, InstallRun};

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

fn frontend_source() -> String {
    [
        include_str!("../ui/index.html"),
        include_str!("../ui/js/runtime.js"),
        include_str!("../ui/js/settings.js"),
        include_str!("../ui/js/extensions.js"),
        include_str!("../ui/js/home.js"),
        include_str!("../ui/js/main.js"),
    ]
    .join("\n")
    .replace("\r\n", "\n")
}

#[test]
fn game_settings_update_requires_import_and_preserves_other_config() {
    let mut config = LauncherConfig::defaults();
    let mut payload = GameSettingsPayload::from(GameSettings {
        initialized: true,
        ..GameSettings::default()
    });
    assert_eq!(
        replace_game_settings(&mut config, payload.clone()).unwrap_err(),
        "game settings are unavailable until a valid retail config.sys is found"
    );

    config.preferences.game.initialized = true;
    let servers = config.servers.clone();
    payload.display_mode = DisplayMode::FullScreen;
    payload.width = 1920;
    payload.height = 1080;
    payload.graphics.multisampling = Multisampling::X4;
    payload.audio.play_in_background = true;
    replace_game_settings(&mut config, payload).unwrap();

    assert!(config.preferences.game.initialized);
    assert_eq!(
        config.preferences.game.display_mode,
        DisplayMode::FullScreen
    );
    assert_eq!(
        (
            config.preferences.game.width,
            config.preferences.game.height
        ),
        (1920, 1080)
    );
    assert_eq!(
        config.preferences.game.graphics.multisampling,
        Multisampling::X4
    );
    assert!(config.preferences.game.audio.play_in_background);
    assert_eq!(config.servers, servers);

    let saved = config.preferences.game.clone();
    for invalid in [
        GameSettingsPayload {
            width: 1234,
            height: 567,
            ..saved.clone().into()
        },
        GameSettingsPayload {
            graphics: GraphicsSettingsPayload {
                general_quality: 0,
                ..saved.graphics.clone().into()
            },
            ..saved.clone().into()
        },
        GameSettingsPayload {
            graphics: GraphicsSettingsPayload {
                background_quality: 6,
                ..saved.graphics.clone().into()
            },
            ..saved.clone().into()
        },
    ] {
        assert!(replace_game_settings(&mut config, invalid).is_err());
        assert_eq!(config.preferences.game, saved);
    }
}

#[test]
fn game_settings_view_exposes_only_supported_resolutions() {
    let view = game_settings_view(GameSettings {
        initialized: true,
        ..GameSettings::default()
    });
    assert!(view.available);
    assert_eq!(
        view.supported_resolutions.len(),
        SUPPORTED_RESOLUTIONS.len()
    );
    assert_eq!(view.supported_resolutions[0], [1024, 768]);
    assert_eq!(view.supported_resolutions.last(), Some(&[2560, 2048]));
}

#[derive(Default)]
struct FakeWindow {
    dragged: std::cell::Cell<bool>,
    minimized: std::cell::Cell<bool>,
    closed: std::cell::Cell<bool>,
}

impl WindowControlTarget for FakeWindow {
    fn start_dragging_window(&self) -> tauri::Result<()> {
        self.dragged.set(true);
        Ok(())
    }

    fn minimize_window(&self) -> tauri::Result<()> {
        self.minimized.set(true);
        Ok(())
    }

    fn close_window(&self) -> tauri::Result<()> {
        self.closed.set(true);
        Ok(())
    }
}

#[test]
fn control_window_dispatches_every_supported_action() {
    let window = FakeWindow::default();
    apply_window_control(&window, WindowControlAction::StartDragging).unwrap();
    assert!(window.dragged.get());

    apply_window_control(&window, WindowControlAction::Minimize).unwrap();
    assert!(window.minimized.get());
    assert!(!window.closed.get());

    apply_window_control(&window, WindowControlAction::Close).unwrap();
    assert!(window.closed.get());
}

#[test]
fn window_size_prefers_1280_by_800_and_clamps_to_work_area() {
    let preferred = window_size_for_work_area(1920.0, 1080.0).unwrap();
    assert_eq!((preferred.width, preferred.height), (1280.0, 800.0));

    let clamped = window_size_for_work_area(1024.0, 700.0).unwrap();
    assert_eq!((clamped.width, clamped.height), (1024.0, 700.0));

    assert!(window_size_for_work_area(0.0, 800.0).is_err());
    assert!(window_size_for_work_area(f64::NAN, 800.0).is_err());
}

#[test]
fn home_lifecycle_resolves_exactly_four_distinct_states() {
    let cases = [
        (
            InstallState::NotFound,
            false,
            HomeLifecycleState::NoValidInstall,
        ),
        (
            InstallState::FoundOutdated {
                game_version: Some("2012.01.01.0000.0000".into()),
            },
            false,
            HomeLifecycleState::OutdatedInstall,
        ),
        (InstallState::Ready, false, HomeLifecycleState::LoggedOut),
        (InstallState::Ready, true, HomeLifecycleState::Ready),
    ];

    for (install, authenticated, expected) in cases {
        let actual = resolve_home_lifecycle(&install, authenticated);
        assert_eq!(actual, expected);
    }
}

#[test]
fn home_lifecycle_diagnostics_are_stable_and_support_facing() {
    assert_eq!(
        home_state_diagnostics(HomeLifecycleState::Ready),
        ("STATE_READY", "ready", "ready", "launch")
    );
    assert_eq!(
        home_state_diagnostics(HomeLifecycleState::OutdatedInstall),
        (
            "STATE_OUTDATED_INSTALL",
            "outdated-install",
            "outdated",
            "install-fresh"
        )
    );
}

#[test]
fn home_presentations_keep_the_account_card_stable() {
    for state in [
        HomeLifecycleState::NoValidInstall,
        HomeLifecycleState::OutdatedInstall,
        HomeLifecycleState::LoggedOut,
        HomeLifecycleState::Ready,
    ] {
        let presentation = home_presentation(state);
        assert_eq!(presentation.title, "Account Login");
        assert_eq!(presentation.eyebrow, "");
        let expected_action = match state {
            HomeLifecycleState::NoValidInstall | HomeLifecycleState::OutdatedInstall => "Install",
            HomeLifecycleState::LoggedOut | HomeLifecycleState::Ready => "Play",
        };
        assert_eq!(presentation.primary_action, expected_action);
    }
}

#[test]
fn external_link_allowlist_has_launcher_targets() {
    assert!(EXTERNAL_LINKS.contains(&("github", "https://github.com/BahamutXIV/bahamut")));
    assert!(EXTERNAL_LINKS.contains(&("discord", "https://discord.gg/PxK5RJYQjm")));
    assert!(EXTERNAL_LINKS.contains(&("youtube", "https://www.youtube.com/@Aeshur")));
    assert!(EXTERNAL_LINKS.contains(&("wiki", "https://bahamut.miraheze.org/wiki/Main_Page")));
    assert!(open_external("website".into()).is_err());
}

fn addon_package(id: &str, supported: bool) -> bahamut_launcher::extensions::AddonPackage {
    bahamut_launcher::extensions::AddonPackage {
        manifest_path: PathBuf::from(format!("addons/{id}/addon.toml")),
        id: id.into(),
        name: id.into(),
        author: "Tester".into(),
        version: "1.0.0".into(),
        description: "Test addon.".into(),
        homepage: None,
        supported_client_builds: vec![if supported {
            bahamut_launcher::extensions::SUPPORTED_GAME_VERSION.into()
        } else {
            "different-build".into()
        }],
        capabilities: Vec::new(),
        commands: Vec::new(),
    }
}

fn overlay_package(id: &str, payload: &str) -> bahamut_launcher::extensions::OverlayPackage {
    bahamut_launcher::extensions::OverlayPackage {
        manifest_path: PathBuf::from(format!("plugins/dats/{id}/overlay.toml")),
        root_path: PathBuf::from(format!("plugins/dats/{id}")),
        id: id.into(),
        name: id.into(),
        author: "Tester".into(),
        version: "1.0.0".into(),
        description: "Test overlay.".into(),
        homepage: None,
        payload_files: vec![PathBuf::from(payload)],
    }
}

#[test]
fn addon_inventory_uses_persisted_state_order_and_compatibility() {
    let mut config = ExtensionsConfig::default();
    config.set_addon_enabled("zeta", true).unwrap();
    config.set_addon_enabled("alpha", false).unwrap();
    config.set_addon_enabled("legacy", true).unwrap();
    let inventory = extension_inventory_view_with_overlays(
        vec![
            addon_package("alpha", true),
            addon_package("legacy", false),
            addon_package("zeta", true),
            addon_package("fps", true),
        ],
        &config,
        Vec::new(),
        &bahamut_launcher::config::extension_config::DatsConfig::default(),
    );
    assert_eq!(
        inventory
            .addons
            .iter()
            .map(|addon| addon.id.as_str())
            .collect::<Vec<_>>(),
        vec!["fps", "zeta", "alpha", "legacy"]
    );
    assert!(inventory.addons[0].enabled);
    assert!(inventory.addons[1].enabled);
    assert!(!inventory.addons[2].enabled);
    assert_eq!(inventory.addons[3].status, "Enabled");
    assert_eq!(
        inventory.addons[3].compatibility,
        "Client build is not listed by this package"
    );
    assert_eq!(inventory.plugins.len(), 2);
    assert_eq!(inventory.plugins[0].id, "screenshot");
    assert_eq!(inventory.plugins[0].author, "Aeshur");
    assert_eq!(inventory.plugins[0].version, "1.0");
    assert_eq!(inventory.plugins[0].status, "Enabled");
    assert!(inventory.plugins[0].enabled);
    assert_eq!(inventory.plugins[0].commands.len(), 1);
    assert_eq!(inventory.plugins[0].commands[0].usage, "/screenshot");
    assert_eq!(inventory.plugins[1].id, "discord-rpc");
    assert_eq!(inventory.plugins[1].name, "DiscordRPC");
    assert!(inventory.plugins[1].enabled);
}

#[test]
fn overlay_inventory_reports_persisted_order_and_conflicting_payloads() {
    let config = ExtensionsConfig::default();
    let dat_config = bahamut_launcher::config::extension_config::DatsConfig {
        packages: vec![
            bahamut_launcher::config::extension_config::ExtensionPreference {
                id: "beta".into(),
                enabled: true,
            },
            bahamut_launcher::config::extension_config::ExtensionPreference {
                id: "alpha".into(),
                enabled: true,
            },
        ],
    };
    let inventory = extension_inventory_view_with_overlays(
        Vec::new(),
        &config,
        vec![
            overlay_package("alpha", "data/shared.DAT"),
            overlay_package("beta", "data/shared.DAT"),
        ],
        &dat_config,
    );
    assert_eq!(
        inventory
            .overlays
            .iter()
            .map(|package| package.id.as_str())
            .collect::<Vec<_>>(),
        vec!["beta", "alpha"]
    );
    assert!(inventory.overlays.iter().all(|package| package.enabled));
    assert_eq!(inventory.overlay_conflicts.len(), 1);
    assert_eq!(
        inventory.overlay_conflicts[0].relative_path,
        "data/shared.DAT"
    );
    assert_eq!(
        inventory.overlay_conflicts[0].package_ids,
        vec!["beta", "alpha"]
    );
}

#[test]
fn frontend_gamepad_poll_reads_only_the_active_controller() {
    let frontend = frontend_source();
    assert!(frontend.contains(
        "readControllerActions(activeGamepad, controllerStates.get(activeGamepad.index))"
    ));
    assert!(!frontend.contains("for (const gamepad of gamepads.filter(Boolean))"));
}

#[test]
fn settings_frontend_matches_flat_backend_contract() {
    let frontend = frontend_source();
    assert!(frontend.contains("invoke('get_game_settings')"));
    assert!(frontend.contains("invoke('set_game_settings', { settings:next })"));
    assert!(frontend.contains("invoke('get_launcher_behavior')"));
    assert!(frontend.contains("invoke('set_close_on_game_start', { closeOnGameStart })"));
    assert!(
        frontend.contains("invoke('set_native_resolution_override', { nativeResolutionOverride })")
    );
    assert!(frontend.contains("invoke(action === 'create' ? 'create_backup' : 'restore_backup'"));
    assert!(frontend.contains("User Settings and Macros"));
    assert!(frontend.contains("data-backup-target=\"extensions\""));
    assert!(frontend.contains("Open Backup Folder"));
    assert!(frontend.contains("Open Install Folder"));
    assert!(frontend.contains("id=\"settings-confirmation-dialog\""));
    assert!(frontend.contains("data-settings-tab=\"general\""));
    assert!(frontend.contains("data-settings-tab=\"graphics\""));
    assert!(frontend.contains("data-settings-tab=\"misc\""));
    assert!(!frontend.contains("maintenance"));
    assert!(!frontend.contains("data-settings-tab=\"controls\""));
    assert!(!frontend.contains("id=\"screenshot-hotkey\""));
    assert!(frontend.contains("Close Launcher on Game Start"));
    assert!(frontend.contains("Native Resolution Override"));
    assert!(!frontend.contains(">Language<"));
    assert!(frontend.contains("data-action=\"open-profiles\""));
    assert!(frontend.contains("data-screen=\"profiles\""));
    assert!(frontend.contains("data-help-action=\"open\""));
    assert!(frontend.contains("Incorrect username or password."));
    assert!(!frontend.contains("The username or password was not accepted."));
    assert!(!frontend.contains("Game settings saved."));
    assert!(!frontend.contains("data-game-settings-tab"));
    assert!(frontend.contains("data-settings-action=\"open-config\""));
    assert!(frontend.contains("config_tool_supported"));
    assert!(frontend.contains("launch_config_tool"));
    assert!(!frontend.contains("data-extension-folder=\"logs\""));
    assert!(!frontend.contains("get_extension_runtime_status"));
    assert!(!frontend.contains("run_extension_diagnostics"));
    assert!(!frontend.contains("Overlay packages"));
    assert!(!frontend.contains("Enabled packages are applied in first-hit order."));
    assert!(!frontend.contains("Open Overlay Folder"));
    assert!(!frontend.contains("data-extension-folder=\"dats\""));
    assert!(!frontend.contains("Overlay scale"));
    assert!(!frontend.contains("Update channel"));
    assert!(!frontend.contains("data-extension-action"));
    assert!(!frontend.contains("value=\"Ctrl+Shift+O\""));
    assert!(!frontend.contains("Use the packaged runtime on the next game launch"));
    assert!(!frontend.contains("Extensions were quarantined"));
    assert!(!frontend.contains("Steam Deck support is unavailable."));
    assert!(frontend.contains("Steam Deck not detected."));
    assert!(frontend.contains("Apply Steam Deck Defaults"));
    assert!(!frontend.contains("The current controller remains active while connected"));
    assert!(!frontend.contains("first connected controller with recent input"));
    assert!(!frontend.contains("id=\"shell-status\""));
    assert!(!frontend.contains(" title=\""));
}

#[test]
fn extension_folder_allowlist_exposes_only_package_roots() {
    let layout = bahamut_launcher::extensions::ExtensionLayout::under_launcher_dir(Path::new(
        "portable-root",
    ));
    assert_eq!(
        extension_folder_path(&layout, "addons").unwrap(),
        Path::new("portable-root/addons")
    );
    assert_eq!(
        extension_folder_path(&layout, "plugins").unwrap(),
        Path::new("portable-root/plugins")
    );
    assert_eq!(
        extension_folder_path(&layout, "logs").unwrap(),
        Path::new("portable-root/logs")
    );
    assert_eq!(
        extension_folder_path(&layout, "screenshots").unwrap(),
        Path::new("portable-root/screenshots")
    );
    assert_eq!(
        extension_folder_path(&layout, "backups").unwrap(),
        bahamut_launcher::config::dirs::backups_dir().unwrap()
    );
    assert_eq!(
        extension_folder_path(&layout, "install").unwrap(),
        Path::new("portable-root")
    );
    assert!(extension_folder_path(&layout, "dats").is_err());
}

/// Build `<parent>/Bahamut Launcher.app` with an `Info.plist` and return its executable path.
fn write_test_bundle(parent: &Path) -> PathBuf {
    let contents = parent.join("Bahamut Launcher.app/Contents");
    std::fs::create_dir_all(contents.join("MacOS")).unwrap();
    std::fs::create_dir_all(contents.join("Resources")).unwrap();
    std::fs::write(contents.join("Info.plist"), "<plist/>\n").unwrap();
    let exe = contents.join("MacOS/bahamut-launcher");
    std::fs::write(&exe, b"").unwrap();
    exe
}

#[test]
fn bundled_extension_folders_open_only_state_root_directories() {
    let fixture = crate::test_support::tempdir().unwrap();
    let exe = write_test_bundle(fixture.path());
    let state = fixture.path().join("home/.bahamut-launcher");
    let roots = bahamut_launcher::config::dirs::resolve_roots(&exe, Some(state.clone())).unwrap();
    let layout = bahamut_launcher::extensions::ExtensionLayout::new(&roots.install, &roots.state);
    for (target, expected) in [
        ("addons", state.join("addons")),
        ("plugins", state.join("plugins")),
        ("logs", state.join("logs")),
        ("screenshots", state.join("screenshots")),
        ("install", state.clone()),
    ] {
        assert_eq!(
            extension_folder_path(&layout, target).unwrap(),
            expected,
            "{target}"
        );
    }
    assert!(!roots.install.starts_with(&state));
    assert!(extension_folder_path(&layout, "dats").is_err());
}

#[test]
fn startup_update_recovery_succeeds_on_a_new_bundled_state_root() {
    let fixture = crate::test_support::tempdir().unwrap();
    let exe = write_test_bundle(fixture.path());
    let bundle = fixture.path().join("Bahamut Launcher.app");
    let state = fixture.path().join("home/.bahamut-launcher");
    let roots = bahamut_launcher::config::dirs::resolve_roots(&exe, Some(state.clone())).unwrap();
    let bundle_entries = || {
        let mut entries = Vec::new();
        let mut pending = vec![bundle.clone()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path.clone());
                }
                entries.push(path);
            }
        }
        entries.sort();
        entries
    };
    let bundle_before = bundle_entries();

    assert!(
        crate::launcher_updates::recover_failed_launcher_update(&roots.state, false).is_err(),
        "a state root that does not exist yet is rejected"
    );
    assert_eq!(
        crate::request_pending_update_recovery_for(&roots, false),
        Ok(false),
        "startup recovery creates the state root before probing it"
    );
    assert_eq!(
        crate::request_pending_update_recovery_for(&roots, true),
        Ok(false)
    );
    assert!(state.join("config/launcher-update.lock").is_file());
    assert!(state.join("config/launcher-update-helper.lock").is_file());
    assert_eq!(bundle_entries(), bundle_before);
}

#[test]
fn auth_error_mapping_uses_exact_login_kinds_and_retry_after() {
    let invalid = translate_client_error(
        AuthClientError::Api {
            status: 401,
            code: ErrorCode::InvalidCredentials.as_str().into(),
            message: "invalid credentials".into(),
            known: Some(ErrorCode::InvalidCredentials),
            retry_after_seconds: None,
        },
        AuthOp::Login,
    );
    assert_eq!(invalid.kind, AUTH_KIND_INVALID_CREDENTIALS);
    assert!(invalid.retry_after.is_none());

    let limited = translate_client_error(
        AuthClientError::Api {
            status: 429,
            code: ErrorCode::RateLimited.as_str().into(),
            message: "too many attempts".into(),
            known: Some(ErrorCode::RateLimited),
            retry_after_seconds: Some(3),
        },
        AuthOp::Login,
    );
    assert_eq!(limited.kind, AUTH_KIND_RATE_LIMITED);
    assert_eq!(limited.retry_after, Some(3));

    let mismatched = translate_client_error(
        AuthClientError::Api {
            status: 400,
            code: ErrorCode::RateLimited.as_str().into(),
            message: "malformed rate response".into(),
            known: Some(ErrorCode::RateLimited),
            retry_after_seconds: Some(3),
        },
        AuthOp::Login,
    );
    assert_eq!(mismatched.kind, "server");
}

#[test]
fn registration_server_error_uses_stable_create_failure_kind() {
    let error = translate_client_error(
        AuthClientError::Api {
            status: 500,
            code: "server_error".to_string(),
            message: "database detail".to_string(),
            known: Some(ErrorCode::ServerError),
            retry_after_seconds: None,
        },
        AuthOp::Register,
    );

    assert_eq!(error.kind, "create-failed");
    assert!(error.message.is_none());

    let mismatched = translate_client_error(
        AuthClientError::Api {
            status: 503,
            code: "server_error".to_string(),
            message: "unexpected status".to_string(),
            known: Some(ErrorCode::ServerError),
            retry_after_seconds: None,
        },
        AuthOp::Register,
    );

    assert_eq!(mismatched.kind, "server");
}

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
