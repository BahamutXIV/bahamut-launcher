//! Linux Wine backend: patch `ffxivgame.exe` on a working copy and launch it with system Wine, or,
//! when the packaged extension artifacts are present, run the x86 loader under system Wine.

use std::path::{Path, PathBuf};

use super::on_disk_patch::apply_patches_on_disk;
use super::wine::extension_launch::{WINE_HELPER_READY_TIMEOUT_MS, WineDosPaths};
use super::wine::{WineRuntime, copy_exe_for_patching, monotonic_ms_since_boot};
use super::{ExtensionArtifacts, LaunchError, LaunchOptions, LaunchedGame, dxvk};
use crate::extensions::{
    ExtensionLaunchRequest, GuestPathMapper, HelperInvocation, LaunchPlanError,
    plan_wine_extension_launch,
};
use crate::launcher::launch_args::build_launch_argument;
use crate::launcher::pe_patch::{
    PePatch, plan_contract_patches, plan_wine_patches, plan_wine_stability_patches,
};

const CLIENT_EXE: &str = "ffxivgame.exe";

const PATCHED_EXE: &str = "ffxivgame.patched.exe";

/// Linux has no registry; use the `game_location` preference.
pub fn detect_game_install() -> Option<PathBuf> {
    None
}

/// Launch the client under system Wine and return its process id.
pub fn launch_game(
    game_dir: &Path,
    lobby_host: &str,
    session_id: &str,
    options: &LaunchOptions,
) -> Result<LaunchedGame, LaunchError> {
    let exe_path = game_dir.join(CLIENT_EXE);
    if !exe_path.is_file() {
        return Err(LaunchError::MissingClientBinary(exe_path));
    }

    let runtime = WineRuntime::discover(game_dir)?;
    tracing::info!(
        prefix = %runtime.prefix.display(),
        game_dir = %game_dir.display(),
        "resolved Wine runtime"
    );
    runtime.ensure_prefix_initialized()?;

    // DXVK is best-effort; None selects the wined3d fallback.
    let dxvk_overrides = dxvk::ensure_dxvk(&runtime);

    // The tick must agree with the client's GetTickCount clock.
    let tick = monotonic_ms_since_boot();
    let launch_args = build_launch_argument(session_id, tick)?;

    if let Some(artifacts) = &options.extension_artifacts {
        let invocation = plan_extension_helper(
            &runtime.prefix,
            game_dir,
            lobby_host,
            launch_args.encoded_argument,
            options,
            artifacts,
            plan_wine_extension_launch,
        )?;
        return runtime.launch_extension_helper(
            &invocation,
            options.verbose_wine_debug,
            dxvk_overrides.as_deref(),
        );
    }

    // Patch a working copy, never the original client binary.
    let patched_exe = game_dir.join(PATCHED_EXE);
    copy_exe_for_patching(&exe_path, &patched_exe)?;
    let patches = plan_wine_patches(lobby_host)?;
    apply_patches_on_disk(&patched_exe, &patches)?;
    tracing::info!(patches = patches.len(), exe = %patched_exe.display(), "applied on-disk PE patches");

    runtime.launch(
        &patched_exe,
        &launch_args.encoded_argument,
        options.verbose_wine_debug,
        dxvk_overrides.as_deref(),
    )
}

/// Signature of [`plan_wine_extension_launch`]; tests substitute its ungated core.
type WinePlanner = fn(
    &ExtensionLaunchRequest,
    GuestPathMapper<'_>,
    &[PePatch],
    u32,
) -> Result<HelperInvocation, LaunchPlanError>;

