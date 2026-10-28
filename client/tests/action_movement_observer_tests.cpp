#include "action_movement_observer.h"

#include <MinHook.h>
#include <windows.h>

#include <array>
#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <immintrin.h>
#include <limits>
#include <string_view>
#include <thread>
#include <vector>

#if defined(_MSC_VER)
#include <intrin.h>
#endif

namespace
{

using namespace bahamut_client;
using Stop = ActionMovementObservationStop;

bool Check(bool passed, const char* message)
{
    if (!passed)
        std::fprintf(stderr, "%s\n", message);
    return passed;
}

struct Arena
{
    unsigned char* data = static_cast<unsigned char*>(VirtualAlloc(nullptr, 0x4000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE));

    Arena()
    {
        if (data == nullptr)
            std::terminate();
        Put(0, reinterpret_cast<std::uintptr_t>(data + 0x3000));
        Put(0x118, reinterpret_cast<std::uintptr_t>(data + 0x3500));
        Put(0xBF0, reinterpret_cast<std::uintptr_t>(data));
        Put(0x3000 + 0x268, 0x12345678u);
        Put(0x3500 + 0xD0, reinterpret_cast<std::uintptr_t>(data));
        Put(0x2B70, 0x240u);
        Put(0xBF0 + 4, 0xAABBCC04u);
        Put(0xBF0 + 0x1CC, 0x3FA00000u);
        Put(0xBF0 + 0x1D4, 0xBF800000u);
        Put(0x2858 + 0x22C, 0x7FC00001u);
    }

    ~Arena()
    {
        if (data != nullptr)
            VirtualFree(data, 0, MEM_RELEASE);
    }

