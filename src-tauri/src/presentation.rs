use bahamut_launcher::config::launcher_ini::LauncherConfig;
use bahamut_launcher::config::preferences::{
    AudioSettings, DisplayMode, GameSettings, GraphicsSettings, Multisampling,
    SUPPORTED_RESOLUTIONS, ShadowDetail, TextureFiltering, TextureQuality,
};
use bahamut_launcher::content::Phase;
use bahamut_launcher::install_check::InstallState;
use bahamut_launcher::platform::{self, BorderlessMonitorSnapshot};
use bahamut_launcher::profiles::ServerProfile;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct BorderlessMonitorItemView {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) primary: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct BorderlessMonitorView {
    pub(crate) supported: bool,
    pub(crate) monitors: Vec<BorderlessMonitorItemView>,
    pub(crate) selected: Option<String>,
}

pub(crate) fn borderless_monitor_view(
    snapshot: BorderlessMonitorSnapshot,
    selected: Option<String>,
) -> BorderlessMonitorView {
    BorderlessMonitorView {
        supported: snapshot.supported,
        monitors: snapshot
            .monitors
            .into_iter()
            .filter(|monitor| !monitor.id.is_empty())
            .map(|monitor| BorderlessMonitorItemView {
                id: monitor.id,
                name: monitor.name,
                width: monitor.width,
                height: monitor.height,
                primary: monitor.primary,
            })
            .collect(),
        selected,
    }
}

#[cfg(test)]
mod borderless_monitor_view_tests {
    use super::*;
    use bahamut_launcher::platform::BorderlessMonitor;

