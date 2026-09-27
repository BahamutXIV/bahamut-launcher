//! Extension-enabled launch planning without process spawning.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::config::preferences::{DisplayMode, ScreenshotFormat, ScreenshotHotkey};
use crate::launcher::pe_patch::PePatch;

use super::helper::{HelperPlanError, path_arg, reject_empty, validate_helper_path};
use super::identity::{IdentityGateError, IdentityMismatch, gate_install};

pub const ARG_CLIENT: &str = "--client";
pub const ARG_MODULE: &str = "--module";
pub const ARG_SERVER_UTC: &str = "--server-utc";
pub const ARG_LOBBY_HOST: &str = "--lobby-host";
pub const ARG_LAUNCH_ARGUMENT: &str = "--launch-argument";
pub const ARG_PATCH: &str = "--patch";
pub const ARG_TIMEOUT_MS: &str = "--timeout-ms";
pub const ARG_WAIT_FOR_CLIENT: &str = "--wait-for-client";
pub const ADDON_MANIFESTS_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_ADDON_MANIFESTS";
pub const ADDON_CATALOG_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_ADDON_CATALOG";
pub const ADDON_SETTINGS_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_ADDON_SETTINGS";
pub const CHAT_LOGS_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_CHAT_LOGS";
pub const DAT_PACKAGE_ROOTS_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_DAT_PACKAGE_ROOTS";
pub const STARTUP_SCRIPT_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_STARTUP_SCRIPT";
pub const SCREENSHOT_PLUGIN_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_SCREENSHOT_PLUGIN";
pub const SCREENSHOTS_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_SCREENSHOTS";
pub const SCREENSHOT_FORMAT_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_SCREENSHOT_FORMAT";
pub const SCREENSHOT_HIDE_OVERLAYS_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_SCREENSHOT_HIDE_OVERLAYS";
pub const SCREENSHOT_HOTKEY_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_SCREENSHOT_HOTKEY";
pub const SCREENSHOT_ENABLED_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_SCREENSHOT_ENABLED";
pub const DISCORD_RPC_PLUGIN_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_DISCORD_PLUGIN";
pub const DISCORD_RPC_APPLICATION_ID_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_DISCORD_APPLICATION_ID";
pub const DISCORD_RPC_ENABLED_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_DISCORD_ENABLED";
pub const OBJECT_DISTANCE_ENABLED_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_OBJECT_DISTANCE_ENABLED";
pub const OBJECT_DISTANCE_PERCENT_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_OBJECT_DISTANCE_PERCENT";
pub const CAMERA_ZOOM_ENABLED_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_CAMERA_ZOOM_ENABLED";
pub const CAMERA_ZOOM_LIMIT_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_CAMERA_ZOOM_LIMIT";
pub const BORDERLESS_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_BORDERLESS";
pub const BORDERLESS_MONITOR_ENVIRONMENT: &str = "BAHAMUT_RUNTIME_BORDERLESS_MONITOR";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionLaunchRequest {
    pub game_dir: PathBuf,
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
    pub display_mode: DisplayMode,
    pub borderless_monitor: Option<String>,
    pub server_utc_patch: Vec<u8>,
    pub lobby_host_patch: Vec<u8>,
    pub launch_argument: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperInvocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
    pub current_dir: PathBuf,
}

