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
    let client_pid = match await_readiness(&mut child, &mut protocol, &logs, readiness_deadline) {
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

    fn run_script(dir: &Path, body: &str, deadline: Duration) -> Result<LaunchedGame, LaunchError> {
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
