use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use thiserror::Error;

use crate::config::preferences::{DisplayMode, ScreenshotFormat, ScreenshotHotkey};
use crate::extensions::ExtensionLaunchRequest;
use crate::launcher::launch_args::LaunchArgsError;
use crate::launcher::pe_patch::{PePatch, PePatchError};

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("ffxivgame.exe was not found inside {0}")]
    MissingClientBinary(PathBuf),

    #[error("ffxivconfig.exe was not found inside {0}")]
    MissingConfigBinary(PathBuf),

    #[error(transparent)]
    PatchPlanning(#[from] PePatchError),

    #[error(transparent)]
    LaunchArgs(#[from] LaunchArgsError),

    #[error("command-line argument contains an interior NUL byte: {0}")]
    ArgsContainNul(String),

    #[error("could not derive client image base from suspended thread context")]
    ImageBaseUnknown,

    #[error("patch verification mismatch at RVA 0x{rva:08X}")]
    PatchVerifyMismatch { rva: u32 },

    #[error("{context}: {source}")]
    WindowsApi {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    WineNotFound(String),

    #[error("{0}")]
    WinePrefix(String),

    #[error("{context}: {source}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("could not parse client PE: {0}")]
    PeParse(String),

    #[error("patch RVA 0x{0:08X} is not mapped to any PE section")]
    RvaUnmapped(u32),

    #[error("{0}")]
    WineExit(String),

    #[error("{0}")]
    Wine(String),

    #[error("platform launch is not implemented for this OS yet")]
    Unsupported,

    #[error(transparent)]
    ExtensionPlan(#[from] crate::extensions::LaunchPlanError),

    #[error(transparent)]
    ExtensionBootstrap(#[from] crate::extensions::HelperFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionArtifacts {
    pub helper_path: PathBuf,
    pub client_module_path: PathBuf,
    pub addon_manifests: Vec<PathBuf>,
    pub addon_catalog: Vec<PathBuf>,
    pub addon_settings_path: PathBuf,
    pub chat_logs_path: PathBuf,
    pub dat_roots: Vec<PathBuf>,
    pub startup_script_path: PathBuf,
    pub screenshot_plugin_path: PathBuf,
    pub screenshots_path: PathBuf,
    pub screenshot_format: ScreenshotFormat,
    pub screenshot_hide_overlays: bool,
    pub screenshot_hotkey: ScreenshotHotkey,
    pub screenshot_enabled: bool,
    pub discord_rpc_plugin_path: PathBuf,
    pub discord_rpc_application_id: String,
    pub discord_rpc_enabled: bool,
    pub object_distance_enabled: bool,
    pub object_distance_percent: u16,
    pub camera_zoom_enabled: bool,
    pub camera_zoom_limit: u8,
}

impl ExtensionArtifacts {
    /// Build the shared helper request from `plan_contract_patches` output (server-UTC, then lobby host).
    pub fn to_request(
        &self,
        game_dir: &Path,
        options: &LaunchOptions,
        contract_patches: &[PePatch; 2],
        launch_argument: String,
    ) -> ExtensionLaunchRequest {
        ExtensionLaunchRequest {
            game_dir: game_dir.to_path_buf(),
            helper_path: self.helper_path.clone(),
            client_module_path: self.client_module_path.clone(),
            addon_manifests: self.addon_manifests.clone(),
            addon_catalog: self.addon_catalog.clone(),
            addon_settings_path: self.addon_settings_path.clone(),
            chat_logs_path: self.chat_logs_path.clone(),
            dat_roots: self.dat_roots.clone(),
            startup_script_path: self.startup_script_path.clone(),
            screenshot_plugin_path: self.screenshot_plugin_path.clone(),
            screenshots_path: self.screenshots_path.clone(),
            screenshot_format: self.screenshot_format,
            screenshot_hide_overlays: self.screenshot_hide_overlays,
            screenshot_hotkey: self.screenshot_hotkey,
            screenshot_enabled: self.screenshot_enabled,
            discord_rpc_plugin_path: self.discord_rpc_plugin_path.clone(),
            discord_rpc_application_id: self.discord_rpc_application_id.clone(),
            discord_rpc_enabled: self.discord_rpc_enabled,
            object_distance_enabled: self.object_distance_enabled,
            object_distance_percent: self.object_distance_percent,
            camera_zoom_enabled: self.camera_zoom_enabled,
            camera_zoom_limit: self.camera_zoom_limit,
            display_mode: options.display_mode,
            borderless_monitor: options.borderless_monitor.clone(),
            server_utc_patch: contract_patches[0].bytes.clone(),
            lobby_host_patch: contract_patches[1].bytes.clone(),
            launch_argument,
        }
    }
}

/// Launch options shared by the platform backends.
#[derive(Debug, Clone, Default)]
pub struct LaunchOptions {
    /// Enable verbose Wine debug channels in the per-launch log.
    pub verbose_wine_debug: bool,
    /// Selects the persisted client display mode for runtime launch behavior.
    pub display_mode: DisplayMode,
    /// Optional Windows display-interface identity used by borderless mode.
    pub borderless_monitor: Option<String>,
    /// `None` launches the game without extensions.
    pub extension_artifacts: Option<ExtensionArtifacts>,
}

mod monitors;
pub use monitors::{
    BorderlessMonitor, BorderlessMonitorSnapshot, enumerate_borderless_monitors,
    selected_monitor_resolution,
};

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "windows")]
pub use windows::{detect_game_install, launch_config_tool, launch_game};

#[cfg(not(target_os = "windows"))]
pub fn launch_config_tool(_game_dir: &std::path::Path) -> Result<u32, LaunchError> {
    Err(LaunchError::Unsupported)
}

/// One client process launched by this launcher and its authoritative exit signal.
pub struct LaunchedGame {
    pub pid: u32,
    exited: Receiver<()>,
}

impl LaunchedGame {
    /// Creates the launch result and the one sender owned by its process waiter.
    pub fn pending(pid: u32) -> (Self, Sender<()>) {
        let (exited, receiver) = mpsc::channel();
        (
            Self {
                pid,
                exited: receiver,
            },
            exited,
        )
    }

    /// Returns true only after the process waiter authoritatively observed exit.
    pub fn has_exited(&self) -> bool {
        self.exited.try_recv().is_ok()
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod wine;

#[cfg(any(target_os = "linux", target_os = "macos", test))]
mod runtime_archive;

#[cfg(any(target_os = "linux", all(test, unix)))]
mod wine_engine;

#[cfg(target_os = "linux")]
mod dxvk;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod on_disk_patch;

#[cfg(target_os = "linux")]
pub use linux::{detect_game_install, launch_game};

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{detect_game_install, launch_game};

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod stub {
    use super::*;
    use std::path::Path;

    pub fn launch_game(
        _game_dir: &Path,
        _lobby_host: &str,
        _session_id: &str,
        _options: &LaunchOptions,
    ) -> Result<LaunchedGame, LaunchError> {
        Err(LaunchError::Unsupported)
    }

    pub fn detect_game_install() -> Option<PathBuf> {
        None
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub use stub::{detect_game_install, launch_game};
