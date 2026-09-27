#![cfg_attr(not(windows), allow(dead_code))]

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(any(windows, test))]
use std::time::Duration;

#[cfg(any(windows, test))]
use reqwest::{blocking::Client, redirect::Policy};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

const CATALOG_JSON: &str = include_str!("../../manifests/windows-prerequisites.json");
const WEBVIEW2_ASSET_ID: &str = "webview2-evergreen-bootstrapper";
const X86_VC_ASSET_ID: &str = "vc-redist-x86";
const WEBVIEW2_SOURCE_URL: &str =
    "https://pub-f164b0f74f9d45769188b8a2924542d0.r2.dev/MicrosoftEdgeWebView2Setup.exe";
const X86_VC_SOURCE_URL: &str =
    "https://pub-f164b0f74f9d45769188b8a2924542d0.r2.dev/vc_redist.x86.exe";
static INSTALLER_COPY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum PrerequisiteError {
    #[error("The Windows prerequisite catalog is invalid: {0}")]
    InvalidCatalog(String),
    #[error("Could not read installer `{asset_id}`: {source}")]
    InstallerRead {
        asset_id: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Could not stage trusted installer `{asset_id}`: {detail}")]
    InstallerStage { asset_id: String, detail: String },
    #[error("Could not download installer `{asset_id}`: {detail}")]
    InstallerDownload { asset_id: String, detail: String },
    #[error("Could not start downloaded installer `{asset_id}`: {source}")]
    InstallerLaunch {
        asset_id: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Installer `{asset_id}` has length {actual}; expected {expected}")]
    InstallerLength {
        asset_id: String,
        expected: u64,
        actual: u64,
    },
    #[error("Installer `{asset_id}` does not match its catalog SHA-256")]
    InstallerDigest { asset_id: String },
    #[error("Could not inspect Windows runtime registry key `{key}`: {source}")]
    Registry {
        key: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("Installation of {prerequisite} was cancelled")]
    Cancelled { prerequisite: &'static str },
    #[error("{prerequisite} installer requires a Windows restart (exit code {exit_code})")]
    RebootRequired {
        prerequisite: &'static str,
        exit_code: i32,
    },
    #[error("{prerequisite} installer failed (exit code {exit_code:?})")]
    InstallerFailed {
        prerequisite: &'static str,
        exit_code: Option<i32>,
    },
    #[error("{prerequisite} was not detected after its installer succeeded")]
    VerificationFailed { prerequisite: &'static str },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrerequisiteCatalog {
    schema_version: u32,
    assets: Vec<InstallerAsset>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallerAsset {
    id: String,
    file_name: String,
    source_url: String,
    length: u64,
    sha256: String,
    version: String,
    minimum_runtime_version: String,
    architecture: String,
    install_args: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RuntimeVersion([u32; 4]);

impl RuntimeVersion {
    fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        let value = value
            .strip_prefix('v')
            .or_else(|| value.strip_prefix('V'))
            .unwrap_or(value);
        let mut components = value.split('.');
        let version = Self([
            components.next()?.parse().ok()?,
            components.next()?.parse().ok()?,
            components.next()?.parse().ok()?,
            components.next()?.parse().ok()?,
        ]);
        components.next().is_none().then_some(version)
    }

    fn is_nonzero(self) -> bool {
        self.0.iter().any(|component| *component != 0)
    }
}

fn parse_catalog() -> Result<PrerequisiteCatalog, PrerequisiteError> {
    let catalog: PrerequisiteCatalog = serde_json::from_str(CATALOG_JSON)
        .map_err(|error| PrerequisiteError::InvalidCatalog(error.to_string()))?;
    if catalog.schema_version != 1 {
        return Err(PrerequisiteError::InvalidCatalog(format!(
            "unsupported schema version {}",
            catalog.schema_version
        )));
    }

    let mut ids = std::collections::HashSet::new();
    if catalog
        .assets
        .iter()
        .any(|asset| !ids.insert(asset.id.as_str()))
    {
        return Err(PrerequisiteError::InvalidCatalog(
            "asset ids must be unique".into(),
        ));
    }
    Ok(catalog)
}

fn catalog_asset(
    catalog: &PrerequisiteCatalog,
    id: &'static str,
) -> Result<InstallerAsset, PrerequisiteError> {
    let asset = catalog
        .assets
        .iter()
        .find(|asset| asset.id == id)
        .cloned()
        .ok_or_else(|| {
            PrerequisiteError::InvalidCatalog(format!("required asset `{id}` is missing"))
        })?;

    let (expected_architecture, expected_url, expected_name) = match id {
        WEBVIEW2_ASSET_ID => (
            "bootstrapper",
            WEBVIEW2_SOURCE_URL,
            "MicrosoftEdgeWebView2Setup.exe",
        ),
        X86_VC_ASSET_ID => ("x86", X86_VC_SOURCE_URL, "vc_redist.x86.exe"),
        _ => {
            return Err(PrerequisiteError::InvalidCatalog(format!(
                "asset `{id}` is not a supported Windows prerequisite"
            )));
        }
    };
    if asset.length == 0
        || asset.sha256.len() != 64
        || !asset
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || asset.file_name != expected_name
        || asset.source_url != expected_url
        || asset.architecture != expected_architecture
        || RuntimeVersion::parse(&asset.version).is_none()
        || RuntimeVersion::parse(&asset.minimum_runtime_version).is_none()
        || asset.install_args.is_empty()
    {
        return Err(PrerequisiteError::InvalidCatalog(format!(
            "asset `{id}` has invalid metadata"
        )));
    }
    Ok(asset)
}

fn verify_installer_bytes(
    asset_id: &str,
    expected_length: u64,
    expected_sha256: &str,
    bytes: &[u8],
) -> Result<(), PrerequisiteError> {
    let actual_length = bytes.len() as u64;
    if actual_length != expected_length {
        return Err(PrerequisiteError::InstallerLength {
            asset_id: asset_id.to_owned(),
            expected: expected_length,
            actual: actual_length,
        });
    }
    let actual_sha256 = format!("{:x}", Sha256::digest(bytes));
    if actual_sha256 != expected_sha256 {
        return Err(PrerequisiteError::InstallerDigest {
            asset_id: asset_id.to_owned(),
        });
    }
    Ok(())
}

fn verify_installer_file(asset: &InstallerAsset, path: &Path) -> Result<(), PrerequisiteError> {
    use std::io::Read;

    let file = std::fs::File::open(path).map_err(|source| PrerequisiteError::InstallerRead {
        asset_id: asset.id.clone(),
        source,
    })?;
    let actual_length = file
        .metadata()
        .map_err(|source| PrerequisiteError::InstallerRead {
            asset_id: asset.id.clone(),
            source,
        })?
        .len();
    if actual_length != asset.length {
        return Err(PrerequisiteError::InstallerLength {
            asset_id: asset.id.clone(),
            expected: asset.length,
            actual: actual_length,
        });
    }

    let capacity = usize::try_from(asset.length).map_err(|_| {
        PrerequisiteError::InvalidCatalog(format!("asset `{}` is too large", asset.id))
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(asset.length.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| PrerequisiteError::InstallerRead {
            asset_id: asset.id.clone(),
            source,
        })?;
    verify_installer_bytes(&asset.id, asset.length, &asset.sha256, &bytes)
}

fn ensure_with_operations<Detect, Install>(
    prerequisite: &'static str,
    mut detect: Detect,
    install: Install,
) -> Result<(), PrerequisiteError>
where
    Detect: FnMut() -> Result<bool, PrerequisiteError>,
    Install: FnOnce() -> Result<Option<i32>, PrerequisiteError>,
{
    if detect()? {
        return Ok(());
    }
    match classify_installer_exit(prerequisite, install()?) {
        Ok(()) => {
            if detect()? {
                Ok(())
            } else {
                Err(PrerequisiteError::VerificationFailed { prerequisite })
            }
        }
        Err(error @ PrerequisiteError::RebootRequired { .. }) => {
            detect()?;
            Err(error)
        }
        Err(error) => Err(error),
    }
}

fn classify_installer_exit(
    prerequisite: &'static str,
    exit_code: Option<i32>,
) -> Result<(), PrerequisiteError> {
    match exit_code {
        Some(0) => Ok(()),
        Some(1602 | 1223) => Err(PrerequisiteError::Cancelled { prerequisite }),
        Some(code @ (3010 | 1641)) => Err(PrerequisiteError::RebootRequired {
            prerequisite,
            exit_code: code,
        }),
        other => Err(PrerequisiteError::InstallerFailed {
            prerequisite,
            exit_code: other,
        }),
    }
}

#[cfg(windows)]
fn show_native_message(message: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_TASKMODAL, MessageBoxW};
    use windows::core::PCWSTR;

    let message = wide_null(message);
    let title = wide_null("Bahamut Launcher");
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_TASKMODAL | MB_ICONERROR,
        );
    }
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn run_installer(
    asset: &InstallerAsset,
    prerequisite: &'static str,
) -> Result<Option<i32>, PrerequisiteError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
    use windows::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows::core::PCWSTR;

    let staged = download_verified_installer(asset)?;
    let path: Vec<u16> = staged
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let parameters = wide_null(&asset.install_args.join(" "));
    let verb = wide_null("runas");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(path.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    let status = (|| {
        if let Err(error) = unsafe { ShellExecuteExW(&mut info) } {
            if error.code() == ERROR_CANCELLED.to_hresult() {
                return Err(PrerequisiteError::Cancelled { prerequisite });
            }
            return Err(PrerequisiteError::InstallerLaunch {
                asset_id: asset.id.clone(),
                source: std::io::Error::other(error.to_string()),
            });
        }
        let result = (|| {
            if unsafe { WaitForSingleObject(info.hProcess, INFINITE) } != WAIT_OBJECT_0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut exit_code = 0_u32;
            unsafe { GetExitCodeProcess(info.hProcess, &mut exit_code) }
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            Ok(Some(exit_code as i32))
        })();
        let _ = unsafe { CloseHandle(info.hProcess) };
        result.map_err(|source| PrerequisiteError::InstallerLaunch {
            asset_id: asset.id.clone(),
            source,
        })
    })();
    let _ = fs::remove_file(&staged);
    status
}

#[cfg(windows)]
fn download_verified_installer(asset: &InstallerAsset) -> Result<PathBuf, PrerequisiteError> {
    let response = fetch_installer(asset)?;
    stage_verified_installer(asset, response)
}

#[cfg(any(windows, test))]
fn fetch_installer(
    asset: &InstallerAsset,
) -> Result<reqwest::blocking::Response, PrerequisiteError> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .redirect(Policy::none())
        .build()
        .map_err(|error| PrerequisiteError::InstallerDownload {
            asset_id: asset.id.clone(),
            detail: error.to_string(),
        })?;
    let response = client
        .get(&asset.source_url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .map_err(|error| PrerequisiteError::InstallerDownload {
            asset_id: asset.id.clone(),
            detail: error.to_string(),
        })?;
    if response.status() != reqwest::StatusCode::OK
        || response
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .is_some_and(|value| {
                value
                    .to_str()
                    .map(|text| !text.eq_ignore_ascii_case("identity"))
                    .unwrap_or(true)
            })
        || response
            .content_length()
            .is_some_and(|length| length > asset.length)
    {
        return Err(PrerequisiteError::InstallerDownload {
            asset_id: asset.id.clone(),
            detail: "unexpected HTTP status, encoding, or length".into(),
        });
    }
    Ok(response)
}

#[cfg(windows)]
fn stage_verified_installer(
    asset: &InstallerAsset,
    input: impl Read,
) -> Result<PathBuf, PrerequisiteError> {
    let local_app_data =
        std::env::var_os("LOCALAPPDATA").ok_or_else(|| PrerequisiteError::InstallerStage {
            asset_id: asset.id.clone(),
            detail: "LOCALAPPDATA is unavailable".into(),
        })?;
    let mut directory = PathBuf::from(local_app_data);
    for component in ["BahamutXIV", "Launcher", "prerequisites"] {
        directory.push(component);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.is_dir() && !is_reparse(&metadata) => {}
            Ok(_) => {
                return Err(PrerequisiteError::InstallerStage {
                    asset_id: asset.id.clone(),
                    detail: format!("{} is not a plain directory", directory.display()),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&directory) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(PrerequisiteError::InstallerStage {
                            asset_id: asset.id.clone(),
                            detail: format!("could not create {}: {error}", directory.display()),
                        });
                    }
                }
                let metadata = fs::symlink_metadata(&directory).map_err(|error| {
                    PrerequisiteError::InstallerStage {
                        asset_id: asset.id.clone(),
                        detail: format!("could not inspect {}: {error}", directory.display()),
                    }
                })?;
                if !metadata.is_dir() || is_reparse(&metadata) {
                    return Err(PrerequisiteError::InstallerStage {
                        asset_id: asset.id.clone(),
                        detail: format!("{} is not a plain directory", directory.display()),
                    });
                }
            }
            Err(error) => {
                return Err(PrerequisiteError::InstallerStage {
                    asset_id: asset.id.clone(),
                    detail: format!("could not inspect {}: {error}", directory.display()),
                });
            }
        }
    }
    stage_verified_installer_in(asset, input, &directory)
}

fn stage_verified_installer_in(
    asset: &InstallerAsset,
    input: impl Read,
    directory: &Path,
) -> Result<PathBuf, PrerequisiteError> {
    let mut input = input.take(asset.length.saturating_add(1));
    let mut output_path = None;
    let mut output = None;
    for _ in 0..16 {
        let sequence = INSTALLER_COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(
            "{}-{}-{}-{sequence}.exe",
            asset.id,
            std::process::id(),
            &asset.sha256[..12]
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                output_path = Some(path);
                output = Some(file);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(PrerequisiteError::InstallerStage {
                    asset_id: asset.id.clone(),
                    detail: format!("could not create private copy: {error}"),
                });
            }
        }
    }
    let path = output_path.ok_or_else(|| PrerequisiteError::InstallerStage {
        asset_id: asset.id.clone(),
        detail: "could not allocate a unique private copy".into(),
    })?;
    let mut output = output.take().expect("a created path has an open file");
    let copied = (|| {
        let mut hasher = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            total = total.saturating_add(read as u64);
            hasher.update(&buffer[..read]);
            output.write_all(&buffer[..read])?;
        }
        output.sync_all()?;
        if total != asset.length {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("copied {total} bytes; expected {}", asset.length),
            ));
        }
        let digest = format!("{:x}", hasher.finalize());
        if !digest.eq_ignore_ascii_case(&asset.sha256) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "copied bytes do not match the catalog SHA-256",
            ));
        }
        Ok::<(), std::io::Error>(())
    })();
    drop(output);
    if let Err(error) = copied {
        let _ = fs::remove_file(&path);
        return Err(PrerequisiteError::InstallerStage {
            asset_id: asset.id.clone(),
            detail: error.to_string(),
        });
    }
    if let Err(error) = verify_installer_file(asset, &path) {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

#[cfg(windows)]
fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn ensure_installed(
    asset: &InstallerAsset,
    prerequisite: &'static str,
    mut detect: impl FnMut() -> Result<bool, PrerequisiteError>,
) -> Result<(), PrerequisiteError> {
    ensure_with_operations(prerequisite, &mut detect, || {
        run_installer(asset, prerequisite)
    })
}

#[cfg(windows)]
pub fn ensure_webview2_for_startup() -> Result<(), PrerequisiteError> {
    let catalog = parse_catalog()?;
    let asset = catalog_asset(&catalog, WEBVIEW2_ASSET_ID)?;
    ensure_installed(&asset, "Microsoft Edge WebView2 Runtime", || {
        detect_webview2_runtime().map(|version| version.is_some_and(RuntimeVersion::is_nonzero))
    })
}

#[cfg(not(windows))]
pub fn ensure_webview2_for_startup() -> Result<(), PrerequisiteError> {
    Ok(())
}

#[cfg(windows)]
pub fn ensure_x86_vc_runtime_for_game() -> Result<(), PrerequisiteError> {
    let catalog = parse_catalog()?;
    let asset = catalog_asset(&catalog, X86_VC_ASSET_ID)?;
    let minimum = RuntimeVersion::parse(&asset.minimum_runtime_version).ok_or_else(|| {
        PrerequisiteError::InvalidCatalog(format!(
            "asset `{}` has an invalid minimum runtime version",
            asset.id
        ))
    })?;
    ensure_installed(&asset, "Microsoft Visual C++ x86 Runtime", || {
        detect_x86_vc_runtime().map(|version| version.is_some_and(|version| version >= minimum))
    })
}

#[cfg(not(windows))]
pub fn ensure_x86_vc_runtime_for_game() -> Result<(), PrerequisiteError> {
    Ok(())
}

pub fn show_startup_error_dialog(message: &str) {
    #[cfg(windows)]
    show_native_message(message);

    #[cfg(not(windows))]
    eprintln!("{message}");
}

#[cfg(windows)]
fn detect_webview2_runtime() -> Result<Option<RuntimeVersion>, PrerequisiteError> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};

    const HKLM_KEY: &str =
        r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    const HKCU_KEY: &str =
        r"Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

    for (root, path) in [
        (RegKey::predef(HKEY_LOCAL_MACHINE), HKLM_KEY),
        (RegKey::predef(HKEY_CURRENT_USER), HKCU_KEY),
    ] {
        if let Some(value) = read_registry_string(&root, path, "pv", KEY_READ)?
            && let Some(version) = RuntimeVersion::parse(&value).filter(|v| v.is_nonzero())
        {
            return Ok(Some(version));
        }
    }
    Ok(None)
}

