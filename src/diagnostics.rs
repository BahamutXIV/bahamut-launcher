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

/// A `/proc/bus/input/devices` entry reduced to identifiers. Names, `Phys`, `Uniq`, and `Sysfs`
/// are dropped because they can carry user-chosen names and serial numbers and logs are pasted
/// publicly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDeviceEntry {
    pub vendor: u16,
    pub product: u16,
    pub handlers: String,
}

const MAX_INPUT_DEVICE_ENTRIES: usize = 256;
const MAX_SHOWN_INPUT_DEVICES: usize = 64;
const VALVE_VENDOR_ID: u16 = 0x28de;
const MAX_HANDLERS_CHARS: usize = 120;
const MAX_ENV_VALUE_CHARS: usize = 200;

/// Parse the `I:` and `H:` lines of each blank-line separated stanza, keeping at most 256 entries.
pub fn parse_proc_input_devices(text: &str) -> Vec<InputDeviceEntry> {
    fn flush(
        entries: &mut Vec<InputDeviceEntry>,
        ids: &mut Option<(u16, u16)>,
        handlers: &mut String,
    ) {
        if let Some((vendor, product)) = ids.take()
            && entries.len() < MAX_INPUT_DEVICE_ENTRIES
        {
            entries.push(InputDeviceEntry {
                vendor,
                product,
                handlers: std::mem::take(handlers),
            });
        }
        handlers.clear();
    }

    let mut entries = Vec::new();
    let mut ids = None;
    let mut handlers = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            flush(&mut entries, &mut ids, &mut handlers);
        } else if let Some(rest) = line.strip_prefix("I:") {
            let field = |name: &str| {
                rest.split_whitespace()
                    .find_map(|token| token.strip_prefix(name))
                    .and_then(|value| u16::from_str_radix(value, 16).ok())
            };
            ids = field("Vendor=").zip(field("Product="));
        } else if let Some(rest) = line.strip_prefix("H: Handlers=") {
            let joined = rest.split_whitespace().collect::<Vec<_>>().join(" ");
            handlers = truncate_chars(&sanitize(&joined), MAX_HANDLERS_CHARS);
        }
    }
    flush(&mut entries, &mut ids, &mut handlers);
    entries
}

impl InputDeviceEntry {
    /// Whether the entry could be a game controller: a joystick handler (`js<N>`), a Valve
    /// vendor ID, or neither a keyboard (`kbd`) nor a mouse (`mouse<N>`) handler.
    pub fn could_be_gamepad(&self) -> bool {
        let numbered = |token: &str, prefix: &str| {
            token
                .strip_prefix(prefix)
                .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        };
        let mut keyboard_or_mouse = false;
        for token in self.handlers.split_whitespace() {
            if numbered(token, "js") {
                return true;
            }
            keyboard_or_mouse |= token == "kbd" || numbered(token, "mouse");
        }
        self.vendor == VALVE_VENDOR_ID || !keyboard_or_mouse
    }
}

/// Count line plus one line per entry that could be a game controller, at most 64 entries.
/// Keyboards and mice are counted but not listed.
pub fn input_device_lines(entries: &[InputDeviceEntry]) -> Vec<String> {
    let shown: Vec<&InputDeviceEntry> = entries
        .iter()
        .filter(|entry| entry.could_be_gamepad())
        .take(MAX_SHOWN_INPUT_DEVICES)
        .collect();
    let mut lines = vec![format!(
        "Host-reported input devices (procfs, not filtered by the sandbox) = {} ({} shown)",
        entries.len(),
        shown.len()
    )];
    lines.extend(shown.into_iter().map(|entry| {
        format!(
            "host-reported input device = {:04x}:{:04x} {}",
            entry.vendor, entry.product, entry.handlers
        )
    }));
    lines
}

/// The `devices=` value of the `[Context]` section of `/.flatpak-info`.
pub fn parse_flatpak_devices(text: &str) -> Option<String> {
    flatpak_info_value(text, "Context", "devices")
}

