#include "action_movement.h"
#include "action_movement_admission.h"
#include "retail_identity.h"
#include "runtime_contract.h"

#include <windows.h>

#include <array>
#include <atomic>
#include <cstdint>
#include <cstring>
#include <filesystem>
#include <iostream>
#include <limits>
#include <process.h>
#include <span>
#include <thread>

namespace
{

using namespace bahamut_client;
using Result = ActionMovementCallAdmission;

bool Check(bool passed, const char* message)
{
    if (!passed)
        std::cerr << message << '\n';
    return passed;
}

// Independently decoded from the identity-pinned PE at the cited native boundaries.
constexpr std::array<std::uint8_t, 22> kSetter = {
    0xF3, 0x0F, 0x2A, 0x44, 0x24, 0x04, 0xF3, 0x0F, 0x11, 0x81, 0xCC, 0x01, 0x00, 0x00, 0xE8, 0x7D, 0xF6, 0xFF, 0xFF, 0xC2, 0x04, 0x00
};
constexpr std::array<std::uint8_t, 45> kWrapper = {
    0x56, 0x8B, 0x74, 0x24, 0x08, 0x85, 0xF6, 0x74, 0x20, 0x8B, 0x06, 0x8B, 0x90, 0x68, 0x02, 0x00, 0x00, 0x8B, 0xCE, 0xFF, 0xD2, 0x85, 0xC0, 0x75, 0x10, 0x8B, 0x44, 0x24, 0x0C, 0x50, 0x8D, 0x8E, 0xF0, 0x0B, 0x00, 0x00, 0xE8, 0xA7, 0x19, 0x15, 0x00, 0x5E, 0xC2, 0x08, 0x00
};
constexpr std::array<std::uint8_t, 5> kProducerCall = { 0xE8, 0x82, 0x68, 0xE3, 0xFF };

bool TargetTests()
{
    bool passed = Check(MatchActionMovementTargetBytes(0x400000u, kSetter, kWrapper, kProducerCall), "Pinned byte windows rejected");
    passed &= Check(!MatchActionMovementTargetBytes(0x500000u, kSetter, kWrapper, kProducerCall), "Relocated image admitted");
    passed &= Check(!MatchActionMovementTargetBytes(0x400000u, std::span(kSetter).first(21), kWrapper, kProducerCall), "Short setter admitted");
    passed &= Check(!MatchActionMovementTargetBytes(0x400000u, kSetter, std::span(kWrapper).first(44), kProducerCall), "Short wrapper admitted");
    passed &= Check(!MatchActionMovementTargetBytes(0x400000u, kSetter, kWrapper, std::span(kProducerCall).first(4)), "Short producer call admitted");
    for (std::size_t i = 0; i < kSetter.size(); ++i)
    {
        auto bytes = kSetter;
        bytes[i] ^= 1;
        passed &= Check(!MatchActionMovementTargetBytes(0x400000u, bytes, kWrapper, kProducerCall), "Altered setter admitted");
    }
    for (std::size_t i = 0; i < kWrapper.size(); ++i)
    {
        auto bytes = kWrapper;
        bytes[i] ^= 1;
        passed &= Check(!MatchActionMovementTargetBytes(0x400000u, kSetter, bytes, kProducerCall), "Altered wrapper admitted");
    }
    for (std::size_t i = 0; i < kProducerCall.size(); ++i)
    {
        auto bytes = kProducerCall;
        bytes[i] ^= 1;
        passed &= Check(!MatchActionMovementTargetBytes(0x400000u, kSetter, kWrapper, bytes), "Altered producer call admitted");
    }
    passed &= Check(!VerifyRuntimeHostIdentity(false) && !VerifyActionMovementRetailTarget(), "Native test executable admitted as retail");
    return passed;
}

bool StubIdentityTest(const std::filesystem::path& fixture)
{
    std::filesystem::create_directories(fixture);
    wchar_t     path[32768]{};
    const DWORD length = GetModuleFileNameW(nullptr, path, ARRAYSIZE(path));
    if (length == 0 || length >= ARRAYSIZE(path))
        return false;
    const auto executable = fixture / L"bahamut-test-client.exe";
    std::filesystem::copy_file(path, executable, std::filesystem::copy_options::overwrite_existing);
    return Check(_wspawnl(_P_WAIT, executable.c_str(), executable.c_str(), L"--stub-identity", nullptr) == 0,
                 "Default stub identity or strict retail rejection failed");
}

ActionMovementCallContext Context(std::uintptr_t actor, std::uint64_t session)
{
    ActionMovementCallContext context;
    context.targetVerified = true;
    context.session = context.currentSession = session;
    context.callerReturn                     = 0x8284AEu;
    context.actor = context.ownerActor = context.controllerActor = actor;
    context.actorVptr                                            = 0xFC0D34u;
    context.gateTarget                                           = 0x669650u;
    context.actorId = context.localActorId = 0x48u;
    context.originalDuration               = 125;
    context.readsComplete                  = true;
    return context;
}

bool ContextTests()
{
    const auto valid  = Context(0x100000u, 1);
    bool       passed = Check(AdmitActionMovementCall(valid, 125, 50) == Result::Admitted, "Synthetic wrapper evidence rejected");
    const auto reject = [&](auto mutate, Result expected)
    {
        auto context = valid;
        mutate(context);
        return Check(AdmitActionMovementCall(context, 125, 50) == expected, "Missing or inconsistent evidence admitted");
    };
    passed &= reject([](auto& c)
                     {
                         c.targetVerified = false;
                     },
                     Result::TargetUnavailable);
    passed &= reject([](auto& c)
                     {
                         c.session = 0;
                     },
                     Result::SessionUnavailable);
    passed &= reject([](auto& c)
                     {
                         c.currentSession = 0;
                     },
                     Result::SessionUnavailable);
    passed &= reject([](auto& c)
                     {
                         ++c.currentSession;
                     },
                     Result::SessionUnavailable);
    passed &= reject([](auto& c)
                     {
                         c.readsComplete = false;
                     },
                     Result::ReadsIncomplete);
    passed &= reject([](auto& c)
                     {
                         ++c.callerReturn;
                     },
                     Result::CallerMismatch);
    passed &= reject([](auto& c)
                     {
                         c.callerReturn = 0x65ED59u;
                     },
                     Result::CallerMismatch);
    passed &= reject([](auto& c)
                     {
                         ++c.originalDuration;
                     },
                     Result::CallerMismatch);
    passed &= reject([](auto& c)
                     {
                         c.actor = c.ownerActor = c.controllerActor = 0;
                     },
                     Result::IdentityMismatch);
    passed &= reject([](auto& c)
                     {
                         ++c.ownerActor;
                     },
                     Result::IdentityMismatch);
    passed &= reject([](auto& c)
                     {
                         ++c.controllerActor;
                     },
                     Result::IdentityMismatch);
    passed &= reject([](auto& c)
                     {
                         ++c.actorVptr;
                     },
                     Result::IdentityMismatch);
    passed &= reject([](auto& c)
                     {
                         ++c.gateTarget;
                     },
                     Result::IdentityMismatch);
    passed &= reject([](auto& c)
                     {
                         ++c.localActorId;
                     },
                     Result::IdentityMismatch);
    passed &= reject([](auto& c)
                     {
                         c.localActorId = 0;
                     },
                     Result::IdentityMismatch);
    passed &= reject([](auto& c)
                     {
                         c.actorId = 0;
                     },
                     Result::IdentityMismatch);
    passed &= reject([](auto& c)
                     {
                         c.actorId = c.localActorId = 0;
                     },
                     Result::IdentityMismatch);
    passed &= reject([](auto& c)
                     {
                         c.actorId = c.localActorId = kActionMovementNoActorId;
                     },
                     Result::IdentityMismatch);
    passed &= reject([](auto& c)
                     {
                         c.actorWord2B70 = 1u << 6;
                     },
                     Result::GateMismatch);
    passed &= reject([](auto& c)
                     {
                         c.controllerFlags = 4;
                     },
                     Result::GateMismatch);
    auto open            = valid;
    open.actorWord2B70   = ~(1u << 6);
    open.controllerFlags = ~4u;
    passed &= Check(AdmitActionMovementCall(open, 125, 50) == Result::Admitted, "Unrelated gate or flag bits rejected a call");
    // A substituted identity admits only under that identity; production uses the defaults.
    auto                   substituted = valid;
    ActionMovementIdentity fake{ 0x3000u, 0x1000u, 0x2000u };
    substituted.callerReturn = 0x3000u;
    substituted.actorVptr    = 0x1000u;
    substituted.gateTarget   = 0x2000u;
    passed &= Check(AdmitActionMovementCall(substituted, 125, 50, fake) == Result::Admitted && AdmitActionMovementCall(substituted, 125, 50) == Result::CallerMismatch,
                    "Substituted identity was ignored or leaked into the defaults");
    passed &= Check(AdmitActionMovementCall(valid, 125, 50, fake) == Result::CallerMismatch, "Retail constants admitted under a substituted identity");
    passed &= Check(AdmitActionMovementCall(valid, 125, 0) == Result::ScaledNonPositive, "Scaled zero admitted");
    passed &= Check(AdmitActionMovementCall(valid, 125, -1) == Result::ScaledNonPositive, "Negative scaled duration admitted");
    passed &= Check(AdmitActionMovementCall(valid, 0, 0) == Result::NonPositive, "Zero input classified as a scale result");
    passed &= Check(AdmitActionMovementCall(valid, -1, -1) == Result::NonPositive, "Negative input admitted");
    return passed;
}

struct Arena
{
    unsigned char* data = static_cast<unsigned char*>(VirtualAlloc(nullptr, 0x4000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE));

