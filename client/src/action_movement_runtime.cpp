#include "action_movement_runtime.h"

#include "action_movement_admission.h"
#include "native_state.h"
#include "player_state.h"

#include <windows.h>

#if defined(_MSC_VER)
#include <intrin.h>
#pragma intrinsic(_ReturnAddress)
#define BAHAMUT_RETURN_ADDRESS() _ReturnAddress()
#else
#define BAHAMUT_RETURN_ADDRESS() __builtin_return_address(0)
#endif

namespace bahamut_client
{
namespace
{

std::atomic<ActionMovementRuntime*> gInstalled{ nullptr };

std::uint32_t PlayerStateActiveActorId(const void* playerState) noexcept
{
    return static_cast<const PlayerStateService*>(playerState)->ActiveActorId();
}

// Keep gInstalled set while the hook is reachable so threads paused across shutdown
// can still forward through the resident runtime.
void __stdcall WrapperDetour(void* actor, std::int32_t duration)
{
    const auto callerReturn = reinterpret_cast<std::uintptr_t>(BAHAMUT_RETURN_ADDRESS());
    gInstalled.load(std::memory_order_acquire)->ForwardWrapper(actor, duration, callerReturn);
}

} // namespace

ActionMovementLocalActorSource ActionMovementLocalActorFrom(const PlayerStateService& playerState) noexcept
{
    return { &playerState, &PlayerStateActiveActorId };
}

ActionMovementRuntime::ActionMovementRuntime()
: hook_(service_)
{
}

std::uint64_t ActionMovementRuntime::Session() const
{
    return session_.load(std::memory_order_acquire);
}

bool ActionMovementRuntime::Install(const PlayerStateService& playerState)
{
    return InstallAt(reinterpret_cast<void*>(kActionMovementWrapperVa), VerifyActionMovementRetailTarget(), ActionMovementLocalActorFrom(playerState));
}

bool ActionMovementRuntime::InstallAt(void* target, bool verified, ActionMovementLocalActorSource localActor, ActionMovementIdentity identity)
{
    ActionMovementRuntime* expected = nullptr;
    if (localActor.activeActorId == nullptr || !gInstalled.compare_exchange_strong(expected, this, std::memory_order_acq_rel))
        return false;
    verified_   = verified;
    localActor_ = localActor;
    identity_   = identity;
    if (!hook_.Install(target, reinterpret_cast<void*>(&WrapperDetour), verified))
    {
        // A hook that was created stays routable: a failed enable can still have run a detour.
        if (hook_.Status().target == nullptr)
            gInstalled.store(nullptr, std::memory_order_release);
        return false;
    }
    const std::uint64_t session = service_.BeginSession();
    session_.store(session, std::memory_order_release);
    if (session == 0)
    {
        static_cast<void>(Shutdown());
        return false;
    }
    return true;
}

void ActionMovementRuntime::ForwardWrapper(void* actor, std::int32_t duration, std::uintptr_t callerReturn)
{
    // The trampoline is published before enable and stays valid while the detour is reachable.
    const ActionMovementWrapper original = hook_.Original();
    if (stopped_.load(std::memory_order_acquire) || callerReturn != identity_.producerReturn || duration <= 0)
    {
        original(actor, duration);
        return;
    }
    ActionMovementCallContext context;
    {
        NativeState state;
        SaveNativeState(state);
        context.targetVerified = verified_;
        context.session        = session_.load(std::memory_order_acquire);
        context.localActorId   = localActor_.activeActorId(localActor_.context);
        static_cast<void>(CaptureActionMovementCallContext(actor, callerReturn, duration, context));
        RestoreNativeState(state);
    }
    service_.ForwardWrapper(actor, duration, context, original, identity_);
}

ActionMovementHookShutdown ActionMovementRuntime::Shutdown()
{
    stopped_.store(true, std::memory_order_release);
    session_.store(0, std::memory_order_release);
    return hook_.Shutdown(false);
}

ActionMovementHookShutdown ActionMovementRuntime::RemoveSyntheticHookForTesting()
{
    static_cast<void>(Shutdown());
    const auto result = hook_.Shutdown(true);
    if (result == ActionMovementHookShutdown::Complete)
    {
        ActionMovementRuntime* self = this;
        static_cast<void>(gInstalled.compare_exchange_strong(self, nullptr, std::memory_order_acq_rel));
    }
    return result;
}

} // namespace bahamut_client
