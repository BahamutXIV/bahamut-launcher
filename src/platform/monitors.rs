//! Stable monitor identities and physical display geometry for borderless mode.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorderlessMonitor {
    /// Windows monitor device-interface path returned by `EnumDisplayDevicesW`.
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorderlessMonitorSnapshot {
    pub supported: bool,
    pub monitors: Vec<BorderlessMonitor>,
}

/// Resolve an explicit monitor identity, falling back to primary and then the first active monitor.
/// A missing selection returns `None` so the caller can retain its existing nearest-window policy.
pub fn selected_monitor_resolution(
    monitors: &[BorderlessMonitor],
    selected: Option<&str>,
) -> Option<(u32, u32)> {
    let selected = selected?;
    monitors
        .iter()
        .find(|monitor| monitor.id.eq_ignore_ascii_case(selected))
        .or_else(|| monitors.iter().find(|monitor| monitor.primary))
        .or_else(|| monitors.first())
        .map(|monitor| (monitor.width, monitor.height))
}

pub fn enumerate_borderless_monitors() -> Result<BorderlessMonitorSnapshot, String> {
    #[cfg(target_os = "windows")]
    {
        windows::enumerate()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(BorderlessMonitorSnapshot {
            supported: false,
            monitors: Vec::new(),
        })
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::mem::size_of;

    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
        MONITORINFO, MONITORINFOEXW,
    };
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME;
    use windows::core::{BOOL, PCSTR, PCWSTR, w};

    use super::{BorderlessMonitor, BorderlessMonitorSnapshot};

    type SetThreadDpiAwarenessContextFn =
        unsafe extern "system" fn(DPI_AWARENESS_CONTEXT) -> DPI_AWARENESS_CONTEXT;

    struct PhysicalCoordinateContext(Option<DPI_AWARENESS_CONTEXT>);

    impl PhysicalCoordinateContext {
        fn enter() -> Self {
            unsafe {
                let module = GetModuleHandleW(w!("user32.dll"));
                let Ok(module) = module else {
                    return Self(None);
                };
                let address = GetProcAddress(
                    module,
                    PCSTR(c"SetThreadDpiAwarenessContext".as_ptr().cast()),
                );
                let Some(address) = address else {
                    return Self(None);
                };
                let set_context: SetThreadDpiAwarenessContextFn = std::mem::transmute(address);
                let previous = set_context(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
                (!previous.0.is_null())
                    .then_some(previous)
                    .map_or(Self(None), |previous| Self(Some(previous)))
            }
        }
    }

    impl Drop for PhysicalCoordinateContext {
        fn drop(&mut self) {
            let Some(previous) = self.0 else {
                return;
            };
            unsafe {
                let Ok(module) = GetModuleHandleW(w!("user32.dll")) else {
                    return;
                };
                let Some(address) = GetProcAddress(
                    module,
                    PCSTR(c"SetThreadDpiAwarenessContext".as_ptr().cast()),
                ) else {
                    return;
                };
                let set_context: SetThreadDpiAwarenessContextFn = std::mem::transmute(address);
                let _ = set_context(previous);
            }
        }
    }

    struct EnumState {
        monitors: Vec<BorderlessMonitor>,
        failed: bool,
    }

    pub(super) fn enumerate() -> Result<BorderlessMonitorSnapshot, String> {
        let _physical_coordinates = PhysicalCoordinateContext::enter();
        let mut state = EnumState {
            monitors: Vec::new(),
            failed: false,
        };
        let result = unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(enumerate_monitor),
                LPARAM((&mut state as *mut EnumState) as isize),
            )
        };
        if !result.as_bool() || state.failed {
            return Err("Windows could not enumerate active display monitors.".into());
        }
        Ok(BorderlessMonitorSnapshot {
            supported: true,
            monitors: state.monitors,
        })
    }

    unsafe extern "system" fn enumerate_monitor(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let state = unsafe { &mut *(data.0 as *mut EnumState) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        let info_ptr = (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>();
        if !unsafe { GetMonitorInfoW(monitor, info_ptr) }.as_bool() {
            state.failed = true;
            return BOOL(0);
        }

        let id = monitor_device_interface_id(&info.szDevice).unwrap_or_default();
        let bounds = info.monitorInfo.rcMonitor;
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        if width <= 0 || height <= 0 {
            return BOOL(1);
        }
        let name = monitor_display_name(&info.szDevice).unwrap_or_else(|| utf16(&info.szDevice));
        state.monitors.push(BorderlessMonitor {
            id,
            name,
            width: width as u32,
            height: height as u32,
            primary: info.monitorInfo.dwFlags & 1 != 0,
        });
        BOOL(1)
    }

    fn monitor_device_interface_id(display_name: &[u16; 32]) -> Option<String> {
        let display_name = utf16(display_name);
        if display_name.is_empty() {
            return None;
        }
        let mut adapter_index = 0;
        loop {
            let mut adapter = DISPLAY_DEVICEW {
                cb: size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            let found = unsafe {
                EnumDisplayDevicesW(
                    PCWSTR::null(),
                    adapter_index,
                    &mut adapter,
                    EDD_GET_DEVICE_INTERFACE_NAME,
                )
            };
            if !found.as_bool() {
                break;
            }
            adapter_index += 1;
            if utf16(&adapter.DeviceName).eq_ignore_ascii_case(&display_name) {
                let mut child_index = 0;
                loop {
                    let mut device = DISPLAY_DEVICEW {
                        cb: size_of::<DISPLAY_DEVICEW>() as u32,
                        ..Default::default()
                    };
                    let found = unsafe {
                        EnumDisplayDevicesW(
                            PCWSTR(adapter.DeviceName.as_ptr()),
                            child_index,
                            &mut device,
                            EDD_GET_DEVICE_INTERFACE_NAME,
                        )
                    };
                    if !found.as_bool() {
                        break;
                    }
                    child_index += 1;
                    let id = utf16(&device.DeviceID);
                    if !id.is_empty() {
                        return Some(id);
                    }
                }
            }
        }
        None
    }

    fn monitor_display_name(display_name: &[u16; 32]) -> Option<String> {
        let mut adapter_index = 0;
        loop {
            let mut adapter = DISPLAY_DEVICEW {
                cb: size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            let found = unsafe {
                EnumDisplayDevicesW(
                    PCWSTR::null(),
                    adapter_index,
                    &mut adapter,
                    EDD_GET_DEVICE_INTERFACE_NAME,
                )
            };
            if !found.as_bool() {
                break;
            }
            adapter_index += 1;
            if adapter.DeviceName == *display_name {
                let mut device = DISPLAY_DEVICEW {
                    cb: size_of::<DISPLAY_DEVICEW>() as u32,
                    ..Default::default()
                };
                if unsafe {
                    EnumDisplayDevicesW(
                        PCWSTR(adapter.DeviceName.as_ptr()),
                        0,
                        &mut device,
                        EDD_GET_DEVICE_INTERFACE_NAME,
                    )
                }
                .as_bool()
                {
                    let name = utf16(&device.DeviceString);
                    if !name.is_empty() {
                        return Some(name);
                    }
                }
            }
        }
        None
    }

    fn utf16(value: &[u16]) -> String {
        let end = value
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(value.len());
        String::from_utf16_lossy(&value[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(id: &str, width: u32, height: u32, primary: bool) -> BorderlessMonitor {
        BorderlessMonitor {
            id: id.into(),
            name: id.into(),
            width,
            height,
            primary,
        }
    }

    #[test]
    fn explicit_identity_uses_its_geometry_and_missing_identity_falls_back_to_primary() {
        let monitors = [
            monitor("\\\\?\\DISPLAY#SECOND", 2560, 1440, false),
            monitor("\\\\?\\DISPLAY#PRIMARY", 1920, 1080, true),
        ];

        assert_eq!(
            selected_monitor_resolution(&monitors, Some("\\\\?\\display#second")),
            Some((2560, 1440))
        );
        assert_eq!(
            selected_monitor_resolution(&monitors, Some("disconnected")),
            Some((1920, 1080))
        );
    }

    #[test]
    fn idless_primary_and_missing_primary_keep_their_fallback_geometry() {
        let monitors = [
            monitor("", 1600, 900, true),
            monitor("secondary", 1280, 720, false),
        ];
        assert_eq!(
            selected_monitor_resolution(&monitors, Some("disconnected")),
            Some((1600, 900))
        );
        assert_eq!(
            selected_monitor_resolution(&monitors[1..], Some("disconnected")),
            Some((1280, 720))
        );
        assert_eq!(selected_monitor_resolution(&monitors, None), None);
    }
}
