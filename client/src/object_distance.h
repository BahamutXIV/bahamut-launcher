#pragma once

namespace bahamut_client
{

float ScaleObjectDistance(float original, float multiplier);

class ObjectDistanceService
{
public:
    bool               InstallHook();
    bool               Shutdown();
    [[nodiscard]] bool IsInstalled() const;

private:
    void* hookTarget_ = nullptr;
};

} // namespace bahamut_client
