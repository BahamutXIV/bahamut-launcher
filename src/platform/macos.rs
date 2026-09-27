//! macOS backend: manage the Sikarugir runtime, then either patch a working copy before Wine
//! launch or, when the packaged extension artifacts are present, run the x86 loader under Wine.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::runtime_archive::{
    MACOS_ENGINE_WS12_23_7_1_4, MACOS_WRAPPER_1_0_11, RuntimeArchivePin, download_verified,
};
use crate::config;
use crate::extensions::{
    ExtensionLaunchRequest, GuestPathMapper, HelperInvocation, LaunchPlanError,
    plan_wine_extension_launch,
};
use crate::launcher::launch_args::build_launch_argument;
use crate::launcher::pe_patch::{
    PePatch, plan_contract_patches, plan_wine_patches, plan_wine_stability_patches,
};
use crate::platform::wine::extension_launch::{WINE_HELPER_READY_TIMEOUT_MS, WineDosPaths};
use crate::platform::wine::{
    PREFIX_FFXIV_SUBPATH, WineRuntime, apply_patches_on_disk, copy_exe_for_patching,
    ensure_prefix_initialized, launch_extension_helper, launch_ffxiv_game, monotonic_ms_since_boot,
};
use crate::platform::{ExtensionArtifacts, LaunchError, LaunchOptions, LaunchedGame};

const WRAPPER_VERSION: &str = "1.0.11";
// WineCX 23.7.1 rather than 24.0.7 or an upstream-based build: CX 24's mac
// driver fails to initialize on macOS 27, unpatched wined3d-GL engines render
// this title too slowly to play, and DXVK is blocked on macOS (winevulkan
// wow64 feature thunking + MoltenVK gaps), so the CX-patched D3D9 path is
// required.
const ENGINE_NAME: &str = "WS12WineCX23.7.1_4";

const CLIENT_EXE: &str = "ffxivgame.exe";
const PATCHED_EXE: &str = "ffxivgame.patched.exe";

fn mac_err(context: impl Into<String>) -> LaunchError {
    LaunchError::Wine(context.into())
}

fn data_dir() -> Result<PathBuf, LaunchError> {
    config::dirs::data_dir().map_err(|e| mac_err(format!("resolving data dir: {e}")))
}

fn managed_prefix_dir() -> Result<PathBuf, LaunchError> {
    Ok(data_dir()?.join("prefix"))
}

fn managed_install_dir() -> Result<PathBuf, LaunchError> {
    Ok(managed_prefix_dir()?.join(PREFIX_FFXIV_SUBPATH))
}

fn runtime_root() -> Result<PathBuf, LaunchError> {
    Ok(data_dir()?.join("runtime"))
}

fn wine_bin() -> Result<PathBuf, LaunchError> {
    Ok(runtime_root()?.join("wswine.bundle/bin/wine"))
}

fn wineserver_bin() -> Result<PathBuf, LaunchError> {
    Ok(runtime_root()?.join("wswine.bundle/bin/wineserver"))
}

fn runtime_for_game_dir(game_dir: &Path) -> Result<WineRuntime, LaunchError> {
    let prefix = derive_prefix_from_game_location(game_dir)
        .inspect(
            |p| tracing::info!(prefix = %p.display(), "using WINEPREFIX derived from game dir"),
        )
        .map(Ok)
        .unwrap_or_else(managed_prefix_dir)?;
    let root = runtime_root()?;
    Ok(WineRuntime {
        root: data_dir()?,
        prefix,
        wine_bin: wine_bin()?,
        wineserver_bin: wineserver_bin()?,
        dyld_fallback_paths: vec![
            root.join("Frameworks"),
            root.join("Frameworks/GStreamer.framework/Versions/Current/lib"),
            root.join("wswine.bundle/lib"),
            PathBuf::from("/usr/local/lib"),
            PathBuf::from("/usr/lib"),
        ],
        gst_plugin_path: Some(
            root.join("Frameworks/GStreamer.framework/Versions/Current/lib/gstreamer-1.0"),
        ),
    })
}

pub fn detect_game_install() -> Option<PathBuf> {
    let managed = managed_install_dir().ok()?;
    if managed.join(CLIENT_EXE).exists() {
        Some(managed)
    } else {
        None
    }
}

