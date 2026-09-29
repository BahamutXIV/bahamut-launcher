//! Wine backends. The plain launch patches a working copy because the native launcher's
//! `WriteProcessMemory` cannot cross the Wine boundary (see `crate::launcher::pe_patch`); the
//! extension launch runs the x86 loader inside Wine, which patches the original client in memory.

#[cfg(target_os = "linux")]
pub use system::*;

#[cfg(target_os = "macos")]
pub use bundled::*;

/// Linux launch backend: run the client under `BAHAMUT_WINE`, else the managed Wine engine on
/// x86_64 hosts, else the system Wine on PATH.
#[cfg(target_os = "linux")]
mod system {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use crate::config::dirs;
    use crate::extensions::HelperInvocation;
    use crate::platform::wine_engine;
    use crate::platform::{LaunchError, LaunchedGame};

    const WINE_OVERRIDE_ENV: &str = "BAHAMUT_WINE";

    /// Default `WINEDEBUG`: includes `+debugstr` and `warn+d3d`, omits `+seh` noise.
    const WINEDEBUG_DEFAULT: &str = "fixme-all,err+all,+debugstr,warn+d3d";

    const WINEDEBUG_VERBOSE: &str = "err+all,+seh,+tid,+loaddll,+module,+winsock,+ws2_32";

    pub struct WineRuntime {
        pub wine_bin: PathBuf,
        pub prefix: PathBuf,
    }

    impl WineRuntime {
        pub fn discover(game_dir: &Path) -> Result<Self, LaunchError> {
            let wine_bin = which_wine()?;
            let prefix = resolve_prefix(game_dir)?;
            Ok(Self { wine_bin, prefix })
        }

        fn configure_env(&self, cmd: &mut Command, wine_debug: Option<&str>) {
            cmd.env("WINEPREFIX", &self.prefix);
            if let Some(value) = wine_debug {
                cmd.env("WINEDEBUG", value);
            } else if std::env::var_os("WINEDEBUG").is_none() {
                cmd.env("WINEDEBUG", WINEDEBUG_DEFAULT);
            }
        }

        /// Run `wineboot --init` only when `system.reg` is absent; existing user-managed prefixes are untouched.
        pub fn ensure_prefix_initialized(&self) -> Result<(), LaunchError> {
            if self.prefix.join("system.reg").exists() {
                return Ok(());
            }
            std::fs::create_dir_all(&self.prefix).map_err(|source| LaunchError::Io {
                context: "creating the Wine prefix directory",
                source,
            })?;
            tracing::info!(prefix = %self.prefix.display(), "initialising Wine prefix (wineboot --init)");
            let mut cmd = Command::new(&self.wine_bin);
            cmd.arg("wineboot").arg("--init");
            self.configure_env(&mut cmd, None);
            let status = cmd.status().map_err(|source| LaunchError::Io {
                context: "running wineboot --init",
                source,
            })?;
            if !status.success() {
                return Err(LaunchError::WinePrefix(format!(
                    "wineboot --init exited with {status}"
                )));
            }
            Ok(())
        }

        /// Spawn the patched client without waiting; append its exit status to the per-launch log and report a non-zero exit with the log tail.
        pub fn launch(
            &self,
            exe_path: &Path,
            encoded_argument: &str,
            verbose_wine_debug: bool,
            dxvk_overrides: Option<&str>,
        ) -> Result<LaunchedGame, LaunchError> {
            use std::fs::OpenOptions;
            use std::io::Write;
            use std::process::Stdio;

            let wine_debug = verbose_wine_debug.then_some(WINEDEBUG_VERBOSE);

            let log_path = wine_log_path()?;
            let log_file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&log_path)
                .map_err(|source| LaunchError::Io {
                    context: "opening the Wine log",
                    source,
                })?;
            // Redact the launch token: its legacy encryption is quickly recoverable and the log is persisted.
            let _ = writeln!(
                &log_file,
                "=== bahamut-launcher wine launch ===\nwine:   {}\nexe:    {}\narg:    <redacted: encrypted session launch token>\nprefix: {}\nWINEDEBUG: {}\n",
                self.wine_bin.display(),
                exe_path.display(),
                self.prefix.display(),
                wine_debug.unwrap_or("(default)"),
            );
            let stdout_log = log_file.try_clone().map_err(|source| LaunchError::Io {
                context: "cloning the Wine log for stdout",
                source,
            })?;
            let stderr_log = log_file.try_clone().map_err(|source| LaunchError::Io {
                context: "cloning the Wine log for stderr",
                source,
            })?;

            let mut cmd = Command::new(&self.wine_bin);
            cmd.arg(exe_path)
                .arg(encoded_argument)
                .stdout(Stdio::from(stdout_log))
                .stderr(Stdio::from(stderr_log));
            if let Some(cwd) = exe_path.parent() {
                cmd.current_dir(cwd);
            }
            self.configure_env(&mut cmd, wine_debug);

            // Scope `WINEDLLOVERRIDES` to this launch; `n,b` prefers DXVK and falls back to builtin DLLs.
            if let Some(overrides) = dxvk_overrides {
                tracing::info!(overrides, "WINEDLLOVERRIDES");
                cmd.env("WINEDLLOVERRIDES", overrides);
            }

            let mut child = cmd.spawn().map_err(|source| LaunchError::Io {
                context: "launching the client via Wine",
                source,
            })?;
            let pid = child.id();
            tracing::info!(pid, wine = %self.wine_bin.display(), log = %log_path.display(), "client launched via Wine");
            let (game, exited) = LaunchedGame::pending(pid);
            let started = std::time::Instant::now();
            // An orderly early exit prints nothing under the default WINEDEBUG filter, so the status is the only trace of it.
            if let Err(error) = std::thread::Builder::new()
                .name("bahamut-wine-session".to_owned())
                .spawn(move || match child.wait() {
                    Ok(status) => {
                        let elapsed = started.elapsed();
                        let _ = writeln!(&log_file, "\n=== ffxivgame exit: {status} after {elapsed:.1?} ===");
                        if status.success() {
                            tracing::info!(pid, elapsed_ms = elapsed.as_millis() as u64, "ffxivgame under Wine exited");
                        } else {
                            let tail = read_log_tail(&log_path, WINE_LOG_TAIL_LINES);
                            tracing::error!(
                                "ffxivgame exited with an error ({status}) after {elapsed:.1?}; see {}\n{tail}",
                                log_path.display(),
                            );
                        }
                        let _ = exited.send(());
                    }
                    Err(error) => {
                        tracing::error!(%error, "waiting on ffxivgame under Wine failed");
                    }
                })
            {
                tracing::error!(%error, "could not start Wine session monitor");
            }
            Ok(game)
        }

