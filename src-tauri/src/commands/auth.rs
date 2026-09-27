use bahamut_launcher::auth::client::{AuthClient, AuthClientError, SessionValidation};
use bahamut_launcher::auth::expiry::parse_rfc3339_utc_ms;
use bahamut_launcher::auth::types::{
    ErrorCode, LoginRequest, RegisterRequest, is_contract_session_id,
};
use bahamut_launcher::config::dirs;
use bahamut_launcher::config::launcher_ini::LauncherConfig;
use bahamut_launcher::config::preferences::DisplayMode;
use bahamut_launcher::config::retail_game;
use bahamut_launcher::install_check::{self, InstallState};
use bahamut_launcher::login::dev_token::parse_dev_token;
use bahamut_launcher::patcher;
use bahamut_launcher::platform;
use bahamut_launcher::profiles::ServerProfile;
use tauri::{Manager, State};
use zeroize::Zeroizing;

use crate::extensions;
use crate::presentation::{
    AuthError, AuthOp, LoginShellResponse, RegisterShellResponse, map_launch_error,
};
use crate::shell_config::{
    load_extensions_config, load_launcher_config, load_screenshot_config, resolve_game_dir,
    resolve_game_dir_for_preferences,
};
use crate::state::GameIpcState;

#[tauri::command]
pub(crate) fn game_status(state: State<'_, GameIpcState>) -> bool {
    state.is_active()
}

/// Launch with the retained session's exact profile and issuing endpoint.
#[tauri::command]
pub(crate) async fn launch_game(
    app: tauri::AppHandle,
    state: State<'_, GameIpcState>,
    token: Option<String>,
    server: Option<String>,
    auth_endpoint: Option<String>,
) -> Result<(), AuthError> {
    let reservation = state.begin_launch().ok_or_else(|| {
        if state.is_active() {
            AuthError::with_message("game-running", "The game is already running.")
        } else {
            AuthError::server("A backup restore is in progress. Wait for it to finish.")
        }
    })?;
    let worker_app = app.clone();
    let (close_on_game_start, pid) = tauri::async_runtime::spawn_blocking(move || {
        let (close, game) = launch_game_inner(&worker_app, token, server, auth_endpoint)?;
        let pid = game.pid;
        reservation.complete_launch(game);
        Ok::<_, AuthError>((close, pid))
    })
    .await
    .map_err(|error| AuthError::server(format!("Launch worker failed: {error}")))??;
    tracing::info!(pid, "launcher now owns the active game session");
    if close_on_game_start
        && let Some(window) = app.get_webview_window("main")
        && let Err(error) = window.close()
    {
        tracing::warn!(error = %error, "could not close launcher after game start");
    }
    Ok(())
}

pub(crate) fn launch_game_inner(
    app: &tauri::AppHandle,
    token: Option<String>,
    server: Option<String>,
    auth_endpoint: Option<String>,
) -> Result<(bool, platform::LaunchedGame), AuthError> {
    let raw = token.ok_or_else(|| AuthError::server("no active session"))?;
    let token = parse_dev_token(&raw).map_err(|e| AuthError::server(e.to_string()))?;

    let extension_config = load_extensions_config().map_err(AuthError::server)?;
    let screenshot = load_screenshot_config().map_err(AuthError::server)?;
    // Query monitor geometry before taking the config lock: monitor lookup can wait for the UI thread.
    let monitors = platform::enumerate_borderless_monitors();
    let launcher_resolution = launcher_window_native_resolution(app);
    let _config_write =
        LauncherConfig::write_lock().map_err(|error| AuthError::server(error.to_string()))?;
    let mut config =
        LauncherConfig::load().map_err(|error| AuthError::server(error.to_string()))?;
    let profile = resolve_profile_from_config(&config, server.as_deref()).map_err(|_| {
        AuthError::with_message(
            "session-endpoint-changed",
            "The saved server profile is unavailable. Log in again before launching.",
        )
    })?;
    AuthClient::for_session(&profile.api_base(), auth_endpoint.as_deref())
        .map_err(|error| translate_client_error(error, AuthOp::Login))?;
    let game_dir = resolve_game_dir_for_preferences(&config.preferences)
        .ok_or_else(|| AuthError::bare("no-install"))?;
    if game_dir.is_dir() && patcher::repair::recovery_pending(&game_dir) {
        return Err(AuthError::server(
            "Game repair recovery is required. Choose Repair Install in Settings before launching.",
        ));
    }
    if !patcher::check_game_version(&game_dir) {
        return Err(AuthError::bare("not-patched"));
    }
    let native_resolution = if config.preferences.launcher.native_resolution_override {
        let selected_resolution = selected_borderless_monitor_resolution(
            config.preferences.game.display_mode,
            config.preferences.launcher.borderless_monitor.as_deref(),
            &monitors,
        )?;
        Some(match selected_resolution {
            Some(resolution) => resolution,
            None => launcher_resolution?,
        })
    } else {
        None
    };
    let game_settings = retail_game::prepare_at_paths_with_native_resolution(
        &mut config,
        &dirs::launcher_config_path().map_err(|error| AuthError::server(error.to_string()))?,
        &dirs::retail_config_sys_path().map_err(|error| AuthError::server(error.to_string()))?,
        native_resolution,
    )
    .map_err(|error| {
        AuthError::server(format!("could not prepare retail game settings: {error}"))
    })?;
    drop(_config_write);

    tracing::info!(
        host = %profile.host,
        game_dir = %game_dir.display(),
        game_settings = ?game_settings,
        "Tauri launch_game invoked"
    );

    crate::prerequisites::ensure_x86_vc_runtime_for_game()
        .map_err(|error| AuthError::with_message("prerequisite", error.to_string()))?;

    let extension_artifacts =
        extensions::launch_artifacts(&extension_config, &screenshot).map_err(AuthError::server)?;
    let options = platform::LaunchOptions {
        verbose_wine_debug: config.preferences.developer.enable_verbose_wine_debug,
        display_mode: config.preferences.game.display_mode,
        borderless_monitor: config.preferences.launcher.borderless_monitor.clone(),
        extension_artifacts,
    };

    let game = platform::launch_game(&game_dir, profile.lobby_address(), &token, &options)
        .map_err(map_launch_error)?;

    Ok((config.preferences.launcher.close_on_game_start, game))
}

