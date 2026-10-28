#include "action_movement.h"
#include "action_movement_admission.h"
#include "action_movement_hook.h"
#include "action_movement_runtime.h"
#include "player_state.h"

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

bahamut_client::ActionMovementWrapper      gOriginal           = nullptr;
bahamut_client::ActionMovementService*     gService            = nullptr;
bahamut_client::ActionMovementHook*        gLifecycle          = nullptr;
bahamut_client::ActionMovementRuntime*     gRuntime            = nullptr;
bool                                       gShutdownDuringCall = false;
bahamut_client::ActionMovementHookShutdown gShutdownResult     = bahamut_client::ActionMovementHookShutdown::Complete;
bool                                       gRuntimeShutdown    = false;
bahamut_client::ActionMovementHookShutdown gRuntimeResult      = bahamut_client::ActionMovementHookShutdown::Complete;
std::uint64_t                              gRevokeDuringCall   = 0;
bool                                       gRevokedDuringCall  = false;
bahamut_client::ActionMovementCallContext  gContext;
unsigned int                               gCalls    = 0;
void*                                      gActor    = nullptr;
std::int32_t                               gDuration = 0;
Environment                                gEntry;
Environment                                gAfter;
bool                                       gFreed        = false;
std::uintptr_t                             gStackBefore  = 0;
std::uintptr_t                             gStackAfter   = 0;
std::uint32_t                              gPreservedEbx = 0;
std::uint32_t                              gPreservedEsi = 0;
std::uint32_t                              gPreservedEdi = 0;
std::uint32_t                              gPreservedEbp = 0;

extern "C" void __cdecl RecordWrapper(void* actor, std::int32_t duration)
{
    // Record before any other Win32 call; release detects an actor read after forwarding.
    gEntry = CaptureEnvironment();
    ++gCalls;
    gActor    = actor;
    gDuration = duration;
    if (gShutdownDuringCall)
    {
        gShutdownResult = gLifecycle->Shutdown(false);
    }
    if (gRuntimeShutdown)
    {
        gRuntimeResult = gRuntime->Shutdown();
    }
    if (gRevokeDuringCall != 0)
    {
        gRevokedDuringCall = gService->RevokeConsumer(gRevokeDuringCall);
    }
    gFreed = VirtualFree(actor, 0, MEM_RELEASE) != FALSE;
    SetEnvironment(_RC_UP, 0x5F81u, 0x2468u);
    gAfter = CaptureEnvironment();
}

// Retail shape: stdcall (actor, duration), RET 8, with room for a five-byte patch.
extern "C" __declspec(naked) void FakeWrapper()
{
    __asm
        {
        nop
        nop
        nop
        nop
        nop
        nop
        push dword ptr [esp+8]
        push dword ptr [esp+8]
        call RecordWrapper
        add esp, 8
        ret 8
        }
}

void __stdcall HookWrapper(void* actor, std::int32_t duration)
{
    const auto original = gLifecycle != nullptr ? gLifecycle->Original() : gOriginal;
    gService->ForwardWrapper(actor, duration, gContext, original);
}

__declspec(naked) void __stdcall InvokeWrapper(void*, std::int32_t)
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
        push dword ptr [esp+24]
        push dword ptr [esp+24]
        call FakeWrapper
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

bahamut_client::ActionMovementCallContext MakeContext(std::uint64_t session, std::int32_t duration)
{
    bahamut_client::ActionMovementCallContext context;
    context.targetVerified = true;
    context.session = context.currentSession = session;
    context.callerReturn                     = 0x8284AEu;
    context.actor = context.ownerActor = context.controllerActor = 0x100000u;
    context.actorVptr                                            = 0xFC0D34u;
    context.gateTarget                                           = 0x669650u;
    context.actorId = context.localActorId = 0x48u;
    context.originalDuration               = duration;
    context.readsComplete                  = true;
    return context;
}

