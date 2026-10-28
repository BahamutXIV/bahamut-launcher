#pragma once

#include "action_movement.h"
#include "action_movement_admission.h"
#include "action_movement_hook.h"

#include <atomic>
#include <cstdint>

namespace bahamut_client
{

class PlayerStateService;

// Validated player-state active actor id, or zero. Copied once per wrapper entry.
struct ActionMovementLocalActorSource
{
    const void* context                                  = nullptr;
    std::uint32_t (*activeActorId)(const void*) noexcept = nullptr;
};

[[nodiscard]] ActionMovementLocalActorSource ActionMovementLocalActorFrom(const PlayerStateService& playerState) noexcept;

// At most one runtime owns the process hook. The runtime, trampoline and detour must stay
// resident for the process lifetime because shutdown cannot prove call quiescence.
class ActionMovementRuntime
{
public:
    ActionMovementRuntime();
    ActionMovementRuntime(const ActionMovementRuntime&)            = delete;
    ActionMovementRuntime& operator=(const ActionMovementRuntime&) = delete;

    // Requires host-initialized MinHook and serialization with Shutdown; playerState must outlive every call.
    // A target mismatch leaves the image unchanged and reports TargetMismatch. Success starts a session.
    [[nodiscard]] bool Install(const PlayerStateService& playerState);
    // Native fixtures supply a synthetic target, its verification result and optional identity constants.
    [[nodiscard]] bool InstallAt(void* target, bool verified, ActionMovementLocalActorSource localActor, ActionMovementIdentity identity = {});

    // Other caller sites and calls entering after Shutdown begins forward unchanged.
    // Calls the original exactly once; reads happen only before it.
    void ForwardWrapper(void* actor, std::int32_t duration, std::uintptr_t callerReturn);

    // Stops policy, session and reads, then disables the hook without removing it.
    // AwaitingCalls is normal; late calls still forward unchanged exactly once.
    ActionMovementHookShutdown Shutdown();
    // Fixture-only removal requires proven call quiescence and releases the process slot.
    ActionMovementHookShutdown RemoveSyntheticHookForTesting();

    [[nodiscard]] ActionMovementService& Service()
    {
        return service_;
    }

    // Zero when no session is active.
    [[nodiscard]] std::uint64_t Session() const;

private:
    static_assert(std::atomic<std::uint64_t>::is_always_lock_free);
    ActionMovementService      service_;
    ActionMovementHook         hook_;
    std::atomic<bool>          stopped_{ false };
    std::atomic<std::uint64_t> session_{ 0 };
    // Written before the hook is enabled and constant afterwards.
    bool                           verified_ = false;
    ActionMovementLocalActorSource localActor_;
    ActionMovementIdentity         identity_;
};

} // namespace bahamut_client