fn selected_borderless_monitor_resolution(
    display_mode: DisplayMode,
    selected: Option<&str>,
    monitors: &Result<platform::BorderlessMonitorSnapshot, String>,
) -> Result<Option<(u32, u32)>, AuthError> {
    if display_mode != DisplayMode::Borderless {
        return Ok(None);
    }
    let Some(id) = selected else {
        return Ok(None);
    };
    match monitors {
        Ok(snapshot) if snapshot.supported => {
            platform::selected_monitor_resolution(&snapshot.monitors, Some(id))
                .map(Some)
                .ok_or_else(|| {
                    AuthError::server(
                        "could not resolve the selected or fallback monitor resolution",
                    )
                })
        }
        Err(error) if cfg!(target_os = "windows") => Err(AuthError::server(format!(
            "could not enumerate monitors for native resolution override: {error}"
        ))),
        _ => Ok(None),
    }
}

fn launcher_window_native_resolution(app: &tauri::AppHandle) -> Result<(u32, u32), AuthError> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| AuthError::server("could not resolve the launcher window monitor"))?;
    let monitor = window
        .current_monitor()
        .map_err(|error| {
            AuthError::server(format!("could not resolve the launcher monitor: {error}"))
        })?
        .ok_or_else(|| AuthError::server("could not resolve the launcher window monitor"))?;
    let size = monitor.size();
    if size.width == 0 || size.height == 0 {
        return Err(AuthError::server(
            "launcher window monitor reported an invalid physical resolution",
        ));
    }
    Ok((size.width, size.height))
}

/// POSTs credentials to the selected server's `api_base` + `/sessions` per `docs/auth.md`; returns `{ token, authEndpoint, expiresAt }` with milliseconds since the Unix epoch.
#[tauri::command]
pub(crate) async fn login(
    username: String,
    password: String,
    server: Option<String>,
) -> Result<LoginShellResponse, AuthError> {
    match install_check::check_install(resolve_game_dir().as_deref()).state {
        InstallState::NotFound => return Err(AuthError::bare("no-install")),
        InstallState::FoundNeedsPatch { .. } => return Err(AuthError::bare("not-patched")),
        InstallState::Ready => {}
    }

    let password = Zeroizing::new(password);
    tracing::debug!(username = %username, password_len = password.len(), "login invoked");
    let config = load_launcher_config().map_err(AuthError::server)?;
    let profile = resolve_profile_from_config(&config, server.as_deref())?;
    let profile_name = profile.display_name.clone();
    let client = AuthClient::new(&profile.api_base()).map_err(|e| {
        tracing::error!(error = %e, "auth client construction failed");
        translate_client_error(e, AuthOp::Login)
    })?;
    ensure_login_profile_binding(&config, &profile_name, client.endpoint())?;
    let resp = client
        .login(&LoginRequest { username, password })
        .await
        .map_err(|e| {
            tracing::debug!(error = %e, "login request failed");
            translate_client_error(e, AuthOp::Login)
        })?;
    let current_config = load_launcher_config().map_err(AuthError::server)?;
    ensure_login_profile_binding(&current_config, &profile_name, client.endpoint())?;
    let expires_at_ms = parse_rfc3339_utc_ms(&resp.expires_at).map_err(|e| {
        tracing::warn!(raw = %resp.expires_at, error = %e, "server returned unparseable expires_at");
        AuthError::server(format!(
            "server returned an invalid expires_at timestamp: {e}"
        ))
    })?;
    if !is_contract_session_id(&resp.session_id) {
        return Err(AuthError::server(
            "server returned an invalid session_id; expected 56 lowercase hexadecimal characters",
        ));
    }
    Ok(LoginShellResponse {
        token: resp.session_id,
        auth_endpoint: client.endpoint().to_owned(),
        expires_at_ms,
    })
}

