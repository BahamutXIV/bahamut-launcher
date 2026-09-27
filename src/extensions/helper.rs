//! Pure parsing and mapping for the x86 bootstrap helper's line protocol.

use std::path::{Path, PathBuf};

pub const LOADER_BINARY_NAME: &str = "bahamut-loader.exe";
pub const CLIENT_MODULE_NAME: &str = "bahamut.dll";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperErrorCode {
    InvalidArguments,
    WrongClientIdentity,
    EventCreateFailed,
    EnvironmentSetupFailed,
    TargetCreateFailed,
    TargetAffinityQueryFailed,
    TargetAffinitySetFailed,
    MissingRuntimeDll,
    PatchApplyFailed,
    RemoteAllocFailed,
    RemoteWriteFailed,
    LoadLibraryAddressFailed,
    LoadLibraryAddressMismatch,
    RemoteThreadCreateFailed,
    RuntimeLoadFailed,
    RuntimeInitializeAddressFailed,
    RuntimeInitializeThreadFailed,
    RuntimeInitializeFailed,
    RuntimeIdentityFailed,
    RuntimeTelemetryFailed,
    RuntimeStartupScriptFailed,
    RuntimeNativePluginFailed,
    RenderBoundaryFailed,
    StubValidationFailed,
    OverlayValidationFailed,
    RuntimeReadyTimeout,
    TargetResumeFailed,
    TargetEntryTimeout,
    ProcessHandleTransferFailed,
    TargetTerminationFailed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperFailure {
    pub code: HelperErrorCode,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelperSuccess {
    pub client_pid: u32,
    pub client_process_handle: u64,
}

impl std::fmt::Display for HelperFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            return write!(f, "bootstrap helper failed: {:?}", self.code);
        }
        write!(
            f,
            "bootstrap helper failed ({:?}): {}",
            self.code, self.detail
        )
    }
}

impl std::error::Error for HelperFailure {}

/// Parse the first structured helper error line from stdout or stderr.
/// Unknown codes are retained as `Unknown` so diagnostics are not discarded.
pub fn parse_helper_error(output: &str) -> Option<HelperFailure> {
    output.lines().find_map(parse_error_line)
}

pub fn parse_helper_output(stdout: &str, stderr: &str) -> Result<HelperSuccess, HelperFailure> {
    if let Some(error) = parse_helper_error(stderr).or_else(|| parse_helper_error(stdout)) {
        return Err(error);
    }
    for line in stdout.lines().chain(stderr.lines()) {
        let Some(payload) = line.trim().strip_prefix("SUCCESS ") else {
            continue;
        };
        let pid = payload
            .split_ascii_whitespace()
            .find_map(|field| field.strip_prefix("pid="))
            .and_then(|value| value.parse::<u32>().ok());
        let client_process_handle = payload
            .split_ascii_whitespace()
            .find_map(|field| field.strip_prefix("process_handle="))
            .and_then(|value| value.parse::<u64>().ok());
        if let (Some(client_pid), Some(client_process_handle)) = (pid, client_process_handle)
            && client_process_handle != 0
        {
            return Ok(HelperSuccess {
                client_pid,
                client_process_handle,
            });
        }
    }
    Err(HelperFailure {
        code: HelperErrorCode::Unknown,
        detail: "helper produced no SUCCESS pid and process_handle or ERROR line".to_owned(),
    })
}