/// The `flatpak-version=` value of the `[Instance]` section of `/.flatpak-info`.
pub fn parse_flatpak_version(text: &str) -> Option<String> {
    flatpak_info_value(text, "Instance", "flatpak-version")
}

fn flatpak_info_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_section = header == section;
        } else if in_section
            && let Some((name, value)) = line.split_once('=')
            && name.trim() == key
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// Selected SDL and Steam variables, one `NAME = value` line each. Allowlisted names only: the
/// launch environment is never dumped, and path-like or identifying values report set or unset.
pub fn controller_environment_lines(get: impl Fn(&str) -> Option<String>) -> Vec<String> {
    enum Report {
        Value,
        Presence,
    }
    const ALLOWLIST: &[(&str, Report)] = &[
        (
            "SDL_GAMECONTROLLER_ALLOW_STEAM_VIRTUAL_GAMEPAD",
            Report::Value,
        ),
        ("SDL_GAMECONTROLLER_IGNORE_DEVICES", Report::Value),
        ("SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT", Report::Value),
        ("SDL_GAMECONTROLLERCONFIG", Report::Presence),
        ("SteamVirtualGamepadInfo", Report::Presence),
        ("SteamAppId", Report::Presence),
        ("SteamGameId", Report::Presence),
        ("STEAM_COMPAT_APP_ID", Report::Presence),
        ("XDG_CURRENT_DESKTOP", Report::Value),
    ];

    ALLOWLIST
        .iter()
        .map(|(name, report)| {
            let shown = match (get(name), report) {
                (None, _) => "(unset)".to_string(),
                (Some(_), Report::Presence) => "(set)".to_string(),
                (Some(value), Report::Value) if value.is_empty() => "(empty)".to_string(),
                (Some(value), Report::Value) => {
                    truncate_chars(&sanitize(&value), MAX_ENV_VALUE_CHARS)
                }
            };
            format!("{name} = {shown}")
        })
        .collect()
}

/// `controller_environment_lines` over this process's environment.
pub fn process_controller_environment_lines() -> Vec<String> {
    controller_environment_lines(|name| std::env::var(name).ok())
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn truncate_chars(value: &str, max: usize) -> String {
    match value.char_indices().nth(max) {
        Some((end, _)) => format!("{}...(truncated)", &value[..end]),
        None => value.to_string(),
    }
}

/// Controller-relevant sandbox and input state for the support log, at most 200 lines.
#[cfg(target_os = "linux")]
pub fn controller_diagnostics() -> Vec<String> {
    const MAX_LINES: usize = 200;

    let mut lines = Vec::new();
    match std::fs::read_to_string("/.flatpak-info") {
        Ok(info) => {
            let version = parse_flatpak_version(&info).unwrap_or_else(|| "(unknown)".to_string());
            lines.push(format!("Flatpak version = {version}"));
            let devices = match parse_flatpak_devices(&info) {
                Some(devices) if !devices.is_empty() => devices,
                Some(_) => "(empty)".to_string(),
                None => "(no devices entry)".to_string(),
            };
            lines.push(format!("Flatpak devices = {devices}"));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            lines.push("Flatpak = not running in a Flatpak sandbox".to_string());
        }
        Err(error) => lines.push(format!("Flatpak = unavailable ({:?})", error.kind())),
    }

    match std::fs::read_to_string("/proc/bus/input/devices") {
        Ok(text) => {
            lines.extend(input_device_lines(&parse_proc_input_devices(&text)));
        }
        Err(error) => lines.push(format!(
            "Host-reported input devices = unavailable ({:?})",
            error.kind()
        )),
    }

    lines.extend(input_node_lines());

    let sdl = sdl_library_paths();
    if sdl.is_empty() {
        lines.push("libSDL2 = not found under the common library directories".to_string());
    } else {
        lines.push(format!("libSDL2 = present: {}", sdl.join(", ")));
    }

    lines.extend(process_controller_environment_lines());

    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES - 1);
        lines.push("(output truncated)".to_string());
    }
    lines
}