        /// Run the x86 extension helper under the selected Wine; returns once `helper.log` reports readiness.
        pub fn launch_extension_helper(
            &self,
            invocation: &HelperInvocation,
            verbose_wine_debug: bool,
            dxvk_overrides: Option<&str>,
        ) -> Result<LaunchedGame, LaunchError> {
            let wine_log = wine_log_path()?;
            let mut cmd = Command::new(&self.wine_bin);
            self.configure_env(&mut cmd, verbose_wine_debug.then_some(WINEDEBUG_VERBOSE));
            if let Some(overrides) = dxvk_overrides {
                tracing::info!(overrides, "WINEDLLOVERRIDES");
                cmd.env("WINEDLLOVERRIDES", overrides);
            }
            super::extension_launch::launch_wait_mode_helper(
                cmd,
                invocation,
                &wine_log,
                super::extension_launch::WINE_HELPER_READINESS_DEADLINE,
            )
        }
    }

    const WINE_LOG_TAIL_LINES: usize = 12;

    fn read_log_tail(log_path: &Path, max_lines: usize) -> String {
        match std::fs::read_to_string(log_path) {
            Ok(text) => {
                let lines: Vec<&str> = text.lines().collect();
                let start = lines.len().saturating_sub(max_lines);
                lines[start..].join("\n")
            }
            Err(_) => String::new(),
        }
    }

    /// Copy the client to a patchable working copy, leaving the original untouched.
    pub fn copy_exe_for_patching(source_exe: &Path, dest_exe: &Path) -> Result<(), LaunchError> {
        if let Some(parent) = dest_exe.parent() {
            std::fs::create_dir_all(parent).map_err(|source| LaunchError::Io {
                context: "creating the working-copy directory",
                source,
            })?;
        }
        std::fs::copy(source_exe, dest_exe).map_err(|source| LaunchError::Io {
            context: "copying the client to a working copy",
            source,
        })?;
        Ok(())
    }

    /// Return 32-bit `CLOCK_BOOTTIME` milliseconds since boot: Wine 11's `GetTickCount` keeps counting through suspend, where `CLOCK_MONOTONIC` falls behind (docs/handshake.md#wine-launch).
    pub fn monotonic_ms_since_boot() -> u32 {
        let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
        if rc != 0 {
            return 0;
        }
        let ms = (ts.tv_sec as u64).wrapping_mul(1_000) + (ts.tv_nsec as u64 / 1_000_000);
        ms as u32
    }

    /// Reuse a prefix above `drive_c`; otherwise use the launcher's managed prefix.
    fn resolve_prefix(game_dir: &Path) -> Result<PathBuf, LaunchError> {
        for ancestor in game_dir.ancestors() {
            if ancestor.file_name().and_then(|n| n.to_str()) == Some("drive_c")
                && let Some(prefix) = ancestor.parent()
            {
                return Ok(prefix.to_path_buf());
            }
        }
        data_subdir("prefix")
    }

    fn data_subdir(name: &str) -> Result<PathBuf, LaunchError> {
        dirs::data_dir().map(|dir| dir.join(name)).map_err(|e| {
            LaunchError::WinePrefix(format!("resolving the launcher data directory: {e}"))
        })
    }

    fn wine_log_path() -> Result<PathBuf, LaunchError> {
        let dir = data_subdir("logs")?;
        std::fs::create_dir_all(&dir).map_err(|source| LaunchError::Io {
            context: "creating the Wine log directory",
            source,
        })?;
        Ok(dir.join("wine.log"))
    }

    fn wine_version(wine_bin: &Path) -> Option<String> {
        let out = Command::new(wine_bin).arg("--version").output().ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Why a Wine binary was selected.
    #[derive(Debug, PartialEq, Eq)]
    enum WineSource {
        Override,
        ManagedEngine,
        SystemFallback { reason: String },
    }

    /// Outcome of offering the managed engine on this host.
    #[derive(Debug, PartialEq, Eq)]
    enum ManagedEngine {
        Unsupported,
        Installed(PathBuf),
        Failed(String),
    }

    /// The pinned engine is an x86_64 build; other hosts never download it.
    fn managed_engine(
        host_is_x86_64: bool,
        install: impl FnOnce() -> Result<PathBuf, String>,
    ) -> ManagedEngine {
        if !host_is_x86_64 {
            return ManagedEngine::Unsupported;
        }
        match install() {
            Ok(engine_dir) => ManagedEngine::Installed(wine_engine::wine_binary(&engine_dir)),
            Err(error) => ManagedEngine::Failed(error),
        }
    }

    /// Selection order: override, managed engine, system Wine. The later inputs are only
    /// evaluated when needed, so an override never triggers an engine download.
    fn select_wine(
        override_path: Option<PathBuf>,
        managed: impl FnOnce() -> ManagedEngine,
        system: impl FnOnce() -> Option<PathBuf>,
    ) -> Result<(PathBuf, WineSource), LaunchError> {
        if let Some(path) = override_path {
            if path.is_file() {
                return Ok((path, WineSource::Override));
            }
            return Err(LaunchError::WineNotFound(format!(
                "{WINE_OVERRIDE_ENV} is set but {} is not a file",
                path.display()
            )));
        }

        let reason = match managed() {
            ManagedEngine::Installed(path) => return Ok((path, WineSource::ManagedEngine)),
            ManagedEngine::Failed(error) => {
                format!("the managed Wine engine could not be installed: {error}")
            }
            ManagedEngine::Unsupported => {
                "the managed Wine engine is only available on x86_64 hosts".to_string()
            }
        };

        match system() {
            Some(path) => Ok((path, WineSource::SystemFallback { reason })),
            None => Err(LaunchError::WineNotFound(format!(
                "{reason}, and no `wine` binary is on PATH; install Wine 7 or newer with \
                 32-bit support, or set {WINE_OVERRIDE_ENV} to a wine binary"
            ))),
        }
    }

    fn which_wine() -> Result<PathBuf, LaunchError> {
        let (path, source) = select_wine(
            std::env::var_os(WINE_OVERRIDE_ENV).map(PathBuf::from),
            || managed_engine(cfg!(target_arch = "x86_64"), install_managed_engine),
            system_wine_on_path,
        )?;
        let why = match &source {
            WineSource::Override => WINE_OVERRIDE_ENV,
            WineSource::ManagedEngine => "managed engine",
            WineSource::SystemFallback { reason } => {
                tracing::warn!("{reason}; falling back to system Wine {}", path.display());
                "system fallback"
            }
        };
        tracing::info!(
            wine = %path.display(),
            version = wine_version(&path).as_deref().unwrap_or("unknown"),
            source = why,
            "using Wine"
        );
        Ok(path)
    }

    fn install_managed_engine() -> Result<PathBuf, String> {
        let cache_root = data_subdir("runtime").map_err(|e| e.to_string())?;
        wine_engine::ensure_engine(&cache_root)
    }

    fn system_wine_on_path() -> Option<PathBuf> {
        let out = match Command::new("sh").arg("-c").arg("command -v wine").output() {
            Ok(out) => out,
            Err(e) => {
                tracing::warn!("locating wine via `command -v`: {e}");
                return None;
            }
        };
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!text.is_empty()).then(|| PathBuf::from(text))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn unreachable_managed() -> ManagedEngine {
            panic!("the managed engine must not be consulted")
        }

        fn unreachable_system() -> Option<PathBuf> {
            panic!("system Wine must not be probed")
        }

        fn not_found_message(result: Result<(PathBuf, WineSource), LaunchError>) -> String {
            match result {
                Err(LaunchError::WineNotFound(message)) => message,
                other => panic!("expected WineNotFound, got {other:?}"),
            }
        }

        #[test]
        fn override_file_wins_without_consulting_the_engine() {
            let temp = tempfile::tempdir().unwrap();
            let wine = temp.path().join("wine");
            std::fs::write(&wine, b"").unwrap();

            let (path, source) =
                select_wine(Some(wine.clone()), unreachable_managed, unreachable_system).unwrap();

            assert_eq!(path, wine);
            assert_eq!(source, WineSource::Override);
        }

        #[test]
        fn override_that_is_not_a_file_is_an_error() {
            let temp = tempfile::tempdir().unwrap();

            let message = not_found_message(select_wine(
                Some(temp.path().to_path_buf()),
                unreachable_managed,
                unreachable_system,
            ));

            assert!(message.contains(WINE_OVERRIDE_ENV), "got {message:?}");
        }

        #[test]
        fn installed_engine_is_used_before_system_wine() {
            let engine = PathBuf::from("/cache/wine-11.18-x/bin/wine");

            let (path, source) = select_wine(
                None,
                || ManagedEngine::Installed(engine.clone()),
                unreachable_system,
            )
            .unwrap();

            assert_eq!(path, engine);
            assert_eq!(source, WineSource::ManagedEngine);
        }

        #[test]
        fn failed_engine_falls_back_to_system_wine_with_the_reason() {
            let system = PathBuf::from("/usr/bin/wine");

            let (path, source) = select_wine(
                None,
                || ManagedEngine::Failed("SHA-256 mismatch".to_string()),
                || Some(system.clone()),
            )
            .unwrap();

            assert_eq!(path, system);
            match source {
                WineSource::SystemFallback { reason } => {
                    assert!(reason.contains("SHA-256 mismatch"), "got {reason:?}")
                }
                other => panic!("expected a system fallback, got {other:?}"),
            }
        }

        #[test]
        fn unsupported_host_falls_back_to_system_wine() {
            let system = PathBuf::from("/usr/bin/wine");

            let (path, source) =
                select_wine(None, || ManagedEngine::Unsupported, || Some(system.clone())).unwrap();

            assert_eq!(path, system);
            match source {
                WineSource::SystemFallback { reason } => {
                    assert!(reason.contains("x86_64"), "got {reason:?}")
                }
                other => panic!("expected a system fallback, got {other:?}"),
            }
        }

        #[test]
        fn no_engine_and_no_system_wine_names_the_engine_failure() {
            let message = not_found_message(select_wine(
                None,
                || ManagedEngine::Failed("GET timed out".to_string()),
                || None,
            ));

            assert!(message.contains("GET timed out"), "got {message:?}");
            assert!(message.contains("Wine 7 or newer"), "got {message:?}");
            assert!(message.contains(WINE_OVERRIDE_ENV), "got {message:?}");
        }

        #[test]
        fn unsupported_host_without_system_wine_is_an_error() {
            let message =
                not_found_message(select_wine(None, || ManagedEngine::Unsupported, || None));

            assert!(message.contains("x86_64"), "got {message:?}");
            assert!(message.contains("Wine 7 or newer"), "got {message:?}");
        }

        #[test]
        fn managed_engine_is_only_installed_on_x86_64() {
            assert_eq!(
                managed_engine(false, || panic!("must not install off x86_64")),
                ManagedEngine::Unsupported
            );
            assert_eq!(
                managed_engine(true, || Ok(PathBuf::from("/cache/engine"))),
                ManagedEngine::Installed(PathBuf::from("/cache/engine/bin/wine"))
            );
            assert_eq!(
                managed_engine(true, || Err("no network".to_string())),
                ManagedEngine::Failed("no network".to_string())
            );
        }

        /// /proc/uptime's first field is the kernel boottime clock, the same clock as the launch tick.
        #[test]
        fn launch_tick_tracks_proc_uptime() {
            let uptime =
                std::fs::read_to_string("/proc/uptime").expect("/proc/uptime is readable on Linux");
            let seconds: f64 = uptime
                .split_whitespace()
                .next()
                .expect("uptime field")
                .parse()
                .expect("uptime seconds");
            // A direct f64 -> u32 cast saturates past 49.7 days; go through u64 so it wraps like the tick.
            let uptime_ms = (seconds * 1000.0) as u64 as u32;
            let tick = monotonic_ms_since_boot();
            let skew = tick.wrapping_sub(uptime_ms) as i32;
            assert!(
                skew.abs() < 5_000,
                "tick {tick} ms vs /proc/uptime {uptime_ms} ms: skew {skew} ms"
            );
        }
    }
}