    #[test]
    fn idless_fallback_geometry_is_not_exposed_as_a_selectable_monitor() {
        let view = borderless_monitor_view(
            BorderlessMonitorSnapshot {
                supported: true,
                monitors: vec![
                    BorderlessMonitor {
                        id: String::new(),
                        name: "Unaddressable primary".into(),
                        width: 1920,
                        height: 1080,
                        primary: true,
                    },
                    BorderlessMonitor {
                        id: "\\\\?\\DISPLAY#SECONDARY".into(),
                        name: "Secondary".into(),
                        width: 2560,
                        height: 1440,
                        primary: false,
                    },
                ],
            },
            None,
        );

        assert_eq!(view.monitors.len(), 1);
        assert_eq!(view.monitors[0].id, "\\\\?\\DISPLAY#SECONDARY");
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct LauncherLogView {
    pub(crate) content: String,
    pub(crate) log_path: String,
    pub(crate) truncated: bool,
    pub(crate) updated_at: Option<u64>,
}

/// One-moment install snapshot returned to the WebView; JS redraws it on a fixed cadence.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct InstallStatusView {
    pub(crate) phase: &'static str,
    pub(crate) download_idx: usize,
    pub(crate) file_idx: usize,
    pub(crate) total_files: usize,
    pub(crate) bytes_downloaded: u64,
    pub(crate) previous_completed_bytes: u64,
    pub(crate) total_download_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    pub(crate) is_running: bool,
    pub(crate) is_paused: bool,
    pub(crate) pause_requested: bool,
    pub(crate) is_terminal: bool,
}

impl InstallStatusView {
    pub(crate) fn idle() -> Self {
        Self {
            phase: "idle",
            download_idx: 0,
            file_idx: 0,
            total_files: 0,
            bytes_downloaded: 0,
            previous_completed_bytes: 0,
            total_download_bytes: 0,
            error: None,
            is_running: false,
            is_paused: false,
            pause_requested: false,
            is_terminal: false,
        }
    }
}

pub(crate) fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Starting => "starting",
        Phase::Downloading => "downloading",
        Phase::Installing => "installing",
        Phase::ValidatingFiles => "validating-files",
        Phase::Done => "done",
        Phase::Error => "error",
        Phase::Cancelled => "cancelled",
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum HomeLifecycleState {
    NoValidInstall,
    OutdatedInstall,
    LoggedOut,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HomePresentation {
    pub(crate) eyebrow: &'static str,
    pub(crate) title: &'static str,
    pub(crate) primary_action: &'static str,
}

pub(crate) fn resolve_home_lifecycle(
    install: &InstallState,
    authenticated: bool,
) -> HomeLifecycleState {
    match install {
        InstallState::NotFound => HomeLifecycleState::NoValidInstall,
        InstallState::FoundOutdated { .. } => HomeLifecycleState::OutdatedInstall,
        InstallState::Ready if authenticated => HomeLifecycleState::Ready,
        InstallState::Ready => HomeLifecycleState::LoggedOut,
    }
}

pub(crate) fn home_presentation(state: HomeLifecycleState) -> HomePresentation {
    match state {
        HomeLifecycleState::NoValidInstall => HomePresentation {
            eyebrow: "",
            title: "Account Login",
            primary_action: "Install",
        },
        HomeLifecycleState::OutdatedInstall => HomePresentation {
            eyebrow: "",
            title: "Account Login",
            primary_action: "Install",
        },
        HomeLifecycleState::LoggedOut => HomePresentation {
            eyebrow: "",
            title: "Account Login",
            primary_action: "Play",
        },
        HomeLifecycleState::Ready => HomePresentation {
            eyebrow: "",
            title: "Account Login",
            primary_action: "Play",
        },
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HomeStatusView {
    pub(crate) state: HomeLifecycleState,
    pub(crate) eyebrow: &'static str,
    pub(crate) title: &'static str,
    pub(crate) primary_action: &'static str,
    pub(crate) game_dir: Option<String>,
    pub(crate) default_game_dir: Option<String>,
    pub(crate) game_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ExtensionCommandView {
    pub(crate) name: String,
    pub(crate) usage: String,
    pub(crate) description: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ExtensionItemView {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) author: String,
    pub(crate) version: String,
    pub(crate) description: String,
    pub(crate) homepage: Option<String>,
    pub(crate) commands: Vec<ExtensionCommandView>,
    pub(crate) capabilities: Vec<String>,
    pub(crate) compatibility: String,
    pub(crate) status: String,
    pub(crate) trust: &'static str,
    pub(crate) enabled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct OverlayPackageView {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) author: String,
    pub(crate) version: String,
    pub(crate) description: String,
    pub(crate) homepage: Option<String>,
    pub(crate) enabled: bool,
    pub(crate) status: String,
    pub(crate) trust: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct OverlayConflictView {
    pub(crate) relative_path: String,
    pub(crate) package_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct ExtensionInventoryView {
    pub(crate) addons: Vec<ExtensionItemView>,
    pub(crate) plugins: Vec<ExtensionItemView>,
    pub(crate) overlays: Vec<OverlayPackageView>,
    pub(crate) overlay_conflicts: Vec<OverlayConflictView>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ServerSettingsView {
    pub(crate) selected_server: String,
    pub(crate) servers: Vec<ServerProfile>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct GameSettingsView {
    pub(crate) available: bool,
    pub(crate) settings: GameSettingsPayload,
    pub(crate) supported_resolutions: Vec<[u32; 2]>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct LauncherBehaviorView {
    pub(crate) close_on_game_start: bool,
    pub(crate) native_resolution_override: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct GameSettingsPayload {
    pub(crate) display_mode: DisplayMode,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) graphics: GraphicsSettingsPayload,
    pub(crate) audio: AudioSettingsPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct GraphicsSettingsPayload {
    pub(crate) multisampling: Multisampling,
    pub(crate) general_quality: u8,
    pub(crate) background_quality: u8,
    pub(crate) shadow_detail: ShadowDetail,
    pub(crate) ambient_occlusion: bool,
    pub(crate) depth_of_field: bool,
    pub(crate) cutscene_effects: bool,
    pub(crate) hardware_mouse: bool,
    pub(crate) texture_quality: TextureQuality,
    pub(crate) texture_filtering: TextureFiltering,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AudioSettingsPayload {
    pub(crate) enabled: bool,
    pub(crate) play_in_background: bool,
}

impl From<GameSettings> for GameSettingsPayload {
    fn from(settings: GameSettings) -> Self {
        Self {
            display_mode: settings.display_mode,
            width: settings.width,
            height: settings.height,
            graphics: settings.graphics.into(),
            audio: AudioSettingsPayload {
                enabled: settings.audio.enabled,
                play_in_background: settings.audio.play_in_background,
            },
        }
    }
}

impl From<GraphicsSettings> for GraphicsSettingsPayload {
    fn from(settings: GraphicsSettings) -> Self {
        Self {
            multisampling: settings.multisampling,
            general_quality: settings.general_quality,
            background_quality: settings.background_quality,
            shadow_detail: settings.shadow_detail,
            ambient_occlusion: settings.ambient_occlusion,
            depth_of_field: settings.depth_of_field,
            cutscene_effects: settings.cutscene_effects,
            hardware_mouse: settings.hardware_mouse,
            texture_quality: settings.texture_quality,
            texture_filtering: settings.texture_filtering,
        }
    }
}

impl GameSettingsPayload {
    pub(crate) fn into_settings(self) -> GameSettings {
        GameSettings {
            initialized: true,
            display_mode: self.display_mode,
            width: self.width,
            height: self.height,
            graphics: GraphicsSettings {
                multisampling: self.graphics.multisampling,
                general_quality: self.graphics.general_quality,
                background_quality: self.graphics.background_quality,
                shadow_detail: self.graphics.shadow_detail,
                ambient_occlusion: self.graphics.ambient_occlusion,
                depth_of_field: self.graphics.depth_of_field,
                cutscene_effects: self.graphics.cutscene_effects,
                hardware_mouse: self.graphics.hardware_mouse,
                texture_quality: self.graphics.texture_quality,
                texture_filtering: self.graphics.texture_filtering,
            },
            audio: AudioSettings {
                enabled: self.audio.enabled,
                play_in_background: self.audio.play_in_background,
            },
        }
    }
}

pub(crate) fn game_settings_view(settings: GameSettings) -> GameSettingsView {
    GameSettingsView {
        available: settings.initialized,
        settings: settings.into(),
        supported_resolutions: SUPPORTED_RESOLUTIONS
            .iter()
            .map(|&(width, height)| [width, height])
            .collect(),
    }
}

pub(crate) fn replace_game_settings(
    config: &mut LauncherConfig,
    settings: GameSettingsPayload,
) -> Result<(), String> {
    if !config.preferences.game.initialized {
        return Err(
            "game settings are unavailable until a valid retail config.sys is found".into(),
        );
    }
    let settings = settings.into_settings();
    settings.validate().map_err(|error| error.to_string())?;
    config.preferences.game = settings;
    Ok(())
}

/// Install information exposed to the frontend.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct GameInstallInfo {
    /// Preference override wins over registry detection for the launch path.
    pub(crate) detected: Option<String>,
    /// Source is `preferences`, `registry`, or `none`.
    pub(crate) source: String,
}

/// Frontend error envelope: `{ kind, message?, retryAfter? }`; `kind` selects the alert path.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AuthError {
    pub(crate) kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "retryAfter")]
    pub(crate) retry_after: Option<u32>,
}

pub(crate) const AUTH_KIND_INVALID_CREDENTIALS: &str = "invalid-credentials";
pub(crate) const AUTH_KIND_RATE_LIMITED: &str = "rate-limited";

impl AuthError {
    pub(crate) fn server(msg: impl Into<String>) -> Self {
        Self {
            kind: "server",
            message: Some(msg.into()),
            retry_after: None,
        }
    }

    pub(crate) fn bare(kind: &'static str) -> Self {
        Self {
            kind,
            message: None,
            retry_after: None,
        }
    }

    pub(crate) fn with_message(kind: &'static str, msg: impl Into<String>) -> Self {
        Self {
            kind,
            message: Some(msg.into()),
            retry_after: None,
        }
    }

    pub(crate) fn invalid_credentials() -> Self {
        Self::bare(AUTH_KIND_INVALID_CREDENTIALS)
    }

    pub(crate) fn rate_limited(message: impl Into<String>, retry_after: Option<u32>) -> Self {
        Self {
            kind: AUTH_KIND_RATE_LIMITED,
            message: Some(message.into()),
            retry_after,
        }
    }
}

/// Maps launch failures to the frontend's `kind` envelope; patch failures use `patch`, missing binaries use `no-install` or `server`.
pub(crate) fn map_launch_error(err: platform::LaunchError) -> AuthError {
    use platform::LaunchError::*;
    match err {
        MissingClientBinary(_) => AuthError::bare("no-install"),
        e @ MissingConfigBinary(_) => AuthError::server(e.to_string()),
        e @ (PatchPlanning(_)
        | ImageBaseUnknown
        | PatchVerifyMismatch { .. }
        | PeParse(_)
        | RvaUnmapped(_)) => AuthError::with_message("patch", e.to_string()),
        e @ (LaunchArgs(_)
        | ArgsContainNul(_)
        | WindowsApi { .. }
        | WineNotFound(_)
        | WinePrefix(_)
        | Io { .. }
        | WineExit(_)
        | Wine(_)
        | Unsupported) => AuthError::server(e.to_string()),
        e @ (ExtensionPlan(_) | ExtensionBootstrap(_)) => {
            AuthError::with_message("extensions", e.to_string())
        }
    }
}

/// Selects the frontend validation-error `kind` set for login versus registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthOp {
    Login,
    Register,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LoginShellResponse {
    pub(crate) token: String,
    #[serde(rename = "authEndpoint")]
    pub(crate) auth_endpoint: String,
    #[serde(rename = "expiresAt")]
    pub(crate) expires_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RegisterShellResponse {
    pub(crate) ok: bool,
}
