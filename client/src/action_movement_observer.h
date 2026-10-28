#pragma once

#include "action_movement.h"
#include "action_movement_admission.h"

#include <atomic>
#include <cstdint>
#include <span>

namespace bahamut_client
{

enum class ActionMovementObservationKind : std::uint32_t
{
    WrapperEntry,
    SetterEntry
};

struct ActionMovementObservation
{
    ActionMovementObservationKind kind               = ActionMovementObservationKind::WrapperEntry;
    std::uint32_t                 ordinal            = 0;
    std::int64_t                  qpc                = 0;
    std::uint32_t                 thread             = 0;
    std::uint32_t                 callerReturn       = 0;
    std::int32_t                  duration           = 0;
    std::uint32_t                 actor              = 0;
    std::uint32_t                 controller         = 0;
    std::uint32_t                 controllerActor    = 0;
    std::uint32_t                 actorVptr          = 0;
    std::uint32_t                 gateTarget         = 0;
    std::uint32_t                 owner              = 0;
    std::uint32_t                 ownerActor         = 0;
    std::uint32_t                 actorWord2B70      = 0;
    std::uint32_t                 controllerFlags    = 0;
    std::uint32_t                 timerPrimaryBits   = 0;
    std::uint32_t                 timerAlternateBits = 0;
    std::uint32_t                 actionTimerBits    = 0;
    bool                          readsComplete      = false;
};

enum class ActionMovementObservationStop : std::uint32_t
{
    Recording,
    Manual,
    Capacity,
    ReadFault,
    InvalidObject,
    ClockFailure,
    ThreadChange,
    ReentrantCollection
};

struct ActionMovementObservationSources
{
    bool (*clock)(std::int64_t&) noexcept = nullptr;
    std::uint32_t (*thread)() noexcept    = nullptr;
};

struct ActionMovementObservationCalls
{
    std::uintptr_t producerReturn = kActionMovementProducerCallVa + 5u;
    std::uintptr_t setterReturn   = 0x0065ED59u;
};

class ActionMovementObserver
{
public:
    // Storage is fixed by the cold owner and must outlive all calls. Alternate call
    // locators/sources serve native fixtures; they confer no movement eligibility.
    explicit ActionMovementObserver(std::span<ActionMovementObservation> storage,
                                    ActionMovementObservationCalls       calls   = {},
                                    ActionMovementObservationSources     sources = {});
    ActionMovementObserver(const ActionMovementObserver&)            = delete;
    ActionMovementObserver& operator=(const ActionMovementObserver&) = delete;

    // Baseline-only forwarding: no duration policy, installation, actor getter or
    // record after original. The detour supplies its actual entry return address.
    void ForwardWrapper(void* actor, std::int32_t duration, std::uintptr_t callerReturn, ActionMovementWrapper original);
    void ForwardSetter(void* controller, std::int32_t duration, std::uintptr_t callerReturn, ActionMovementSetter original);

    void                                        Stop(ActionMovementObservationStop reason = ActionMovementObservationStop::Manual) noexcept;
    [[nodiscard]] ActionMovementObservationStop StopReason() const noexcept;
    [[nodiscard]] std::uint32_t                 GuardedBatches() const noexcept;
    [[nodiscard]] std::uint32_t                 ReadFaults() const noexcept;
    // Stop does not drain calls. Read only after the caller proves all calls have finished;
    // retain the observer, storage and original code until then. Copied tokens are never dereferenced.
    [[nodiscard]] std::span<const ActionMovementObservation> RecordsAfterQuiescence() const noexcept;

private:
    void Record(ActionMovementObservationKind kind, void* object, std::int32_t duration, std::uintptr_t callerReturn);
    bool Capture(ActionMovementObservationKind kind, void* object, ActionMovementObservation& record);

    static_assert(std::atomic<std::uint32_t>::is_always_lock_free);
    static_assert(std::atomic<bool>::is_always_lock_free);
    static_assert(std::atomic<ActionMovementObservationStop>::is_always_lock_free);
    std::span<ActionMovementObservation>       storage_;
    ActionMovementObservationCalls             calls_;
    ActionMovementObservationSources           sources_;
    std::atomic<ActionMovementObservationStop> stop_           = ActionMovementObservationStop::Recording;
    std::atomic<std::uint32_t>                 producerThread_ = 0;
    std::atomic<bool>                          collecting_     = false;
    std::atomic<std::uint32_t>                 stored_         = 0;
    std::atomic<std::uint32_t>                 guardedBatches_ = 0;
    std::atomic<std::uint32_t>                 readFaults_     = 0;
};

} // namespace bahamut_client