// Calls the hooked fake through a stdcall frame that checks stack, callee-saved registers, x87,
// MXCSR and LastError, and frees the actor inside the original to catch a read after forwarding.
bool CheckInvoke(std::int32_t duration, std::int32_t expected)
{
    void* actor = VirtualAlloc(nullptr, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (actor == nullptr)
    {
        return false;
    }
    gCalls                    = 0;
    gFreed                    = false;
    const Environment initial = CaptureEnvironment();
    SetEnvironment(_RC_DOWN, 0x1FA0u, 0x1357u);
    const Environment entry = CaptureEnvironment();
    InvokeWrapper(actor, duration);
    const Environment after  = CaptureEnvironment();
    const bool        passed = gCalls == 1 && gActor == actor && gDuration == expected && gFreed &&
                               gStackBefore == gStackAfter && gPreservedEbp == 0x11223344u &&
                               gPreservedEbx == 0x22334455u && gPreservedEsi == 0x33445566u && gPreservedEdi == 0x44556677u &&
                               SameEnvironment(entry, gEntry) && SameEnvironment(after, gAfter);
    RestoreEnvironment(initial);
    if (!gFreed)
    {
        VirtualFree(actor, 0, MEM_RELEASE);
    }
    if (!passed)
    {
        std::fprintf(stderr, "Forwarding failed: duration=%d expected=%d got=%d calls=%u\n", duration, expected, gDuration, gCalls);
    }
    return passed;
}

bool CheckForwarding(bool eligible, std::int32_t duration, std::int32_t expected)
{
    gContext = eligible ? MakeContext(gService->Status().session, duration) : bahamut_client::ActionMovementCallContext{};
    return CheckInvoke(duration, expected);
}

bool CheckServiceForwarding()
{
    bahamut_client::ActionMovementService service;
    gService       = &service;
    const auto run = [&]()
    {
        if (!CheckForwarding(true, 125, 125))
        {
            return false;
        }
        service.SetBoundaryState(bahamut_client::ActionMovementBoundaryState::Ready);
        const auto session = service.BeginSession();
        const auto owner   = service.BindConsumer(session);
        if (!service.Configure(owner, session, 40, true) || !CheckForwarding(true, 125, 50) ||
            !CheckForwarding(false, 125, 125) || !CheckForwarding(true, 1, 1) ||
            !CheckForwarding(true, -1, -1))
        {
            return false;
        }
        if (!service.Configure(owner, session, 0, false) || !CheckForwarding(true, 125, 125) ||
            !service.Configure(owner, session, 0, true) || !CheckForwarding(true, 125, 125))
        {
            return false;
        }
        if (!service.Configure(owner, session, 40, true))
        {
            return false;
        }
        gRevokeDuringCall   = owner;
        gRevokedDuringCall  = false;
        const bool inFlight = CheckForwarding(true, 125, 50);
        gRevokeDuringCall   = 0;
        if (!inFlight || !gRevokedDuringCall || !CheckForwarding(true, 125, 125) ||
            service.Configure(owner, session, 0, true))
        {
            return false;
        }
        const auto replacement = service.BindConsumer(session);
        if (!service.Configure(replacement, session, 40, true) || service.RevokeConsumer(owner) ||
            !CheckForwarding(true, 125, 50) || !service.EndSession(session) ||
            !CheckForwarding(true, 125, 125))
        {
            return false;
        }
        service.SetBoundaryState(bahamut_client::ActionMovementBoundaryState::CleanupFailed);
        if (!CheckForwarding(true, 125, 125))
        {
            return false;
        }
        service.Shutdown();
        return service.Status().boundary == bahamut_client::ActionMovementBoundaryState::CleanupFailed &&
               CheckForwarding(true, 125, 125);
    };
    const bool passed = run();
    gService          = nullptr;
    gRevokeDuringCall = 0;
    return passed;
}

std::atomic<unsigned int> gConcurrentCalls = 0;
std::atomic<bool>         gConcurrentError = false;

extern "C" void __cdecl RecordConcurrentWrapper(std::int32_t duration)
{
    if (duration != 50 && duration != 125)
    {
        gConcurrentError.store(true);
    }
    gConcurrentCalls.fetch_add(1);
}

__declspec(naked) void ConcurrentWrapper()
{
    __asm
        {
        push dword ptr [esp+8]
        call RecordConcurrentWrapper
        add esp, 4
        ret 8
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
    const auto             context    = MakeContext(session, 125);
    std::atomic<bool>      started    = false;
    constexpr unsigned int iterations = 4096;
    std::thread            reader([&]()
                                  {
                           started.store(true);
                           for (unsigned int index = 0; index < iterations; ++index)
                           {
                               service.ForwardWrapper(nullptr, 125, context, reinterpret_cast<bahamut_client::ActionMovementWrapper>(&ConcurrentWrapper));
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
    void* target     = reinterpret_cast<void*>(&FakeWrapper);
    void* trampoline = nullptr;
    if (MH_CreateHook(target, reinterpret_cast<void*>(&HookWrapper), &trampoline) != MH_OK)
    {
        MH_Uninitialize();
        return false;
    }
    gOriginal = reinterpret_cast<bahamut_client::ActionMovementWrapper>(trampoline);
    if (MH_EnableHook(target) != MH_OK)
    {
        MH_RemoveHook(target);
        MH_Uninitialize();
        return false;
    }
    bahamut_client::ActionMovementService service;
    gService = &service;
    service.SetBoundaryState(bahamut_client::ActionMovementBoundaryState::Ready);
    const auto session = service.BeginSession();
    const auto owner   = service.BindConsumer(session);

    const auto policy = [&](std::uint32_t percent, bool enabled, bool eligible, std::int32_t duration, std::int32_t expected)
    {
        return service.Configure(owner, session, percent, enabled) && CheckForwarding(eligible, duration, expected);
    };
    const bool passed   = policy(0, false, true, 125, 125) &&
                          policy(40, true, false, 125, 125) &&
                          policy(100, true, true, 125, 125) &&
                          policy(40, true, true, 125, 50) &&
                          policy(0, true, true, 125, 125) &&
                          policy(40, true, true, 1, 1) &&
                          policy(0, true, true, -1, -1) &&
                          policy(40, true, true, 0, 0) &&
                          CheckServiceForwarding();
    gService            = nullptr;
    const bool disabled = MH_DisableHook(target) == MH_OK;
    gService            = &service;
    const bool unhooked = disabled && service.Configure(owner, session, 40, true) && CheckForwarding(true, 125, 125);
    gService            = nullptr;
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
        gHookFaults.probePassed = CheckForwarding(true, 125, 125);
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
    void*      target = reinterpret_cast<void*>(&FakeWrapper);
    void*      detour = reinterpret_cast<void*>(&HookWrapper);
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
            !CheckForwarding(true, 125, 125))
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
            hook.Original() != retained || gHookFaults.removes != 0 || !CheckForwarding(true, 125, 125) ||
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
        if (!service.Configure(owner, session, 40, true) || !CheckForwarding(true, 125, 50) ||
            hook.Install(target, detour, true) || service.Status().effectivePercent != 40)
        {
            return false;
        }
        const auto activeOriginal   = hook.Original();
        gShutdownDuringCall         = true;
        const bool admittedFinished = CheckForwarding(true, 125, 50);
        gShutdownDuringCall         = false;
        if (!admittedFinished || gShutdownResult != Result::AwaitingCalls || !hook.Status().disabled ||
            hook.Original() != activeOriginal || gHookFaults.removes != 0 || service.Status().enabled ||
            service.Configure(owner, session, 0, true) || !CheckForwarding(true, 125, 125))
        {
            return false;
        }
        gHookFaults.removeError = MH_ERROR_MEMORY_PROTECT;
        if (hook.Shutdown(true) != Result::Failed || hook.Status().cleanupFailure != Stage::Remove ||
            hook.Original() != activeOriginal || service.Status().boundary != State::CleanupFailed ||
            !CheckForwarding(true, 125, 125))
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
        return service.Configure(newOwner, newSession, 40, true) && CheckForwarding(true, 125, 50) &&
               hook.Shutdown(true) == Result::Complete && CheckForwarding(true, 125, 125);
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

std::uint32_t gActiveActorId = 0x48u;

std::uint32_t TestActiveActorId(const void*) noexcept
{
    return gActiveActorId;
}

// Substitute gFakeVtable and its gate target so fixtures need no fixed image base.
constexpr std::uint32_t kFakeGateTarget = 0x00112233u;
alignas(16) unsigned char gFakeVtable[0x300];
alignas(16) unsigned char gOtherVtable[0x300];

void PutWord(unsigned char* base, std::size_t offset, std::uintptr_t value)
{
    std::memcpy(base + offset, &value, sizeof(value));
}

// The fake original frees this actor to expose reads after forwarding.
unsigned char* AllocActor()
{
    auto* data = static_cast<unsigned char*>(VirtualAlloc(nullptr, 0x4000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE));
    if (data == nullptr)
    {
        std::terminate();
    }
    PutWord(data, 0, reinterpret_cast<std::uintptr_t>(gFakeVtable));
    PutWord(data, 0xDC, 0x48u);
    PutWord(data, 0x118, reinterpret_cast<std::uintptr_t>(data + 0x3500));
    PutWord(data, 0xBF0, reinterpret_cast<std::uintptr_t>(data));
    PutWord(data, 0x2B70, 0x200u);
    PutWord(data, 0x3500 + 0xD0, reinterpret_cast<std::uintptr_t>(data));
    return data;
}

// Direct runtime entry supplies the caller site; the original must see the entry environment.
// Reads must finish before the original frees the actor.
template <class Mutate>
bool CheckRuntimeCall(Mutate mutate, std::int32_t duration, std::uintptr_t callerReturn, std::int32_t expected, const char* name)
{
    unsigned char* actor = AllocActor();
    mutate(actor);
    gCalls                    = 0;
    gFreed                    = false;
    const Environment initial = CaptureEnvironment();
    SetEnvironment(_RC_DOWN, 0x1FA0u, 0x1357u);
    const Environment entry = CaptureEnvironment();
    gRuntime->ForwardWrapper(actor, duration, callerReturn);
    const Environment after  = CaptureEnvironment();
    const bool        passed = gCalls == 1 && gActor == actor && gDuration == expected && gFreed &&
                               SameEnvironment(entry, gEntry) && SameEnvironment(after, gAfter);
    RestoreEnvironment(initial);
    if (!gFreed)
    {
        VirtualFree(actor, 0, MEM_RELEASE);
    }
    if (!passed)
    {
        std::fprintf(stderr, "Runtime case failed: %s: calls=%u duration=%d expected=%d\n", name, gCalls, gDuration, expected);
    }
    return passed;
}

bool CheckRuntime()
{
    using State  = bahamut_client::ActionMovementBoundaryState;
    using Result = bahamut_client::ActionMovementHookShutdown;
    if (MH_Initialize() != MH_OK)
    {
        return false;
    }
    PutWord(gFakeVtable, 0x268, kFakeGateTarget);
    PutWord(gOtherVtable, 0x268, kFakeGateTarget);
    const std::uintptr_t                                 producer = 0x8284AEu;
    const bahamut_client::ActionMovementIdentity         identity{ producer, reinterpret_cast<std::uintptr_t>(gFakeVtable), kFakeGateTarget };
    const bahamut_client::ActionMovementLocalActorSource source{ nullptr, &TestActiveActorId };
    void*                                                target = reinterpret_cast<void*>(&FakeWrapper);
    const auto                                           none   = [](unsigned char*)
    {
    };
    const auto run = [&]()
    {
        bahamut_client::ActionMovementRuntime unverified;
        if (unverified.InstallAt(target, false, source, identity) || unverified.Service().Status().boundary != State::TargetMismatch ||
            unverified.Session() != 0 || MH_DisableHook(target) != MH_ERROR_NOT_CREATED)
        {
            return false;
        }
        bahamut_client::PlayerStateService    playerState;
        bahamut_client::ActionMovementRuntime retail;
        if (retail.Install(playerState) || retail.Service().Status().boundary != State::TargetMismatch ||
            MH_DisableHook(reinterpret_cast<void*>(bahamut_client::kActionMovementWrapperVa)) != MH_ERROR_NOT_CREATED)
        {
            return false;
        }
        bahamut_client::ActionMovementRuntime runtime;
        gRuntime = &runtime;
        if (!runtime.InstallAt(target, true, source, identity) || runtime.Service().Status().boundary != State::Ready || runtime.Session() == 0)
        {
            return false;
        }
        bahamut_client::ActionMovementRuntime second;
        if (second.InstallAt(target, true, source, identity) || second.Service().Status().boundary == State::Ready)
        {
            return false;
        }
        const auto session = runtime.Session();
        const auto owner   = runtime.Service().BindConsumer(session);
        gService           = &runtime.Service();
        // Through the real detour the caller is the test frame, not the producer call.
        if (!runtime.Service().Configure(owner, session, 40, true) || !CheckInvoke(125, 125))
        {
            return false;
        }
        bool admitted = CheckRuntimeCall(none, 125, producer, 50, "admitted");
        admitted &= CheckRuntimeCall(none, 1, producer, 1, "scaled zero");
        admitted &= CheckRuntimeCall(none, 0, producer, 0, "zero");
        admitted &= CheckRuntimeCall(none, -1, producer, -1, "negative");
        admitted &= CheckRuntimeCall(none, 125, producer + 1, 125, "caller");
        admitted &= CheckRuntimeCall([](unsigned char* actor)
                                     {
                                         PutWord(actor, 0x2B70, 0x240u);
                                     },
                                     125,
                                     producer,
                                     125,
                                     "gate bit");
        admitted &= CheckRuntimeCall([](unsigned char* actor)
                                     {
                                         actor[0xBF0 + 4] = 0x04;
                                     },
                                     125,
                                     producer,
                                     125,
                                     "timer flag");
        admitted &= CheckRuntimeCall([](unsigned char* actor)
                                     {
                                         PutWord(actor, 0xDC, 0x49u);
                                     },
                                     125,
                                     producer,
                                     125,
                                     "actor id");
        admitted &= CheckRuntimeCall([](unsigned char* actor)
                                     {
                                         PutWord(actor, 0, reinterpret_cast<std::uintptr_t>(gOtherVtable));
                                     },
                                     125,
                                     producer,
                                     125,
                                     "vtable");
        admitted &= CheckRuntimeCall([](unsigned char* actor)
                                     {
                                         PutWord(actor, 0x3500 + 0xD0, 0);
                                     },
                                     125,
                                     producer,
                                     125,
                                     "owner");
        gActiveActorId = 0x49u;
        admitted &= CheckRuntimeCall(none, 125, producer, 125, "local id");
        gActiveActorId = 0x48u;
        PutWord(gOtherVtable, 0x268, 0x1u);
        admitted &= CheckRuntimeCall([](unsigned char* actor)
                                     {
                                         PutWord(actor, 0, reinterpret_cast<std::uintptr_t>(gOtherVtable));
                                     },
                                     125,
                                     producer,
                                     125,
                                     "gate target");
        if (!admitted)
        {
            return false;
        }
        // An unreadable actor at the producer site is forwarded unchanged without a fault.
        gCalls = 0;
        gRuntime->ForwardWrapper(reinterpret_cast<void*>(0x1000u), 125, producer);
        if (gCalls != 1 || gDuration != 125)
        {
            return false;
        }
        // Shutdown from inside an admitted call: the call completes, and the hook stays resident.
        gRuntimeShutdown    = true;
        gRuntimeResult      = Result::Complete;
        const bool inFlight = CheckRuntimeCall(none, 125, producer, 50, "shutdown in flight");
        gRuntimeShutdown    = false;
        if (!inFlight || gRuntimeResult != Result::AwaitingCalls || runtime.Service().Status().enabled || runtime.Session() != 0 ||
            runtime.Service().Status().boundary != State::Unavailable || MH_DisableHook(target) != MH_ERROR_DISABLED)
        {
            return false;
        }
        // Late detour entries must forward unchanged exactly once.
        if (!CheckInvoke(125, 125) || !CheckRuntimeCall(none, 125, producer, 125, "straggler") ||
            runtime.Shutdown() != Result::AwaitingCalls || second.InstallAt(target, true, source, identity))
        {
            return false;
        }
        // The routing slot and trampoline outlive Shutdown; only a fixture may remove them.
        if (MH_EnableHook(target) != MH_OK || !CheckInvoke(125, 125) || !CheckRuntimeCall(none, 125, producer, 125, "re-entered straggler") ||
            MH_DisableHook(target) != MH_OK)
        {
            return false;
        }
        if (runtime.RemoveSyntheticHookForTesting() != Result::Complete || MH_DisableHook(target) != MH_ERROR_NOT_CREATED || !CheckInvoke(125, 125))
        {
            return false;
        }
        gRuntime = &second;
        if (!second.InstallAt(target, true, source, identity) || second.Session() == 0)
        {
            return false;
        }
        return second.RemoveSyntheticHookForTesting() == Result::Complete && MH_DisableHook(target) == MH_ERROR_NOT_CREATED;
    };
    const bool passed        = run();
    gRuntime                 = nullptr;
    gService                 = nullptr;
    const bool uninitialized = MH_Uninitialize() == MH_OK;
    if (!passed)
    {
        std::fprintf(stderr, "Runtime checks failed\n");
    }
    return passed && uninitialized;
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
    if (!CheckRuntime())
    {
        return 5;
    }
#endif
    return 0;
}