/// macOS launch backend: run the client under a bundled Sikarugir Wine engine.
#[cfg(target_os = "macos")]
mod bundled {
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use object::read::pe::PeFile32;
    use object::{Object, ObjectSection};

    use crate::extensions::HelperInvocation;
    use crate::launcher::pe_patch::PePatch;
    use crate::platform::{LaunchError, LaunchedGame};

    /// Default `WINEDEBUG` for the bundled runtime, including assert and D3D diagnostics.
    const WINEDEBUG_DEFAULT: &str = "fixme-all,err+all,+debugstr,warn+d3d";

    /// InstallShield-default FFXIV path inside the Wine prefix.
    pub const PREFIX_FFXIV_SUBPATH: &str =
        "drive_c/Program Files (x86)/SquareEnix/FINAL FANTASY XIV";

    fn wine_err(context: impl Into<String>) -> LaunchError {
        LaunchError::Wine(context.into())
    }

    /// Return the `CLOCK_MONOTONIC` millisecond tick matching Wine's `GetTickCount()` and launch-key derivation.
    pub fn monotonic_ms_since_boot() -> u32 {
        let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        if rc != 0 {
            return 0;
        }
        let ms = (ts.tv_sec as u64).wrapping_mul(1_000) + (ts.tv_nsec as u64 / 1_000_000);
        ms as u32
    }

    pub struct WineRuntime {
        #[allow(dead_code)]
        pub root: PathBuf,
        pub prefix: PathBuf,
        pub wine_bin: PathBuf,
        #[allow(dead_code)]
        pub wineserver_bin: PathBuf,
        /// Extra macOS `DYLD_FALLBACK_LIBRARY_PATH` entries.
        pub dyld_fallback_paths: Vec<PathBuf>,
        /// Bundled `gstreamer-1.0` directory exported through both GST plugin variables.
        pub gst_plugin_path: Option<PathBuf>,
    }

