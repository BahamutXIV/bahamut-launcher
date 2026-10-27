use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use object::read::pe::PeFile32;
use object::{Object, ObjectSection};

use crate::extensions::HelperInvocation;
use crate::launcher::pe_patch::PePatch;
use crate::platform::{LaunchError, LaunchedGame};

use super::log_tail::{WINE_LOG_TAIL_LINES, read_log_tail};

/// Default `WINEDEBUG` for the bundled runtime, including assert and D3D diagnostics.
const WINEDEBUG_DEFAULT: &str = "fixme-all,err+all,+debugstr,warn+d3d";

/// InstallShield-default FFXIV path inside the Wine prefix.
pub const PREFIX_FFXIV_SUBPATH: &str = "drive_c/Program Files (x86)/SquareEnix/FINAL FANTASY XIV";

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