/// Mode, ownership, and a read-only open result for each `event*` and `js*` node. Subdirectories
/// such as `by-id` are skipped because their names can embed device names and serial numbers.
#[cfg(target_os = "linux")]
fn input_node_lines() -> Vec<String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    const MAX_NODES: usize = 64;
    let directory = Path::new("/dev/input");
    let reader = match std::fs::read_dir(directory) {
        Ok(reader) => reader,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return vec!["/dev/input = missing".to_string()];
        }
        Err(error) => return vec![format!("/dev/input = unreadable ({:?})", error.kind())],
    };

    let is_node = |name: &str| {
        ["event", "js"].iter().any(|prefix| {
            name.strip_prefix(prefix)
                .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        })
    };
    let mut names: Vec<String> = reader
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| is_node(name))
        .collect();
    names.sort_by_key(|name| {
        let split = name.find(|c: char| c.is_ascii_digit()).unwrap_or(0);
        (
            name[..split].to_string(),
            name[split..].parse::<u32>().unwrap_or(0),
        )
    });

    let mut lines = vec![format!("/dev/input nodes (event*, js*) = {}", names.len())];
    for name in names.into_iter().take(MAX_NODES) {
        let path = directory.join(&name);
        let mode = match std::fs::metadata(&path) {
            Ok(meta) => format!(
                "mode={:04o} uid={} gid={}",
                meta.mode() & 0o7777,
                meta.uid(),
                meta.gid()
            ),
            Err(error) => format!("stat={:?}", error.kind()),
        };
        // Opening read-only without reading is enough to tell whether the sandbox grants access.
        let open = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(_) => "readable".to_string(),
            Err(error) => format!("{:?}", error.kind()),
        };
        lines.push(format!("{} {mode} open={open}", path.display()));
    }
    lines
}