fn ensure_login_profile_binding(
    config: &LauncherConfig,
    profile_name: &str,
    issuing_endpoint: &str,
) -> Result<(), AuthError> {
    let changed = || {
        AuthError::with_message(
            "session-endpoint-changed",
            "The selected server changed during login. Log in again.",
        )
    };
    if config.selected_server != profile_name {
        return Err(changed());
    }
    let profile = resolve_profile_from_config(config, Some(profile_name)).map_err(|_| changed())?;
    let current = AuthClient::new(&profile.api_base()).map_err(|_| changed())?;
    if current.endpoint() != issuing_endpoint {
        return Err(changed());
    }
    Ok(())
}

/// POSTs credentials to the selected `api_base` + `/accounts`; a 201 advances registration, and a separate login mints the session because registration returns no token.
#[tauri::command]
pub(crate) async fn register(
    username: String,
    password: String,
    server: Option<String>,
) -> Result<RegisterShellResponse, AuthError> {
    let password = Zeroizing::new(password);
    tracing::debug!(username = %username, password_len = password.len(), "register invoked");
    let profile = resolve_profile(server.as_deref())?;
    let client = AuthClient::new(&profile.api_base()).map_err(|e| {
        tracing::error!(error = %e, "auth client construction failed");
        translate_client_error(e, AuthOp::Register)
    })?;
    client
        .register(&RegisterRequest { username, password })
        .await
        .map_err(|e| {
            tracing::debug!(error = %e, "register request failed");
            translate_client_error(e, AuthOp::Register)
        })?;
    Ok(RegisterShellResponse { ok: true })
}

/// Validates against the selected server; local profile removal is distinct from an inconclusive network result.
#[tauri::command]
pub(crate) async fn validate_session(
    token: String,
    server: Option<String>,
    auth_endpoint: Option<String>,
) -> String {
    let config = match load_launcher_config() {
        Ok(config) => config,
        Err(_) => return "unknown".to_string(),
    };
    let profile = match resolve_profile_from_config(&config, server.as_deref()) {
        Ok(profile) => profile,
        Err(_) => return "missing-profile".to_string(),
    };
    let client = match AuthClient::for_session(&profile.api_base(), auth_endpoint.as_deref()) {
        Ok(client) => client,
        Err(AuthClientError::SessionEndpointMismatch) => return "endpoint-mismatch".to_string(),
        Err(_) => return "unknown".to_string(),
    };
    match client.validate_session(&token).await {
        SessionValidation::Valid => "valid".to_string(),
        SessionValidation::Invalid => "invalid".to_string(),
        SessionValidation::Unknown => "unknown".to_string(),
    }
}

pub(crate) fn resolve_profile(server: Option<&str>) -> Result<ServerProfile, AuthError> {
    let config = load_launcher_config().map_err(AuthError::server)?;
    resolve_profile_from_config(&config, server)
}

pub(crate) fn resolve_profile_from_config(
    config: &LauncherConfig,
    server: Option<&str>,
) -> Result<ServerProfile, AuthError> {
    let requested = server.unwrap_or(&config.selected_server);
    config
        .servers
        .iter()
        .find(|profile| profile.display_name == requested)
        .cloned()
        .ok_or_else(|| AuthError::server("no server configured"))
}

pub(crate) fn translate_client_error(err: AuthClientError, op: AuthOp) -> AuthError {
    match err {
        AuthClientError::SessionEndpointMismatch => AuthError::with_message(
            "session-endpoint-changed",
            "The server address changed. Log in again before launching.",
        ),
        AuthClientError::Transport(_) => AuthError::with_message(
            "network",
            "Could not reach the server. Check your connection or the server address.",
        ),
        AuthClientError::Malformed(msg) => AuthError::server(msg),
        AuthClientError::BadUrl { url, reason } => {
            AuthError::server(format!("invalid api_base URL {url:?}: {reason}"))
        }
        AuthClientError::InsecureNonLoopback(host) => AuthError::server(format!(
            "refusing to send credentials over plain HTTP to non-loopback host {host:?}"
        )),
        AuthClientError::Api {
            status,
            code,
            message,
            known,
            retry_after_seconds,
        } => match known {
            Some(ErrorCode::InvalidCredentials) if op == AuthOp::Login => {
                AuthError::invalid_credentials()
            }
            Some(ErrorCode::UsernameTaken) if op == AuthOp::Register => {
                AuthError::bare("username-taken")
            }
            Some(ErrorCode::ValidationError) => {
                let lower = message.to_lowercase();
                let kind: &'static str = if lower.contains("username") {
                    "username-invalid"
                } else if lower.contains("password") {
                    "password-invalid"
                } else {
                    "server"
                };
                AuthError::with_message(kind, message)
            }
            Some(ErrorCode::RateLimited) if status == 429 => {
                AuthError::rate_limited(message, retry_after_seconds)
            }
            Some(ErrorCode::ServerError) if op == AuthOp::Register && status == 500 => {
                AuthError::bare("create-failed")
            }
            _ => AuthError::server(format!("HTTP {status}: {code} ({message})")),
        },
    }
}

