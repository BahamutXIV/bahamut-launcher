use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::dirs;
use crate::extensions::HelperInvocation;
use crate::platform::wine_engine;
use crate::platform::{LaunchError, LaunchedGame};

use super::log_tail::{WINE_LOG_TAIL_LINES, read_log_tail};

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
        log_controller_environment();

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
        log_controller_environment();
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

/// Allowlisted SDL and Steam variables only; the launch environment carries a session token.
fn log_controller_environment() {
    tracing::info!(
        "Controller env at launch = {}",
        crate::diagnostics::process_controller_environment_lines().join(" | ")
    );
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
    dirs::data_dir()
        .map(|dir| dir.join(name))
        .map_err(|e| LaunchError::WinePrefix(format!("resolving the launcher data directory: {e}")))
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
        let message = not_found_message(select_wine(None, || ManagedEngine::Unsupported, || None));

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