pub fn launch_game(
    game_dir: &Path,
    lobby_host: &str,
    session_id: &str,
    options: &LaunchOptions,
) -> Result<LaunchedGame, LaunchError> {
    ensure_rosetta_available()?;
    ensure_runtime_downloaded()?;

    let runtime = runtime_for_game_dir(game_dir)?;
    ensure_prefix_initialized(&runtime)?;

    let tick = monotonic_ms_since_boot();
    let launch = build_launch_argument(session_id, tick)?;

    let src_exe = game_dir.join(CLIENT_EXE);
    if !src_exe.exists() {
        return Err(LaunchError::MissingClientBinary(src_exe));
    }
    if let Some(artifacts) = &options.extension_artifacts {
        return launch_with_extensions(
            &runtime,
            game_dir,
            lobby_host,
            launch.encoded_argument,
            options,
            artifacts,
        );
    }
    let patched_exe = game_dir.join(PATCHED_EXE);
    copy_exe_for_patching(&src_exe, &patched_exe)?;

    let patches = plan_wine_patches(lobby_host)?;
    apply_patches_on_disk(&patched_exe, &patches)?;

    launch_ffxiv_game(&runtime, &patched_exe, &launch.encoded_argument, None)
}

/// Signature of [`plan_wine_extension_launch`]; tests substitute its ungated core.
type WinePlanner = fn(
    &ExtensionLaunchRequest,
    GuestPathMapper<'_>,
    &[PePatch],
    u32,
) -> Result<HelperInvocation, LaunchPlanError>;

/// Run the x86 loader under Wine against the original `ffxivgame.exe`, which the loader requires
/// by name and retail hash; the loader applies the contract and stability patches in memory.
fn launch_with_extensions(
    runtime: &WineRuntime,
    game_dir: &Path,
    lobby_host: &str,
    launch_argument: String,
    options: &LaunchOptions,
    artifacts: &ExtensionArtifacts,
) -> Result<LaunchedGame, LaunchError> {
    let invocation = plan_extension_helper(
        &runtime.prefix,
        game_dir,
        lobby_host,
        launch_argument,
        options,
        artifacts,
        plan_wine_extension_launch,
    )?;
    launch_extension_helper(runtime, &invocation)
}

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

fn derive_prefix_from_game_location(game_dir: &Path) -> Option<PathBuf> {
    let mut current = game_dir;
    while let Some(parent) = current.parent() {
        if current.file_name() == Some(OsStr::new("drive_c")) {
            return Some(parent.to_path_buf());
        }
        current = parent;
    }
    None
}

/// Verify Rosetta 2 for the x86_64 Wine engine without installing it.
fn ensure_rosetta_available() -> Result<(), LaunchError> {
    if !cfg!(target_arch = "aarch64") {
        return Ok(());
    }
    let status = Command::new("/usr/bin/arch")
        .arg("-x86_64")
        .arg("/usr/bin/true")
        .status()
        .map_err(|e| mac_err(format!("running /usr/bin/arch to probe Rosetta 2: {e}")))?;
    if !status.success() {
        return Err(mac_err(
            "Rosetta 2 is required to run the x86_64 Wine engine on Apple Silicon. \
             Install it with: softwareupdate --install-rosetta --agree-to-license",
        ));
    }
    Ok(())
}