    void Put(std::size_t offset, std::uint32_t value)
    {
        std::memcpy(data + offset, &value, sizeof(value));
    }
};

std::atomic<unsigned> gOriginalCalls = 0;

void __stdcall CountWrapper(void*, std::int32_t)
{
    gOriginalCalls.fetch_add(1, std::memory_order_relaxed);
}

void __fastcall CountSetter(void*, void*, std::int32_t)
{
    gOriginalCalls.fetch_add(1, std::memory_order_relaxed);
}

const auto kCountSetter = reinterpret_cast<ActionMovementSetter>(&CountSetter);

bool BoundsTests()
{
    Arena                                    arena;
    std::array<ActionMovementObservation, 4> rows{};
    ActionMovementObserver                   observer(rows);
    gOriginalCalls = 0;
    observer.ForwardWrapper(reinterpret_cast<void*>(1), 123, 0, &CountWrapper);
    observer.ForwardSetter(reinterpret_cast<void*>(1), 123, 0, kCountSetter);
    bool passed = Check(observer.GuardedBatches() == 0 && observer.RecordsAfterQuiescence().empty(), "Unmatched callers performed reads");
    observer.ForwardWrapper(arena.data, 125, 0x8284AEu, &CountWrapper);
    observer.ForwardSetter(arena.data + 0xBF0, -1, 0x65ED59u, kCountSetter);
    auto records = observer.RecordsAfterQuiescence();
    passed &= Check(records.size() == 2 && records[0].kind == ActionMovementObservationKind::WrapperEntry &&
                        records[1].kind == ActionMovementObservationKind::SetterEntry && records[0].readsComplete && records[1].readsComplete &&
                        records[0].actor == reinterpret_cast<std::uintptr_t>(arena.data) && records[1].controller == reinterpret_cast<std::uintptr_t>(arena.data + 0xBF0) &&
                        records[1].actor == records[0].actor && records[0].ownerActor == records[0].actor && records[0].controllerActor == records[0].actor &&
                        records[0].controllerFlags == 4 && records[0].timerPrimaryBits == 0x3FA00000u && records[0].timerAlternateBits == 0xBF800000u &&
                        records[0].actionTimerBits == 0x7FC00001u && records[0].gateTarget == 0x12345678u && records[1].duration == -1,
                    "Entry fields, signed argument or raw scalar widths changed");
    // Zero timers still have a finite recording budget; there is no countdown hook.
    arena.Put(0xBF0 + 0x1CC, 0);
    arena.Put(0xBF0 + 0x1D4, 0);
    arena.Put(0x2858 + 0x22C, 0);
    for (unsigned i = 0; i < 100000; ++i)
        observer.ForwardWrapper(arena.data, 0, 0x8284AEu, &CountWrapper);
    passed &= Check(observer.StopReason() == Stop::Capacity && observer.GuardedBatches() == 4 &&
                        observer.RecordsAfterQuiescence().size() == 4 && gOriginalCalls == 100004,
                    "Capacity did not bound repeated zero snapshots");
    observer.Stop();
    passed &= Check(observer.StopReason() == Stop::Capacity, "Later stop hid the first failure");

    std::array<ActionMovementObservation, 2> faultRows{};
    ActionMovementObserver                   fault(faultRows);
    arena.Put(0, 1);
    gOriginalCalls = 0;
    for (unsigned i = 0; i < 100000; ++i)
        fault.ForwardWrapper(arena.data, 125, 0x8284AEu, &CountWrapper);
    passed &= Check(fault.StopReason() == Stop::ReadFault && fault.GuardedBatches() == 1 && fault.ReadFaults() == 1 &&
                        fault.RecordsAfterQuiescence().size() == 1 && !fault.RecordsAfterQuiescence()[0].readsComplete && gOriginalCalls == 100000,
                    "Unreadable extra field caused repeated faults or valid-looking output");
    ActionMovementObserver invalid(faultRows);
    invalid.ForwardWrapper(nullptr, 125, 0x8284AEu, &CountWrapper);
    passed &= Check(invalid.StopReason() == Stop::InvalidObject && invalid.GuardedBatches() == 0, "Null wrapper actor reached a read");
    ActionMovementObserver overflow(faultRows);
    overflow.ForwardSetter(reinterpret_cast<void*>(std::numeric_limits<std::uintptr_t>::max()), 125, 0x65ED59u, kCountSetter);
    passed &= Check(overflow.StopReason() == Stop::InvalidObject && overflow.GuardedBatches() == 0, "Overflowing native address reached a read");
    ActionMovementObserver empty({});
    empty.ForwardWrapper(reinterpret_cast<void*>(1), 125, 0x8284AEu, &CountWrapper);
    passed &= Check(empty.StopReason() == Stop::Capacity && empty.GuardedBatches() == 0, "Empty storage attempted recording");
    return passed;
}

bool FailedClock(std::int64_t&) noexcept
{
    SetLastError(0xFFFFu);
    return false;
}

ActionMovementObserver* gReentrant = nullptr;

bool ReentrantClock(std::int64_t& value) noexcept
{
    gReentrant->ForwardWrapper(reinterpret_cast<void*>(1), 12, 0x8284AEu, &CountWrapper);
    value = 123;
    return true;
}

bool SourceTests()
{
    std::array<ActionMovementObservation, 2> rows{};
    ActionMovementObserver                   failed(rows, {}, { &FailedClock, nullptr });
    failed.ForwardWrapper(reinterpret_cast<void*>(1), 125, 0x8284AEu, &CountWrapper);
    bool                   passed = Check(failed.StopReason() == Stop::ClockFailure && failed.GuardedBatches() == 0 &&
                                              failed.RecordsAfterQuiescence().size() == 1 && !failed.RecordsAfterQuiescence()[0].readsComplete,
                                          "Failed clock attempted native reads");
    ActionMovementObserver reentrant(rows, {}, { &ReentrantClock, nullptr });
    gReentrant     = &reentrant;
    gOriginalCalls = 0;
    reentrant.ForwardWrapper(reinterpret_cast<void*>(1), 125, 0x8284AEu, &CountWrapper);
    passed &= Check(reentrant.StopReason() == Stop::ReentrantCollection && reentrant.GuardedBatches() == 0 && gOriginalCalls == 2,
                    "Collection reentry spun or performed reads");
    gReentrant = nullptr;
    Arena                  arena;
    ActionMovementObserver threads(rows);
    threads.ForwardWrapper(arena.data, 125, 0x8284AEu, &CountWrapper);
    std::thread other([&]
                      {
                          threads.ForwardWrapper(arena.data, 125, 0x8284AEu, &CountWrapper);
                      });
    other.join();
    passed &= Check(threads.StopReason() == Stop::ThreadChange && threads.GuardedBatches() == 1 &&
                        threads.RecordsAfterQuiescence().size() == 1,
                    "Second producer thread performed native reads");
    return passed;
}

#if defined(_MSC_VER) && defined(_M_IX86)
struct alignas(16) Environment
{
    std::array<unsigned char, 512> fp;
    DWORD                          error;
    std::array<unsigned char, 12>  reserved;
};

Environment CaptureEnvironment()
{
    Environment result;
    _fxsave(result.fp.data());
    result.error = GetLastError();
    return result;
}

void RestoreEnvironment(const Environment& state)
{
    SetLastError(state.error);
    _fxrstor(const_cast<unsigned char*>(state.fp.data()));
}

bool SameEnvironment(const Environment& a, const Environment& b)
{
    bool same = std::memcmp(a.fp.data(), b.fp.data(), 5) == 0 &&
                std::memcmp(a.fp.data() + 24, b.fp.data() + 24, 4) == 0 && a.error == b.error;
    for (unsigned i = 0; i < 8; ++i)
        same &= std::memcmp(a.fp.data() + 32 + i * 16, b.fp.data() + 32 + i * 16, 10) == 0;
    return same;
}

void SetEnvironment(unsigned rounding, unsigned mxcsr, DWORD error)
{
    unsigned ignored;
    (void)_controlfp_s(&ignored, rounding, _MCW_RC);
    _clearfp();
    _mm_setcsr(mxcsr);
    SetLastError(error);
}

bool ClobberClock(std::int64_t& value) noexcept
{
    SetEnvironment(_RC_UP, 0x5F81u, 0xFFFFu);
    __asm fldz
        value = 123;
    return true;
}

ActionMovementObserver* gObserver        = nullptr;
ActionMovementWrapper   gOriginalWrapper = nullptr;
ActionMovementSetter    gOriginalSetter  = nullptr;
void*                   gReceiver        = nullptr;
std::int32_t            gDuration        = 0;
unsigned                gCalls           = 0;
unsigned                gFree            = 0;
bool                    gFreed           = false;
Environment             gEntry;
Environment             gAfter;
std::uintptr_t          gWrapperReturn = 0;
std::uintptr_t          gSetterReturn  = 0;
std::uintptr_t          gStackBefore   = 0;
std::uintptr_t          gStackAfter    = 0;
std::uint32_t           gEbp           = 0;
std::uint32_t           gEbx           = 0;
std::uint32_t           gEsi           = 0;
std::uint32_t           gEdi           = 0;

extern "C" void __cdecl RecordOriginal(void* receiver, std::int32_t duration)
{
    gEntry    = CaptureEnvironment();
    gReceiver = receiver;
    gDuration = duration;
    ++gCalls;
    if (gFree)
        gFreed = VirtualFree(gFree == 2 ? static_cast<unsigned char*>(receiver) - 0xBF0 : receiver, 0, MEM_RELEASE) != FALSE;
    SetEnvironment(_RC_UP, 0x5F81u, 0x2468u);
    gAfter = CaptureEnvironment();
}

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
        call RecordOriginal
        add esp, 8
        ret 8
        }
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
        call RecordOriginal
        add esp, 8
        ret 4
        }
}

