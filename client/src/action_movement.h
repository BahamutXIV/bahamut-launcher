#pragma once

#include <atomic>
#include <cstdint>
#include <mutex>

namespace bahamut_client
{

struct ActionMovementCallContext;
struct ActionMovementIdentity;

// Retail 1.23b FUN_007B0700: ECX receiver, signed [ESP+4], void return, RET 4.
// https://github.com/XIVLegacy/xivl-client-structs/blob/3227221932dc99a9c1183e388bd6684564f4c732/docs/action-movement-wait.md#native-boundary
using ActionMovementSetter = void(__thiscall*)(void*, std::int32_t);

// Retail 1.23b FUN_0065ED30: stack actor, signed [ESP+8] duration, void return, RET 8.
// https://github.com/XIVLegacy/xivl-client-structs/blob/3227221932dc99a9c1183e388bd6684564f4c732/docs/action-movement-wait.md#native-boundary
using ActionMovementWrapper = void(__stdcall*)(void*, std::int32_t);

// Integer truncation is policy, not a claim about retail timer units or rounding.
[[nodiscard]] std::int32_t ScaleActionMovementDuration(std::int32_t duration, std::uint32_t retainPercent) noexcept;

enum class ActionMovementBoundaryState
{
    Unavailable,
    Ready,
    TargetMismatch,
    InstallFailed,
    CleanupFailed
};

struct ActionMovementStatus
{
    ActionMovementBoundaryState boundary         = ActionMovementBoundaryState::Unavailable;
    std::uint64_t               session          = 0;
    std::uint64_t               owner            = 0;
    std::uint32_t               requestedPercent = 0;
    std::uint32_t               effectivePercent = 100;
    bool                        enabled          = false;
};

class ActionMovementService
{
public:
    // The host qualifies and installs the hook before reporting Ready, then starts a service session.
    // Consumer admission selects the intended healthy addon; each wrapper call checks copied actor evidence.
    void                        SetBoundaryState(ActionMovementBoundaryState state);
    [[nodiscard]] std::uint64_t BeginSession();
    [[nodiscard]] std::uint64_t BindConsumer(std::uint64_t session);
    [[nodiscard]] bool          Configure(std::uint64_t owner, std::uint64_t session, std::uint32_t retainPercent, bool enabled);
    [[nodiscard]] bool          RevokeConsumer(std::uint64_t owner);
    [[nodiscard]] bool          EndSession(std::uint64_t session);
    // Policy shutdown does not remove a hook or clear a reported boundary failure.
    void                               Shutdown();
    [[nodiscard]] ActionMovementStatus Status() const;

    // Uses copied evidence and atomic policy only; stamps currentSession without native getters or locks.
    // Calls original exactly once, scaling only admitted calls. Revocation affects later loads.
    // An admitted call may finish; the service and trampoline must outlive every call.
    void ForwardWrapper(void* actor, std::int32_t duration, const ActionMovementCallContext& context, ActionMovementWrapper original, const ActionMovementIdentity& identity) const;
    void ForwardWrapper(void* actor, std::int32_t duration, const ActionMovementCallContext& context, ActionMovementWrapper original) const;

private:
    void ClearConsumer();
    void ClearSession();

    static_assert(std::atomic<std::uint32_t>::is_always_lock_free);
    static_assert(std::atomic<std::uint64_t>::is_always_lock_free);
    mutable std::mutex          mutex_;
    std::atomic<std::uint32_t>  published_        = 0;
    std::atomic<std::uint64_t>  publishedSession_ = 0;
    ActionMovementBoundaryState boundary_         = ActionMovementBoundaryState::Unavailable;
    std::uint64_t               sessionSerial_    = 0;
    std::uint64_t               ownerSerial_      = 0;
    std::uint64_t               session_          = 0;
    std::uint64_t               owner_            = 0;
    std::uint32_t               requestedPercent_ = 0;
};

} // namespace bahamut_client
