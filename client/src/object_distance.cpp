#include "object_distance.h"
#include "fault_guard.h"

#include "runtime_contract.h"

#include <MinHook.h>
#include <windows.h>

#include <array>
#include <cmath>
#include <cstdint>
#include <cstring>

namespace
{

constexpr std::uintptr_t               kObjectFarRva       = 0x002247B0u;
constexpr std::array<std::uint8_t, 31> kObjectFarSignature = {
    0x8B, 0x44, 0x24, 0x08, 0x85, 0xC0, 0x74, 0x61, 0x8B, 0x89, 0x14, 0x01, 0x00, 0x00, 0x81, 0xB8, 0x50, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x8B, 0x01, 0x8B, 0x50, 0x18, 0x75, 0x0B
};

using ObjectFarFunction               = float(__thiscall*)(void*, float, void*);
ObjectFarFunction gOriginalObjectFar  = nullptr;
float             gDistanceMultiplier = 2.0F;

bool ReadMultiplier(float* multiplier)
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(L"BAHAMUT_RUNTIME_OBJECT_DISTANCE_PERCENT", value, ARRAYSIZE(value));
    if (count == 0 || count >= ARRAYSIZE(value))
    {
        return false;
    }
    unsigned percent = 0;
    for (DWORD index = 0; index < count; ++index)
    {
        if (value[index] < L'0' || value[index] > L'9')
        {
            return false;
        }
        percent = percent * 10 + static_cast<unsigned>(value[index] - L'0');
    }
    if (percent < 125 || percent > 200 || percent % 25 != 0)
    {
        return false;
    }
    *multiplier = static_cast<float>(percent) / 100.0F;
    return true;
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
        return std::memcmp(address, kObjectFarSignature.data(), kObjectFarSignature.size()) == 0;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

float __fastcall HookedObjectFar(void* manager, void*, float distance, void* actor)
{
    const float original = gOriginalObjectFar(manager, distance, actor);
    return bahamut_client::ScaleObjectDistance(original, gDistanceMultiplier);
}

} // namespace

namespace bahamut_client
{

float ScaleObjectDistance(float original, float multiplier)
{
    if (!std::isfinite(original) || original < 0.0F)
    {
        return original;
    }
    const float scaled = original * multiplier;
    return std::isfinite(scaled) ? scaled : original;
}

bool ObjectDistanceService::InstallHook()
{
    if (hookTarget_ != nullptr || HasValue(L"BAHAMUT_RUNTIME_TEST_STUB"))
    {
        return true;
    }
    if (!ReadMultiplier(&gDistanceMultiplier))
    {
        return false;
    }
    HMODULE image = GetModuleHandleW(nullptr);
    if (image == nullptr || reinterpret_cast<std::uintptr_t>(image) != bahamut_runtime_contract::kRetailImageBase)
    {
        return false;
    }
    void* target = reinterpret_cast<void*>(bahamut_runtime_contract::kRetailImageBase + kObjectFarRva);
    if (!HasSignature(target))
    {
        return false;
    }
    void* original = nullptr;
    if (MH_CreateHook(target, reinterpret_cast<void*>(&HookedObjectFar), &original) != MH_OK)
    {
        return false;
    }
    if (original == nullptr)
    {
        static_cast<void>(MH_RemoveHook(target));
        return false;
    }
    gOriginalObjectFar = reinterpret_cast<ObjectFarFunction>(original);
    hookTarget_        = target;
    if (MH_EnableHook(target) != MH_OK)
    {
        static_cast<void>(Shutdown());
        return false;
    }
    return true;
}

bool ObjectDistanceService::Shutdown()
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
    gOriginalObjectFar = nullptr;
    hookTarget_        = nullptr;
    return true;
}

bool ObjectDistanceService::IsInstalled() const
{
    return hookTarget_ != nullptr;
}

} // namespace bahamut_client