void __stdcall HookWrapper(void* receiver, std::int32_t duration)
{
    gObserver->ForwardWrapper(receiver, duration, reinterpret_cast<std::uintptr_t>(_ReturnAddress()), gOriginalWrapper);
}

void __fastcall HookSetter(void* receiver, void*, std::int32_t duration)
{
    gObserver->ForwardSetter(receiver, duration, reinterpret_cast<std::uintptr_t>(_ReturnAddress()), gOriginalSetter);
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
        mov gStackBefore, esp
        mov gWrapperReturn, offset wrapper_return
        push dword ptr [esp+24]
        push dword ptr [esp+24]
        call FakeWrapper
wrapper_return:
        mov gStackAfter, esp
        mov gEbp, ebp
        mov gEbx, ebx
        mov gEsi, esi
        mov gEdi, edi
        pop edi
        pop esi
        pop ebx
        pop ebp
        ret 8
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
        mov gStackBefore, esp
        mov gSetterReturn, offset setter_return
        mov ecx, dword ptr [esp+20]
        push dword ptr [esp+24]
        call FakeSetter
setter_return:
        mov gStackAfter, esp
        mov gEbp, ebp
        mov gEbx, ebx
        mov gEsi, esi
        mov gEdi, edi
        pop edi
        pop esi
        pop ebx
        pop ebp
        ret 8
        }
}

