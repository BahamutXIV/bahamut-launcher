#pragma once

#include <windows.h>

#if defined(_MSC_VER)
#include <intrin.h>
#endif

#include <array>

namespace bahamut_client
{

// Preserve x87/SSE state and LastError across hook bookkeeping so the hooked
// function's caller sees the environment it left.
struct alignas(16) NativeState
{
    std::array<unsigned char, 512> fp;
    DWORD                          lastError;
    // Keep the complete object a multiple of FXSAVE's required alignment.
    std::array<unsigned char, 12> reserved;
};

inline void SaveNativeState(NativeState& state) noexcept
{
#if defined(_MSC_VER)
    _fxsave(state.fp.data());
#else
    __asm__ volatile("fxsave %0" : "=m"(state.fp) : : "memory");
#endif
    state.lastError = GetLastError();
}

inline void RestoreNativeState(const NativeState& state) noexcept
{
    SetLastError(state.lastError);
#if defined(_MSC_VER)
    _fxrstor(const_cast<unsigned char*>(state.fp.data()));
#else
    __asm__ volatile("fxrstor %0" : : "m"(state.fp) : "memory");
#endif
}

} // namespace bahamut_client