/// Translates a host path into the path the x86 helper sees, or fails with
/// [`HelperPlanError::GuestPath`]. Native Windows uses the identity mapping.
pub type GuestPathMapper<'a> = &'a dyn Fn(&'static str, &Path) -> Result<OsString, HelperPlanError>;

#[derive(Debug, thiserror::Error)]
pub enum LaunchPlanError {
    #[error(transparent)]
    Identity(#[from] IdentityGateError),
    #[error(transparent)]
    IdentityMismatch(#[from] IdentityMismatch),
    #[error(transparent)]
    HelperArguments(#[from] HelperPlanError),
}

pub fn plan_extension_launch(
    request: &ExtensionLaunchRequest,
) -> Result<HelperInvocation, LaunchPlanError> {
    gate_install(&request.game_dir)?;
    plan_verified_extension_launch(request)
}

/// Plan the helper for a Wine backend: `program` and `current_dir` stay host
/// paths for `wine`, every path the helper reads goes through `guest`, and the
/// Wine-only `--patch`, `--timeout-ms`, and `--wait-for-client` arguments follow
/// the shared ten. Wine has no borderless monitor contract, so a borderless
/// request is planned with windowed semantics.
pub fn plan_wine_extension_launch(
    request: &ExtensionLaunchRequest,
    guest: GuestPathMapper<'_>,
    stability_patches: &[PePatch],
    ready_timeout_ms: u32,
) -> Result<HelperInvocation, LaunchPlanError> {
    gate_install(&request.game_dir)?;
    plan_verified_wine_extension_launch(request, guest, stability_patches, ready_timeout_ms)
}

fn plan_verified_wine_extension_launch(
    request: &ExtensionLaunchRequest,
    guest: GuestPathMapper<'_>,
    stability_patches: &[PePatch],
    ready_timeout_ms: u32,
) -> Result<HelperInvocation, LaunchPlanError> {
    let mut request = request.clone();
    if request.display_mode == DisplayMode::Borderless {
        request.display_mode = DisplayMode::Windowed;
    }
    request.borderless_monitor = None;
    let mut invocation = plan_verified_extension_launch_mapped(&request, guest)?;
    for patch in stability_patches {
        reject_patch("stability patch", &patch.bytes)?;
        invocation.args.push(ARG_PATCH.into());
        invocation.args.push(encode_patch_argument(patch).into());
    }
    invocation.args.push(ARG_TIMEOUT_MS.into());
    invocation.args.push(ready_timeout_ms.to_string().into());
    invocation.args.push(ARG_WAIT_FOR_CLIENT.into());
    Ok(invocation)
}

/// [`plan_wine_extension_launch`] past the identity gate, for backend tests without a retail client.
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
pub(crate) fn plan_wine_extension_launch_ungated(
    request: &ExtensionLaunchRequest,
    guest: GuestPathMapper<'_>,
    stability_patches: &[PePatch],
    ready_timeout_ms: u32,
) -> Result<HelperInvocation, LaunchPlanError> {
    plan_verified_wine_extension_launch(request, guest, stability_patches, ready_timeout_ms)
}

/// Encode one `--patch` value as `<RVA>:<bytes>`, both uppercase hex without `0x`; the RVA is eight digits.
pub fn encode_patch_argument(patch: &PePatch) -> String {
    format!("{:08X}:{}", patch.rva, encode_hex(&patch.bytes))
}

fn host_path(_field: &'static str, path: &Path) -> Result<OsString, HelperPlanError> {
    Ok(path.as_os_str().to_owned())
}

fn plan_verified_extension_launch(
    request: &ExtensionLaunchRequest,
) -> Result<HelperInvocation, LaunchPlanError> {
    plan_verified_extension_launch_mapped(request, &host_path)
}

fn plan_verified_extension_launch_mapped(
    request: &ExtensionLaunchRequest,
    guest: GuestPathMapper<'_>,
) -> Result<HelperInvocation, LaunchPlanError> {
    validate_helper_path(&request.helper_path)?;
    let client_path = guest_path_arg(guest, "client", &request.game_dir.join("ffxivgame.exe"))?;
    let client_module_path = guest_path_arg(guest, "client module", &request.client_module_path)?;
    let startup_script_path =
        guest_path_arg(guest, "startup script", &request.startup_script_path)?;
    let screenshots_path = guest_path_arg(guest, "screenshots", &request.screenshots_path)?;
    let addon_settings_path =
        guest_path_arg(guest, "addon settings", &request.addon_settings_path)?;
    let chat_logs_path = guest_path_arg(guest, "chat logs", &request.chat_logs_path)?;
    let screenshot_plugin_path =
        guest_path_arg(guest, "screenshot plugin", &request.screenshot_plugin_path)?;
    let discord_rpc_plugin_path =
        guest_path_arg(guest, "DiscordRPC plugin", &request.discord_rpc_plugin_path)?;
    reject_empty(
        "Discord application id",
        &request.discord_rpc_application_id,
    )?;
    reject_patch("server-utc", &request.server_utc_patch)?;
    reject_patch("lobby-host", &request.lobby_host_patch)?;
    reject_empty("launch-argument", &request.launch_argument)?;
    let current_dir = path_arg("working directory", &request.game_dir)?;

    let mut args = Vec::with_capacity(10);
    push_path_arg(&mut args, ARG_CLIENT, client_path);
    push_path_arg(&mut args, ARG_MODULE, client_module_path);
    args.push(ARG_SERVER_UTC.into());
    args.push(encode_hex(&request.server_utc_patch).into());
    args.push(ARG_LOBBY_HOST.into());
    args.push(encode_hex(&request.lobby_host_patch).into());
    args.push(ARG_LAUNCH_ARGUMENT.into());
    args.push(request.launch_argument.clone().into());
    let mut environment = vec![(STARTUP_SCRIPT_ENVIRONMENT.into(), startup_script_path)];
    environment.push((SCREENSHOT_PLUGIN_ENVIRONMENT.into(), screenshot_plugin_path));
    environment.push((SCREENSHOTS_ENVIRONMENT.into(), screenshots_path));
    environment.push((
        SCREENSHOT_FORMAT_ENVIRONMENT.into(),
        request.screenshot_format.to_string().into(),
    ));
    environment.push((
        SCREENSHOT_HIDE_OVERLAYS_ENVIRONMENT.into(),
        request.screenshot_hide_overlays.to_string().into(),
    ));
    environment.push((
        SCREENSHOT_HOTKEY_ENVIRONMENT.into(),
        request.screenshot_hotkey.to_string().into(),
    ));
    environment.push((
        SCREENSHOT_ENABLED_ENVIRONMENT.into(),
        request.screenshot_enabled.to_string().into(),
    ));
    environment.push((
        DISCORD_RPC_PLUGIN_ENVIRONMENT.into(),
        discord_rpc_plugin_path,
    ));
    environment.push((
        DISCORD_RPC_APPLICATION_ID_ENVIRONMENT.into(),
        request.discord_rpc_application_id.clone().into(),
    ));
    environment.push((
        DISCORD_RPC_ENABLED_ENVIRONMENT.into(),
        request.discord_rpc_enabled.to_string().into(),
    ));
    environment.push((
        ADDON_CATALOG_ENVIRONMENT.into(),
        encode_path_list(guest, "addon catalog manifest", &request.addon_catalog)?,
    ));
    environment.push((
        ADDON_MANIFESTS_ENVIRONMENT.into(),
        encode_path_list(guest, "addon manifest", &request.addon_manifests)?,
    ));
    environment.push((ADDON_SETTINGS_ENVIRONMENT.into(), addon_settings_path));
    environment.push((CHAT_LOGS_ENVIRONMENT.into(), chat_logs_path));
    environment.push((
        DAT_PACKAGE_ROOTS_ENVIRONMENT.into(),
        encode_path_list(guest, "DAT overlay root", &request.dat_roots)?,
    ));
    environment.push((
        BORDERLESS_ENVIRONMENT.into(),
        (request.display_mode == DisplayMode::Borderless)
            .to_string()
            .into(),
    ));
    environment.push((
        BORDERLESS_MONITOR_ENVIRONMENT.into(),
        if request.display_mode == DisplayMode::Borderless {
            request
                .borderless_monitor
                .clone()
                .unwrap_or_default()
                .into()
        } else {
            String::new().into()
        },
    ));
    environment.push((
        OBJECT_DISTANCE_ENABLED_ENVIRONMENT.into(),
        request.object_distance_enabled.to_string().into(),
    ));
    environment.push((
        OBJECT_DISTANCE_PERCENT_ENVIRONMENT.into(),
        request.object_distance_percent.to_string().into(),
    ));
    environment.push((
        CAMERA_ZOOM_ENABLED_ENVIRONMENT.into(),
        request.camera_zoom_enabled.to_string().into(),
    ));
    environment.push((
        CAMERA_ZOOM_LIMIT_ENVIRONMENT.into(),
        request.camera_zoom_limit.to_string().into(),
    ));
    Ok(HelperInvocation {
        program: request.helper_path.clone(),
        args,
        environment,
        current_dir,
    })
}

fn guest_path_arg(
    guest: GuestPathMapper<'_>,
    field: &'static str,
    path: &Path,
) -> Result<OsString, HelperPlanError> {
    guest(field, &path_arg(field, path)?)
}

fn encode_path_list(
    guest: GuestPathMapper<'_>,
    field: &'static str,
    paths: &[PathBuf],
) -> Result<OsString, LaunchPlanError> {
    let mut encoded = OsString::new();
    for (index, path) in paths.iter().enumerate() {
        let path = path_arg(field, path)?;
        if path.as_os_str().to_string_lossy().contains(['\r', '\n']) {
            return Err(HelperPlanError::LineBreak { field }.into());
        }
        if index != 0 {
            encoded.push("\n");
        }
        encoded.push(guest(field, &path)?);
    }
    Ok(encoded)
}

fn reject_patch(field: &'static str, patch: &[u8]) -> Result<(), LaunchPlanError> {
    if patch.is_empty() {
        return Err(HelperPlanError::EmptyArgument { field }.into());
    }
    Ok(())
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02X}");
    }
    output
}

fn push_path_arg(args: &mut Vec<OsString>, flag: &str, path: OsString) {
    args.push(flag.into());
    args.push(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::pe_patch::plan_wine_stability_patches;

    fn request() -> ExtensionLaunchRequest {
        ExtensionLaunchRequest {
            game_dir: PathBuf::from("mock-game"),
            helper_path: PathBuf::from("mock-client/bahamut-loader.exe"),
            client_module_path: PathBuf::from("mock-client/bahamut.dll"),
            addon_manifests: vec![
                PathBuf::from("mock-runtime/addons/alpha/addon.toml"),
                PathBuf::from("mock-runtime/addons/fps/addon.toml"),
            ],
            addon_catalog: vec![
                PathBuf::from("mock-runtime/addons/alpha/addon.toml"),
                PathBuf::from("mock-runtime/addons/fps/addon.toml"),
                PathBuf::from("mock-runtime/addons/distance/addon.toml"),
            ],
            addon_settings_path: PathBuf::from("mock-root/config/addons"),
            chat_logs_path: PathBuf::from("mock-root/logs/chat"),
            dat_roots: vec![PathBuf::from("mock-root/plugins/dats/alpha")],
            startup_script_path: PathBuf::from("mock-root/scripts/default.txt"),
            screenshot_plugin_path: PathBuf::from("mock-root/plugins/screenshot.dll"),
            screenshots_path: PathBuf::from("mock-root/screenshots"),
            screenshot_format: ScreenshotFormat::Png,
            screenshot_hide_overlays: true,
            screenshot_hotkey: ScreenshotHotkey::PrintScreen,
            screenshot_enabled: true,
            discord_rpc_plugin_path: PathBuf::from("mock-root/plugins/discord-rpc.dll"),
            discord_rpc_application_id: "1546272666833522798".into(),
            discord_rpc_enabled: true,
            object_distance_enabled: false,
            object_distance_percent: 200,
            camera_zoom_enabled: false,
            camera_zoom_limit: 15,
            display_mode: DisplayMode::Windowed,
            borderless_monitor: None,
            server_utc_patch: vec![0xB8, 0x12, 0xE8, 0xE0, 0x50],
            lobby_host_patch: b"127.0.0.1\0".to_vec(),
            launch_argument: "sqex0002abc!////".to_owned(),
        }
    }

    #[test]
    fn enabled_plan_builds_expected_x86_helper_arguments() {
        let invocation = plan_verified_extension_launch(&request()).unwrap();
        assert_eq!(invocation.program, request().helper_path);
        let args: Vec<String> = invocation
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "--client");
        assert_eq!(args[2], "--module");
        assert_eq!(args[4], "--server-utc");
        assert_eq!(args[5], "B812E8E050");
        assert_eq!(args[6], "--lobby-host");
        assert_eq!(args[7], "3132372E302E302E3100");
        assert_eq!(args[8], "--launch-argument");
        assert_eq!(args[9], request().launch_argument);
        assert_eq!(invocation.environment.len(), 21);
        assert_eq!(invocation.environment[0].0, STARTUP_SCRIPT_ENVIRONMENT);
        assert_eq!(invocation.environment[0].1, request().startup_script_path);
        assert_eq!(invocation.environment[1].0, SCREENSHOT_PLUGIN_ENVIRONMENT);
        assert_eq!(
            invocation.environment[1].1,
            request().screenshot_plugin_path
        );
        assert_eq!(invocation.environment[2].0, SCREENSHOTS_ENVIRONMENT);
        assert_eq!(invocation.environment[2].1, request().screenshots_path);
        assert_eq!(invocation.environment[3].0, SCREENSHOT_FORMAT_ENVIRONMENT);
        assert_eq!(invocation.environment[3].1, "png");
        assert_eq!(
            invocation.environment[4].0,
            SCREENSHOT_HIDE_OVERLAYS_ENVIRONMENT
        );
        assert_eq!(invocation.environment[4].1, "true");
        assert_eq!(invocation.environment[5].0, SCREENSHOT_HOTKEY_ENVIRONMENT);
        assert_eq!(invocation.environment[5].1, "print_screen");
        assert_eq!(invocation.environment[6].0, SCREENSHOT_ENABLED_ENVIRONMENT);
        assert_eq!(invocation.environment[6].1, "true");
        assert_eq!(invocation.environment[7].0, DISCORD_RPC_PLUGIN_ENVIRONMENT);
        assert_eq!(
            invocation.environment[7].1,
            request().discord_rpc_plugin_path
        );
        assert_eq!(
            invocation.environment[8].0,
            DISCORD_RPC_APPLICATION_ID_ENVIRONMENT
        );
        assert_eq!(
            invocation.environment[8].1,
            OsString::from(request().discord_rpc_application_id)
        );
        assert_eq!(invocation.environment[9].0, DISCORD_RPC_ENABLED_ENVIRONMENT);
        assert_eq!(invocation.environment[9].1, "true");
        assert_eq!(invocation.environment[10].0, ADDON_CATALOG_ENVIRONMENT);
        assert_eq!(
            invocation.environment[10].1,
            OsString::from(
                "mock-runtime/addons/alpha/addon.toml\nmock-runtime/addons/fps/addon.toml\nmock-runtime/addons/distance/addon.toml"
            )
        );
        assert_eq!(invocation.environment[11].0, ADDON_MANIFESTS_ENVIRONMENT);
        assert_eq!(
            invocation.environment[11].1,
            OsString::from(
                "mock-runtime/addons/alpha/addon.toml\nmock-runtime/addons/fps/addon.toml"
            )
        );
        assert_eq!(invocation.environment[12].0, ADDON_SETTINGS_ENVIRONMENT);
        assert_eq!(invocation.environment[12].1, request().addon_settings_path);
        assert_eq!(invocation.environment[13].0, CHAT_LOGS_ENVIRONMENT);
        assert_eq!(invocation.environment[13].1, request().chat_logs_path);
        assert_eq!(invocation.environment[14].0, DAT_PACKAGE_ROOTS_ENVIRONMENT);
        assert_eq!(
            invocation.environment[14].1,
            OsString::from("mock-root/plugins/dats/alpha")
        );
        assert_eq!(invocation.environment[15].0, BORDERLESS_ENVIRONMENT);
        assert_eq!(invocation.environment[15].1, "false");
        assert_eq!(invocation.environment[16].0, BORDERLESS_MONITOR_ENVIRONMENT);
        assert_eq!(invocation.environment[16].1, "");
        assert_eq!(
            invocation.environment[17].0,
            OBJECT_DISTANCE_ENABLED_ENVIRONMENT
        );
        assert_eq!(invocation.environment[17].1, "false");
        assert_eq!(
            invocation.environment[18].0,
            OBJECT_DISTANCE_PERCENT_ENVIRONMENT
        );
        assert_eq!(invocation.environment[18].1, "200");
        assert_eq!(
            invocation.environment[19].0,
            CAMERA_ZOOM_ENABLED_ENVIRONMENT
        );
        assert_eq!(invocation.environment[19].1, "false");
        assert_eq!(invocation.environment[20].0, CAMERA_ZOOM_LIMIT_ENVIRONMENT);
        assert_eq!(invocation.environment[20].1, "15");
    }

    #[test]
    fn camera_zoom_selection_reaches_runtime_environment() {
        let mut request = request();
        request.camera_zoom_enabled = true;
        request.camera_zoom_limit = 12;
        request.object_distance_percent = 150;
        let invocation = plan_verified_extension_launch(&request).unwrap();
        assert_eq!(
            invocation.environment[19].0,
            CAMERA_ZOOM_ENABLED_ENVIRONMENT
        );
        assert_eq!(invocation.environment[19].1, "true");
        assert_eq!(invocation.environment[18].1, "150");
        assert_eq!(invocation.environment[20].1, "12");
    }

    #[test]
    fn helper_path_nul_is_rejected_before_command_construction() {
        let mut request = request();
        request.helper_path = PathBuf::from("helper\0.exe");
        assert!(matches!(
            plan_verified_extension_launch(&request),
            Err(LaunchPlanError::HelperArguments(_))
        ));
    }

    #[test]
    fn launch_argument_nul_is_rejected_before_command_construction() {
        let mut request = request();
        request.launch_argument.push('\0');
        assert!(matches!(
            plan_verified_extension_launch(&request),
            Err(LaunchPlanError::HelperArguments(_))
        ));
    }

    #[test]
    fn addon_manifest_line_break_is_rejected_before_environment_construction() {
        let mut request = request();
        request.addon_manifests = vec![PathBuf::from("bad\nmanifest.toml")];
        assert!(matches!(
            plan_verified_extension_launch(&request),
            Err(LaunchPlanError::HelperArguments(
                HelperPlanError::LineBreak { .. }
            ))
        ));
    }

    #[test]
    fn disabled_screenshot_keeps_its_binding_separate_from_auto_load() {
        let mut request = request();
        request.screenshot_enabled = false;
        let invocation = plan_verified_extension_launch(&request).unwrap();
        assert_eq!(invocation.environment[1].0, SCREENSHOT_PLUGIN_ENVIRONMENT);
        assert_eq!(invocation.environment[1].1, request.screenshot_plugin_path);
        assert_eq!(invocation.environment[5].0, SCREENSHOT_HOTKEY_ENVIRONMENT);
        assert_eq!(invocation.environment[5].1, "print_screen");
        assert_eq!(invocation.environment[6].0, SCREENSHOT_ENABLED_ENVIRONMENT);
        assert_eq!(invocation.environment[6].1, "false");
    }

    #[test]
    fn borderless_plan_enables_the_runtime_borderless_mode() {
        let mut request = request();
        request.display_mode = DisplayMode::Borderless;
        let invocation = plan_verified_extension_launch(&request).unwrap();

        assert_eq!(
            invocation
                .environment
                .iter()
                .find(|(name, _)| name == BORDERLESS_ENVIRONMENT)
                .map(|(_, value)| value),
            Some(&OsString::from("true"))
        );
    }

    #[test]
    fn non_borderless_plans_disable_the_runtime_borderless_mode() {
        for display_mode in [DisplayMode::Windowed, DisplayMode::FullScreen] {
            let mut request = request();
            request.display_mode = display_mode;
            let invocation = plan_verified_extension_launch(&request).unwrap();
            assert_eq!(
                invocation
                    .environment
                    .iter()
                    .find(|(name, _)| name == BORDERLESS_ENVIRONMENT)
                    .map(|(_, value)| value),
                Some(&OsString::from("false"))
            );
            assert_eq!(
                invocation
                    .environment
                    .iter()
                    .find(|(name, _)| name == BORDERLESS_MONITOR_ENVIRONMENT)
                    .map(|(_, value)| value),
                Some(&OsString::new())
            );
        }
    }

    #[test]
    fn borderless_plan_transports_selected_device_identity_and_empty_default() {
        let mut request = request();
        request.display_mode = DisplayMode::Borderless;
        request.borderless_monitor = Some(r"\\?\DISPLAY#MONITOR&ID".into());
        let invocation = plan_verified_extension_launch(&request).unwrap();
        assert_eq!(
            invocation
                .environment
                .iter()
                .find(|(name, _)| name == BORDERLESS_MONITOR_ENVIRONMENT)
                .map(|(_, value)| value),
            Some(&OsString::from(r"\\?\DISPLAY#MONITOR&ID"))
        );

        request.borderless_monitor = None;
        let invocation = plan_verified_extension_launch(&request).unwrap();
        assert_eq!(
            invocation
                .environment
                .iter()
                .find(|(name, _)| name == BORDERLESS_MONITOR_ENVIRONMENT)
                .map(|(_, value)| value),
            Some(&OsString::new())
        );
    }

    fn fake_dos(_field: &'static str, path: &Path) -> Result<OsString, HelperPlanError> {
        Ok(format!("Z:\\{}", path.to_string_lossy().replace('/', "\\")).into())
    }

    fn wine_plan(request: &ExtensionLaunchRequest) -> HelperInvocation {
        plan_verified_wine_extension_launch(
            request,
            &fake_dos,
            &plan_wine_stability_patches(),
            10_000,
        )
        .unwrap()
    }

    fn env<'a>(invocation: &'a HelperInvocation, name: &str) -> &'a OsString {
        invocation
            .environment
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
            .unwrap()
    }

    #[test]
    fn wine_plan_maps_helper_paths_and_appends_wait_mode_arguments() {
        let invocation = wine_plan(&request());
        assert_eq!(invocation.program, request().helper_path);
        assert_eq!(invocation.current_dir, request().game_dir);
        let args: Vec<String> = invocation
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--client",
                r"Z:\mock-game\ffxivgame.exe",
                "--module",
                r"Z:\mock-client\bahamut.dll",
                "--server-utc",
                "B812E8E050",
                "--lobby-host",
                "3132372E302E302E3100",
                "--launch-argument",
                "sqex0002abc!////",
                "--patch",
                "00648BBF:FF1564E1F3008D6424FC909090909090",
                "--patch",
                "00492550:85C9740E8B49048B018B5014FFD28A08EB0230C98B4424048808C20400",
                "--patch",
                "00494B70:90909090",
                "--timeout-ms",
                "10000",
                "--wait-for-client",
            ]
        );
    }

    #[test]
    fn wine_plan_maps_path_environment_and_keeps_scalars() {
        let wine = wine_plan(&request());
        let host = plan_verified_extension_launch(&request()).unwrap();
        assert_eq!(wine.environment.len(), 21);
        let mapped = [
            (
                STARTUP_SCRIPT_ENVIRONMENT,
                r"Z:\mock-root\scripts\default.txt",
            ),
            (
                SCREENSHOT_PLUGIN_ENVIRONMENT,
                r"Z:\mock-root\plugins\screenshot.dll",
            ),
            (SCREENSHOTS_ENVIRONMENT, r"Z:\mock-root\screenshots"),
            (
                DISCORD_RPC_PLUGIN_ENVIRONMENT,
                r"Z:\mock-root\plugins\discord-rpc.dll",
            ),
            (
                ADDON_CATALOG_ENVIRONMENT,
                "Z:\\mock-runtime\\addons\\alpha\\addon.toml\nZ:\\mock-runtime\\addons\\fps\\addon.toml\nZ:\\mock-runtime\\addons\\distance\\addon.toml",
            ),
            (
                ADDON_MANIFESTS_ENVIRONMENT,
                "Z:\\mock-runtime\\addons\\alpha\\addon.toml\nZ:\\mock-runtime\\addons\\fps\\addon.toml",
            ),
            (ADDON_SETTINGS_ENVIRONMENT, r"Z:\mock-root\config\addons"),
            (CHAT_LOGS_ENVIRONMENT, r"Z:\mock-root\logs\chat"),
            (
                DAT_PACKAGE_ROOTS_ENVIRONMENT,
                r"Z:\mock-root\plugins\dats\alpha",
            ),
        ];
        for (name, value) in mapped {
            assert_eq!(env(&wine, name), &OsString::from(value), "{name}");
        }
        for ((wine_name, wine_value), (host_name, host_value)) in
            wine.environment.iter().zip(&host.environment)
        {
            assert_eq!(wine_name, host_name);
            if !mapped.iter().any(|(name, _)| wine_name == name) {
                assert_eq!(wine_value, host_value, "{wine_name:?}");
            }
        }
    }

    #[test]
    fn wine_plan_never_requests_borderless() {
        let mut request = request();
        request.display_mode = DisplayMode::Borderless;
        request.borderless_monitor = Some(r"\\?\DISPLAY#MONITOR&ID".into());
        let invocation = wine_plan(&request);
        assert_eq!(env(&invocation, BORDERLESS_ENVIRONMENT), "false");
        assert_eq!(env(&invocation, BORDERLESS_MONITOR_ENVIRONMENT), "");

        request.display_mode = DisplayMode::FullScreen;
        assert_eq!(env(&wine_plan(&request), BORDERLESS_ENVIRONMENT), "false");
    }

    #[test]
    fn wine_plan_surfaces_unmappable_paths() {
        let unmappable = |field: &'static str, path: &Path| {
            if field == "chat logs" {
                return Err(HelperPlanError::GuestPath {
                    field,
                    path: path.to_path_buf(),
                });
            }
            fake_dos(field, path)
        };
        assert!(matches!(
            plan_verified_wine_extension_launch(
                &request(),
                &unmappable,
                &plan_wine_stability_patches(),
                10_000,
            ),
            Err(LaunchPlanError::HelperArguments(
                HelperPlanError::GuestPath {
                    field: "chat logs",
                    ..
                }
            ))
        ));
    }

    #[test]
    fn wine_plan_rejects_host_line_breaks_before_mapping() {
        let mut request = request();
        request.dat_roots = vec![PathBuf::from("bad\nroot")];
        let mapped_line_break = |field: &'static str, path: &Path| {
            assert_ne!(field, "DAT overlay root", "line break reached the mapper");
            fake_dos(field, path)
        };
        assert!(matches!(
            plan_verified_wine_extension_launch(
                &request,
                &mapped_line_break,
                &plan_wine_stability_patches(),
                10_000,
            ),
            Err(LaunchPlanError::HelperArguments(
                HelperPlanError::LineBreak { .. }
            ))
        ));
    }

    #[test]
    fn patch_argument_encodes_rva_and_bytes_in_uppercase_hex() {
        let patch = PePatch {
            rva: 0x0000_0A0B,
            bytes: vec![0x00, 0xAB, 0x0F],
        };
        assert_eq!(encode_patch_argument(&patch), "00000A0B:00AB0F");
    }
}