#[cfg(target_os = "linux")]
fn sdl_library_paths() -> Vec<String> {
    const DIRECTORIES: &[&str] = &[
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib64",
        "/usr/lib",
        "/app/lib",
    ];
    DIRECTORIES
        .iter()
        .map(|directory| Path::new(directory).join("libSDL2-2.0.so.0"))
        .filter(|path| path.exists())
        .map(|path| path.display().to_string())
        .collect()
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

    const PROC_FIXTURE: &str = "\
I: Bus=0003 Vendor=28de Product=11ff Version=0001
N: Name=\"Microsoft X-Box 360 pad 0\"
P: Phys=usb-0000:00:14.0-3/input0
S: Sysfs=/devices/virtual/input/input99
U: Uniq=SERIAL-1234
H: Handlers=js0 event5 
B: PROP=0

I: Bus=0003 Vendor=046d Product=c31c Version=0110
N: Name=\"Alice's Keyboard\"
P: Phys=usb-0000:00:14.0-2/input0
S: Sysfs=/devices/pci0000:00/usb1/1-2/input/input3
U: Uniq=
H: Handlers=sysrq kbd event3
B: EV=120013
";

    #[test]
    fn proc_input_devices_keep_ids_and_handlers_only() {
        let entries = parse_proc_input_devices(PROC_FIXTURE);
        assert_eq!(
            entries,
            vec![
                InputDeviceEntry {
                    vendor: 0x28de,
                    product: 0x11ff,
                    handlers: "js0 event5".to_string(),
                },
                InputDeviceEntry {
                    vendor: 0x046d,
                    product: 0xc31c,
                    handlers: "sysrq kbd event3".to_string(),
                },
            ]
        );
        let rendered = format!("{entries:?}");
        for hidden in [
            "X-Box",
            "Alice",
            "Keyboard",
            "usb-0000",
            "SERIAL-1234",
            "Sysfs",
            "/devices/",
        ] {
            assert!(!rendered.contains(hidden), "{hidden} leaked: {rendered}");
        }
    }

    #[test]
    fn proc_input_devices_handle_missing_trailing_blank_and_bad_ids() {
        assert!(parse_proc_input_devices("").is_empty());
        assert!(parse_proc_input_devices("I: Bus=0003 Vendor=zzzz Product=0001\n").is_empty());
        let entries = parse_proc_input_devices("I: Bus=0019 Vendor=0000 Product=0001 Version=0\n");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].handlers.is_empty());
    }

    #[test]
    fn proc_input_device_handlers_are_sanitized_and_capped() {
        let text = format!(
            "I: Bus=0003 Vendor=0001 Product=0002 Version=0\nH: Handlers=js0\u{7}\u{1b}[31m {}\n",
            "event0 ".repeat(40)
        );
        let entries = parse_proc_input_devices(&text);
        assert_eq!(entries.len(), 1);
        let handlers = &entries[0].handlers;
        assert!(handlers.chars().all(|c| !c.is_control()), "{handlers:?}");
        assert!(handlers.starts_with("js0??[31m event0"));
        assert!(handlers.ends_with("...(truncated)"));
        assert!(handlers.chars().count() <= MAX_HANDLERS_CHARS + "...(truncated)".len());
    }

    #[test]
    fn input_device_lines_list_only_possible_gamepads() {
        let entries = parse_proc_input_devices(PROC_FIXTURE);
        let lines = input_device_lines(&entries);
        assert_eq!(
            lines,
            vec![
                "Host-reported input devices (procfs, not filtered by the sandbox) = 2 (1 shown)"
                    .to_string(),
                "host-reported input device = 28de:11ff js0 event5".to_string(),
            ]
        );
    }

    #[test]
    fn possible_gamepads_are_recognized_by_handler_and_vendor() {
        let entry = |vendor, handlers: &str| InputDeviceEntry {
            vendor,
            product: 1,
            handlers: handlers.to_string(),
        };
        assert!(entry(0x045e, "js2 event9").could_be_gamepad());
        assert!(entry(0x045e, "event9").could_be_gamepad());
        assert!(entry(0x28de, "kbd mouse1 event9").could_be_gamepad());
        assert!(entry(0x045e, "").could_be_gamepad());
        assert!(!entry(0x046d, "sysrq kbd event3").could_be_gamepad());
        assert!(!entry(0x046d, "mouse0 event4").could_be_gamepad());
        assert!(!entry(0x046d, "kbd leds event3").could_be_gamepad());
        assert!(entry(0x046d, "mousefoo event4").could_be_gamepad());
    }

    #[test]
    fn input_device_lines_cap_the_listed_devices() {
        let entries: Vec<InputDeviceEntry> = (0..MAX_SHOWN_INPUT_DEVICES + 10)
            .map(|index| InputDeviceEntry {
                vendor: 0x045e,
                product: 0x028e,
                handlers: format!("js{index} event{index}"),
            })
            .collect();
        let lines = input_device_lines(&entries);
        assert_eq!(lines.len(), MAX_SHOWN_INPUT_DEVICES + 1);
        assert!(lines[0].ends_with("(64 shown)"));
    }

    #[test]
    fn proc_input_devices_cap_the_entry_count() {
        let stanza = "I: Bus=0003 Vendor=0001 Product=0002 Version=0\nH: Handlers=event0\n\n";
        let entries = parse_proc_input_devices(&stanza.repeat(MAX_INPUT_DEVICE_ENTRIES + 10));
        assert_eq!(entries.len(), MAX_INPUT_DEVICE_ENTRIES);
    }

    #[test]
    fn flatpak_devices_come_from_the_context_section() {
        let info = "[Application]\nname=io.example.App\n\n[Context]\nshared=network;ipc;\n\
                    devices=dri;input;\nsockets=x11;\n\n[Instance]\nflatpak-version=1.16.6\n";
        assert_eq!(parse_flatpak_devices(info).as_deref(), Some("dri;input;"));
        assert_eq!(parse_flatpak_version(info).as_deref(), Some("1.16.6"));
    }

    #[test]
    fn flatpak_devices_absent_without_a_context_devices_line() {
        let info = "[Context]\nshared=network;\n\n[Other]\ndevices=all;\n";
        assert_eq!(parse_flatpak_devices(info), None);
        assert_eq!(parse_flatpak_devices(""), None);
        assert_eq!(parse_flatpak_version(info), None);
    }

    #[test]
    fn controller_environment_reports_allowlisted_names_only() {
        let env: std::collections::HashMap<&str, String> = [
            (
                "SteamVirtualGamepadInfo",
                "/run/user/1000/secret/path".to_string(),
            ),
            ("SteamAppId", "123456".to_string()),
            ("SDL_GAMECONTROLLERCONFIG", "deadbeef,Pad,a:b0".to_string()),
            ("XDG_CURRENT_DESKTOP", "KDE".to_string()),
            (
                "SDL_GAMECONTROLLER_ALLOW_STEAM_VIRTUAL_GAMEPAD",
                "1".to_string(),
            ),
            ("SDL_GAMECONTROLLER_IGNORE_DEVICES", "x".repeat(500)),
            ("SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT", String::new()),
            ("WINEPREFIX", "/home/user/prefix".to_string()),
            ("BAHAMUT_SESSION_TOKEN", "token-value".to_string()),
        ]
        .into_iter()
        .collect();
        let lines = controller_environment_lines(|name| env.get(name).cloned());
        let text = lines.join("\n");

        assert_eq!(lines.len(), 9);
        assert!(text.contains("SteamVirtualGamepadInfo = (set)"));
        assert!(text.contains("SteamAppId = (set)"));
        assert!(text.contains("SteamGameId = (unset)"));
        assert!(text.contains("STEAM_COMPAT_APP_ID = (unset)"));
        assert!(text.contains("SDL_GAMECONTROLLERCONFIG = (set)"));
        assert!(text.contains("XDG_CURRENT_DESKTOP = KDE"));
        assert!(text.contains("SDL_GAMECONTROLLER_ALLOW_STEAM_VIRTUAL_GAMEPAD = 1"));
        assert!(text.contains("SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT = (empty)"));
        for hidden in [
            "secret/path",
            "123456",
            "deadbeef",
            "WINEPREFIX",
            "token-value",
        ] {
            assert!(!text.contains(hidden), "{hidden} leaked: {text}");
        }

        let ignore = lines
            .iter()
            .find(|line| line.starts_with("SDL_GAMECONTROLLER_IGNORE_DEVICES = "))
            .unwrap();
        assert!(ignore.contains(&"x".repeat(MAX_ENV_VALUE_CHARS)));
        assert!(!ignore.contains(&"x".repeat(MAX_ENV_VALUE_CHARS + 1)));
        assert!(ignore.ends_with("...(truncated)"));
    }

    #[test]
    fn controller_environment_marks_every_name_unset_when_empty() {
        let lines = controller_environment_lines(|_| None);
        assert_eq!(lines.len(), 9);
        assert!(lines.iter().all(|line| line.ends_with(" = (unset)")));
    }

    #[test]
    fn controller_environment_values_cannot_inject_lines() {
        let lines = controller_environment_lines(|name| {
            (name == "XDG_CURRENT_DESKTOP").then(|| "KDE\nFake = line".to_string())
        });
        assert!(lines.iter().all(|line| !line.contains('\n')));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn controller_diagnostics_are_bounded_and_describe_the_sandbox() {
        let lines = controller_diagnostics();
        assert!(!lines.is_empty() && lines.len() <= 200);
        assert!(lines.iter().any(|line| line.starts_with("Flatpak")));
        assert!(lines.iter().any(|line| line.starts_with("libSDL2")));
        assert!(
            lines
                .iter()
                .all(|line| !line.contains("Name=") && !line.contains("Phys"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("SDL_GAMECONTROLLER_IGNORE_DEVICES = "))
        );
    }
}
