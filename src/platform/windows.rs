//! Native Windows backend for the 64-bit launcher and 32-bit FFXIV 1.23b client:
//! spawn suspended, read the image base with WOW64 APIs, patch and verify it, then resume.
//!
//! Authored from `docs/handshake.md` and the public Win32
//! documentation set.
//! The 64-bit launcher uses `Wow64GetThreadContext`; 32-bit builds are unsupported.

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use windows::Win32::Foundation::{
    CloseHandle, HANDLE, WAIT_EVENT, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Diagnostics::Debug::{
    FlushInstructionCache, ReadProcessMemory, WOW64_CONTEXT, WOW64_CONTEXT_FULL,
    Wow64GetThreadContext, WriteProcessMemory,
};
use windows::Win32::System::Memory::{
    PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS, VirtualProtectEx,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessA, GetExitCodeProcess, GetProcessAffinityMask, GetProcessId,
    NORMAL_PRIORITY_CLASS, PROCESS_INFORMATION, ResumeThread, STARTUPINFOA, SetProcessAffinityMask,
    TerminateProcess, WaitForSingleObject,
};
use windows::core::{PCSTR, PSTR};

use crate::extensions::{
    HelperErrorCode, HelperFailure, HelperInvocation, parse_helper_output, plan_extension_launch,
};
use crate::launcher::launch_args::build_launch_argument;
use crate::launcher::pe_patch::{PePatch, plan_contract_patches};

use super::{LaunchError, LaunchedGame};

const CLIENT_EXE: &str = "ffxivgame.exe";
const CONFIG_EXE: &str = "ffxivconfig.exe";
const TERMINATION_WAIT_MS: u32 = 2_000;

/// 32-bit PEB offset of `ImageBaseAddress`.
const PEB_IMAGE_BASE_OFFSET: usize = 0x8;

/// Fixed InstallShield uninstall key for FFXIV 1.x installs.
const UNINSTALLER_KEY: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{F2C4E6E0-EB78-4824-A212-6DF6AF0E8E82}";

/// Detect the InstallShield install through 32-bit then 64-bit registry views; return `None` for incomplete or nonexistent paths.
pub fn detect_game_install() -> Option<PathBuf> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_WOW64_32KEY, KEY_WOW64_64KEY};

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for view_flag in [KEY_WOW64_32KEY, KEY_WOW64_64KEY] {
        let Ok(key) = hklm.open_subkey_with_flags(UNINSTALLER_KEY, KEY_QUERY_VALUE | view_flag)
        else {
            continue;
        };
        let install_location: String = key.get_value("InstallLocation").unwrap_or_default();
        let display_name: String = key.get_value("DisplayName").unwrap_or_default();
        if install_location.is_empty() || display_name.is_empty() {
            continue;
        }
        let path = PathBuf::from(install_location).join(display_name);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// Spawn the retail configuration utility without lobby arguments or client patches.
pub fn launch_config_tool(game_dir: &Path) -> Result<u32, LaunchError> {
    let exe_path = game_dir.join(CONFIG_EXE);
    if !exe_path.is_file() {
        return Err(LaunchError::MissingConfigBinary(exe_path));
    }
    let child = Command::new(&exe_path)
        .current_dir(game_dir)
        .spawn()
        .map_err(|source| LaunchError::Io {
            context: "spawning ffxivconfig.exe",
            source,
        })?;
    Ok(child.id())
}

/// Launch `ffxivgame.exe` for a lobby host and session; terminate the suspended process on post-create failure.
pub fn launch_game(
    game_dir: &Path,
    lobby_host: &str,
    session_id: &str,
    options: &super::LaunchOptions,
) -> Result<LaunchedGame, LaunchError> {
    let exe_path = game_dir.join(CLIENT_EXE);
    if !exe_path.is_file() {
        return Err(LaunchError::MissingClientBinary(exe_path));
    }

    let patches = plan_contract_patches(lobby_host)?;
    let tick = unsafe { GetTickCount() };
    let launch_args = build_launch_argument(session_id, tick)?;

    if let Some(artifacts) = &options.extension_artifacts {
        let request =
            artifacts.to_request(game_dir, options, &patches, launch_args.encoded_argument);
        return run_extension_helper(plan_extension_launch(&request)?);
    }

    // CreateProcessA is ANSI; the supported 1.23b install path must be ASCII.
    let command_line = format!(
        "\"{}\" {}",
        exe_path.display(),
        launch_args.encoded_argument
    );
    let command_line_c = CString::new(command_line).map_err(nul_to_err)?;
    let working_dir_c =
        CString::new(game_dir.to_string_lossy().into_owned()).map_err(nul_to_err)?;

    let mut startup: STARTUPINFOA = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOA>() as u32;
    let mut proc_info = PROCESS_INFORMATION::default();

    tracing::info!(
        exe = %exe_path.display(),
        lobby_host,
        tick,
        "spawning client suspended"
    );

    unsafe {
        CreateProcessA(
            PCSTR::null(),
            Some(PSTR(command_line_c.as_ptr() as *mut u8)),
            None,
            None,
            false,
            CREATE_SUSPENDED | NORMAL_PRIORITY_CLASS,
            None,
            PCSTR(working_dir_c.as_ptr() as *const u8),
            &startup,
            &mut proc_info,
        )
    }
    .map_err(|err| windows_api("CreateProcessA", err))?;

    // After creation, either resume or terminate the process. A successful
    // launch retains the process handle until the client exits.
    let result = cap_game_process_affinity(proc_info.hProcess)
        .and_then(|()| apply_patches_in_process(proc_info.hProcess, proc_info.hThread, &patches))
        .and_then(|()| {
            resume_main_thread(proc_info.hThread)?;
            Ok(proc_info.dwProcessId)
        });

    match result {
        Ok(pid) => {
            unsafe {
                let _ = CloseHandle(proc_info.hThread);
            }
            tracing::info!(pid, "client resumed");
            Ok(start_runtime_session_monitor(proc_info.hProcess, pid))
        }
        Err(err) => {
            let termination = terminate_process_and_wait(proc_info.hProcess, proc_info.hThread);
            if let Err(termination_error) = termination {
                tracing::error!(
                    error = %termination_error,
                    "client termination was not confirmed; handle reaper retained ownership"
                );
                return Err(termination_error);
            }
            close_handles(proc_info.hProcess, proc_info.hThread);
            Err(err)
        }
    }
}

fn run_extension_helper(invocation: HelperInvocation) -> Result<LaunchedGame, LaunchError> {
    tracing::info!(
        helper = %invocation.program.display(),
        "spawning x86 extension bootstrap helper"
    );
    let helper = match Command::new(&invocation.program)
        .args(&invocation.args)
        .envs(
            invocation
                .environment
                .iter()
                .map(|(name, value)| (name, value)),
        )
        .current_dir(&invocation.current_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(helper) => helper,
        Err(source) => {
            return Err(LaunchError::Io {
                context: "spawning the x86 extension bootstrap helper",
                source,
            });
        }
    };
    let output = match helper.wait_with_output() {
        Ok(output) => output,
        Err(source) => {
            return Err(LaunchError::Io {
                context: "waiting for the x86 extension bootstrap helper",
                source,
            });
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let success = match parse_helper_output(&stdout, &stderr) {
        Ok(success) => success,
        Err(error) => return Err(LaunchError::ExtensionBootstrap(error)),
    };
    let client_process_value = match usize::try_from(success.client_process_handle) {
        Ok(value) if value != 0 && value != usize::MAX => value,
        _ => {
            return Err(LaunchError::ExtensionBootstrap(HelperFailure {
                code: HelperErrorCode::Unknown,
                detail: "helper returned an invalid client process handle".to_owned(),
            }));
        }
    };
    let client_process = HANDLE(client_process_value as *mut _);
    if unsafe { GetProcessId(client_process) } != success.client_pid {
        unsafe {
            let _ = CloseHandle(client_process);
        }
        return Err(LaunchError::ExtensionBootstrap(HelperFailure {
            code: HelperErrorCode::Unknown,
            detail: "helper returned a client process handle for the wrong process".to_owned(),
        }));
    }
    if !output.status.success() {
        unsafe {
            let _ = CloseHandle(client_process);
        }
        return Err(LaunchError::ExtensionBootstrap(HelperFailure {
            code: HelperErrorCode::Unknown,
            detail: format!("helper reported success but exited with {}", output.status),
        }));
    }
    let game = start_runtime_session_monitor(client_process, success.client_pid);
    tracing::info!(pid = success.client_pid, "client resumed with extensions");
    Ok(game)
}

fn start_runtime_session_monitor(client_process: HANDLE, client_pid: u32) -> LaunchedGame {
    let (game, exited) = LaunchedGame::pending(client_pid);
    let client_process_value = client_process.0 as usize;
    if let Err(error) = std::thread::Builder::new()
        .name("bahamut-runtime-session".to_owned())
        .spawn(move || {
            if monitor_runtime_session(HANDLE(client_process_value as *mut _), client_pid)
                .is_confirmed_exit()
            {
                let _ = exited.send(());
            }
        })
    {
        unsafe {
            let _ = CloseHandle(client_process);
        }
        tracing::warn!(%error, "could not start runtime session monitor");
    }
    game
}

#[derive(Debug, PartialEq, Eq)]
enum RuntimeSessionResult {
    Exited(Result<u32, i32>),
    Unconfirmed(u32),
}

impl RuntimeSessionResult {
    fn is_confirmed_exit(&self) -> bool {
        matches!(self, Self::Exited(_))
    }
}

fn monitor_runtime_session_with<W, Q, C>(
    wait: W,
    query_exit_code: Q,
    close: C,
) -> RuntimeSessionResult
where
    W: FnOnce() -> WAIT_EVENT,
    Q: FnOnce() -> Result<u32, i32>,
    C: FnOnce(),
{
    let wait_result = wait();
    let result = if wait_result == WAIT_OBJECT_0 {
        RuntimeSessionResult::Exited(query_exit_code())
    } else {
        RuntimeSessionResult::Unconfirmed(wait_result.0)
    };
    close();
    result
}

fn monitor_runtime_session(client_process: HANDLE, client_pid: u32) -> RuntimeSessionResult {
    let result = monitor_runtime_session_with(
        || unsafe { WaitForSingleObject(client_process, u32::MAX) },
        || {
            let mut exit_code = 0;
            unsafe { GetExitCodeProcess(client_process, &mut exit_code) }
                .map(|()| exit_code)
                .map_err(|error| error.code().0)
        },
        || unsafe {
            let _ = CloseHandle(client_process);
        },
    );
    match result {
        RuntimeSessionResult::Exited(Ok(exit_code)) => {
            tracing::info!(client_pid, exit_code, "client exited");
        }
        RuntimeSessionResult::Exited(Err(query_error)) => {
            tracing::warn!(
                client_pid,
                query_error,
                "client exited; final exit code is unavailable"
            );
        }
        RuntimeSessionResult::Unconfirmed(wait_result) => {
            tracing::warn!(
                client_pid,
                wait_result,
                "client termination was not authoritative"
            );
        }
    }
    result
}

fn apply_patches_in_process(
    process: HANDLE,
    thread: HANDLE,
    patches: &[PePatch],
) -> Result<(), LaunchError> {
    let image_base = read_image_base(process, thread)?;
    tracing::debug!(
        image_base = format_args!("0x{:08X}", image_base),
        "client image base"
    );
    for patch in patches {
        let remote_addr = image_base
            .checked_add(patch.rva as usize)
            .ok_or(LaunchError::ImageBaseUnknown)?;
        write_patch(process, remote_addr, &patch.bytes)?;
        verify_patch(process, remote_addr, &patch.bytes, patch.rva)?;
        tracing::debug!(
            rva = format_args!("0x{:08X}", patch.rva),
            bytes = patch.bytes.len(),
            "patch applied and verified"
        );
    }
    Ok(())
}

fn read_image_base(process: HANDLE, thread: HANDLE) -> Result<usize, LaunchError> {
    let mut ctx: WOW64_CONTEXT = unsafe { std::mem::zeroed() };
    ctx.ContextFlags = WOW64_CONTEXT_FULL;
    unsafe { Wow64GetThreadContext(thread, &mut ctx) }
        .map_err(|err| windows_api("Wow64GetThreadContext", err))?;

    let peb_addr = ctx.Ebx as usize;
    let image_base_addr = peb_addr
        .checked_add(PEB_IMAGE_BASE_OFFSET)
        .ok_or(LaunchError::ImageBaseUnknown)?;
    let mut image_base: u32 = 0;
    let mut bytes_read: usize = 0;
    unsafe {
        ReadProcessMemory(
            process,
            image_base_addr as *const _,
            &mut image_base as *mut u32 as *mut _,
            std::mem::size_of::<u32>(),
            Some(&mut bytes_read),
        )
    }
    .map_err(|err| windows_api("ReadProcessMemory(PebBase + 0x8)", err))?;

    if bytes_read != std::mem::size_of::<u32>() || image_base == 0 {
        return Err(LaunchError::ImageBaseUnknown);
    }
    Ok(image_base as usize)
}

fn write_patch(process: HANDLE, addr: usize, bytes: &[u8]) -> Result<(), LaunchError> {
    let mut old_protect = PAGE_PROTECTION_FLAGS(0);
    unsafe {
        VirtualProtectEx(
            process,
            addr as *mut _,
            bytes.len(),
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    }
    .map_err(|err| windows_api("VirtualProtectEx(RWX)", err))?;

    let mut bytes_written: usize = 0;
    let write_result = unsafe {
        WriteProcessMemory(
            process,
            addr as *const _,
            bytes.as_ptr() as *const _,
            bytes.len(),
            Some(&mut bytes_written),
        )
    };

    // Always try to restore the original page protection, even on write failure.
    let mut ignored = PAGE_PROTECTION_FLAGS(0);
    let restore_result = unsafe {
        VirtualProtectEx(
            process,
            addr as *mut _,
            bytes.len(),
            old_protect,
            &mut ignored,
        )
    }
    .map_err(|err| windows_api("VirtualProtectEx(restore)", err));

    let write_result = write_result
        .map(|_| bytes_written)
        .map_err(|err| windows_api("WriteProcessMemory", err));
    finish_direct_patch(write_result, bytes.len(), restore_result, || {
        unsafe { FlushInstructionCache(process, Some(addr as *const _), bytes.len()) }
            .map_err(|err| windows_api("FlushInstructionCache", err))
    })
}

fn finish_direct_patch<F>(
    write_result: Result<usize, LaunchError>,
    expected_len: usize,
    restore_result: Result<(), LaunchError>,
    flush: F,
) -> Result<(), LaunchError>
where
    F: FnOnce() -> Result<(), LaunchError>,
{
    restore_result?;
    let bytes_written = write_result?;
    if bytes_written != expected_len {
        return Err(LaunchError::WindowsApi {
            context: "WriteProcessMemory (short write)",
            source: std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                format!(
                    "expected {} bytes written, got {}",
                    expected_len, bytes_written
                ),
            ),
        });
    }
    flush()
}

fn termination_wait_result(wait: WAIT_EVENT) -> Result<(), LaunchError> {
    if wait == WAIT_OBJECT_0 {
        return Ok(());
    }
    let source = if wait == WAIT_TIMEOUT {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "the client did not terminate before the confirmation deadline",
        )
    } else if wait == WAIT_FAILED {
        std::io::Error::last_os_error()
    } else {
        std::io::Error::other(format!("unexpected wait result 0x{:08X}", wait.0))
    };
    Err(LaunchError::WindowsApi {
        context: "WaitForSingleObject(termination)",
        source,
    })
}

fn retain_handles_until_termination(process: HANDLE, thread: HANDLE) {
    // The windows crate intentionally does not mark raw HANDLE wrappers as Send;
    // carry the values as integers and recreate the wrappers in the reaper.
    let process_value = process.0 as usize;
    let thread_value = thread.0 as usize;
    let spawn_result = std::thread::Builder::new()
        .name("bahamut-client-handle-reaper".to_owned())
        .spawn(move || {
            let process = HANDLE(process_value as *mut std::ffi::c_void);
            let thread = HANDLE(thread_value as *mut std::ffi::c_void);
            let wait = unsafe { WaitForSingleObject(process, u32::MAX) };
            if wait == WAIT_OBJECT_0 {
                close_handles(process, thread);
            } else {
                tracing::error!(
                    result = wait.0,
                    "client handle reaper could not confirm termination; handles remain owned"
                );
            }
        });
    if let Err(error) = spawn_result {
        tracing::error!(%error, "could not start client handle reaper; process handle remains open");
    }
}

fn terminate_and_wait_with<T, W, R>(terminate: T, wait: W, retain: R) -> Result<(), LaunchError>
where
    T: FnOnce() -> Result<(), LaunchError>,
    W: FnOnce() -> WAIT_EVENT,
    R: FnOnce(),
{
    if let Err(error) = terminate() {
        retain();
        return Err(error);
    }
    match termination_wait_result(wait()) {
        Ok(()) => Ok(()),
        Err(error) => {
            retain();
            Err(error)
        }
    }
}

fn terminate_process_and_wait(process: HANDLE, thread: HANDLE) -> Result<(), LaunchError> {
    terminate_and_wait_with(
        || {
            unsafe { TerminateProcess(process, 1) }
                .map_err(|err| windows_api("TerminateProcess", err))
        },
        || unsafe { WaitForSingleObject(process, TERMINATION_WAIT_MS) },
        || retain_handles_until_termination(process, thread),
    )
}

fn verify_patch(
    process: HANDLE,
    addr: usize,
    expected: &[u8],
    rva: u32,
) -> Result<(), LaunchError> {
    let mut buf = vec![0u8; expected.len()];
    let mut bytes_read: usize = 0;
    unsafe {
        ReadProcessMemory(
            process,
            addr as *const _,
            buf.as_mut_ptr() as *mut _,
            expected.len(),
            Some(&mut bytes_read),
        )
    }
    .map_err(|err| windows_api("ReadProcessMemory(verify)", err))?;

    if bytes_read != expected.len() || buf != expected {
        return Err(LaunchError::PatchVerifyMismatch { rva });
    }
    Ok(())
}

fn resume_main_thread(thread: HANDLE) -> Result<(), LaunchError> {
    let prev = unsafe { ResumeThread(thread) };
    if prev == u32::MAX {
        return Err(LaunchError::WindowsApi {
            context: "ResumeThread",
            source: std::io::Error::last_os_error(),
        });
    }
    Ok(())
}

fn cap_game_process_affinity(process: HANDLE) -> Result<(), LaunchError> {
    let mut process_mask = 0usize;
    let mut system_mask = 0usize;
    unsafe { GetProcessAffinityMask(process, &mut process_mask, &mut system_mask) }
        .map_err(|err| windows_api("GetProcessAffinityMask", err))?;
    if process_mask == 0 {
        return Err(LaunchError::WindowsApi {
            context: "GetProcessAffinityMask",
            source: std::io::Error::other("empty process affinity mask"),
        });
    }
    let limited_mask = limit_game_processor_mask(process_mask);
    if limited_mask != process_mask {
        unsafe { SetProcessAffinityMask(process, limited_mask) }
            .map_err(|err| windows_api("SetProcessAffinityMask", err))?;
    }
    Ok(())
}

fn limit_game_processor_mask(mask: usize) -> usize {
    let mut remaining = mask;
    let mut limited = 0;
    for _ in 0..15 {
        if remaining == 0 {
            break;
        }
        let bit = 1usize << remaining.trailing_zeros();
        limited |= bit;
        remaining &= !bit;
    }
    limited
}

fn close_handles(process: HANDLE, thread: HANDLE) {
    unsafe {
        let _ = CloseHandle(process);
        let _ = CloseHandle(thread);
    }
}

fn windows_api(context: &'static str, err: windows::core::Error) -> LaunchError {
    LaunchError::WindowsApi {
        context,
        source: std::io::Error::from_raw_os_error(err.code().0),
    }
}

fn nul_to_err(err: std::ffi::NulError) -> LaunchError {
    LaunchError::ArgsContainNul(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_affinity_cap_preserves_allowed_processor_bits() {
        assert_eq!(limit_game_processor_mask(0x0fff), 0x0fff);
        assert_eq!(limit_game_processor_mask(0xffff), 0x7fff);
        assert_eq!(limit_game_processor_mask(0xaaaa), 0xaaaa);
        assert_eq!(limit_game_processor_mask(0xaaaaaaaa), 0x2aaaaaaa);
    }

    fn test_error(context: &'static str) -> LaunchError {
        LaunchError::WindowsApi {
            context,
            source: std::io::Error::other("fixture failure"),
        }
    }

    #[test]
    fn direct_patch_restore_failure() {
        let result = finish_direct_patch(Ok(4), 4, Err(test_error("restore")), || Ok(()));
        assert!(matches!(
            result,
            Err(LaunchError::WindowsApi {
                context: "restore",
                ..
            })
        ));
    }

    #[test]
    fn config_tool_requires_the_retail_executable() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            launch_config_tool(root.path()),
            Err(LaunchError::MissingConfigBinary(path)) if path == root.path().join(CONFIG_EXE)
        ));
    }

    #[test]
    fn direct_launch_termination_unconfirmed() {
        let retained = std::cell::Cell::new(false);
        let result = terminate_and_wait_with(|| Ok(()), || WAIT_TIMEOUT, || retained.set(true));
        assert!(matches!(
            result,
            Err(LaunchError::WindowsApi {
                context: "WaitForSingleObject(termination)",
                source,
            }) if source.kind() == std::io::ErrorKind::TimedOut
        ));
        assert!(
            retained.get(),
            "unconfirmed termination must retain ownership"
        );

        let retained_after_terminate_failure = std::cell::Cell::new(false);
        let result = terminate_and_wait_with(
            || Err(test_error("terminate")),
            || WAIT_OBJECT_0,
            || retained_after_terminate_failure.set(true),
        );
        assert!(matches!(
            result,
            Err(LaunchError::WindowsApi {
                context: "terminate",
                ..
            })
        ));
        assert!(
            retained_after_terminate_failure.get(),
            "failed termination must retain ownership"
        );
    }

    #[test]
    fn runtime_exit_codes_keep_zero_and_nonzero_results() {
        for exit_code in [0, 17] {
            let result = monitor_runtime_session_with(|| WAIT_OBJECT_0, || Ok(exit_code), || {});
            assert_eq!(result, RuntimeSessionResult::Exited(Ok(exit_code)));
            assert!(result.is_confirmed_exit());
        }
    }

    #[test]
    fn runtime_exit_code_259_is_final_after_signal() {
        let events = std::cell::RefCell::new(Vec::new());
        let result = monitor_runtime_session_with(
            || {
                events.borrow_mut().push("wait");
                WAIT_OBJECT_0
            },
            || {
                events.borrow_mut().push("query");
                Ok(259)
            },
            || events.borrow_mut().push("close"),
        );
        assert_eq!(result, RuntimeSessionResult::Exited(Ok(259)));
        assert_eq!(*events.borrow(), ["wait", "query", "close"]);
    }

    #[test]
    fn runtime_exit_query_failure_preserves_confirmed_exit() {
        let query_count = std::cell::Cell::new(0);
        let close_count = std::cell::Cell::new(0);
        let result = monitor_runtime_session_with(
            || WAIT_OBJECT_0,
            || {
                query_count.set(query_count.get() + 1);
                Err(5)
            },
            || close_count.set(close_count.get() + 1),
        );
        assert_eq!(result, RuntimeSessionResult::Exited(Err(5)));
        assert!(result.is_confirmed_exit());
        assert_eq!(query_count.get(), 1);
        assert_eq!(close_count.get(), 1);
    }

    #[test]
    fn runtime_wait_failure_skips_exit_query_and_closes_once() {
        let query_count = std::cell::Cell::new(0);
        let close_count = std::cell::Cell::new(0);
        let result = monitor_runtime_session_with(
            || WAIT_FAILED,
            || {
                query_count.set(query_count.get() + 1);
                Ok(0)
            },
            || close_count.set(close_count.get() + 1),
        );
        assert_eq!(result, RuntimeSessionResult::Unconfirmed(WAIT_FAILED.0));
        assert!(!result.is_confirmed_exit());
        assert_eq!(query_count.get(), 0);
        assert_eq!(close_count.get(), 1);
    }

    fn create_test_client_process(exit_code: u32) -> PROCESS_INFORMATION {
        let mut command = CString::new(format!("cmd.exe /C exit {exit_code}"))
            .unwrap()
            .into_bytes_with_nul();
        let mut startup: STARTUPINFOA = unsafe { std::mem::zeroed() };
        startup.cb = std::mem::size_of::<STARTUPINFOA>() as u32;
        let mut process = PROCESS_INFORMATION::default();
        unsafe {
            CreateProcessA(
                PCSTR::null(),
                Some(PSTR(command.as_mut_ptr())),
                None,
                None,
                false,
                windows::Win32::System::Threading::CREATE_NO_WINDOW,
                None,
                PCSTR::null(),
                &startup,
                &mut process,
            )
        }
        .unwrap();
        process
    }

    #[test]
    fn direct_create_process_handle_can_be_waited_and_queried() {
        let process = create_test_client_process(0);
        unsafe {
            let _ = CloseHandle(process.hThread);
        }
        assert_eq!(
            monitor_runtime_session(process.hProcess, process.dwProcessId),
            RuntimeSessionResult::Exited(Ok(0))
        );
    }

    #[test]
    fn helper_transferred_same_access_handle_can_be_waited_and_queried() {
        use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
        use windows::Win32::System::Threading::GetCurrentProcess;

        let process = create_test_client_process(17);
        let mut duplicate = HANDLE::default();
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                process.hProcess,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
        }
        .unwrap();
        unsafe {
            let _ = CloseHandle(process.hProcess);
            let _ = CloseHandle(process.hThread);
        }
        assert_eq!(
            monitor_runtime_session(duplicate, process.dwProcessId),
            RuntimeSessionResult::Exited(Ok(17))
        );
    }
}