#[cfg(windows)]
fn detect_x86_vc_runtime() -> Result<Option<RuntimeVersion>, PrerequisiteError> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY};

    const KEY: &str = r"SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x86";
    let root = RegKey::predef(HKEY_LOCAL_MACHINE);
    let runtime = match root.open_subkey_with_flags(KEY, KEY_READ | KEY_WOW64_32KEY) {
        Ok(key) => key,
        Err(error) if is_missing_registry_entry(&error) => return Ok(None),
        Err(source) => return Err(PrerequisiteError::Registry { key: KEY, source }),
    };
    let installed = match runtime.get_value::<u32, _>("Installed") {
        Ok(value) => value != 0,
        Err(error) if is_missing_registry_entry(&error) => false,
        Err(source) => return Err(PrerequisiteError::Registry { key: KEY, source }),
    };
    if !installed {
        return Ok(None);
    }
    let value = match runtime.get_value::<String, _>("Version") {
        Ok(value) => value,
        Err(error) if is_missing_registry_entry(&error) => return Ok(None),
        Err(source) => return Err(PrerequisiteError::Registry { key: KEY, source }),
    };
    Ok(RuntimeVersion::parse(&value))
}

#[cfg(windows)]
fn read_registry_string(
    root: &winreg::RegKey,
    key: &'static str,
    value: &str,
    access: u32,
) -> Result<Option<String>, PrerequisiteError> {
    let subkey = match root.open_subkey_with_flags(key, access) {
        Ok(subkey) => subkey,
        Err(error) if is_missing_registry_entry(&error) => return Ok(None),
        Err(source) => return Err(PrerequisiteError::Registry { key, source }),
    };
    match subkey.get_value::<String, _>(value) {
        Ok(value) => Ok(Some(value)),
        Err(error) if is_missing_registry_entry(&error) => Ok(None),
        Err(source) if source.kind() == std::io::ErrorKind::InvalidData => Ok(None),
        Err(source) => Err(PrerequisiteError::Registry { key, source }),
    }
}

