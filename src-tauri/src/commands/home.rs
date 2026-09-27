use bahamut_launcher::install_check::{self, InstallState};
use bahamut_launcher::news::{self, NewsItem};
use bahamut_launcher::patcher::content;
use std::sync::Mutex;

use crate::presentation::{
    HomeLifecycleState, HomeStatusView, home_presentation, resolve_home_lifecycle,
};
use crate::shell_config::{default_game_dir_hint, resolve_game_dir};

static LAST_LOGGED_HOME_STATE: Mutex<Option<HomeLifecycleState>> = Mutex::new(None);

#[tauri::command]
pub(crate) async fn get_home_status(authenticated: bool) -> Result<HomeStatusView, String> {
    tauri::async_runtime::spawn_blocking(move || get_home_status_for(authenticated))
        .await
        .map_err(|error| error.to_string())
}

pub(crate) fn get_home_status_for(authenticated: bool) -> HomeStatusView {
    let status = install_check::check_install(resolve_game_dir().as_deref());
    let state = resolve_home_lifecycle(&status.state, authenticated);
    let presentation = home_presentation(state);
    let game_version = match status.state {
        InstallState::FoundNeedsPatch { game_version } => game_version,
        InstallState::NotFound | InstallState::Ready => None,
    };
    let view = HomeStatusView {
        state,
        eyebrow: presentation.eyebrow,
        title: presentation.title,
        primary_action: presentation.primary_action,
        game_dir: status
            .game_dir
            .map(|path| path.to_string_lossy().into_owned()),
        default_game_dir: default_game_dir_hint().map(|path| path.to_string_lossy().into_owned()),
        game_version,
        hosted_patches: content::hosted_patches().unwrap_or(false),
    };
    log_home_transition(&view);
    view
}

fn log_home_transition(view: &HomeStatusView) {
    let mut previous = LAST_LOGGED_HOME_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *previous == Some(view.state) {
        return;
    }
    *previous = Some(view.state);
    let (state, phase, game, action) = home_state_diagnostics(view.state);
    tracing::info!("");
    tracing::info!("LAUNCHER STATE: {state}");
    tracing::info!("Phase: {phase} -- Game: {game} -- Action: {action}");
}

pub(crate) fn home_state_diagnostics(
    state: HomeLifecycleState,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match state {
        HomeLifecycleState::NoValidInstall => (
            "STATE_NO_VALID_INSTALL",
            "install-required",
            "missing",
            "select-install",
        ),
        HomeLifecycleState::PatchRequired => (
            "STATE_PATCH_REQUIRED",
            "patch-required",
            "needs-patch",
            "update-client",
        ),
        HomeLifecycleState::LoggedOut => ("STATE_LOGGED_OUT", "account-login", "ready", "login"),
        HomeLifecycleState::Ready => ("STATE_READY", "ready", "ready", "launch"),
    }
}

/// Build-time `git describe` identity shown in the About panel.
#[tauri::command]
pub(crate) fn launcher_version() -> &'static str {
    bahamut_launcher::version::LAUNCHER_VERSION
}

/// Loads portable news, falling back to the embedded default so the UI has no error path.
#[tauri::command]
pub(crate) fn list_news() -> Vec<NewsItem> {
    news::list()
}