/// Parse one transcript line from a helper started with `--wait-for-client`.
///
/// Returns `None` unless the trimmed line starts with `SUCCESS ` or `ERROR `, so
/// Wine debug lines such as `0024:err:...` never match. A wait-mode helper keeps
/// the client handle itself, so its `SUCCESS` line must carry a Win32 `pid=` and
/// either no `process_handle=` or `process_handle=0`; the returned value is that pid.
pub fn parse_wait_mode_line(line: &str) -> Option<Result<u32, HelperFailure>> {
    if let Some(error) = parse_error_line(line) {
        return Some(Err(error));
    }
    let payload = line.trim().strip_prefix("SUCCESS ")?;
    let field = |name: &str| {
        payload
            .split_ascii_whitespace()
            .find_map(|field| field.strip_prefix(name))
    };
    let Some(client_pid) = field("pid=").and_then(|value| value.parse::<u32>().ok()) else {
        return Some(Err(HelperFailure {
            code: HelperErrorCode::Unknown,
            detail: "helper SUCCESS line carried no client pid".to_owned(),
        }));
    };
    match field("process_handle=").map(|value| value.parse::<u64>()) {
        None | Some(Ok(0)) => Some(Ok(client_pid)),
        Some(_) => Some(Err(HelperFailure {
            code: HelperErrorCode::Unknown,
            detail: "helper transferred a client process handle instead of waiting for the client"
                .to_owned(),
        })),
    }
}

