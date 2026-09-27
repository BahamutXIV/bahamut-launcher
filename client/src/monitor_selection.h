#pragma once

#include <optional>
#include <string>
#include <string_view>
#include <vector>

#include <windows.h>

struct BorderlessMonitorGeometry
{
    std::wstring id;
    RECT         bounds{};
    bool         primary{};
};

class PhysicalCoordinateContext
{
public:
    PhysicalCoordinateContext();
    ~PhysicalCoordinateContext();

    PhysicalCoordinateContext(const PhysicalCoordinateContext&)            = delete;
    PhysicalCoordinateContext& operator=(const PhysicalCoordinateContext&) = delete;

private:
    HANDLE previous_{};
};

std::vector<BorderlessMonitorGeometry> EnumerateBorderlessMonitors();
const BorderlessMonitorGeometry*       SelectBorderlessMonitor(
    const std::vector<BorderlessMonitorGeometry>& monitors,
    std::wstring_view                             requestedId);
std::optional<std::wstring> ReadBorderlessMonitorId();