/// Install missing or stale runtime components after every needed archive has
/// been verified and extracted into staging.
fn ensure_runtime_downloaded() -> Result<(), LaunchError> {
    let root = runtime_root()?;
    ensure_runtime_components_at(&root, MACOS_WRAPPER_1_0_11, MACOS_ENGINE_WS12_23_7_1_4)?;

    let wine = root.join("wswine.bundle/bin/wine");
    let engine_marker = root.join("engine-version");

    match Command::new(&wine).arg("--version").output() {
        Ok(out) if out.status.success() => {
            tracing::info!(
                "wine --version: {}",
                String::from_utf8_lossy(&out.stdout).trim()
            );
            fs::write(&engine_marker, ENGINE_NAME)
                .map_err(|e| mac_err(format!("writing {}: {e}", engine_marker.display())))?;
            Ok(())
        }
        Ok(out) => Err(mac_err(format!(
            "wine --version failed (exit {:?}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ))),
        Err(e) => Err(mac_err(format!(
            "failed to execute {}: {e}",
            wine.display()
        ))),
    }
}

fn ensure_runtime_components_at(
    root: &Path,
    wrapper_pin: RuntimeArchivePin,
    engine_pin: RuntimeArchivePin,
) -> Result<(), LaunchError> {
    let frameworks = root.join("Frameworks");
    let wswine_bundle = root.join("wswine.bundle");
    let wine = wswine_bundle.join("bin/wine");
    let wrapper_marker = root.join("wrapper-version");
    let engine_marker = root.join("engine-version");

    let install_wrapper = install_needed(
        frameworks.exists(),
        fs::read_to_string(&wrapper_marker).ok().as_deref(),
        WRAPPER_VERSION,
    );
    let install_engine = install_needed(
        wine.exists(),
        fs::read_to_string(&engine_marker).ok().as_deref(),
        ENGINE_NAME,
    );
    if !install_wrapper && !install_engine {
        return Ok(());
    }

    let staging = tempfile::tempdir().map_err(|e| mac_err(format!("tmp dir: {e}")))?;
    let staged_frameworks = if install_wrapper {
        tracing::info!("downloading Sikarugir wrapper v{WRAPPER_VERSION}");
        let archive = staging.path().join("wrapper.tar.xz");
        download_verified(wrapper_pin, &archive)
            .map_err(|e| mac_err(format!("verifying {}: {e}", wrapper_pin.name)))?;
        let extraction = staging.path().join("wrapper");
        fs::create_dir_all(&extraction)
            .map_err(|e| mac_err(format!("creating {}: {e}", extraction.display())))?;
        extract_tar_xz(&archive, &extraction)?;
        let src = extraction.join(format!(
            "Template-{WRAPPER_VERSION}.app/Contents/Frameworks"
        ));
        if !src.is_dir() {
            return Err(mac_err(format!(
                "wrapper archive missing expected path {}",
                src.display()
            )));
        }
        Some(src)
    } else {
        None
    };

    let staged_engine = if install_engine {
        tracing::info!("downloading Wine engine {ENGINE_NAME}");
        let archive = staging.path().join("engine.tar.xz");
        download_verified(engine_pin, &archive)
            .map_err(|e| mac_err(format!("verifying {}: {e}", engine_pin.name)))?;
        let extraction = staging.path().join("engine");
        fs::create_dir_all(&extraction)
            .map_err(|e| mac_err(format!("creating {}: {e}", extraction.display())))?;
        extract_tar_xz(&archive, &extraction)?;
        let src = extraction.join("wswine.bundle");
        if !src.is_dir() {
            return Err(mac_err(format!(
                "engine archive missing expected wswine.bundle at {}",
                src.display()
            )));
        }
        Some(src)
    } else {
        None
    };

    fs::create_dir_all(root)
        .map_err(|e| mac_err(format!("creating runtime dir {}: {e}", root.display())))?;

    if let Some(src) = staged_frameworks {
        if frameworks.exists() {
            fs::remove_dir_all(&frameworks)
                .map_err(|e| mac_err(format!("removing stale {}: {e}", frameworks.display())))?;
        }
        copy_dir_preserving_symlinks(&src, &frameworks)?;
        fs::write(&wrapper_marker, WRAPPER_VERSION)
            .map_err(|e| mac_err(format!("writing {}: {e}", wrapper_marker.display())))?;
        tracing::info!(path = %frameworks.display(), "installed Frameworks");
    }

    if let Some(src) = staged_engine {
        if wswine_bundle.exists() {
            fs::remove_dir_all(&wswine_bundle)
                .map_err(|e| mac_err(format!("removing stale {}: {e}", wswine_bundle.display())))?;
        }
        if fs::rename(&src, &wswine_bundle).is_err() {
            copy_dir_preserving_symlinks(&src, &wswine_bundle)?;
        }
        tracing::info!(path = %wswine_bundle.display(), "installed Wine engine");
    }

    Ok(())
}

fn install_needed(present: bool, marker: Option<&str>, want: &str) -> bool {
    !present || marker.is_none_or(|m| m.trim() != want)
}

fn extract_tar_xz(archive: &Path, dst: &Path) -> Result<(), LaunchError> {
    let status = Command::new("tar")
        .arg("-xJf")
        .arg(archive)
        .arg("-C")
        .arg(dst)
        .status()
        .map_err(|e| mac_err(format!("running tar -xJf: {e}")))?;
    if !status.success() {
        return Err(mac_err(format!(
            "tar -xJf {} -> {} failed with {status:?}",
            archive.display(),
            dst.display()
        )));
    }
    Ok(())
}

/// Recursively copy a runtime while preserving symlinks.
fn copy_dir_preserving_symlinks(src: &Path, dst: &Path) -> Result<(), LaunchError> {
    fs::create_dir_all(dst).map_err(|e| mac_err(format!("creating {}: {e}", dst.display())))?;
    for entry in
        fs::read_dir(src).map_err(|e| mac_err(format!("reading {}: {e}", src.display())))?
    {
        let entry = entry.map_err(|e| mac_err(format!("reading dir entry: {e}")))?;
        let ty = entry
            .file_type()
            .map_err(|e| mac_err(format!("stat {}: {e}", entry.path().display())))?;
        let source = entry.path();
        let destination = dst.join(entry.file_name());
        if ty.is_symlink() {
            let link_target = fs::read_link(&source)
                .map_err(|e| mac_err(format!("readlink {}: {e}", source.display())))?;
            let _ = fs::remove_file(&destination);
            std::os::unix::fs::symlink(&link_target, &destination).map_err(|e| {
                mac_err(format!(
                    "symlink {} -> {}: {e}",
                    destination.display(),
                    link_target.display()
                ))
            })?;
        } else if ty.is_dir() {
            copy_dir_preserving_symlinks(&source, &destination)?;
        } else {
            fs::copy(&source, &destination).map_err(|e| {
                mac_err(format!(
                    "copying {} -> {}: {e}",
                    source.display(),
                    destination.display()
                ))
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::preferences::{ScreenshotFormat, ScreenshotHotkey};
    use crate::extensions::launch_plan::plan_wine_extension_launch_ungated;
    use crate::extensions::{
        CHAT_LOGS_ENVIRONMENT, IdentityGateError, IdentityMismatch, SUPPORTED_GAME_VERSION,
    };
    use httpmock::prelude::*;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::symlink;

    fn test_pin(name: &str, url: &str, size: u64, sha256: &str) -> RuntimeArchivePin {
        RuntimeArchivePin {
            name: Box::leak(name.to_owned().into_boxed_str()),
            url: Box::leak(url.to_owned().into_boxed_str()),
            size,
            sha256: Box::leak(sha256.to_owned().into_boxed_str()),
        }
    }

    fn wrapper_archive() -> Vec<u8> {
        let temp = tempfile::tempdir().unwrap();
        let wrapper = temp.path().join(format!("Template-{WRAPPER_VERSION}.app"));
        let frameworks = wrapper.join("Contents/Frameworks");
        fs::create_dir_all(&frameworks).unwrap();
        fs::write(frameworks.join("new-runtime.txt"), b"new wrapper").unwrap();
        let archive = temp.path().join("wrapper.tar.xz");
        let status = Command::new("tar")
            .arg("-cJf")
            .arg(&archive)
            .arg("-C")
            .arg(temp.path())
            .arg(format!("Template-{WRAPPER_VERSION}.app"))
            .status()
            .unwrap();
        assert!(
            status.success(),
            "creating wrapper test archive: {status:?}"
        );
        fs::read(archive).unwrap()
    }

    fn create_old_runtime(root: &Path) {
        let frameworks = root.join("Frameworks");
        fs::create_dir_all(&frameworks).unwrap();
        fs::write(frameworks.join("old-runtime.txt"), b"old wrapper").unwrap();
        fs::write(root.join("wrapper-version"), "old-wrapper").unwrap();

        let wine = root.join("wswine.bundle/bin/wine");
        fs::create_dir_all(wine.parent().unwrap()).unwrap();
        fs::write(wine, b"old engine").unwrap();
        fs::write(root.join("engine-version"), "old-engine").unwrap();
    }

    fn assert_old_runtime_unchanged(root: &Path) {
        assert_eq!(
            fs::read(root.join("Frameworks/old-runtime.txt")).unwrap(),
            b"old wrapper"
        );
        assert!(!root.join("Frameworks/new-runtime.txt").exists());
        assert_eq!(
            fs::read_to_string(root.join("wrapper-version")).unwrap(),
            "old-wrapper"
        );
        assert_eq!(
            fs::read(root.join("wswine.bundle/bin/wine")).unwrap(),
            b"old engine"
        );
        assert_eq!(
            fs::read_to_string(root.join("engine-version")).unwrap(),
            "old-engine"
        );
    }

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

    #[test]
    fn extension_plan_builds_the_documented_wine_helper_arguments() {
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
        let args: Vec<String> = invocation
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--client",
                r"C:\Program Files (x86)\SquareEnix\FINAL FANTASY XIV\ffxivgame.exe",
                "--module",
                r"C:\launcher\bahamut.dll",
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
        assert_eq!(args.len(), 19);
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

    #[test]
    fn derives_prefix_from_nested_game_dir() {
        let game = PathBuf::from(
            "/Users/me/Library/Application Support/com.BahamutXIV.Launcher/prefix/drive_c/Program Files (x86)/SquareEnix/FINAL FANTASY XIV",
        );
        assert_eq!(
            derive_prefix_from_game_location(&game),
            Some(PathBuf::from(
                "/Users/me/Library/Application Support/com.BahamutXIV.Launcher/prefix"
            ))
        );
    }

    #[test]
    fn no_prefix_when_no_drive_c() {
        assert_eq!(
            derive_prefix_from_game_location(&PathBuf::from("/tmp/ffxiv")),
            None
        );
    }

    #[test]
    fn install_needed_quadrants() {
        assert!(install_needed(false, None, ENGINE_NAME));
        assert!(install_needed(false, Some(ENGINE_NAME), ENGINE_NAME));
        assert!(!install_needed(true, Some(ENGINE_NAME), ENGINE_NAME));
        assert!(!install_needed(
            true,
            Some(&format!("{ENGINE_NAME}\n")),
            ENGINE_NAME
        ));
        assert!(install_needed(true, None, ENGINE_NAME));
        assert!(install_needed(
            true,
            Some("WS12WineCX24.0.7_7"),
            ENGINE_NAME
        ));
    }

    #[test]
    fn rejected_wrapper_digest_preserves_runtime_components_and_markers() {
        let server = MockServer::start();
        let body = b"bad wrapper archive";
        let wrapper = server.mock(|when, then| {
            when.method(GET).path("/wrapper");
            then.status(200).body(body.as_slice());
        });
        let engine = server.mock(|when, then| {
            when.method(GET).path("/engine");
            then.status(200).body("should not be requested");
        });
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        fs::create_dir_all(&root).unwrap();
        create_old_runtime(&root);

        let error = ensure_runtime_components_at(
            &root,
            test_pin(
                "test wrapper",
                &server.url("/wrapper"),
                body.len() as u64,
                "0000000000000000000000000000000000000000000000000000000000000000",
            ),
            test_pin("test engine", &server.url("/engine"), 1, "0"),
        )
        .unwrap_err();

        wrapper.assert();
        engine.assert_hits(0);
        assert!(error.to_string().contains("SHA-256 mismatch"));
        assert_old_runtime_unchanged(&root);
    }

    #[test]
    fn rejected_engine_digest_preserves_wrapper_engine_and_markers() {
        let server = MockServer::start();
        let wrapper_bytes = wrapper_archive();
        let wrapper = server.mock(|when, then| {
            when.method(GET).path("/wrapper");
            then.status(200).body(wrapper_bytes.as_slice());
        });
        let engine_bytes = b"bad engine archive";
        let engine = server.mock(|when, then| {
            when.method(GET).path("/engine");
            then.status(200).body(engine_bytes.as_slice());
        });
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        fs::create_dir_all(&root).unwrap();
        create_old_runtime(&root);
        let wrapper_sha256 = format!("{:x}", Sha256::digest(&wrapper_bytes));

        let error = ensure_runtime_components_at(
            &root,
            test_pin(
                "test wrapper",
                &server.url("/wrapper"),
                wrapper_bytes.len() as u64,
                &wrapper_sha256,
            ),
            test_pin(
                "test engine",
                &server.url("/engine"),
                engine_bytes.len() as u64,
                "0000000000000000000000000000000000000000000000000000000000000000",
            ),
        )
        .unwrap_err();

        wrapper.assert();
        engine.assert();
        assert!(error.to_string().contains("SHA-256 mismatch"));
        assert_old_runtime_unchanged(&root);
    }
}