fn parse_error_line(line: &str) -> Option<HelperFailure> {
    let line = line.trim();
    let payload = line.strip_prefix("ERROR ")?;
    let (code_text, detail) = payload
        .split_once(' ')
        .map_or((payload, ""), |(code, detail)| (code, detail.trim()));
    let code = match code_text {
        "InvalidArguments" => HelperErrorCode::InvalidArguments,
        "WrongClientIdentity" => HelperErrorCode::WrongClientIdentity,
        "EventCreateFailed" => HelperErrorCode::EventCreateFailed,
        "EnvironmentSetupFailed" => HelperErrorCode::EnvironmentSetupFailed,
        "TargetCreateFailed" => HelperErrorCode::TargetCreateFailed,
        "TargetAffinityQueryFailed" => HelperErrorCode::TargetAffinityQueryFailed,
        "TargetAffinitySetFailed" => HelperErrorCode::TargetAffinitySetFailed,
        "MissingRuntimeDll" => HelperErrorCode::MissingRuntimeDll,
        "PatchApplyFailed" => HelperErrorCode::PatchApplyFailed,
        "RemoteAllocFailed" => HelperErrorCode::RemoteAllocFailed,
        "RemoteWriteFailed" => HelperErrorCode::RemoteWriteFailed,
        "LoadLibraryAddressFailed" => HelperErrorCode::LoadLibraryAddressFailed,
        "LoadLibraryAddressMismatch" => HelperErrorCode::LoadLibraryAddressMismatch,
        "RemoteThreadCreateFailed" => HelperErrorCode::RemoteThreadCreateFailed,
        "RuntimeLoadFailed" => HelperErrorCode::RuntimeLoadFailed,
        "RuntimeInitializeAddressFailed" => HelperErrorCode::RuntimeInitializeAddressFailed,
        "RuntimeInitializeThreadFailed" => HelperErrorCode::RuntimeInitializeThreadFailed,
        "RuntimeInitializeFailed" => HelperErrorCode::RuntimeInitializeFailed,
        "RuntimeIdentityFailed" => HelperErrorCode::RuntimeIdentityFailed,
        "RuntimeTelemetryFailed" => HelperErrorCode::RuntimeTelemetryFailed,
        "RuntimeStartupScriptFailed" => HelperErrorCode::RuntimeStartupScriptFailed,
        "RuntimeNativePluginFailed" => HelperErrorCode::RuntimeNativePluginFailed,
        "RenderBoundaryFailed" => HelperErrorCode::RenderBoundaryFailed,
        "StubValidationFailed" => HelperErrorCode::StubValidationFailed,
        "OverlayValidationFailed" => HelperErrorCode::OverlayValidationFailed,
        "RuntimeReadyTimeout" => HelperErrorCode::RuntimeReadyTimeout,
        "TargetResumeFailed" => HelperErrorCode::TargetResumeFailed,
        "TargetEntryTimeout" => HelperErrorCode::TargetEntryTimeout,
        "ProcessHandleTransferFailed" => HelperErrorCode::ProcessHandleTransferFailed,
        "TargetTerminationFailed" => HelperErrorCode::TargetTerminationFailed,
        code if code.contains("Patch") => HelperErrorCode::PatchApplyFailed,
        code if code.ends_with("Failed") => HelperErrorCode::Unknown,
        _ => HelperErrorCode::Unknown,
    };
    Some(HelperFailure {
        code,
        detail: detail.to_owned(),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum HelperPlanError {
    #[error("helper path is empty")]
    EmptyHelperPath,
    #[error("helper argument contains an interior NUL byte: {field}")]
    InteriorNul { field: &'static str },
    #[error("helper argument is empty: {field}")]
    EmptyArgument { field: &'static str },
    #[error("helper argument contains a line break: {field}")]
    LineBreak { field: &'static str },
    #[error("helper path has no Wine drive mapping: {field} ({})", path.display())]
    GuestPath { field: &'static str, path: PathBuf },
}

pub fn validate_helper_path(path: &Path) -> Result<(), HelperPlanError> {
    if path.as_os_str().is_empty() {
        return Err(HelperPlanError::EmptyHelperPath);
    }
    reject_nul("helper path", path)
}

pub fn reject_nul(field: &'static str, path: &Path) -> Result<(), HelperPlanError> {
    if path.as_os_str().to_string_lossy().contains('\0') {
        return Err(HelperPlanError::InteriorNul { field });
    }
    Ok(())
}

pub fn reject_empty(field: &'static str, value: &str) -> Result<(), HelperPlanError> {
    if value.is_empty() {
        return Err(HelperPlanError::EmptyArgument { field });
    }
    if value.contains('\0') {
        return Err(HelperPlanError::InteriorNul { field });
    }
    Ok(())
}

pub fn path_arg(field: &'static str, path: &Path) -> Result<PathBuf, HelperPlanError> {
    reject_nul(field, path)?;
    if path.as_os_str().is_empty() {
        return Err(HelperPlanError::EmptyArgument { field });
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_specific_helper_error() {
        let cases = [
            ("InvalidArguments", HelperErrorCode::InvalidArguments),
            ("WrongClientIdentity", HelperErrorCode::WrongClientIdentity),
            ("EventCreateFailed", HelperErrorCode::EventCreateFailed),
            (
                "EnvironmentSetupFailed",
                HelperErrorCode::EnvironmentSetupFailed,
            ),
            ("TargetCreateFailed", HelperErrorCode::TargetCreateFailed),
            (
                "TargetAffinityQueryFailed",
                HelperErrorCode::TargetAffinityQueryFailed,
            ),
            (
                "TargetAffinitySetFailed",
                HelperErrorCode::TargetAffinitySetFailed,
            ),
            ("MissingRuntimeDll", HelperErrorCode::MissingRuntimeDll),
            ("PatchApplyFailed", HelperErrorCode::PatchApplyFailed),
            ("RemoteAllocFailed", HelperErrorCode::RemoteAllocFailed),
            ("RemoteWriteFailed", HelperErrorCode::RemoteWriteFailed),
            (
                "LoadLibraryAddressFailed",
                HelperErrorCode::LoadLibraryAddressFailed,
            ),
            (
                "LoadLibraryAddressMismatch",
                HelperErrorCode::LoadLibraryAddressMismatch,
            ),
            (
                "RemoteThreadCreateFailed",
                HelperErrorCode::RemoteThreadCreateFailed,
            ),
            ("RuntimeLoadFailed", HelperErrorCode::RuntimeLoadFailed),
            (
                "RuntimeStartupScriptFailed",
                HelperErrorCode::RuntimeStartupScriptFailed,
            ),
            (
                "RuntimeNativePluginFailed",
                HelperErrorCode::RuntimeNativePluginFailed,
            ),
            ("RuntimeReadyTimeout", HelperErrorCode::RuntimeReadyTimeout),
            ("TargetResumeFailed", HelperErrorCode::TargetResumeFailed),
            ("TargetEntryTimeout", HelperErrorCode::TargetEntryTimeout),
            (
                "TargetTerminationFailed",
                HelperErrorCode::TargetTerminationFailed,
            ),
        ];
        for (code, expected) in cases {
            let error = parse_helper_error(&format!("noise\nERROR {code} detail")).unwrap();
            assert_eq!(error.code, expected);
            assert_eq!(error.detail, "detail");
        }
    }

    #[test]
    fn unknown_error_code_is_preserved_as_unknown() {
        let error = parse_helper_error("ERROR FutureFailure details").unwrap();
        assert_eq!(error.code, HelperErrorCode::Unknown);
        assert_eq!(error.detail, "details");
    }

    #[test]
    fn render_owner_diagnostics_are_retained_for_the_ui() {
        let error = parse_helper_error(
            "ERROR RenderBoundaryFailed target_terminated pid=42 stage=8 \
owner_path=\"C:\\client\\bahamut.dll\"",
        )
        .unwrap();

        assert_eq!(error.code, HelperErrorCode::RenderBoundaryFailed);
        assert_eq!(
            error.detail,
            "target_terminated pid=42 stage=8 \
owner_path=\"C:\\client\\bahamut.dll\""
        );
        assert!(error.to_string().contains("stage=8"));
        assert!(error.to_string().contains("bahamut.dll"));
    }

    #[test]
    fn non_error_success_line_is_ignored() {
        assert!(parse_helper_error("SUCCESS\nready").is_none());
    }

    #[test]
    fn parses_success_and_error_output_without_spawning() {
        let success = parse_helper_output("SUCCESS pid=42 process_handle=43 ready", "").unwrap();
        assert_eq!(success.client_pid, 42);
        assert_eq!(success.client_process_handle, 43);
        let error = parse_helper_output("", "ERROR MissingRuntimeDll missing").unwrap_err();
        assert_eq!(error.code, HelperErrorCode::MissingRuntimeDll);
    }

    #[test]
    fn wait_mode_success_accepts_a_pid_with_a_zero_or_absent_handle() {
        assert_eq!(
            parse_wait_mode_line("SUCCESS pid=42 process_handle=0 ready_signal resumed"),
            Some(Ok(42))
        );
        assert_eq!(
            parse_wait_mode_line("  SUCCESS pid=7 ready_signal\r"),
            Some(Ok(7))
        );
    }

    #[test]
    fn wait_mode_success_rejects_a_transferred_handle_or_missing_pid() {
        for line in [
            "SUCCESS pid=42 process_handle=43 ready_signal",
            "SUCCESS pid=42 process_handle=zero",
            "SUCCESS process_handle=0 ready_signal",
            "SUCCESS pid=-1 process_handle=0",
        ] {
            let failure = parse_wait_mode_line(line).unwrap().unwrap_err();
            assert_eq!(failure.code, HelperErrorCode::Unknown, "{line}");
        }
    }

    #[test]
    fn wait_mode_error_line_keeps_its_code_and_detail() {
        let failure = parse_wait_mode_line("ERROR RuntimeReadyTimeout target_terminated")
            .unwrap()
            .unwrap_err();
        assert_eq!(failure.code, HelperErrorCode::RuntimeReadyTimeout);
        assert_eq!(failure.detail, "target_terminated");
    }

    #[test]
    fn wait_mode_ignores_wine_debug_and_unrelated_lines() {
        for line in [
            "0024:err:module:import_dll Library MSVCP140.dll not found",
            "0024:trace:debugstr:OutputDebugStringA \"ERROR in assert\"",
            "=== bahamut-launcher extension launch ===",
            "SUCCESS",
            "ERRORS 3",
            "",
        ] {
            assert_eq!(parse_wait_mode_line(line), None, "{line}");
        }
    }

    #[test]
    fn helper_path_validation_rejects_nul_without_shell_parsing() {
        let path = Path::new("helper\0.exe");
        assert!(matches!(
            validate_helper_path(path),
            Err(HelperPlanError::InteriorNul { .. })
        ));
    }
}