bool NativeTests()
{
    const auto initial = CaptureEnvironment();
    Arena      first;
    InvokeWrapper(first.data, 1);
    InvokeSetter(first.data + 0xBF0, 1);
    if (MH_Initialize() != MH_OK)
        return false;
    void* wrapper    = nullptr;
    void* setter     = nullptr;
    bool  passed     = MH_CreateHook(&FakeWrapper, &HookWrapper, &wrapper) == MH_OK &&
                       MH_CreateHook(&FakeSetter, &HookSetter, &setter) == MH_OK;
    gOriginalWrapper = reinterpret_cast<ActionMovementWrapper>(wrapper);
    gOriginalSetter  = reinterpret_cast<ActionMovementSetter>(setter);
    passed &= MH_EnableHook(&FakeWrapper) == MH_OK && MH_EnableHook(&FakeSetter) == MH_OK;
    for (unsigned mode = 0; mode < 4; ++mode)
        for (bool setterCase : { false, true })
        {
            Arena                                    arena;
            std::array<ActionMovementObservation, 2> rows{};
            const bool                               unmatched = mode == 2;
            ActionMovementObserver                   observer(rows, { unmatched ? 0 : gWrapperReturn, unmatched ? 0 : gSetterReturn }, { &ClobberClock, nullptr });
            if (mode == 1)
                arena.Put(0, 1);
            if (mode == 3)
                observer.Stop();
            gObserver = &observer;
            gCalls    = 0;
            gFree     = setterCase ? 2u : 1u;
            gFreed    = false;
            SetEnvironment(_RC_DOWN, 0x1FA0u, 0x1357u);
            const auto entry    = CaptureEnvironment();
            void*      receiver = setterCase ? arena.data + 0xBF0 : arena.data;
            if (setterCase)
                InvokeSetter(receiver, -125);
            else
                InvokeWrapper(receiver, 125);
            const auto after   = CaptureEnvironment();
            const auto records = observer.RecordsAfterQuiescence();
            passed &= Check(gCalls == 1 && gReceiver == receiver && gDuration == (setterCase ? -125 : 125) &&
                                gFreed && SameEnvironment(entry, gEntry) && SameEnvironment(after, gAfter) &&
                                gStackBefore == gStackAfter && gEbp == 0x11223344u && gEbx == 0x22334455u && gEsi == 0x33445566u && gEdi == 0x44556677u &&
                                (mode >= 2 ? records.empty() && observer.GuardedBatches() == 0 : records.size() == 1 && records[0].readsComplete == (mode == 0) && records[0].callerReturn == (setterCase ? gSetterReturn : gWrapperReturn)) &&
                                (mode != 1 || (observer.StopReason() == Stop::ReadFault && observer.ReadFaults() == 1)),
                            "Native observer changed ABI, FP/LastError state, receiver lifetime or actual caller");
            if (gFreed)
                arena.data = nullptr;
            gFree = 0;
            observer.Stop();
        }
    passed &= MH_DisableHook(&FakeWrapper) == MH_OK && MH_DisableHook(&FakeSetter) == MH_OK;
    passed &= MH_RemoveHook(&FakeWrapper) == MH_OK && MH_RemoveHook(&FakeSetter) == MH_OK;
    passed &= MH_Uninitialize() == MH_OK;
    gObserver = nullptr;
    RestoreEnvironment(initial);
    return passed;
}
#else
bool NativeTests()
{
    std::puts("MSVC x86 naked ABI fixture is unavailable on this compiler.");
    return true;
}
#endif

void Benchmark()
{
    constexpr unsigned                     calls = 100000;
    Arena                                  arena;
    std::vector<ActionMovementObservation> rows(calls);
    LARGE_INTEGER                          frequency;
    QueryPerformanceFrequency(&frequency);
    const auto measure = [&](const char* name, auto invoke)
    {
        LARGE_INTEGER start, end;
        QueryPerformanceCounter(&start);
        for (unsigned i = 0; i < calls; ++i)
            invoke();
        QueryPerformanceCounter(&end);
        std::printf("%s mean_us=%.6f calls=%u\n", name, static_cast<double>(end.QuadPart - start.QuadPart) * 1000000.0 / static_cast<double>(frequency.QuadPart) / calls, calls);
    };
    measure("direct", [&]
            {
                CountWrapper(arena.data, 125);
            });
    ActionMovementObserver unknown(rows);
    measure("unmatched_caller", [&]
            {
                unknown.ForwardWrapper(arena.data, 125, 0, &CountWrapper);
            });
    ActionMovementObserver readable(rows);
    measure("readable_entry", [&]
            {
                readable.ForwardWrapper(arena.data, 125, 0x8284AEu, &CountWrapper);
            });
    arena.Put(0, 1);
    ActionMovementObserver fault(rows);
    measure("first_fault_then_stopped", [&]
            {
                fault.ForwardWrapper(arena.data, 125, 0x8284AEu, &CountWrapper);
            });
    std::printf("fault_guarded_batches=%u faults=%u records=%zu\n", fault.GuardedBatches(), fault.ReadFaults(), fault.RecordsAfterQuiescence().size());
    ActionMovementObserver stopped(rows);
    stopped.Stop();
    measure("manual_stop", [&]
            {
                stopped.ForwardWrapper(arena.data, 125, 0x8284AEu, &CountWrapper);
            });
}

} // namespace

int main(int argc, char** argv)
{
    if (argc == 2 && std::string_view(argv[1]) == "--benchmark")
    {
        Benchmark();
        return 0;
    }
    return BoundsTests() && SourceTests() && NativeTests() ? 0 : 1;
}