/// Plan the x86 loader against the original `ffxivgame.exe`, which it requires by name and retail
/// hash; the loader applies the contract and stability patches in memory.
fn plan_extension_helper(
    prefix: &Path,
    game_dir: &Path,
    lobby_host: &str,
    launch_argument: String,
    options: &LaunchOptions,
    artifacts: &ExtensionArtifacts,
    planner: WinePlanner,
) -> Result<HelperInvocation, LaunchError> {
    let contract_patches = plan_contract_patches(lobby_host)?;
    let request = artifacts.to_request(game_dir, options, &contract_patches, launch_argument);
    let dos_paths = WineDosPaths::from_prefix(prefix)?;
    Ok(planner(
        &request,
        &|field, path| dos_paths.map(field, path),
        &plan_wine_stability_patches(),
        WINE_HELPER_READY_TIMEOUT_MS,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::preferences::{ScreenshotFormat, ScreenshotHotkey};
    use crate::extensions::launch_plan::plan_wine_extension_launch_ungated;
    use crate::extensions::{
        ARG_CLIENT, ARG_LAUNCH_ARGUMENT, ARG_LOBBY_HOST, ARG_MODULE, ARG_PATCH, ARG_SERVER_UTC,
        ARG_TIMEOUT_MS, ARG_WAIT_FOR_CLIENT, CHAT_LOGS_ENVIRONMENT, IdentityGateError,
        IdentityMismatch, SUPPORTED_GAME_VERSION,
    };
    use std::fs;
    use std::os::unix::fs::symlink;

    const PREFIX_FFXIV_SUBPATH: &str = "drive_c/Program Files (x86)/SquareEnix/FINAL FANTASY XIV";

    /// A prefix with `c:` on `drive_c` and `z:` on `/`; the launcher files sit in `C:\launcher`.
    fn extension_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, ExtensionArtifacts) {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("prefix");
        let game_dir = prefix.join(PREFIX_FFXIV_SUBPATH);
        let launcher = prefix.join("drive_c/launcher");
        fs::create_dir_all(&game_dir).unwrap();
        fs::create_dir_all(&launcher).unwrap();
        fs::create_dir_all(prefix.join("dosdevices")).unwrap();
        symlink("../drive_c", prefix.join("dosdevices/c:")).unwrap();
        symlink("/", prefix.join("dosdevices/z:")).unwrap();
        let artifacts = ExtensionArtifacts {
            helper_path: launcher.join("bahamut-loader.exe"),
            client_module_path: launcher.join("bahamut.dll"),
            addon_manifests: vec![launcher.join("addons/alpha/addon.toml")],
            addon_catalog: vec![launcher.join("addons/alpha/addon.toml")],
            addon_settings_path: launcher.join("config/addons"),
            chat_logs_path: launcher.join("logs/chat"),
            dat_roots: Vec::new(),
            startup_script_path: launcher.join("scripts/default.txt"),
            screenshot_plugin_path: launcher.join("plugins/screenshot.dll"),
            screenshots_path: launcher.join("screenshots"),
            screenshot_format: ScreenshotFormat::Png,
            screenshot_hide_overlays: false,
            screenshot_hotkey: ScreenshotHotkey::PrintScreen,
            screenshot_enabled: true,
            discord_rpc_plugin_path: launcher.join("plugins/discord-rpc.dll"),
            discord_rpc_application_id: "1546272666833522798".into(),
            discord_rpc_enabled: false,
            object_distance_enabled: false,
            object_distance_percent: 200,
            camera_zoom_enabled: false,
            camera_zoom_limit: 15,
        };
        (temp, prefix, game_dir, artifacts)
    }

    fn string_args(invocation: &HelperInvocation) -> Vec<String> {
        invocation
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn extension_plan_maps_paths_to_the_prefix_drive_and_waits_for_the_client() {
        let (_temp, prefix, game_dir, artifacts) = extension_fixture();
        let invocation = plan_extension_helper(
            &prefix,
            &game_dir,
            "127.0.0.1",
            "sqex0002abc!////".into(),
            &LaunchOptions::default(),
            &artifacts,
            plan_wine_extension_launch_ungated,
        )
        .unwrap();
        let args = string_args(&invocation);
        // Ten shared arguments, three stability patches, then the two Wine-only options.
        assert_eq!(args.len(), 19);
        assert_eq!(
            &args[..4],
            [
                ARG_CLIENT,
                r"C:\Program Files (x86)\SquareEnix\FINAL FANTASY XIV\ffxivgame.exe",
                ARG_MODULE,
                r"C:\launcher\bahamut.dll",
            ]
        );
        assert_eq!(args[4], ARG_SERVER_UTC);
        assert_eq!(args[6], ARG_LOBBY_HOST);
        assert_eq!(&args[8..10], [ARG_LAUNCH_ARGUMENT, "sqex0002abc!////"]);
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == ARG_PATCH).count(),
            3
        );
        assert_eq!(&args[16..], [ARG_TIMEOUT_MS, "10000", ARG_WAIT_FOR_CLIENT]);
        assert_eq!(invocation.program, artifacts.helper_path);
        assert_eq!(invocation.current_dir, game_dir);
        assert_eq!(
            invocation
                .environment
                .iter()
                .find(|(name, _)| name == CHAT_LOGS_ENVIRONMENT)
                .map(|(_, value)| value.to_string_lossy().into_owned()),
            Some(r"C:\launcher\logs\chat".to_owned())
        );
    }

    /// A staged tree outside the prefix, the Linux layout, reaches the helper through the `z:` root.
    #[test]
    fn extension_plan_maps_a_launcher_tree_outside_the_prefix_to_the_root_drive() {
        let (temp, prefix, game_dir, mut artifacts) = extension_fixture();
        let launcher = temp.path().join("launcher");
        fs::create_dir_all(&launcher).unwrap();
        artifacts.client_module_path = launcher.join("bahamut.dll");
        let invocation = plan_extension_helper(
            &prefix,
            &game_dir,
            "127.0.0.1",
            "sqex0002abc!////".into(),
            &LaunchOptions::default(),
            &artifacts,
            plan_wine_extension_launch_ungated,
        )
        .unwrap();
        let mut expected = String::from("Z:");
        for component in fs::canonicalize(&launcher).unwrap().components().skip(1) {
            expected.push('\\');
            expected.push_str(&component.as_os_str().to_string_lossy());
        }
        expected.push_str(r"\bahamut.dll");
        assert_eq!(string_args(&invocation)[3], expected);
    }

    #[test]
    fn extension_plan_gates_the_client_identity() {
        let (_temp, prefix, game_dir, artifacts) = extension_fixture();
        fs::write(game_dir.join(CLIENT_EXE), b"not the retail client").unwrap();
        fs::write(game_dir.join("game.ver"), SUPPORTED_GAME_VERSION).unwrap();
        let result = plan_extension_helper(
            &prefix,
            &game_dir,
            "127.0.0.1",
            "sqex0002abc!////".into(),
            &LaunchOptions::default(),
            &artifacts,
            plan_wine_extension_launch,
        );
        assert!(matches!(
            result,
            Err(LaunchError::ExtensionPlan(LaunchPlanError::Identity(
                IdentityGateError::Mismatch(IdentityMismatch::ExecutableSha256 { .. })
            )))
        ));
    }
}
