#include "monitor_selection.h"

#include <algorithm>
#include <array>
#include <cstdint>
#include <cwctype>
#include <iterator>

namespace
{

constexpr size_t  kMaximumMonitorIdCharacters     = 1024;
constexpr wchar_t kBorderlessMonitorEnvironment[] = L"BAHAMUT_RUNTIME_BORDERLESS_MONITOR";

using SetThreadDpiAwarenessContextFn = HANDLE(WINAPI*)(HANDLE);

SetThreadDpiAwarenessContextFn SetDpiAwarenessContextFunction()
{
    HMODULE user32 = GetModuleHandleW(L"user32.dll");
    return user32 == nullptr
               ? nullptr
               : reinterpret_cast<SetThreadDpiAwarenessContextFn>(
                     GetProcAddress(user32, "SetThreadDpiAwarenessContext"));
}

std::wstring NullTerminatedString(const wchar_t* value, size_t capacity)
{
    const wchar_t* end = std::find(value, value + capacity, L'\0');
    return std::wstring(value, end);
}

std::optional<std::wstring> DeviceInterfaceId(const wchar_t* displayName)
{
    for (DWORD adapterIndex = 0;; ++adapterIndex)
    {
        DISPLAY_DEVICEW adapter{};
        adapter.cb = sizeof(adapter);
        if (!EnumDisplayDevicesW(nullptr, adapterIndex, &adapter, EDD_GET_DEVICE_INTERFACE_NAME))
        {
            break;
        }
        if (CompareStringOrdinal(adapter.DeviceName, -1, displayName, -1, TRUE) != CSTR_EQUAL)
        {
            continue;
        }

        for (DWORD monitorIndex = 0;; ++monitorIndex)
        {
            DISPLAY_DEVICEW monitor{};
            monitor.cb = sizeof(monitor);
            if (!EnumDisplayDevicesW(adapter.DeviceName,
                                     monitorIndex,
                                     &monitor,
                                     EDD_GET_DEVICE_INTERFACE_NAME))
            {
                break;
            }
            if (monitor.DeviceID[0] != L'\0')
            {
                return NullTerminatedString(monitor.DeviceID, std::size(monitor.DeviceID));
            }
        }
    }
    return std::nullopt;
}

struct EnumerationState
{
    std::vector<BorderlessMonitorGeometry> monitors;
};

BOOL CALLBACK EnumerateMonitor(HMONITOR monitor, HDC, LPRECT, LPARAM data)
{
    auto&          state = *reinterpret_cast<EnumerationState*>(data);
    MONITORINFOEXW info{};
    info.cbSize = sizeof(info);
    if (!GetMonitorInfoW(monitor, reinterpret_cast<MONITORINFO*>(&info)))
    {
        return TRUE;
    }

    state.monitors.push_back({
        DeviceInterfaceId(info.szDevice).value_or(L""),
        info.rcMonitor,
        (info.dwFlags & MONITORINFOF_PRIMARY) != 0,
    });
    return TRUE;
}

bool SameDeviceInterfaceId(std::wstring_view left, std::wstring_view right)
{
    return CompareStringOrdinal(left.data(), static_cast<int>(left.size()), right.data(), static_cast<int>(right.size()), TRUE) == CSTR_EQUAL;
}

} // namespace

PhysicalCoordinateContext::PhysicalCoordinateContext()
{
    if (const auto setContext = SetDpiAwarenessContextFunction(); setContext != nullptr)
    {
        previous_ = setContext(reinterpret_cast<HANDLE>(static_cast<intptr_t>(-4)));
    }
}

PhysicalCoordinateContext::~PhysicalCoordinateContext()
{
    if (previous_ != nullptr)
    {
        if (const auto setContext = SetDpiAwarenessContextFunction(); setContext != nullptr)
        {
            setContext(previous_);
        }
    }
}

std::vector<BorderlessMonitorGeometry> EnumerateBorderlessMonitors()
{
    PhysicalCoordinateContext physicalCoordinates;
    EnumerationState          state;
    if (!EnumDisplayMonitors(nullptr, nullptr, EnumerateMonitor, reinterpret_cast<LPARAM>(&state)))
    {
        return {};
    }
    return state.monitors;
}

const BorderlessMonitorGeometry* SelectBorderlessMonitor(
    const std::vector<BorderlessMonitorGeometry>& monitors,
    std::wstring_view                             requestedId)
{
    const auto selected = std::find_if(monitors.begin(), monitors.end(), [&](const auto& monitor)
                                       {
                                           return !monitor.id.empty() && SameDeviceInterfaceId(monitor.id, requestedId);
                                       });
    if (selected != monitors.end())
    {
        return &*selected;
    }
    const auto primary = std::find_if(monitors.begin(), monitors.end(), [](const auto& monitor)
                                      {
                                          return monitor.primary;
                                      });
    return primary != monitors.end() ? &*primary : (monitors.empty() ? nullptr : &monitors.front());
}

std::optional<std::wstring> ReadBorderlessMonitorId()
{
    std::array<wchar_t, kMaximumMonitorIdCharacters + 1> value{};
    const DWORD                                          count = GetEnvironmentVariableW(kBorderlessMonitorEnvironment,
                                                                                         value.data(),
                                                                                         static_cast<DWORD>(value.size()));
    if (count == 0 || count >= static_cast<DWORD>(value.size()))
    {
        return std::nullopt;
    }
    for (DWORD index = 0; index < count; ++index)
    {
        if (std::iswcntrl(value[index]))
        {
            return std::nullopt;
        }
    }
    return std::wstring(value.data(), count);
}
