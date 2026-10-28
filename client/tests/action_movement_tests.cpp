#include "action_movement.h"
#include "action_movement_hook.h"

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <limits>
#include <thread>

#if defined(_MSC_VER) && defined(_M_IX86)
#include <MinHook.h>
#include <windows.h>
#include <xmmintrin.h>

#include <array>
#include <cstring>
#include <float.h>
#endif

namespace
{

bool CheckScaling()
{
    struct Case
    {
        std::int32_t  duration;
        std::uint32_t percent;
        std::int32_t  expected;
    };

    constexpr auto maximum = std::numeric_limits<std::int32_t>::max();
    constexpr Case cases[] = {
        { 125, 0, 0 }, { 125, 40, 50 }, { 125, 100, 125 }, { 1, 40, 0 }, { 2, 40, 0 }, { 3, 40, 1 }, { 101, 99, 99 }, { 100, 1, 1 }, { 0, 0, 0 }, { -1, 0, -1 }, { -37, 40, -37 }, { std::numeric_limits<std::int32_t>::min(), 40, std::numeric_limits<std::int32_t>::min() }, { maximum, 100, maximum }, { maximum, 40, 858993458 }, { maximum, 0, 0 }, { 125, 101, 125 }, { 125, std::numeric_limits<std::uint32_t>::max(), 125 }
    };
    for (const auto& item : cases)
    {
        if (bahamut_client::ScaleActionMovementDuration(item.duration, item.percent) != item.expected)
        {
            std::fprintf(stderr, "Scaling failed: duration=%d percent=%u\n", item.duration, item.percent);
            return false;
        }
    }
    return true;
}

bool CheckOwnership()
{
    using State = bahamut_client::ActionMovementBoundaryState;
    bahamut_client::ActionMovementService service;
    if (service.Status().boundary != State::Unavailable || service.Status().enabled ||
        service.BeginSession() != 0 || service.BindConsumer(1) != 0 || service.Configure(1, 1, 0, true))
    {
        return false;
    }
    service.SetBoundaryState(State::Ready);
    const auto session = service.BeginSession();
    const auto owner   = service.BindConsumer(session);
    if (session == 0 || owner == 0 || service.Status().enabled ||
        !service.Configure(owner, session, 40, true) || service.Status().effectivePercent != 40)
    {
        return false;
    }
    if (service.Configure(owner, session, 101, false) || service.Configure(0, session, 0, true) ||
        service.Configure(owner, 0, 0, true) || service.Status().effectivePercent != 40)
    {
        return false;
    }
    if (!service.Configure(owner, session, 40, false) || service.Status().enabled ||
        service.Status().requestedPercent != 40 || service.Status().effectivePercent != 100 ||
        !service.Configure(owner, session, 0, true) || service.Status().effectivePercent != 0)
    {
        return false;
    }
    const auto replacement = service.BindConsumer(session);
    if (replacement == 0 || replacement == owner || service.Status().enabled ||
        service.Configure(owner, session, 0, true) || service.RevokeConsumer(owner) ||
        !service.Configure(replacement, session, 40, true) || !service.RevokeConsumer(replacement) ||
        service.Status().enabled || service.Configure(replacement, session, 0, true))
    {
        return false;
    }
    const auto nextOwner = service.BindConsumer(session);
    if (!service.Configure(nextOwner, session, 40, true) || !service.EndSession(session))
    {
        return false;
    }
    const auto nextSession = service.BeginSession();
    const auto freshOwner  = service.BindConsumer(nextSession);
    if (nextSession == 0 || nextSession == session || freshOwner == 0 || service.Status().enabled ||
        service.Configure(nextOwner, session, 0, true) || service.EndSession(session) ||
        service.BindConsumer(session) != 0 || service.Configure(freshOwner, session, 0, true) ||
        !service.Configure(freshOwner, nextSession, 40, true))
    {
        return false;
    }
    for (const auto state : { State::TargetMismatch, State::InstallFailed, State::CleanupFailed, State::Unavailable })
    {
        service.SetBoundaryState(state);
        const auto unavailable = service.Status();
        if (unavailable.boundary != state || unavailable.enabled || unavailable.owner != 0 || unavailable.session != 0 ||
            service.Configure(freshOwner, nextSession, 0, true) || service.BeginSession() != 0)
        {
            return false;
        }
        service.Shutdown();
        if (service.Status().boundary != state)
        {
            return false;
        }
        service.SetBoundaryState(State::Ready);
        if (service.Status().enabled || service.Configure(freshOwner, nextSession, 0, true))
        {
            return false;
        }
    }
    const auto finalSession = service.BeginSession();
    const auto finalOwner   = service.BindConsumer(finalSession);
    if (!service.Configure(finalOwner, finalSession, 0, true))
    {
        return false;
    }
    service.Shutdown();
    return service.Status().boundary == State::Unavailable && !service.Status().enabled &&
           !service.Configure(finalOwner, finalSession, 0, true);
}

#if defined(_MSC_VER) && defined(_M_IX86)

struct Environment
{
    std::array<std::uint8_t, 28> x87{};
    unsigned int                 mxcsr     = 0;
    DWORD                        lastError = 0;
};

Environment CaptureEnvironment()
{
    Environment result;
    void*       destination = result.x87.data();
    __asm
        {
        mov eax, destination
        fnstenv [eax]
        fldenv [eax]
        }
    result.mxcsr     = _mm_getcsr();
    result.lastError = GetLastError();
    return result;
}

void RestoreEnvironment(const Environment& state)
{
    const void* source = state.x87.data();
    __asm
        {
        mov eax, source
        fldenv [eax]
        }
    _mm_setcsr(state.mxcsr);
    SetLastError(state.lastError);
}

bool SameEnvironment(const Environment& first, const Environment& second)
{
    // FPU instruction/data addresses change while capturing; control, status and tags must not.
    return std::memcmp(first.x87.data(), second.x87.data(), 2) == 0 &&
           std::memcmp(first.x87.data() + 4, second.x87.data() + 4, 6) == 0 &&
           first.mxcsr == second.mxcsr && first.lastError == second.lastError;
}

void SetEnvironment(unsigned int rounding, unsigned int mxcsr, DWORD lastError)
{
    unsigned int ignored = 0;
    static_cast<void>(_controlfp_s(&ignored, rounding, _MCW_RC));
    _clearfp();
    __asm
        {
        fldz
        fldz
        fdivp st(1), st(0)
        fstp st(0)
        }
    _mm_setcsr(mxcsr);
    SetLastError(lastError);
}

bahamut_client::ActionMovementSetter       gOriginal = nullptr;
bahamut_client::ActionMovementPolicy       gPolicy;
bahamut_client::ActionMovementService*     gService            = nullptr;
bahamut_client::ActionMovementHook*        gLifecycle          = nullptr;
bool                                       gShutdownDuringCall = false;
bahamut_client::ActionMovementHookShutdown gShutdownResult     = bahamut_client::ActionMovementHookShutdown::Complete;
std::uint64_t                              gRevokeDuringCall   = 0;
bool                                       gRevokedDuringCall  = false;
bool                                       gEligible           = false;
unsigned int                               gCalls              = 0;
void*                                      gReceiver           = nullptr;
std::int32_t                               gDuration           = 0;
Environment                                gEntry;
Environment                                gAfter;
bool                                       gFreed        = false;
std::uintptr_t                             gStackBefore  = 0;
std::uintptr_t                             gStackAfter   = 0;
std::uint32_t                              gPreservedEbx = 0;
std::uint32_t                              gPreservedEsi = 0;
std::uint32_t                              gPreservedEdi = 0;
std::uint32_t                              gPreservedEbp = 0;

extern "C" void __cdecl RecordSetter(void* receiver, std::int32_t duration)
{
    // Record before any other Win32 call; release detects a receiver read after forwarding.
    gEntry = CaptureEnvironment();
    ++gCalls;
    gReceiver = receiver;
    gDuration = duration;
    if (gShutdownDuringCall)
    {
        gShutdownResult = gLifecycle->Shutdown(false);
    }
    if (gRevokeDuringCall != 0)
    {
        gRevokedDuringCall = gService->RevokeConsumer(gRevokeDuringCall);
    }
    gFreed = VirtualFree(receiver, 0, MEM_RELEASE) != FALSE;
    SetEnvironment(_RC_UP, 0x5F81u, 0x2468u);
    gAfter = CaptureEnvironment();
}

extern "C" __declspec(naked) void FakeSetter()
{
    __asm
        {
        nop
        nop
        nop
        nop
        nop
        nop
        push dword ptr [esp+4]
        push ecx
        call RecordSetter
        add esp, 8
        ret 4
        }
}

void __fastcall HookSetter(void* receiver, void*, std::int32_t duration)
{
    if (gService != nullptr)
    {
        const auto original = gLifecycle != nullptr ? gLifecycle->Original() : gOriginal;
        gService->Forward(receiver, duration, gEligible, original);
    }
    else
    {
        bahamut_client::ForwardActionMovement(receiver, duration, gEligible, gPolicy, gOriginal);
    }
}

__declspec(naked) void __stdcall InvokeSetter(void*, std::int32_t)
{
    __asm
        {
        push ebp
        push ebx
        push esi
        push edi
        mov ebp, 11223344h
        mov ebx, 22334455h
        mov esi, 33445566h
        mov edi, 44556677h
        mov dword ptr [gStackBefore], esp
        mov ecx, dword ptr [esp+20]
        push dword ptr [esp+24]
        call FakeSetter
        mov dword ptr [gStackAfter], esp
        mov dword ptr [gPreservedEbp], ebp
        mov dword ptr [gPreservedEbx], ebx
        mov dword ptr [gPreservedEsi], esi
        mov dword ptr [gPreservedEdi], edi
        pop edi
        pop esi
        pop ebx
        pop ebp
        ret 8
        }
}

bool CheckForwarding(bahamut_client::ActionMovementPolicy policy, bool eligible, std::int32_t duration, std::int32_t expected)
{
    void* receiver = VirtualAlloc(nullptr, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (receiver == nullptr)
    {
        return false;
    }
    gPolicy                   = policy;
    gEligible                 = eligible;
    gCalls                    = 0;
    gFreed                    = false;
    const Environment initial = CaptureEnvironment();
    SetEnvironment(_RC_DOWN, 0x1FA0u, 0x1357u);
    const Environment entry = CaptureEnvironment();
    InvokeSetter(receiver, duration);
    const Environment after  = CaptureEnvironment();
    const bool        passed = gCalls == 1 && gReceiver == receiver && gDuration == expected && gFreed &&
                               gStackBefore == gStackAfter && gPreservedEbp == 0x11223344u &&
                               gPreservedEbx == 0x22334455u && gPreservedEsi == 0x33445566u && gPreservedEdi == 0x44556677u &&
                               SameEnvironment(entry, gEntry) && SameEnvironment(after, gAfter);
    RestoreEnvironment(initial);
    if (!gFreed)
    {
        VirtualFree(receiver, 0, MEM_RELEASE);
    }
    if (!passed)
    {
        std::fprintf(stderr, "Forwarding failed: enabled=%d eligible=%d percent=%u duration=%d\n", policy.enabled, eligible, policy.retainPercent, duration);
    }
    return passed;
}

bool CheckServiceForwarding()
{
    bahamut_client::ActionMovementService service;
    gService       = &service;
    const auto run = [&]()
    {
        if (!CheckForwarding({}, true, 125, 125))
        {
            return false;
        }
        service.SetBoundaryState(bahamut_client::ActionMovementBoundaryState::Ready);
        const auto session = service.BeginSession();
        const auto owner   = service.BindConsumer(session);
        if (!service.Configure(owner, session, 40, true) || !CheckForwarding({}, true, 125, 50) ||
            !CheckForwarding({}, false, 125, 125) || !CheckForwarding({}, true, 1, 0) ||
            !CheckForwarding({}, true, -1, -1))
        {
            return false;
        }
        if (!service.Configure(owner, session, 0, false) || !CheckForwarding({}, true, 125, 125) ||
            !service.Configure(owner, session, 0, true) || !CheckForwarding({}, true, 125, 0))
        {
            return false;
        }
        gRevokeDuringCall   = owner;
        gRevokedDuringCall  = false;
        const bool inFlight = CheckForwarding({}, true, 125, 0);
        gRevokeDuringCall   = 0;
        if (!inFlight || !gRevokedDuringCall || !CheckForwarding({}, true, 125, 125) ||
            service.Configure(owner, session, 0, true))
        {
            return false;
        }
        const auto replacement = service.BindConsumer(session);
        if (!service.Configure(replacement, session, 40, true) || service.RevokeConsumer(owner) ||
            !CheckForwarding({}, true, 125, 50) || !service.EndSession(session) ||
            !CheckForwarding({}, true, 125, 125))
        {
            return false;
        }
        service.SetBoundaryState(bahamut_client::ActionMovementBoundaryState::CleanupFailed);
        if (!CheckForwarding({}, true, 125, 125))
        {
            return false;
        }
        service.Shutdown();
        return service.Status().boundary == bahamut_client::ActionMovementBoundaryState::CleanupFailed &&
               CheckForwarding({}, true, 125, 125);
    };
    const bool passed = run();
    gService          = nullptr;
    gRevokeDuringCall = 0;
    return passed;
}

std::atomic<unsigned int> gConcurrentCalls = 0;
std::atomic<bool>         gConcurrentError = false;

extern "C" void __cdecl RecordConcurrentSetter(std::int32_t duration)
{
    if (duration != 50 && duration != 125)
    {
        gConcurrentError.store(true);
    }
    gConcurrentCalls.fetch_add(1);
}

__declspec(naked) void ConcurrentSetter()
{
    __asm
        {
        push dword ptr [esp+4]
        call RecordConcurrentSetter
        add esp, 4
        ret 4
        }
}

bool CheckConcurrentPublication()
{
    bahamut_client::ActionMovementService service;
    service.SetBoundaryState(bahamut_client::ActionMovementBoundaryState::Ready);
    const auto session = service.BeginSession();
    const auto owner   = service.BindConsumer(session);
    if (!service.Configure(owner, session, 0, false))
    {
        return false;
    }
    gConcurrentCalls.store(0);
    gConcurrentError.store(false);
    std::atomic<bool>      started    = false;
    constexpr unsigned int iterations = 4096;
    std::thread            reader([&]()
                                  {
                           started.store(true);
                           for (unsigned int index = 0; index < iterations; ++index)
                           {
                               service.Forward(nullptr, 125, true, reinterpret_cast<bahamut_client::ActionMovementSetter>(&ConcurrentSetter));
                           }
                                  });
    while (!started.load())
    {
        std::this_thread::yield();
    }
    for (unsigned int index = 0; index < iterations; ++index)
    {
        if (!service.Configure(owner, session, 40, true) || !service.Configure(owner, session, 0, false))
        {
            gConcurrentError.store(true);
        }
    }
    reader.join();
    service.Shutdown();
    return gConcurrentCalls.load() == iterations && !gConcurrentError.load();
}

bool CheckNativeAdapter()
{
    if (MH_Initialize() != MH_OK)
    {
        return false;
    }
    void* target     = reinterpret_cast<void*>(&FakeSetter);
    void* trampoline = nullptr;
    if (MH_CreateHook(target, reinterpret_cast<void*>(&HookSetter), &trampoline) != MH_OK)
    {
        MH_Uninitialize();
        return false;
    }
    gOriginal = reinterpret_cast<bahamut_client::ActionMovementSetter>(trampoline);
    if (MH_EnableHook(target) != MH_OK)
    {
        MH_RemoveHook(target);
        MH_Uninitialize();
        return false;
    }
    const bool passed   = CheckForwarding({}, true, 125, 125) &&
                          CheckForwarding({ true, 40 }, false, 125, 125) &&
                          CheckForwarding({ true, 100 }, true, 125, 125) &&
                          CheckForwarding({ true, 40 }, true, 125, 50) &&
                          CheckForwarding({ true, 0 }, true, 125, 0) &&
                          CheckForwarding({ true, 40 }, true, 1, 0) &&
                          CheckForwarding({ true, 0 }, true, -1, -1) &&
                          CheckForwarding({ true, 40 }, true, 0, 0) &&
                          CheckForwarding({ true, 101 }, true, 125, 125) &&
                          CheckServiceForwarding();
    const bool disabled = MH_DisableHook(target) == MH_OK;
    const bool unhooked = disabled && CheckForwarding({ true, 0 }, true, 125, 125);
    const bool removed  = MH_RemoveHook(target) == MH_OK;
    const bool shutdown = MH_Uninitialize() == MH_OK;
    return passed && unhooked && removed && shutdown;
}

struct HookFaults
{
    MH_STATUS    createError       = MH_OK;
    MH_STATUS    enableError       = MH_OK;
    MH_STATUS    disableError      = MH_OK;
    MH_STATUS    removeError       = MH_OK;
    bool         missingTrampoline = false;
    bool         enableBeforeError = false;
    bool         probePassed       = false;
    unsigned int creates           = 0;
    unsigned int enables           = 0;
    unsigned int disables          = 0;
    unsigned int removes           = 0;
};

HookFaults gHookFaults;

MH_STATUS WINAPI TestHookCreate(LPVOID target, LPVOID detour, LPVOID* original)
{
    ++gHookFaults.creates;
    if (gHookFaults.createError != MH_OK)
    {
        return gHookFaults.createError;
    }
    const auto result = MH_CreateHook(target, detour, original);
    if (result == MH_OK && gHookFaults.missingTrampoline)
    {
        *original = nullptr;
    }
    return result;
}

MH_STATUS WINAPI TestHookEnable(LPVOID target)
{
    ++gHookFaults.enables;
    if (gHookFaults.enableError != MH_OK && !gHookFaults.enableBeforeError)
    {
        return gHookFaults.enableError;
    }
    const auto result = MH_EnableHook(target);
    if (result == MH_OK && gHookFaults.enableBeforeError)
    {
        // Entry before Install returns must find a trampoline and disabled policy.
        gHookFaults.probePassed = CheckForwarding({}, true, 125, 125);
        return gHookFaults.enableError;
    }
    return result;
}

MH_STATUS WINAPI TestHookDisable(LPVOID target)
{
    ++gHookFaults.disables;
    return gHookFaults.disableError == MH_OK ? MH_DisableHook(target) : gHookFaults.disableError;
}

MH_STATUS WINAPI TestHookRemove(LPVOID target)
{
    ++gHookFaults.removes;
    return gHookFaults.removeError == MH_OK ? MH_RemoveHook(target) : gHookFaults.removeError;
}

bool CheckHookLifecycle()
{
    using Stage  = bahamut_client::ActionMovementHookStage;
    using Result = bahamut_client::ActionMovementHookShutdown;
    using State  = bahamut_client::ActionMovementBoundaryState;
    if (MH_Initialize() != MH_OK)
    {
        return false;
    }
    bahamut_client::ActionMovementService service;
    bahamut_client::ActionMovementHook    hook(service, { &TestHookCreate, &TestHookEnable, &TestHookDisable, &TestHookRemove });
    gService          = &service;
    gLifecycle        = &hook;
    gHookFaults       = {};
    void*      target = reinterpret_cast<void*>(&FakeSetter);
    void*      detour = reinterpret_cast<void*>(&HookSetter);
    const auto run    = [&]()
    {
        if (hook.Install(target, detour, false) || hook.Install(nullptr, detour, true) || hook.Install(target, nullptr, true) ||
            hook.Status().installFailure != Stage::Target || gHookFaults.creates != 0 || hook.Original() != nullptr)
        {
            return false;
        }
        gHookFaults.createError = MH_ERROR_MEMORY_ALLOC;
        if (hook.Install(target, detour, true) || hook.Status().installError != MH_ERROR_MEMORY_ALLOC ||
            hook.Status().target != nullptr || gHookFaults.enables != 0 ||
            hook.Shutdown(true) != Result::Complete || gHookFaults.removes != 0)
        {
            return false;
        }
        gHookFaults           = {};
        void* foreignOriginal = nullptr;
        if (MH_CreateHook(target, detour, &foreignOriginal) != MH_OK)
        {
            return false;
        }
        const bool foreignPreserved = !hook.Install(target, detour, true) &&
                                      hook.Status().installError == MH_ERROR_ALREADY_CREATED &&
                                      hook.Shutdown(true) == Result::Complete && gHookFaults.removes == 0;
        const bool foreignRemoved   = MH_RemoveHook(target) == MH_OK;
        if (!foreignPreserved || !foreignRemoved)
        {
            return false;
        }
        gHookFaults                   = {};
        gHookFaults.missingTrampoline = true;
        gHookFaults.removeError       = MH_ERROR_MEMORY_PROTECT;
        if (hook.Install(target, detour, true) || hook.Status().installFailure != Stage::Trampoline ||
            hook.Status().cleanupFailure != Stage::Remove || hook.Status().target != target ||
            hook.Original() != nullptr || gHookFaults.enables != 0 || service.Status().boundary != State::CleanupFailed)
        {
            return false;
        }
        gHookFaults.removeError = MH_OK;
        if (hook.Shutdown(true) != Result::Complete || hook.Status().target != nullptr)
        {
            return false;
        }
        gHookFaults             = {};
        gHookFaults.enableError = MH_ERROR_MEMORY_PROTECT;
        if (hook.Install(target, detour, true) || hook.Status().installFailure != Stage::Enable ||
            !hook.Status().disabled || hook.Original() == nullptr || gHookFaults.disables != 1 ||
            gHookFaults.removes != 0 || hook.Shutdown(false) != Result::AwaitingCalls ||
            hook.Shutdown(true) != Result::Complete || hook.Status().installError != MH_ERROR_MEMORY_PROTECT ||
            service.Status().boundary != State::InstallFailed)
        {
            return false;
        }
        gHookFaults                   = {};
        gHookFaults.enableError       = MH_ERROR_MEMORY_PROTECT;
        gHookFaults.enableBeforeError = true;
        gHookFaults.disableError      = MH_ERROR_MEMORY_PROTECT;
        if (hook.Install(target, detour, true) || !gHookFaults.probePassed || hook.Status().disabled ||
            hook.Status().cleanupFailure != Stage::Disable || hook.Status().cleanupError != MH_ERROR_MEMORY_PROTECT ||
            hook.Original() == nullptr || service.Status().boundary != State::CleanupFailed ||
            !CheckForwarding({}, true, 125, 125))
        {
            return false;
        }
        const auto retained = hook.Original();
        const auto creates  = gHookFaults.creates;
        if (hook.Install(target, detour, true) || gHookFaults.creates != creates || hook.Original() != retained ||
            hook.Shutdown(true) != Result::Failed || gHookFaults.removes != 0)
        {
            return false;
        }
        gHookFaults.disableError = MH_OK;
        if (hook.Shutdown(false) != Result::AwaitingCalls || !hook.Status().disabled ||
            hook.Original() != retained || gHookFaults.removes != 0 || !CheckForwarding({}, true, 125, 125) ||
            hook.Shutdown(true) != Result::Complete || hook.Original() != nullptr ||
            hook.Status().cleanupFailure != Stage::None || service.Status().boundary != State::InstallFailed)
        {
            return false;
        }
        gHookFaults = {};
        if (!hook.Install(target, detour, true) || service.Status().boundary != State::Ready || service.Status().enabled)
        {
            return false;
        }
        const auto session = service.BeginSession();
        const auto owner   = service.BindConsumer(session);
        if (!service.Configure(owner, session, 40, true) || !CheckForwarding({}, true, 125, 50) ||
            hook.Install(target, detour, true) || service.Status().effectivePercent != 40)
        {
            return false;
        }
        const auto activeOriginal   = hook.Original();
        gShutdownDuringCall         = true;
        const bool admittedFinished = CheckForwarding({}, true, 125, 50);
        gShutdownDuringCall         = false;
        if (!admittedFinished || gShutdownResult != Result::AwaitingCalls || !hook.Status().disabled ||
            hook.Original() != activeOriginal || gHookFaults.removes != 0 || service.Status().enabled ||
            service.Configure(owner, session, 0, true) || !CheckForwarding({}, true, 125, 125))
        {
            return false;
        }
        gHookFaults.removeError = MH_ERROR_MEMORY_PROTECT;
        if (hook.Shutdown(true) != Result::Failed || hook.Status().cleanupFailure != Stage::Remove ||
            hook.Original() != activeOriginal || service.Status().boundary != State::CleanupFailed ||
            !CheckForwarding({}, true, 125, 125))
        {
            return false;
        }
        gHookFaults.removeError = MH_OK;
        if (hook.Shutdown(true) != Result::Complete || hook.Status().target != nullptr || hook.Original() != nullptr ||
            service.Status().boundary != State::Unavailable || !hook.Install(target, detour, true) ||
            service.Configure(owner, session, 0, true) || service.Status().enabled)
        {
            return false;
        }
        const auto newSession = service.BeginSession();
        const auto newOwner   = service.BindConsumer(newSession);
        return service.Configure(newOwner, newSession, 0, true) && CheckForwarding({}, true, 125, 0) &&
               hook.Shutdown(true) == Result::Complete && CheckForwarding({}, true, 125, 125);
    };
    const bool passed        = run();
    gShutdownDuringCall      = false;
    gHookFaults              = {};
    const bool cleaned       = hook.Shutdown(true) == Result::Complete;
    gLifecycle               = nullptr;
    gService                 = nullptr;
    const bool uninitialized = MH_Uninitialize() == MH_OK;
    if (!passed)
    {
        std::fprintf(stderr, "Hook lifecycle checks failed\n");
    }
    return passed && cleaned && uninitialized;
}

#endif

} // namespace

int main()
{
    if (!CheckScaling() || !CheckOwnership())
    {
        return 1;
    }
#if defined(_MSC_VER) && defined(_M_IX86)
    if (!CheckNativeAdapter())
    {
        return 2;
    }
    if (!CheckConcurrentPublication())
    {
        return 3;
    }
    if (!CheckHookLifecycle())
    {
        return 4;
    }
#endif
    return 0;
}
