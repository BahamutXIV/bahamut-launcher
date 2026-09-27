#include "camera_zoom.h"
#include "fault_guard.h"

#include "runtime_contract.h"

#include <MinHook.h>
#include <windows.h>

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <cstring>

namespace
{

// ffxivgame.exe SHA-256 9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9:
// FUN_00611350 clamps its second argument, then stores it at camera + 0x3F4 + index * 4.
constexpr std::uintptr_t               kCameraSetterRva = 0x00211350u;
constexpr std::array<std::uint8_t, 18> kSetterSignature = {
    0xF3, 0x0F, 0x10, 0x05, 0xC0, 0x70, 0xFB, 0x00, 0x0F, 0x2F, 0x44, 0x24, 0x08, 0xB8, 0xC0, 0x70, 0xFB, 0x00
};

using CameraDistanceSetter                 = void(__thiscall*)(void*, std::uint32_t, float);
CameraDistanceSetter gOriginalCameraSetter = nullptr;
float                gCameraZoomLimit      = 15.0F;

bool ReadZoomLimit(float* limit)
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(L"BAHAMUT_RUNTIME_CAMERA_ZOOM_LIMIT", value, ARRAYSIZE(value));
    if (count == 0 || count >= ARRAYSIZE(value))
    {
        return false;
    }
    unsigned parsed = 0;
    for (DWORD index = 0; index < count; ++index)
    {
        if (value[index] < L'0' || value[index] > L'9')
        {
            return false;
        }
        parsed = parsed * 10 + static_cast<unsigned>(value[index] - L'0');
    }
    if (parsed < 11 || parsed > 15)
    {
        return false;
    }
    *limit = static_cast<float>(parsed);
    return true;
}

void CallOriginalCameraSetter(void* camera, std::uint32_t index, float distance)
{
    gOriginalCameraSetter(camera, index, distance);
}

bool HasValue(const wchar_t* name)
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(name, value, ARRAYSIZE(value));
    return count != 0 && count < ARRAYSIZE(value) && value[0] != L'0';
}

bool HasSignature(const void* address)
{
    BAHAMUT_FAULT_TRY
    {
        return std::memcmp(address, kSetterSignature.data(), kSetterSignature.size()) == 0;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

void __fastcall HookedCameraSetter(void* camera, void*, std::uint32_t index, float distance)
{
    bahamut_client::ApplyCameraZoom(camera, index, distance, gCameraZoomLimit, &CallOriginalCameraSetter);
}

} // namespace

namespace bahamut_client
{

void ApplyCameraZoom(void* camera, std::uint32_t index, float distance, float limit, CameraDistanceWriter original)
{
    if (!std::isfinite(distance) || distance <= 10.0F)
    {
        original(camera, index, distance);
        return;
    }
    const auto address                 = reinterpret_cast<std::uintptr_t>(camera) + 0x3F4u + index * 4u;
    *reinterpret_cast<float*>(address) = std::min(distance, limit);
}

bool CameraZoomService::InstallHook()
{
    if (hookTarget_ != nullptr || HasValue(L"BAHAMUT_RUNTIME_TEST_STUB"))
    {
        return true;
    }
    if (!ReadZoomLimit(&gCameraZoomLimit))
    {
        return false;
    }
    HMODULE image = GetModuleHandleW(nullptr);
    if (image == nullptr || reinterpret_cast<std::uintptr_t>(image) != bahamut_runtime_contract::kRetailImageBase)
    {
        return false;
    }
    void* target = reinterpret_cast<void*>(bahamut_runtime_contract::kRetailImageBase + kCameraSetterRva);
    if (!HasSignature(target))
    {
        return false;
    }
    void* original = nullptr;
    if (MH_CreateHook(target, reinterpret_cast<void*>(&HookedCameraSetter), &original) != MH_OK)
    {
        return false;
    }
    if (original == nullptr)
    {
        static_cast<void>(MH_RemoveHook(target));
        return false;
    }
    gOriginalCameraSetter = reinterpret_cast<CameraDistanceSetter>(original);
    hookTarget_           = target;
    if (MH_EnableHook(target) != MH_OK)
    {
        static_cast<void>(Shutdown());
        return false;
    }
    return true;
}

bool CameraZoomService::Shutdown()
{
    if (hookTarget_ == nullptr)
    {
        return true;
    }
    const MH_STATUS disabled = MH_DisableHook(hookTarget_);
    if (disabled != MH_OK && disabled != MH_ERROR_DISABLED)
    {
        return false;
    }
    const MH_STATUS removed = MH_RemoveHook(hookTarget_);
    if (removed != MH_OK && removed != MH_ERROR_NOT_CREATED)
    {
        return false;
    }
    gOriginalCameraSetter = nullptr;
    hookTarget_           = nullptr;
    return true;
}

bool CameraZoomService::IsInstalled() const
{
    return hookTarget_ != nullptr;
}

} // namespace bahamut_client
