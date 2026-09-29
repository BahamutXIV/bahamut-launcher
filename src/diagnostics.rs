//! Bounded, support-relevant host diagnostics.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemDiagnostics {
    pub os: String,
    pub cpu: String,
    pub logical_cores: usize,
    pub memory_total_bytes: Option<u64>,
    pub memory_available_bytes: Option<u64>,
    pub gpus: Vec<String>,
}

pub fn system_diagnostics() -> SystemDiagnostics {
    let memory = memory_bytes();
    SystemDiagnostics {
        os: os_description(),
        cpu: cpu_description(),
        logical_cores: std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1),
        memory_total_bytes: memory.map(|memory| memory.0),
        memory_available_bytes: memory.map(|memory| memory.1),
        gpus: gpu_descriptions(),
    }
}

/// Binary units labelled KB, MB, and GB, as the launcher UI labels them.
pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.2} GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.2} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.2} KB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

pub fn free_disk_bytes(path: &Path) -> Option<u64> {
    platform_free_disk_bytes(path)
}

#[cfg(target_os = "windows")]
fn os_description() -> String {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};

    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            KEY_READ | KEY_WOW64_64KEY,
        )
        .ok();
    let product = key
        .as_ref()
        .and_then(|key| key.get_value::<String, _>("ProductName").ok())
        .unwrap_or_else(|| "Windows".to_string());
    let display = key
        .as_ref()
        .and_then(|key| key.get_value::<String, _>("DisplayVersion").ok());
    let build = key
        .as_ref()
        .and_then(|key| key.get_value::<String, _>("CurrentBuildNumber").ok());
    let revision = key
        .as_ref()
        .and_then(|key| key.get_value::<u32, _>("UBR").ok());

    let product = normalize_windows_product(product, build.as_deref());
    let mut parts = vec![product];
    if let Some(display) = display {
        parts.push(display);
    }
    if let Some(build) = build {
        parts.push(match revision {
            Some(revision) => format!("build {build}.{revision}"),
            None => format!("build {build}"),
        });
    }
    parts.join(" ")
}

#[cfg(target_os = "windows")]
fn normalize_windows_product(product: String, build: Option<&str>) -> String {
    if build
        .and_then(|build| build.parse::<u32>().ok())
        .unwrap_or(0)
        >= 22_000
    {
        product.replacen("Windows 10", "Windows 11", 1)
    } else {
        product
    }
}

#[cfg(not(target_os = "windows"))]
fn os_description() -> String {
    std::env::consts::OS.to_string()
}

#[cfg(target_os = "windows")]
fn cpu_description() -> String {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;

    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
        .and_then(|key| key.get_value::<String, _>("ProcessorNameString"))
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|_| "unavailable".to_string())
}

#[cfg(not(target_os = "windows"))]
fn cpu_description() -> String {
    "unavailable".to_string()
}

#[cfg(target_os = "windows")]
fn memory_bytes() -> Option<(u64, u64)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some((status.ullTotalPhys, status.ullAvailPhys))
}

#[cfg(target_os = "windows")]
fn gpu_descriptions() -> Vec<String> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
    };

    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut output = Vec::new();
    for index in 0..16 {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let Ok(description) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if description.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let end = description
            .Description
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(description.Description.len());
        let name = String::from_utf16_lossy(&description.Description[..end]);
        output.push(format!(
            "{} ({:04X}:{:04X})",
            name.trim(),
            description.VendorId,
            description.DeviceId
        ));
    }
    output
}

#[cfg(not(target_os = "windows"))]
fn gpu_descriptions() -> Vec<String> {
    Vec::new()
}

#[cfg(not(target_os = "windows"))]
fn memory_bytes() -> Option<(u64, u64)> {
    None
}

#[cfg(target_os = "windows")]
fn platform_free_disk_bytes(path: &Path) -> Option<u64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::HSTRING;

    let path = HSTRING::from(path.as_os_str());
    let mut available = 0_u64;
    unsafe { GetDiskFreeSpaceExW(&path, Some(&mut available), None, None) }.ok()?;
    Some(available)
}

#[cfg(unix)]
fn platform_free_disk_bytes(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    // statvfs field types vary by target; widen before the checked product.
    let unit = i128::from(if info.f_frsize > 0 {
        info.f_frsize
    } else {
        info.f_bsize
    });
    if unit <= 0 {
        return None;
    }
    let blocks = i128::from(info.f_bavail);
    u64::try_from(blocks.checked_mul(unit)?).ok()
}

#[cfg(not(any(target_os = "windows", unix)))]
fn platform_free_disk_bytes(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn unix_free_disk_bytes_stay_within_volume_capacity() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let temp = tempfile::tempdir().unwrap();
        let free = free_disk_bytes(temp.path()).unwrap();
        let path = CString::new(temp.path().as_os_str().as_bytes()).unwrap();
        let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        assert_eq!(
            unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) },
            0
        );
        let info = unsafe { info.assume_init() };
        let unit = i128::from(if info.f_frsize > 0 {
            info.f_frsize
        } else {
            info.f_bsize
        });
        let total = u64::try_from(i128::from(info.f_blocks) * unit).unwrap();
        assert!(free > 0);
        assert!(free <= total, "{free} free exceeds {total} total");
    }

    #[test]
    fn byte_format_is_stable_for_support_logs() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.00 MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.00 GB");
    }

    #[test]
    fn system_diagnostics_never_report_zero_logical_cores() {
        let diagnostics = system_diagnostics();
        assert!(diagnostics.logical_cores >= 1);
        assert!(!diagnostics.os.trim().is_empty());
        assert!(!diagnostics.cpu.trim().is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_11_builds_do_not_report_windows_10() {
        assert_eq!(
            normalize_windows_product("Windows 10 Home".to_string(), Some("26200")),
            "Windows 11 Home"
        );
        assert_eq!(
            normalize_windows_product("Windows 10 Home".to_string(), Some("19045")),
            "Windows 10 Home"
        );
    }
}