#[cfg(windows)]
fn is_missing_registry_entry(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound || matches!(error.raw_os_error(), Some(2 | 3))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::{
        InstallerAsset, PrerequisiteError, RuntimeVersion, WEBVIEW2_ASSET_ID, WEBVIEW2_SOURCE_URL,
        X86_VC_ASSET_ID, X86_VC_SOURCE_URL, catalog_asset, classify_installer_exit,
        ensure_with_operations, fetch_installer, parse_catalog, stage_verified_installer_in,
        verify_installer_bytes, verify_installer_file,
    };

    fn fixture_asset() -> InstallerAsset {
        InstallerAsset {
            id: "fixture".into(),
            file_name: "fixture.exe".into(),
            source_url: "https://example.com/fixture.exe".into(),
            length: 5,
            sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into(),
            version: "1.0.0.0".into(),
            minimum_runtime_version: "1.0.0.0".into(),
            architecture: "x86".into(),
            install_args: vec!["/quiet".into()],
        }
    }

    #[test]
    fn runtime_versions_parse_and_compare_by_all_four_components() {
        let minimum = RuntimeVersion::parse("14.44.35207.0").unwrap();
        assert_eq!(RuntimeVersion::parse("v14.44.35207.0"), Some(minimum));
        assert!(RuntimeVersion::parse("14.44.35208.0").unwrap() > minimum);
        assert!(RuntimeVersion::parse("14.44.35207.1").unwrap() > minimum);
        assert!(!RuntimeVersion::parse("0.0.0.0").unwrap().is_nonzero());
        assert!(RuntimeVersion::parse("1.2.3").is_none());
        assert!(RuntimeVersion::parse("1.2.x.4").is_none());
    }

    #[test]
    fn catalog_contains_the_fixed_ids_versions_hashes_and_install_arguments() {
        let catalog = parse_catalog().unwrap();
        let vc = catalog_asset(&catalog, X86_VC_ASSET_ID).unwrap();
        assert_eq!(vc.file_name, "vc_redist.x86.exe");
        assert_eq!(vc.source_url, X86_VC_SOURCE_URL);
        assert_eq!(vc.length, 6_876_208);
        assert_eq!(
            vc.sha256,
            "e7267c1bdf9237c0b4a28cf027c382b97aa909934f84f1c92d3fb9f04173b33e"
        );
        assert_eq!(vc.version, "14.50.35719.0");
        assert_eq!(vc.minimum_runtime_version, "14.44.35207.0");
        assert_eq!(vc.architecture, "x86");
        assert_eq!(vc.install_args, ["/install", "/passive", "/norestart"]);

        let webview = catalog_asset(&catalog, WEBVIEW2_ASSET_ID).unwrap();
        assert_eq!(webview.file_name, "MicrosoftEdgeWebView2Setup.exe");
        assert_eq!(webview.source_url, WEBVIEW2_SOURCE_URL);
        assert_eq!(webview.length, 1_844_944);
        assert_eq!(
            webview.sha256,
            "81c01751c8cc385a5991abb104205d42ac70094350ee8fb9e8ea580b51bb9554"
        );
        assert_eq!(webview.minimum_runtime_version, "0.0.0.0");
        assert_eq!(webview.architecture, "bootstrapper");
        assert_eq!(webview.install_args, ["/silent", "/install"]);
    }

    #[test]
    fn installer_bytes_require_exact_length_and_digest() {
        let digest = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert!(verify_installer_bytes("fixture", 5, digest, b"hello").is_ok());
        assert!(matches!(
            verify_installer_bytes("fixture", 4, digest, b"hello"),
            Err(PrerequisiteError::InstallerLength { .. })
        ));
        assert!(matches!(
            verify_installer_bytes("fixture", 5, &"00".repeat(32), b"hello"),
            Err(PrerequisiteError::InstallerDigest { .. })
        ));
    }

    #[test]
    fn installer_catalog_rejects_an_unpinned_download_url() {
        let mut catalog = parse_catalog().unwrap();
        catalog.assets[0].source_url = "https://example.com/installer.exe".into();
        assert!(matches!(
            catalog_asset(&catalog, X86_VC_ASSET_ID),
            Err(PrerequisiteError::InvalidCatalog(_))
        ));
    }

    #[test]
    fn downloaded_installer_stream_is_staged_only_after_exact_verification() {
        let directory = tempfile::tempdir().unwrap();
        let asset = fixture_asset();

        let staged =
            stage_verified_installer_in(&asset, b"hello".as_slice(), directory.path()).unwrap();
        verify_installer_file(&asset, &staged).unwrap();
        assert_eq!(std::fs::read(&staged).unwrap(), b"hello");
        for invalid in [
            b"hell".as_slice(),
            b"other".as_slice(),
            b"hello!".as_slice(),
        ] {
            assert!(stage_verified_installer_in(&asset, invalid, directory.path()).is_err());
        }
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn installer_download_rejects_http_errors_and_stages_verified_bytes() {
        for (status, expected_ok) in [("200 OK", true), ("503 Unavailable", false)] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 1024];
                assert!(stream.read(&mut request).unwrap() > 0);
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello"
                )
                .unwrap();
            });
            let mut asset = fixture_asset();
            asset.source_url = format!("http://{address}/fixture.exe");
            let result = fetch_installer(&asset);
            server.join().unwrap();
            if expected_ok {
                let directory = tempfile::tempdir().unwrap();
                let staged =
                    stage_verified_installer_in(&asset, result.unwrap(), directory.path()).unwrap();
                assert_eq!(std::fs::read(staged).unwrap(), b"hello");
            } else {
                assert!(matches!(
                    result,
                    Err(PrerequisiteError::InstallerDownload { .. })
                ));
            }
        }
    }

    #[test]
    fn detected_runtime_does_not_run_installer() {
        let install_called = Cell::new(false);
        let result = ensure_with_operations(
            "fixture",
            || Ok(true),
            || {
                install_called.set(true);
                Ok(Some(0))
            },
        );
        assert!(result.is_ok());
        assert!(!install_called.get());
    }

    #[test]
    fn installer_exit_codes_distinguish_cancel_failure_and_reboot() {
        assert!(matches!(
            classify_installer_exit("fixture", Some(1602)),
            Err(PrerequisiteError::Cancelled { .. })
        ));
        assert!(matches!(
            classify_installer_exit("fixture", Some(1223)),
            Err(PrerequisiteError::Cancelled { .. })
        ));
        assert!(matches!(
            classify_installer_exit("fixture", Some(3010)),
            Err(PrerequisiteError::RebootRequired {
                exit_code: 3010,
                ..
            })
        ));
        assert!(matches!(
            classify_installer_exit("fixture", Some(1641)),
            Err(PrerequisiteError::RebootRequired {
                exit_code: 1641,
                ..
            })
        ));
        assert!(matches!(
            classify_installer_exit("fixture", Some(1603)),
            Err(PrerequisiteError::InstallerFailed {
                exit_code: Some(1603),
                ..
            })
        ));
    }

    #[test]
    fn successful_install_is_followed_by_detection() {
        let detect_count = Cell::new(0);
        let install_count = Cell::new(0);
        let result = ensure_with_operations(
            "fixture",
            || {
                let next = detect_count.get() + 1;
                detect_count.set(next);
                Ok(next >= 2)
            },
            || {
                install_count.set(install_count.get() + 1);
                Ok(Some(0))
            },
        );
        assert!(result.is_ok());
        assert_eq!(install_count.get(), 1);
        assert_eq!(detect_count.get(), 2);
    }

    #[test]
    fn successful_exit_without_detected_runtime_fails_verification() {
        let detect_count = Cell::new(0);
        let result = ensure_with_operations(
            "fixture",
            || {
                detect_count.set(detect_count.get() + 1);
                Ok(false)
            },
            || Ok(Some(0)),
        );
        assert!(matches!(
            result,
            Err(PrerequisiteError::VerificationFailed { .. })
        ));
        assert_eq!(detect_count.get(), 2);
    }

    #[test]
    fn reboot_exit_redetects_before_reporting_reboot_required() {
        for runtime_detected_after_install in [false, true] {
            let detect_count = Cell::new(0);
            let result = ensure_with_operations(
                "fixture",
                || {
                    let next = detect_count.get() + 1;
                    detect_count.set(next);
                    Ok(next >= 2 && runtime_detected_after_install)
                },
                || Ok(Some(3010)),
            );
            assert!(matches!(
                result,
                Err(PrerequisiteError::RebootRequired {
                    exit_code: 3010,
                    ..
                })
            ));
            assert_eq!(detect_count.get(), 2);
        }
    }
}