    impl WineRuntime {
        pub fn configure_command(&self, cmd: &mut Command) {
            self.configure_command_with_debug(cmd, None);
        }

        pub fn configure_command_with_debug(&self, cmd: &mut Command, wine_debug: Option<&str>) {
            cmd.env("WINEPREFIX", &self.prefix);
            if let Some(value) = wine_debug {
                cmd.env("WINEDEBUG", value);
            } else if std::env::var_os("WINEDEBUG").is_none() {
                cmd.env("WINEDEBUG", WINEDEBUG_DEFAULT);
            }
            if !self.dyld_fallback_paths.is_empty()
                && let Ok(joined) = std::env::join_paths(&self.dyld_fallback_paths)
            {
                cmd.env("DYLD_FALLBACK_LIBRARY_PATH", joined);
            }
            if let Some(plugin_dir) = &self.gst_plugin_path {
                cmd.env("GST_PLUGIN_PATH", plugin_dir);
                cmd.env("GST_PLUGIN_SYSTEM_PATH", plugin_dir);
            }
        }
    }

    /// Initialize the prefix only when `system.reg` is absent.
    pub fn ensure_prefix_initialized(runtime: &WineRuntime) -> Result<(), LaunchError> {
        if runtime.prefix.join("system.reg").exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&runtime.prefix)
            .map_err(|e| wine_err(format!("creating prefix {}: {e}", runtime.prefix.display())))?;
        let mut cmd = Command::new(&runtime.wine_bin);
        cmd.arg("wineboot").arg("--init");
        runtime.configure_command(&mut cmd);
        let status = cmd
            .status()
            .map_err(|e| wine_err(format!("running wineboot --init: {e}")))?;
        if !status.success() {
            return Err(wine_err(format!("wineboot --init exited with {status:?}")));
        }
        Ok(())
    }

    /// Spawn the patched client without waiting; report early non-zero exits with the Wine log tail.
    pub fn launch_ffxiv_game(
        runtime: &WineRuntime,
        exe_path: &Path,
        encoded_argument: &str,
        wine_debug_override: Option<&str>,
    ) -> Result<LaunchedGame, LaunchError> {
        let log_path = wine_log_path()?;
        let log_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log_path)
            .map_err(|e| wine_err(format!("opening wine log {}: {e}", log_path.display())))?;
        // Redact the launch token: its legacy encryption is quickly recoverable and the log is tailed to the UI.
        writeln!(
            &log_file,
            "=== bahamut-launcher launch ===\nwine: {}\nexe:  {}\narg:  <redacted: encrypted session launch token>\nprefix: {}\n",
            runtime.wine_bin.display(),
            exe_path.display(),
            runtime.prefix.display(),
        )
        .ok();

        let stdout_log = log_file
            .try_clone()
            .map_err(|e| wine_err(format!("cloning wine log fd: {e}")))?;
        let stderr_log = log_file
            .try_clone()
            .map_err(|e| wine_err(format!("cloning wine log fd: {e}")))?;

        let mut cmd = Command::new(&runtime.wine_bin);
        cmd.arg(exe_path)
            .arg(encoded_argument)
            .stdout(Stdio::from(stdout_log))
            .stderr(Stdio::from(stderr_log));
        if let Some(cwd) = exe_path.parent() {
            cmd.current_dir(cwd);
        }
        runtime.configure_command_with_debug(&mut cmd, wine_debug_override);

        tracing::info!(log = %log_path.display(), "launching ffxivgame via wine");
        let mut child = cmd
            .spawn()
            .map_err(|e| wine_err(format!("launching ffxivgame.exe via wine: {e}")))?;
        let pid = child.id();
        tracing::info!(pid, log = %log_path.display(), "ffxivgame launched via wine");
        let (game, exited) = LaunchedGame::pending(pid);

        // Watch off the launch path so IPC stays non-blocking; report early non-zero exits with the log tail.
        if let Err(error) = std::thread::Builder::new()
            .name("bahamut-wine-session".to_owned())
            .spawn(move || {
                let exited_cleanly = match child.wait() {
                    Ok(status) => {
                        let _ = writeln!(&log_file, "\n=== exit: {status:?} ===");
                        if !status.success() {
                            let tail = read_log_tail(&log_path, WINE_LOG_TAIL_LINES);
                            tracing::error!(
                                "ffxivgame exited with an error ({status}); see {}\n{tail}",
                                log_path.display(),
                            );
                        }
                        true
                    }
                    Err(e) => {
                        tracing::error!("waiting on ffxivgame under wine failed: {e}");
                        false
                    }
                };
                if exited_cleanly {
                    let _ = exited.send(());
                }
            })
        {
            tracing::error!(%error, "could not start Wine session monitor");
        }
        Ok(game)
    }

    /// Run the x86 extension helper under the bundled runtime; returns once `helper.log` reports readiness.
    pub fn launch_extension_helper(
        runtime: &WineRuntime,
        invocation: &HelperInvocation,
    ) -> Result<LaunchedGame, LaunchError> {
        let wine_log = wine_log_path()?;
        let mut cmd = Command::new(&runtime.wine_bin);
        runtime.configure_command(&mut cmd);
        super::extension_launch::launch_wait_mode_helper(
            cmd,
            invocation,
            &wine_log,
            super::extension_launch::WINE_HELPER_READINESS_DEADLINE,
        )
    }

    const WINE_LOG_TAIL_LINES: usize = 12;

    fn read_log_tail(log_path: &Path, max_lines: usize) -> String {
        match std::fs::read_to_string(log_path) {
            Ok(text) => {
                let lines: Vec<&str> = text.lines().collect();
                let start = lines.len().saturating_sub(max_lines);
                lines[start..].join("\n")
            }
            Err(_) => String::new(),
        }
    }

    fn wine_log_path() -> Result<PathBuf, LaunchError> {
        let dir = crate::config::dirs::data_dir()
            .map_err(|e| wine_err(format!("resolving data dir: {e}")))?
            .join("logs");
        std::fs::create_dir_all(&dir)
            .map_err(|e| wine_err(format!("creating wine log dir {}: {e}", dir.display())))?;
        Ok(dir.join("wine.log"))
    }

    /// Replace `dest_exe` with a fresh patch working copy of `source_exe`.
    pub fn copy_exe_for_patching(source_exe: &Path, dest_exe: &Path) -> Result<(), LaunchError> {
        if let Some(parent) = dest_exe.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| wine_err(format!("creating dir {}: {e}", parent.display())))?;
        }
        std::fs::copy(source_exe, dest_exe).map_err(|e| {
            wine_err(format!(
                "copying {} -> {}: {e}",
                source_exe.display(),
                dest_exe.display()
            ))
        })?;
        Ok(())
    }

    /// Write patches to a working copy at the file offsets mapped from their RVAs.
    /// Destructive: never pass the original binary.
    pub fn apply_patches_on_disk(exe_path: &Path, patches: &[PePatch]) -> Result<(), LaunchError> {
        let data = std::fs::read(exe_path)
            .map_err(|e| wine_err(format!("reading {}: {e}", exe_path.display())))?;
        let pe = PeFile32::parse(&*data)
            .map_err(|e| wine_err(format!("parsing PE headers of {}: {e}", exe_path.display())))?;

        let mut plan: Vec<(u64, Vec<u8>)> = Vec::with_capacity(patches.len());
        for patch in patches {
            let file_offset = rva_to_file_offset(&pe, patch.rva).ok_or_else(|| {
                wine_err(format!(
                    "RVA 0x{:X} is not mapped to any PE section of {}",
                    patch.rva,
                    exe_path.display()
                ))
            })?;
            tracing::info!(
                "PE patch: RVA 0x{:X} -> file offset 0x{:X} ({} bytes)",
                patch.rva,
                file_offset,
                patch.bytes.len(),
            );
            plan.push((file_offset, patch.bytes.clone()));
        }

        let mut file = OpenOptions::new()
            .write(true)
            .open(exe_path)
            .map_err(|e| wine_err(format!("opening {} for writing: {e}", exe_path.display())))?;
        for (offset, bytes) in plan {
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| wine_err(format!("seeking to 0x{offset:X}: {e}")))?;
            file.write_all(&bytes)
                .map_err(|e| wine_err(format!("writing patch at 0x{offset:X}: {e}")))?;
        }
        file.flush()
            .map_err(|e| wine_err(format!("flushing {}: {e}", exe_path.display())))?;
        Ok(())
    }

    fn rva_to_file_offset(pe: &PeFile32<'_>, rva: u32) -> Option<u64> {
        // Normalize `ObjectSection::address()` by the PE image base before comparing RVAs.
        let image_base = pe.relative_address_base() as u32;
        for section in pe.sections() {
            let section_rva = (section.address() as u32).checked_sub(image_base)?;
            let vsize = section.size() as u32;
            if rva >= section_rva && rva < section_rva.saturating_add(vsize) {
                let (file_offset, _) = section.file_range()?;
                return Some(file_offset + (rva - section_rva) as u64);
            }
        }
        None
    }
}