/// Always returns success to let the UI leave the session; server-side revoke is best-effort.
#[tauri::command]
pub(crate) async fn logout(
    token: Option<String>,
    server: Option<String>,
    auth_endpoint: Option<String>,
) -> Result<(), String> {
    let token = match token {
        Some(t) if !t.is_empty() => t,
        _ => return Ok(()),
    };
    let profile = match resolve_profile(server.as_deref()) {
        Ok(profile) => profile,
        Err(_) => {
            tracing::info!("logout: no server configured; skipping server-side revoke");
            return Ok(());
        }
    };
    let client = match AuthClient::for_session(&profile.api_base(), auth_endpoint.as_deref()) {
        Ok(client) => client,
        Err(err) => {
            tracing::warn!(error = %err, "logout: could not build auth client");
            return Ok(());
        }
    };
    if let Err(err) = client.logout(&token).await {
        tracing::info!(error = %err, "logout: server-side revoke failed (best-effort)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_completion_requires_the_original_selected_profile_and_endpoint() {
        let config = LauncherConfig::defaults();
        let original = config.servers[0].clone();
        let issuing = AuthClient::new(&original.api_base()).unwrap();
        let profile_name = original.display_name.clone();
        let issuing_endpoint = issuing.endpoint();

        assert!(ensure_login_profile_binding(&config, &profile_name, issuing_endpoint).is_ok());

        let mut edited = config.clone();
        let profile = edited
            .servers
            .iter_mut()
            .find(|profile| profile.display_name == profile_name)
            .unwrap();
        profile.host = "127.0.0.1".into();
        profile.auth_port = 8080;
        profile.use_https = false;
        assert_eq!(
            ensure_login_profile_binding(&edited, &profile_name, issuing_endpoint)
                .unwrap_err()
                .kind,
            "session-endpoint-changed"
        );

        let mut renamed = config.clone();
        renamed.servers[0].display_name = "Renamed server".into();
        renamed.selected_server = "Renamed server".into();
        assert_eq!(
            ensure_login_profile_binding(&renamed, &profile_name, issuing_endpoint)
                .unwrap_err()
                .kind,
            "session-endpoint-changed"
        );

        let mut removed = config.clone();
        removed
            .servers
            .retain(|profile| profile.display_name != profile_name);
        removed.selected_server = removed.servers[0].display_name.clone();
        assert_eq!(
            ensure_login_profile_binding(&removed, &profile_name, issuing_endpoint)
                .unwrap_err()
                .kind,
            "session-endpoint-changed"
        );

        let mut default_changed = config;
        default_changed.selected_server = default_changed.servers[1].display_name.clone();
        assert_eq!(
            ensure_login_profile_binding(&default_changed, &profile_name, issuing_endpoint)
                .unwrap_err()
                .kind,
            "session-endpoint-changed"
        );
    }

    #[test]
    fn saved_monitor_identity_only_changes_native_resolution_in_borderless_mode() {
        let monitors = Ok(platform::BorderlessMonitorSnapshot {
            supported: true,
            monitors: vec![
                platform::BorderlessMonitor {
                    id: "primary".into(),
                    name: "Primary".into(),
                    width: 1920,
                    height: 1080,
                    primary: true,
                },
                platform::BorderlessMonitor {
                    id: "saved-secondary".into(),
                    name: "Secondary".into(),
                    width: 2560,
                    height: 1440,
                    primary: false,
                },
            ],
        });

        for display_mode in [DisplayMode::Windowed, DisplayMode::FullScreen] {
            assert!(
                selected_borderless_monitor_resolution(
                    display_mode,
                    Some("saved-secondary"),
                    &monitors,
                )
                .unwrap()
                .is_none()
            );
        }
        assert_eq!(
            selected_borderless_monitor_resolution(
                DisplayMode::Borderless,
                Some("saved-secondary"),
                &monitors,
            )
            .unwrap(),
            Some((2560, 1440))
        );
    }
}
