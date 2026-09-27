#pragma once

#include <cstdint>

namespace bahamut_client
{

using CameraDistanceWriter = void (*)(void*, std::uint32_t, float);

void ApplyCameraZoom(void* camera, std::uint32_t index, float distance, float limit, CameraDistanceWriter original);

class CameraZoomService
{
public:
    bool               InstallHook();
    bool               Shutdown();
    [[nodiscard]] bool IsInstalled() const;

private:
    void* hookTarget_ = nullptr;
};

} // namespace bahamut_client