    Arena()
    {
        if (data == nullptr)
            std::terminate();
        // A fake vtable, owner and inline controller in one allocation; no fixed image base.
        Put(0, reinterpret_cast<std::uintptr_t>(data + 0x3000));
        Put(0x118, reinterpret_cast<std::uintptr_t>(data + 0x3500));
        Put(0xBF0, reinterpret_cast<std::uintptr_t>(data));
        Put(0xBF0 + 4, 0xAABBCC04u);
        Put(0xDC, 0x48u);
        Put(0x2B70, 0x200u);
        Put(0x3000 + 0x268, 0x669650u);
        Put(0x3500 + 0xD0, reinterpret_cast<std::uintptr_t>(data));
    }

    Arena(const Arena&)            = delete;
    Arena& operator=(const Arena&) = delete;

    ~Arena()
    {
        if (data != nullptr)
            VirtualFree(data, 0, MEM_RELEASE);
    }

    void Put(std::size_t offset, std::uint32_t value) const
    {
        std::memcpy(data + offset, &value, sizeof(value));
    }
};

bool CaptureTests()
{
    Arena                     arena;
    const auto                base = reinterpret_cast<std::uintptr_t>(arena.data);
    ActionMovementCallContext context;
    context.targetVerified = true;
    context.session        = 7;
    context.localActorId   = 0x99u;
    bool passed            = Check(CaptureActionMovementCallContext(arena.data, 0x8284AEu, 125, context), "Valid arena capture failed");
    passed &= Check(context.readsComplete && context.callerReturn == 0x8284AEu && context.originalDuration == 125 && context.actor == base &&
                        context.ownerActor == base && context.controllerActor == base && context.actorVptr == base + 0x3000u &&
                        context.gateTarget == 0x669650u && context.actorId == 0x48u && context.actorWord2B70 == 0x200u &&
                        context.controllerFlags == 0x04u,
                    "Capture copied the wrong fixed-offset values");
    passed &= Check(context.targetVerified && context.session == 7 && context.currentSession == 0 && context.localActorId == 0x99u,
                    "Capture changed host-owned context fields");

    context.currentSession = 7;
    context.localActorId   = 0x48u;
    context.controllerFlags &= ~0x04u;
    passed &= Check(AdmitActionMovementCall(context, 125, 50) == Result::IdentityMismatch, "Non-retail vtable admitted");
    context.actorVptr = kActionMovementPrimaryActorVtable;
    passed &= Check(AdmitActionMovementCall(context, 125, 50) == Result::Admitted, "Captured evidence rejected");

    arena.Put(0x2B70, 0x240u);
    arena.Put(0xBF0 + 4, 0);
    ActionMovementCallContext gated;
    passed &= Check(CaptureActionMovementCallContext(arena.data, 0x8284AEu, 125, gated) && gated.actorWord2B70 == 0x240u && gated.controllerFlags == 0,
                    "Capture missed the gate word or controller flag");

    const auto fails = [&](auto mutate, const char* message)
    {
        Arena bad;
        mutate(bad);
        ActionMovementCallContext captured;
        captured.actor         = 0x1234u;
        captured.readsComplete = true;
        return Check(!CaptureActionMovementCallContext(bad.data, 0x8284AEu, 125, captured) && !captured.readsComplete && captured.actor == 0x1234u, message);
    };
    passed &= fails([](Arena& a)
                    {
                        a.Put(0, 0);
                    },
                    "Null vtable captured");
    passed &= fails([](Arena& a)
                    {
                        a.Put(0, 0x1000u);
                    },
                    "Unreadable vtable captured");
    passed &= fails([](Arena& a)
                    {
                        a.Put(0x118, 0);
                    },
                    "Null owner captured");
    passed &= fails([](Arena& a)
                    {
                        a.Put(0x118, 0x1000u);
                    },
                    "Unreadable owner captured");
    ActionMovementCallContext untouched;
    passed &= Check(!CaptureActionMovementCallContext(nullptr, 0x8284AEu, 125, untouched) && !untouched.readsComplete, "Null actor captured");
    passed &= Check(!CaptureActionMovementCallContext(reinterpret_cast<void*>(0x1000u), 0x8284AEu, 125, untouched) && !untouched.readsComplete, "Unreadable actor captured");
    passed &= Check(!CaptureActionMovementCallContext(reinterpret_cast<void*>(std::numeric_limits<std::uintptr_t>::max() - 8), 0x8284AEu, 125, untouched), "Overflowing actor captured");
    return passed;
}

struct Capture
{
    void*                  actor      = nullptr;
    std::int32_t           duration   = 0;
    unsigned               calls      = 0;
    ActionMovementService* endService = nullptr;
    std::uint64_t          session    = 0;
};

thread_local Capture gCapture;

void __stdcall Wrapper(void* actor, std::int32_t duration)
{
    gCapture.actor    = actor;
    gCapture.duration = duration;
    ++gCapture.calls;
    if (gCapture.endService != nullptr)
        (void)gCapture.endService->EndSession(gCapture.session);
}

bool ForwardTests()
{
    ActionMovementService service;
    service.SetBoundaryState(ActionMovementBoundaryState::Ready);
    auto       session = service.BeginSession();
    auto       owner   = service.BindConsumer(session);
    auto       context = Context(0x100000u, session);
    auto       actor   = reinterpret_cast<void*>(0x100000u);
    const auto forward = [&](const ActionMovementCallContext& evidence, std::int32_t input, std::int32_t expected)
    {
        gCapture = {};
        service.ForwardWrapper(actor, input, evidence, &Wrapper);
        return Check(gCapture.calls == 1 && gCapture.actor == actor && gCapture.duration == expected, "Admission forwarding changed actor, duration or call count");
    };
    bool passed = Check(service.Configure(owner, session, 40, true), "Policy setup failed");
    passed &= forward({}, 125, 125);
    passed &= forward(context, 125, 50);
    passed &= Check(service.Configure(owner, session, 100, true), "Normal setup failed");
    passed &= forward(context, 125, 125);
    passed &= Check(service.Configure(owner, session, 0, true), "Zero setup failed");
    passed &= forward(context, 125, 125);
    context.originalDuration = 1;
    passed &= Check(service.Configure(owner, session, 40, true), "Rounding setup failed");
    passed &= forward(context, 1, 1);
    passed &= forward(context, 0, 0);
    passed &= forward(context, -1, -1);
    context.originalDuration = 125;
    passed &= Check(service.Configure(owner, session, 40, false), "Off setup failed");
    passed &= forward(context, 125, 125);
    auto rejected          = context;
    rejected.actorWord2B70 = 1u << 6;
    passed &= Check(service.Configure(owner, session, 40, true), "Gate setup failed");
    passed &= forward(rejected, 125, 125);
    const auto stale = context;
    session          = service.BeginSession();
    owner            = service.BindConsumer(session);
    passed &= Check(service.Configure(owner, session, 40, true), "Replacement setup failed");
    passed &= forward(stale, 125, 125);
    context.session = context.currentSession = session;
    passed &= forward(context, 125, 50);
    // The service stamps the session, so a caller-supplied currentSession cannot admit a stale context.
    auto forged           = stale;
    forged.currentSession = stale.session;
    passed &= forward(forged, 125, 125);
    gCapture            = {};
    gCapture.endService = &service;
    gCapture.session    = session;
    service.ForwardWrapper(actor, 125, context, &Wrapper);
    passed &= Check(gCapture.calls == 1 && gCapture.duration == 50 && service.Status().session == 0, "Reentrant session end did not preserve admitted original");
    passed &= forward(context, 125, 125);
    service.Shutdown();
    return passed;
}

bool SessionRaceTests()
{
    ActionMovementService service;
    service.SetBoundaryState(ActionMovementBoundaryState::Ready);
    const auto obsolete    = service.BeginSession();
    const auto replacement = service.BeginSession();
    auto       owner       = service.BindConsumer(replacement);
    if (!service.Configure(owner, replacement, 40, true))
        return false;
    std::atomic<bool> ready  = false;
    std::atomic<bool> done   = false;
    bool              passed = true;
    std::thread       reader([&]
                             {
                           const auto stale = Context(0x100000u, obsolete);
                           ready.store(true, std::memory_order_release);
                           do
                           {
                               gCapture = {};
                               service.ForwardWrapper(reinterpret_cast<void*>(0x100000u), 125, stale, &Wrapper);
                               passed &= gCapture.calls == 1 && gCapture.duration == 125;
                           } while (!done.load(std::memory_order_acquire));
                             });
    while (!ready.load(std::memory_order_acquire))
        std::this_thread::yield();
    for (unsigned i = 0; i < 500; ++i)
    {
        const auto session = service.BeginSession();
        owner              = service.BindConsumer(session);
        if (!service.Configure(owner, session, i % 2 == 0 ? 0u : 40u, true))
            std::terminate();
    }
    done.store(true, std::memory_order_release);
    reader.join();
    service.Shutdown();
    return Check(passed, "Obsolete context used a concurrently replaced session policy");
}

} // namespace

int wmain(int argc, wchar_t** argv)
{
    if (argc == 2 && std::wstring_view(argv[1]) == L"--stub-identity")
    {
        // Keep the real stub marker in this executable for the existing identity check.
        std::cout << bahamut_runtime_contract::kStubMarker << '\n';
        SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_TEST_STUB", L"1");
        return VerifyRuntimeHostIdentity() && !VerifyRuntimeHostIdentity(false) && !VerifyActionMovementRetailTarget() ? 0 : 1;
    }
    if (argc != 2)
        return 2;
    return TargetTests() && StubIdentityTest(argv[1]) && ContextTests() && CaptureTests() && ForwardTests() && SessionRaceTests() ? 0 : 1;
}
