#pragma once

#include <atomic>
#include <cstdint>
#include <mutex>

namespace bahamut_client
{

struct ActionMovementPolicy
{
    bool          enabled       = false;
    std::uint32_t retainPercent = 0;
};

// Retail 1.23b FUN_007B0700: ECX receiver, signed [ESP+4], void return, RET 4.
// https://github.com/XIVLegacy/xivl-client-structs/blob/3227221932dc99a9c1183e388bd6684564f4c732/docs/action-movement-wait.md#native-boundary
using ActionMovementSetter = void(__thiscall*)(void*, std::int32_t);

// Integer truncation is policy, not a claim about retail timer units or rounding.
[[nodiscard]] std::int32_t ScaleActionMovementDuration(std::int32_t duration, std::uint32_t retainPercent) noexcept;

// The caller supplies a copied policy and qualified eligibility; no retail hook is installed here.
// original must be callable. The receiver remains borrowed only for that original call.
void ForwardActionMovement(void*                receiver,
                           std::int32_t         duration,
                           bool                 eligible,
                           ActionMovementPolicy policy,
                           ActionMovementSetter original);

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
    // Host-only inputs: Ready follows hook qualification/installation; session and consumer
    // admission follow verified gameplay and the intended healthy addon. This core proves neither.
    void                        SetBoundaryState(ActionMovementBoundaryState state);
    [[nodiscard]] std::uint64_t BeginSession();
    [[nodiscard]] std::uint64_t BindConsumer(std::uint64_t session);
    [[nodiscard]] bool          Configure(std::uint64_t owner, std::uint64_t session, std::uint32_t retainPercent, bool enabled);
    [[nodiscard]] bool          RevokeConsumer(std::uint64_t owner);
    [[nodiscard]] bool          EndSession(std::uint64_t session);
    // Policy shutdown does not remove a hook or clear a reported boundary failure.
    void                               Shutdown();
    [[nodiscard]] ActionMovementStatus Status() const;

    // The atomic load admits this call. Revocation affects later loads; an admitted
    // original call may finish. The service and trampoline must outlive every call.
    void Forward(void* receiver, std::int32_t duration, bool eligible, ActionMovementSetter original) const;

private:
    void ClearConsumer();
    void ClearSession();

    static_assert(std::atomic<std::uint32_t>::is_always_lock_free);
    mutable std::mutex          mutex_;
    std::atomic<std::uint32_t>  published_        = 0;
    ActionMovementBoundaryState boundary_         = ActionMovementBoundaryState::Unavailable;
    std::uint64_t               sessionSerial_    = 0;
    std::uint64_t               ownerSerial_      = 0;
    std::uint64_t               session_          = 0;
    std::uint64_t               owner_            = 0;
    std::uint32_t               requestedPercent_ = 0;
};

} // namespace bahamut_client
