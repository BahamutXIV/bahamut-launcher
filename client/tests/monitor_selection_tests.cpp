#include "monitor_selection.h"

#include <iostream>
#include <vector>

namespace
{

bool Expect(bool condition, const char* message)
{
    if (!condition)
    {
        std::cerr << message << "\n";
    }
    return condition;
}

} // namespace

int main()
{
    const std::vector<BorderlessMonitorGeometry> monitors = {
        { L"\\\\?\\DISPLAY#SECOND", { -1920, -120, 0, 960 }, false },
        { L"\\\\?\\DISPLAY#PRIMARY", { 0, 0, 1920, 1080 }, true },
    };
    bool success = true;

    const auto* selected = SelectBorderlessMonitor(monitors, L"\\\\?\\display#second");
    success &= Expect(selected != nullptr && selected->bounds.left == -1920 &&
                          selected->bounds.top == -120 && selected->bounds.right == 0 &&
                          selected->bounds.bottom == 960,
                      "selected identity did not retain negative monitor coordinates");

    const auto* missing = SelectBorderlessMonitor(monitors, L"disconnected");
    success &= Expect(missing != nullptr && missing->id == L"\\\\?\\DISPLAY#PRIMARY",
                      "missing identity did not fall back to the primary monitor");

    const std::vector<BorderlessMonitorGeometry> withoutPrimary = {
        { L"first", { 100, 0, 1700, 900 }, false },
    };
    const auto* fallback = SelectBorderlessMonitor(withoutPrimary, L"disconnected");
    success &= Expect(fallback == &withoutPrimary.front(),
                      "missing primary did not fall back to the first active monitor");
    return success ? 0 : 1;
}
