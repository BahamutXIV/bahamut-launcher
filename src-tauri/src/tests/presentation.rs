use std::path::PathBuf;

use bahamut_launcher::config::launcher_ini::LauncherConfig;
use bahamut_launcher::config::preferences::{
    DisplayMode, GameSettings, Multisampling, SUPPORTED_RESOLUTIONS,
};
use bahamut_launcher::install_check::InstallState;
use bahamut_launcher::launcher::pe_patch::PePatchError;
use bahamut_launcher::platform::LaunchError;

use crate::commands::home::home_state_diagnostics;
use crate::commands::support::{EXTERNAL_LINKS, open_external};
use crate::commands::window::{
    WindowControlAction, WindowControlTarget, apply_window_control, window_size_for_work_area,
};
use crate::presentation::{
    GameSettingsPayload, GraphicsSettingsPayload, HomeLifecycleState, game_settings_view,
    home_presentation, map_launch_error, replace_game_settings, resolve_home_lifecycle,
};

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

#[test]
fn launch_translation_uses_preparation_for_known_setup_failures() {
    let error = map_launch_error(LaunchError::PatchPlanning(
        PePatchError::LobbyHostHasInteriorNul,
    ));

    assert_eq!(error.kind, "preparation");
    assert_ne!(error.kind, "lobby");
    assert!(error.message.unwrap().contains("interior NUL"));

    let missing_config =
        map_launch_error(LaunchError::MissingConfigBinary(PathBuf::from("C:/game")));
    assert_eq!(missing_config.kind, "preparation");
}