/// Extension launch shared by the Wine backends: DOS path mapping and the wait-mode helper session.
pub mod extension_launch {
    use std::ffi::{OsStr, OsString};
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use crate::extensions::{
        HelperErrorCode, HelperFailure, HelperInvocation, HelperPlanError, parse_wait_mode_line,
    };
    use crate::platform::{LaunchError, LaunchedGame};

    /// Loader `--timeout-ms` under Wine; the loader's native ready timeout is too short for Wine start-up.
    pub const WINE_HELPER_READY_TIMEOUT_MS: u32 = 10_000;

    /// Launcher-side deadline for the helper's first `SUCCESS` or `ERROR` line; a Wine-level hang prints neither.
    pub const WINE_HELPER_READINESS_DEADLINE: Duration = Duration::from_secs(90);

    /// Helper stdout, beside `wine.log`: the protocol stream, kept apart from Wine's debug output.
    pub const HELPER_LOG_FILE_NAME: &str = "helper.log";

    const HELPER_LOG_HEADER: &str = "=== bahamut-launcher extension helper stdout ===";

    const READINESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

    /// Bytes a Windows file name cannot hold, so a host component containing one has no DOS spelling.
    const NON_DOS_NAME_BYTES: &[u8] = b"\\:*?\"<>|";

    /// Drive roots read from `<prefix>/dosdevices`, used to give the x86 helper DOS paths.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WineDosPaths {
        drives: Vec<(char, PathBuf)>,
    }

    impl WineDosPaths {
        /// Resolve every single-letter drive link; `x::` raw-device entries and dangling links are skipped.
        pub fn from_prefix(prefix: &Path) -> Result<Self, LaunchError> {
            let dosdevices = prefix.join("dosdevices");
            let entries = fs::read_dir(&dosdevices).map_err(|error| {
                LaunchError::WinePrefix(format!("reading {}: {error}", dosdevices.display()))
            })?;
            let mut drives = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|error| {
                    LaunchError::WinePrefix(format!("reading {}: {error}", dosdevices.display()))
                })?;
                let Some(letter) = drive_letter(&entry.file_name()) else {
                    continue;
                };
                if let Ok(root) = fs::canonicalize(entry.path()) {
                    drives.push((letter, root));
                }
            }
            if drives.is_empty() {
                return Err(LaunchError::WinePrefix(format!(
                    "{} has no resolvable drive links",
                    dosdevices.display()
                )));
            }
            drives.sort();
            Ok(Self { drives })
        }

        /// Map an absolute host path to `X:\...` through the longest matching drive root, as Wine does.
        /// Equal roots resolve to the lower letter; a component a Windows file name cannot hold yields `None`.
        pub fn to_dos(&self, path: &Path) -> Option<OsString> {
            if !path.is_absolute() {
                return None;
            }
            let resolved = canonicalize_existing_ancestor(path)?;
            let (letter, root) = self
                .drives
                .iter()
                .filter(|(_, root)| resolved.starts_with(root))
                .max_by(|(left_letter, left), (right_letter, right)| {
                    left.components()
                        .count()
                        .cmp(&right.components().count())
                        .then(right_letter.cmp(left_letter))
                })?;
            let mut dos = OsString::from(format!("{letter}:"));
            let mut relative = resolved.strip_prefix(root).ok()?.components().peekable();
            if relative.peek().is_none() {
                dos.push("\\");
            }
            for component in relative {
                let name = component.as_os_str();
                if name
                    .as_encoded_bytes()
                    .iter()
                    .any(|byte| NON_DOS_NAME_BYTES.contains(byte))
                {
                    return None;
                }
                dos.push("\\");
                dos.push(name);
            }
            Some(dos)
        }

        pub fn map(&self, field: &'static str, path: &Path) -> Result<OsString, HelperPlanError> {
            self.to_dos(path).ok_or_else(|| HelperPlanError::GuestPath {
                field,
                path: path.to_path_buf(),
            })
        }
    }

    fn drive_letter(name: &OsStr) -> Option<char> {
        match name.to_str()?.as_bytes() {
            [letter, b':'] if letter.is_ascii_alphabetic() => {
                Some(char::from(letter.to_ascii_uppercase()))
            }
            _ => None,
        }
    }

    /// Canonicalize the deepest existing ancestor and re-append the missing plain names.
    fn canonicalize_existing_ancestor(path: &Path) -> Option<PathBuf> {
        let mut missing = Vec::new();
        let mut current = path;
        loop {
            if let Ok(mut resolved) = fs::canonicalize(current) {
                resolved.extend(missing.iter().rev());
                return Some(resolved);
            }
            missing.push(current.file_name()?.to_owned());
            current = current.parent()?;
        }
    }

    fn create_log(path: &Path, context: &'static str) -> Result<File, LaunchError> {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .map_err(|source| LaunchError::Io { context, source })
    }

    /// Spawn `wine <helper> <args>` in wait mode: helper stdout goes to [`HELPER_LOG_FILE_NAME`]
    /// beside `wine_log_path`, stderr (including Wine's debug channels) to `wine_log_path`.
    ///
    /// `command` runs the Wine binary with the runtime environment. Readiness is read only from the
    /// helper log, past the launcher's header. The returned pid is the host `wine` process, which
    /// the helper keeps alive until the client exits.
    pub fn launch_wait_mode_helper(
        mut command: Command,
        invocation: &HelperInvocation,
        wine_log_path: &Path,
        readiness_deadline: Duration,
    ) -> Result<LaunchedGame, LaunchError> {
        let helper_log_path = wine_log_path.with_file_name(HELPER_LOG_FILE_NAME);
        let wine_log = create_log(wine_log_path, "opening the Wine log")?;
        let mut helper_log = create_log(&helper_log_path, "opening the helper log")?;
        let prefix = command
            .get_envs()
            .find(|(name, _)| *name == "WINEPREFIX")
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        // The helper arguments carry the session launch token, so only paths are logged.
        let _ = writeln!(
            &wine_log,
            "=== bahamut-launcher extension launch ===\nwine:   {}\nhelper: {}\ncwd:    {}\nargs:   <redacted: includes the encrypted session launch token>\nprefix: {prefix}\nstdout: {}\n",
            command.get_program().to_string_lossy(),
            invocation.program.display(),
            invocation.current_dir.display(),
            helper_log_path.display(),
        );
        let _ = writeln!(helper_log, "{HELPER_LOG_HEADER}");
        let protocol_start = helper_log
            .stream_position()
            .map_err(|source| LaunchError::Io {
                context: "reading the helper log position",
                source,
            })?;
        let mut protocol = File::open(&helper_log_path).map_err(|source| LaunchError::Io {
            context: "reading the helper log",
            source,
        })?;
        protocol
            .seek(SeekFrom::Start(protocol_start))
            .map_err(|source| LaunchError::Io {
                context: "reading the helper log",
                source,
            })?;
        let stderr_log = wine_log.try_clone().map_err(|source| LaunchError::Io {
            context: "cloning the Wine log for stderr",
            source,
        })?;
        command
            .arg(&invocation.program)
            .args(&invocation.args)
            .envs(
                invocation
                    .environment
                    .iter()
                    .map(|(name, value)| (name, value)),
            )
            .current_dir(&invocation.current_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(helper_log))
            .stderr(Stdio::from(stderr_log));

        let logs = format!(
            "{} and {}",
            helper_log_path.display(),
            wine_log_path.display()
        );
        tracing::info!(
            helper = %invocation.program.display(),
            log = %wine_log_path.display(),
            helper_log = %helper_log_path.display(),
            "spawning the x86 extension helper via Wine"
        );
        let mut child = command.spawn().map_err(|source| LaunchError::Io {
            context: "launching the x86 extension helper via Wine",
            source,
        })?;
        let wine_pid = child.id();
        let client_pid = match await_readiness(&mut child, &mut protocol, &logs, readiness_deadline)
        {
            Ok(client_pid) => client_pid,
            Err(failure) => {
                reap_in_background(child);
                return Err(LaunchError::ExtensionBootstrap(failure));
            }
        };
        tracing::info!(
            pid = wine_pid,
            client_win32_pid = client_pid,
            log = %wine_log_path.display(),
            helper_log = %helper_log_path.display(),
            "client launched with extensions via Wine"
        );

        let (game, exited) = LaunchedGame::pending(wine_pid);
        if let Err(error) = std::thread::Builder::new()
            .name("bahamut-wine-session".to_owned())
            .spawn(move || match child.wait() {
                Ok(status) => {
                    let _ = writeln!(&wine_log, "\n=== exit: {status:?} ===");
                    if !status.success() {
                        tracing::error!("ffxivgame exited with an error ({status}); see {logs}");
                    }
                    let _ = exited.send(());
                }
                Err(error) => {
                    tracing::error!("waiting on the extension helper under Wine failed: {error}");
                }
            })
        {
            tracing::error!(%error, "could not start Wine session monitor");
        }
        Ok(game)
    }

    fn helper_failure(detail: String) -> HelperFailure {
        HelperFailure {
            code: HelperErrorCode::Unknown,
            detail,
        }
    }

    /// Poll the helper log for the first `SUCCESS` or `ERROR` line, the helper's early exit, or the deadline.
    fn await_readiness(
        child: &mut Child,
        protocol: &mut File,
        logs: &str,
        deadline: Duration,
    ) -> Result<u32, HelperFailure> {
        let mut lines = LogLines::default();
        let started = Instant::now();
        loop {
            if let Some(result) = lines.scan(protocol, false) {
                return result;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    if let Some(result) = lines.scan(protocol, true) {
                        return result;
                    }
                    return Err(helper_failure(format!(
                        "helper exited with {status} before reporting readiness; see {logs}"
                    )));
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(helper_failure(format!(
                        "could not poll the helper process: {error}"
                    )));
                }
            }
            if started.elapsed() >= deadline {
                let _ = child.kill();
                tracing::warn!(
                    "the helper missed its readiness deadline, so a client it already created may still be running in the Wine prefix where the launcher cannot reach it"
                );
                return Err(helper_failure(format!(
                    "helper reported neither SUCCESS nor ERROR within {} ms; see {logs}",
                    deadline.as_millis()
                )));
            }
            std::thread::sleep(READINESS_POLL_INTERVAL);
        }
    }

    /// Incremental line reader over a log that the helper is still appending to.
    #[derive(Default)]
    struct LogLines {
        pending: Vec<u8>,
    }

    impl LogLines {
        fn scan(&mut self, log: &mut File, final_scan: bool) -> Option<Result<u32, HelperFailure>> {
            if let Err(error) = log.read_to_end(&mut self.pending) {
                return Some(Err(helper_failure(format!(
                    "reading the helper log: {error}"
                ))));
            }
            while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                if let Some(result) = parse_wait_mode_line(&String::from_utf8_lossy(&line)) {
                    return Some(result);
                }
            }
            if final_scan && !self.pending.is_empty() {
                let line = std::mem::take(&mut self.pending);
                return parse_wait_mode_line(&String::from_utf8_lossy(&line));
            }
            None
        }
    }

    fn reap_in_background(mut child: Child) {
        let _ = std::thread::Builder::new()
            .name("bahamut-wine-helper-reaper".to_owned())
            .spawn(move || {
                let _ = child.wait();
            });
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::symlink;

        const LAUNCH_TOKEN: &str = "sqex0002abc!////";

        fn prefix_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
            let temp = tempfile::tempdir().unwrap();
            let prefix = temp.path().join("prefix");
            let dosdevices = prefix.join("dosdevices");
            let volume = temp.path().join("volume");
            fs::create_dir_all(prefix.join("drive_c")).unwrap();
            fs::create_dir_all(&dosdevices).unwrap();
            fs::create_dir_all(&volume).unwrap();
            symlink("..", dosdevices.join("a:")).unwrap();
            symlink("../drive_c", dosdevices.join("c:")).unwrap();
            symlink("/", dosdevices.join("z:")).unwrap();
            symlink(&volume, dosdevices.join("m:")).unwrap();
            symlink("/dev/null", dosdevices.join("d::")).unwrap();
            symlink(temp.path().join("unmounted"), dosdevices.join("e:")).unwrap();
            (temp, prefix, volume)
        }

        #[test]
        fn dos_mapper_resolves_relative_and_absolute_drive_links() {
            let (_temp, prefix, _volume) = prefix_fixture();
            let game = prefix.join("drive_c/Program Files (x86)/SquareEnix/FINAL FANTASY XIV");
            fs::create_dir_all(&game).unwrap();
            let dos = WineDosPaths::from_prefix(&prefix).unwrap();
            assert_eq!(
                dos.to_dos(&game.join("ffxivgame.exe")).unwrap(),
                r"C:\Program Files (x86)\SquareEnix\FINAL FANTASY XIV\ffxivgame.exe"
            );
            assert_eq!(dos.to_dos(&prefix.join("drive_c")).unwrap(), r"C:\");
        }

        #[test]
        fn dos_mapper_prefers_the_longest_drive_root() {
            let (temp, prefix, volume) = prefix_fixture();
            let dos = WineDosPaths::from_prefix(&prefix).unwrap();
            assert_eq!(
                dos.to_dos(&volume.join("addons/alpha/addon.toml")).unwrap(),
                r"M:\addons\alpha\addon.toml"
            );
            let outside = temp.path().join("launcher/plugins/screenshot.dll");
            let canonical_temp = fs::canonicalize(temp.path()).unwrap();
            let mut expected = OsString::from("Z:");
            for component in canonical_temp.components().skip(1) {
                expected.push("\\");
                expected.push(component.as_os_str());
            }
            expected.push(r"\launcher\plugins\screenshot.dll");
            assert_eq!(dos.to_dos(&outside).unwrap(), expected);
        }

        #[test]
        fn dos_mapper_breaks_equal_root_ties_toward_the_lower_letter() {
            let (_temp, prefix, volume) = prefix_fixture();
            symlink(&volume, prefix.join("dosdevices/y:")).unwrap();
            let dos = WineDosPaths::from_prefix(&prefix).unwrap();
            assert_eq!(
                dos.to_dos(&volume.join("addons/alpha/addon.toml")).unwrap(),
                r"M:\addons\alpha\addon.toml"
            );
        }

        #[test]
        fn dos_mapper_rejects_components_windows_cannot_name() {
            let (_temp, prefix, volume) = prefix_fixture();
            fs::create_dir_all(volume.join(r"back\slash")).unwrap();
            let dos = WineDosPaths::from_prefix(&prefix).unwrap();
            for name in [
                r"back\slash",
                "drive:colon",
                "star*",
                "what?",
                "quote\"",
                "less<",
                "more>",
                "pipe|",
            ] {
                let path = volume.join(name).join("addon.toml");
                assert_eq!(dos.to_dos(&path), None, "{name}");
                assert!(
                    matches!(
                        dos.map("addon manifest", &path),
                        Err(HelperPlanError::GuestPath {
                            field: "addon manifest",
                            ..
                        })
                    ),
                    "{name}"
                );
            }
            assert_eq!(
                dos.to_dos(&volume.join("plain name (1).toml")).unwrap(),
                r"M:\plain name (1).toml"
            );
        }

        #[test]
        fn dos_mapper_skips_raw_devices_and_dangling_links() {
            let (_temp, prefix, _volume) = prefix_fixture();
            let dos = WineDosPaths::from_prefix(&prefix).unwrap();
            let letters: Vec<char> = dos.drives.iter().map(|(letter, _)| *letter).collect();
            assert_eq!(letters, ['A', 'C', 'M', 'Z']);
        }

        #[test]
        fn dos_mapper_rejects_relative_and_unmatched_paths() {
            let (_temp, prefix, _volume) = prefix_fixture();
            let dos = WineDosPaths::from_prefix(&prefix).unwrap();
            assert_eq!(dos.to_dos(Path::new("relative/addon.toml")), None);
            assert!(matches!(
                dos.map("chat logs", Path::new("logs/chat")),
                Err(HelperPlanError::GuestPath {
                    field: "chat logs",
                    ..
                })
            ));

            fs::remove_file(prefix.join("dosdevices/z:")).unwrap();
            let without_z = WineDosPaths::from_prefix(&prefix).unwrap();
            assert_eq!(without_z.to_dos(Path::new("/usr/bin/true")), None);
        }

        #[test]
        fn dos_mapper_requires_a_resolvable_drive() {
            let temp = tempfile::tempdir().unwrap();
            assert!(matches!(
                WineDosPaths::from_prefix(temp.path()),
                Err(LaunchError::WinePrefix(_))
            ));
            let dosdevices = temp.path().join("dosdevices");
            fs::create_dir_all(&dosdevices).unwrap();
            symlink("/dev/null", dosdevices.join("d::")).unwrap();
            assert!(matches!(
                WineDosPaths::from_prefix(temp.path()),
                Err(LaunchError::WinePrefix(_))
            ));
        }

        #[test]
        fn wine_ready_timeout_is_pinned_inside_the_loader_range() {
            // docs/handshake.md "Helper wait mode and image patches": `--timeout-ms <1-60000>`.
            assert_eq!(WINE_HELPER_READY_TIMEOUT_MS, 10_000);
            assert!((1..=60_000).contains(&WINE_HELPER_READY_TIMEOUT_MS));
        }

        #[test]
        fn readiness_scan_waits_for_a_protocol_line_split_across_polls() {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join(HELPER_LOG_FILE_NAME);
            let mut writer = File::create(&path).unwrap();
            let mut reader = File::open(&path).unwrap();
            let mut lines = LogLines::default();
            writer.write_all(b"starting\nSUCCESS pid=5 proc").unwrap();
            assert_eq!(lines.scan(&mut reader, false), None);
            writer.write_all(b"ess_handle=0 ready_signal\n").unwrap();
            assert_eq!(lines.scan(&mut reader, false), Some(Ok(5)));
        }

        fn script_invocation(dir: &Path, body: &str) -> HelperInvocation {
            let script = dir.join("helper.sh");
            fs::write(&script, body).unwrap();
            HelperInvocation {
                program: script,
                args: vec![
                    "--client".into(),
                    r"C:\game\ffxivgame.exe".into(),
                    "--launch-argument".into(),
                    LAUNCH_TOKEN.into(),
                ],
                environment: vec![("BAHAMUT_RUNTIME_CHAT_LOGS".into(), r"Z:\logs\chat".into())],
                current_dir: dir.to_path_buf(),
            }
        }

        fn run_script(
            dir: &Path,
            body: &str,
            deadline: Duration,
        ) -> Result<LaunchedGame, LaunchError> {
            launch_wait_mode_helper(
                Command::new("/bin/sh"),
                &script_invocation(dir, body),
                &dir.join("wine.log"),
                deadline,
            )
        }

        fn wait_for_exit(game: &LaunchedGame) {
            let started = Instant::now();
            while !game.has_exited() {
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "no exit signal"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn read_log(dir: &Path, name: &str) -> String {
            fs::read_to_string(dir.join(name)).unwrap()
        }

        #[test]
        fn wait_mode_session_reports_the_host_pid_and_exits_with_the_helper() {
            let temp = tempfile::tempdir().unwrap();
            let dir = fs::canonicalize(temp.path()).unwrap();
            let game = run_script(
                &dir,
                "printf '%s\\n' \"args=$*\" \"chat=$BAHAMUT_RUNTIME_CHAT_LOGS\" \"cwd=$(pwd -P)\"\necho $$ > host.pid\necho '0024:err:module:noise' >&2\necho 'SUCCESS pid=4242 process_handle=0 ready_signal'\nwhile [ ! -e release ]; do sleep 0.05; done\nexit 3\n",
                Duration::from_secs(30),
            )
            .unwrap();
            let host_pid: u32 = read_log(&dir, "host.pid").trim().parse().unwrap();
            assert_eq!(game.pid, host_pid);
            assert!(!game.has_exited());

            fs::write(dir.join("release"), b"").unwrap();
            wait_for_exit(&game);
            let helper = read_log(&dir, HELPER_LOG_FILE_NAME);
            assert!(helper.starts_with(&format!("{HELPER_LOG_HEADER}\n")));
            assert!(helper.contains(r"args=--client C:\game\ffxivgame.exe"));
            assert!(helper.contains(r"chat=Z:\logs\chat"));
            assert!(helper.contains(&format!("cwd={}", dir.display())));
            assert!(helper.contains("SUCCESS pid=4242"));
            assert!(!helper.contains("0024:err"));
            let wine = read_log(&dir, "wine.log");
            assert!(wine.starts_with("=== bahamut-launcher extension launch ==="));
            assert!(wine.contains(&format!(
                "stdout: {}",
                dir.join(HELPER_LOG_FILE_NAME).display()
            )));
            assert!(wine.contains("0024:err:module:noise"));
            assert!(!wine.contains("SUCCESS"));
            assert!(wine.contains("=== exit:"));
        }

        #[test]
        fn wait_mode_success_survives_concurrent_wine_debug_output() {
            let temp = tempfile::tempdir().unwrap();
            let game = run_script(
                temp.path(),
                "(i=0; while [ $i -lt 60 ]; do echo '0024:trace:seh:noise'; sleep 0.01; i=$((i+1)); done) >&2 &\nnoise=$!\nprintf 'SUCC'\nsleep 0.3\nprintf 'ESS pid=77 process_handle=0 ready_signal\\n'\nwait $noise\nexit 0\n",
                Duration::from_secs(30),
            )
            .unwrap();
            wait_for_exit(&game);
            let helper = read_log(temp.path(), HELPER_LOG_FILE_NAME);
            assert!(
                helper.contains("\nSUCCESS pid=77 process_handle=0 ready_signal\n"),
                "{helper}"
            );
            let wine = read_log(temp.path(), "wine.log");
            assert!(wine.contains("0024:trace:seh:noise"), "{wine}");
        }

        #[test]
        fn wait_mode_readiness_ignores_launcher_header_text() {
            let temp = tempfile::tempdir().unwrap();
            let dir = temp.path().join("cwd\nERROR InvalidArguments injected");
            fs::create_dir_all(&dir).unwrap();
            let game = run_script(
                &dir,
                "echo 'SUCCESS pid=8 process_handle=0'\nexit 0\n",
                Duration::from_secs(30),
            )
            .unwrap();
            wait_for_exit(&game);
        }

        #[test]
        fn wait_mode_log_headers_never_carry_the_launch_argument() {
            let temp = tempfile::tempdir().unwrap();
            let game = run_script(
                temp.path(),
                "echo 'SUCCESS pid=1 process_handle=0'\nexit 0\n",
                Duration::from_secs(30),
            )
            .unwrap();
            wait_for_exit(&game);
            for name in ["wine.log", HELPER_LOG_FILE_NAME] {
                let log = read_log(temp.path(), name);
                assert!(log.starts_with("=== bahamut-launcher"), "{name}");
                assert!(!log.contains("sqex0002"), "{name}: {log}");
            }
        }

        #[test]
        fn wait_mode_error_line_fails_the_launch() {
            let temp = tempfile::tempdir().unwrap();
            let result = run_script(
                temp.path(),
                "echo 'ERROR MissingRuntimeDll bahamut.dll'\nexit 1\n",
                Duration::from_secs(30),
            );
            assert!(matches!(
                result,
                Err(LaunchError::ExtensionBootstrap(HelperFailure {
                    code: HelperErrorCode::MissingRuntimeDll,
                    ..
                }))
            ));
        }

        #[test]
        fn wait_mode_unterminated_success_before_exit_is_read() {
            let temp = tempfile::tempdir().unwrap();
            let game = run_script(
                temp.path(),
                "printf 'SUCCESS pid=9 process_handle=0'\nexit 0\n",
                Duration::from_secs(30),
            )
            .unwrap();
            wait_for_exit(&game);
        }

        #[test]
        fn wait_mode_exit_without_readiness_fails_the_launch() {
            let temp = tempfile::tempdir().unwrap();
            let result = run_script(
                temp.path(),
                "echo starting\nexit 0\n",
                Duration::from_secs(30),
            );
            let Err(LaunchError::ExtensionBootstrap(failure)) = result else {
                panic!("expected a bootstrap failure");
            };
            assert_eq!(failure.code, HelperErrorCode::Unknown);
            assert!(failure.detail.contains("before reporting readiness"));
            assert!(failure.detail.contains(HELPER_LOG_FILE_NAME));
        }

        #[test]
        fn wait_mode_deadline_kills_a_silent_helper() {
            let temp = tempfile::tempdir().unwrap();
            let result = run_script(
                temp.path(),
                "echo $$ > host.pid\nexec sleep 30\n",
                Duration::from_millis(500),
            );
            let Err(LaunchError::ExtensionBootstrap(failure)) = result else {
                panic!("expected a bootstrap failure");
            };
            assert!(failure.detail.contains("neither SUCCESS nor ERROR"));
            let pid: libc::pid_t = read_log(temp.path(), "host.pid").trim().parse().unwrap();
            // kill(pid, 0) keeps succeeding until the killed helper is reaped.
            let started = Instant::now();
            while unsafe { libc::kill(pid, 0) } == 0 {
                assert!(
                    started.elapsed() < Duration::from_secs(10),
                    "helper {pid} outlived the readiness deadline"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
    }
}
