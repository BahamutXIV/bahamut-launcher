#pragma once

#include <windows.h>

#include <cstddef>
#include <cstdint>

#include "runtime_contract.h"

// This process-boundary declaration is not an addon, rendering, hook, memory,
// or plugin interface. Bootstrap diagnostics follow the fixed launcher-facing
// telemetry prefix.
inline constexpr std::uint32_t kBahamutRuntimeApiVersion =
    bahamut_runtime_contract::kRuntimeApiVersion;

struct BahamutRuntimeTelemetry
{
    volatile LONG frameCount;
    volatile LONG resetCount;
    volatile LONG inputCaptureCount;
    volatile LONG inputForwardCount;
    volatile LONG overlayToggleCount;
    volatile LONG controllerCaptureCount;
    volatile LONG controllerSwitchCount;
    volatile LONG overlayFrameCount;
    volatile LONG overlayFailureCount;
    LARGE_INTEGER firstFrameCounter;
    LARGE_INTEGER lastFrameCounter;
    LARGE_INTEGER overlayTotalCounter;
    LARGE_INTEGER overlayMaxCounter;
    volatile LONG timingSequence;
};

// The 64-bit launcher reads this block out of the mapping by offset, so the
// x86 layout is pinned by the compiler here rather than inferred by a reader.
static_assert(sizeof(BahamutRuntimeTelemetry) == 80,
              "BahamutRuntimeTelemetry size is part of the launcher-facing contract");
static_assert(offsetof(BahamutRuntimeTelemetry, firstFrameCounter) == 40,
              "BahamutRuntimeTelemetry timing offset is part of the launcher-facing contract");

struct BahamutRuntimeBootstrapDiagnostics
{
    wchar_t       renderBoundaryOwnerPath[MAX_PATH];
    volatile LONG startupScriptLine;
    wchar_t       startupScriptMessage[256];
    wchar_t       nativePluginId[64];
    wchar_t       nativePluginMessage[256];
};

struct BahamutRuntimeSharedState
{
    BahamutRuntimeTelemetry            telemetry;
    BahamutRuntimeBootstrapDiagnostics bootstrap;
};

static_assert(sizeof(BahamutRuntimeBootstrapDiagnostics) == 1676,
              "bootstrap diagnostics are shared by the x86 helper and runtime");
static_assert(offsetof(BahamutRuntimeSharedState, telemetry) == 0,
              "launcher telemetry must remain the mapping prefix");
static_assert(offsetof(BahamutRuntimeSharedState, bootstrap) == 80,
              "bootstrap diagnostics must follow the launcher telemetry prefix");
static_assert(sizeof(BahamutRuntimeSharedState) == 1760,
              "shared state size is part of the helper/runtime contract");

struct BahamutRuntimeTimingSnapshot
{
    LONG          frameCount;
    LONG          overlayFrameCount;
    LARGE_INTEGER firstFrameCounter;
    LARGE_INTEGER lastFrameCounter;
    LARGE_INTEGER overlayTotalCounter;
    LARGE_INTEGER overlayMaxCounter;
};

inline void BahamutBeginTimingWrite(BahamutRuntimeTelemetry* telemetry)
{
    for (;;)
    {
        const LONG sequence = InterlockedCompareExchange(
            &telemetry->timingSequence, 0, 0);
        const LONG nextSequence = static_cast<LONG>(
            static_cast<ULONG>(sequence) + 1UL);
        if ((sequence & 1) == 0 && InterlockedCompareExchange(&telemetry->timingSequence,
                                                              nextSequence,
                                                              sequence) == sequence)
        {
            MemoryBarrier();
            return;
        }
        SwitchToThread();
    }
}

inline void BahamutEndTimingWrite(BahamutRuntimeTelemetry* telemetry)
{
    MemoryBarrier();
    InterlockedIncrement(&telemetry->timingSequence);
}

inline bool BahamutReadTimingSnapshot(const BahamutRuntimeTelemetry& telemetry,
                                      BahamutRuntimeTimingSnapshot&  snapshot)
{
    auto* sequenceAddress = const_cast<volatile LONG*>(&telemetry.timingSequence);
    for (int attempt = 0; attempt != 128; ++attempt)
    {
        const LONG before = InterlockedCompareExchange(sequenceAddress, 0, 0);
        if ((before & 1) != 0)
        {
            SwitchToThread();
            continue;
        }
        MemoryBarrier();
        snapshot.frameCount          = telemetry.frameCount;
        snapshot.overlayFrameCount   = telemetry.overlayFrameCount;
        snapshot.firstFrameCounter   = telemetry.firstFrameCounter;
        snapshot.lastFrameCounter    = telemetry.lastFrameCounter;
        snapshot.overlayTotalCounter = telemetry.overlayTotalCounter;
        snapshot.overlayMaxCounter   = telemetry.overlayMaxCounter;
        MemoryBarrier();
        const LONG after = InterlockedCompareExchange(sequenceAddress, 0, 0);
        if (before == after && (after & 1) == 0)
        {
            return true;
        }
    }
    return false;
}

extern "C" __declspec(dllexport) DWORD WINAPI BahamutRuntimeApiVersion(LPVOID parameter);
extern "C" __declspec(dllexport) DWORD WINAPI BahamutRuntimeInitialize(LPVOID parameter);
