#include "action_movement_observer.h"

#include "fault_guard.h"
#include "native_state.h"

#include <windows.h>

#include <limits>

namespace bahamut_client
{
namespace
{

bool Clock(std::int64_t& counter) noexcept
{
    LARGE_INTEGER value;
    if (!QueryPerformanceCounter(&value))
        return false;
    counter = value.QuadPart;
    return true;
}

std::uint32_t Thread() noexcept
{
    return GetCurrentThreadId();
}

template <class T>
T Read(std::uintptr_t base, std::size_t offset)
{
    return *reinterpret_cast<const volatile T*>(base + offset);
}

} // namespace

ActionMovementObserver::ActionMovementObserver(std::span<ActionMovementObservation> storage,
                                               ActionMovementObservationCalls       calls,
                                               ActionMovementObservationSources     sources)
: storage_(storage)
, calls_(calls)
, sources_(sources)
{
    if (sources_.clock == nullptr)
        sources_.clock = &Clock;
    if (sources_.thread == nullptr)
        sources_.thread = &Thread;
    if (storage.empty() || storage.size() > std::numeric_limits<std::uint32_t>::max())
        Stop(ActionMovementObservationStop::Capacity);
}

void ActionMovementObserver::Stop(ActionMovementObservationStop reason) noexcept
{
    if (reason == ActionMovementObservationStop::Recording)
        return;
    auto expected = ActionMovementObservationStop::Recording;
    (void)stop_.compare_exchange_strong(expected, reason, std::memory_order_acq_rel);
}

ActionMovementObservationStop ActionMovementObserver::StopReason() const noexcept
{
    return stop_.load(std::memory_order_acquire);
}

std::uint32_t ActionMovementObserver::GuardedBatches() const noexcept
{
    return guardedBatches_.load(std::memory_order_relaxed);
}

std::uint32_t ActionMovementObserver::ReadFaults() const noexcept
{
    return readFaults_.load(std::memory_order_relaxed);
}

std::span<const ActionMovementObservation> ActionMovementObserver::RecordsAfterQuiescence() const noexcept
{
    return storage_.first(stored_.load(std::memory_order_acquire));
}

bool ActionMovementObserver::Capture(ActionMovementObservationKind kind, void* object, ActionMovementObservation& record)
{
    const auto token = reinterpret_cast<std::uintptr_t>(object);
    if (token == 0 || token > std::numeric_limits<std::uintptr_t>::max() - 0x2B70u - 3u)
    {
        Stop(ActionMovementObservationStop::InvalidObject);
        return false;
    }
    guardedBatches_.fetch_add(1, std::memory_order_relaxed);
    // One fault guard covers all reads; no virtual calls or reads after original.
    // Offsets and structural meanings come from the source cited in action_movement_admission.h.
    BAHAMUT_FAULT_TRY
    {
        const auto actor = kind == ActionMovementObservationKind::WrapperEntry
                               ? token
                               : Read<std::uintptr_t>(token, 0);
        if (actor == 0 || actor > std::numeric_limits<std::uintptr_t>::max() - 0x2B70u - 3u)
        {
            Stop(ActionMovementObservationStop::InvalidObject);
            return false;
        }
        const auto controller  = kind == ActionMovementObservationKind::SetterEntry ? token : actor + 0xBF0u;
        record.actor           = static_cast<std::uint32_t>(actor);
        record.controller      = static_cast<std::uint32_t>(controller);
        record.controllerActor = static_cast<std::uint32_t>(Read<std::uintptr_t>(controller, 0));
        const auto vptr        = Read<std::uintptr_t>(actor, 0);
        const auto owner       = Read<std::uintptr_t>(actor, 0x118u);
        if (vptr == 0 || vptr > std::numeric_limits<std::uintptr_t>::max() - 0x268u ||
            owner == 0 || owner > std::numeric_limits<std::uintptr_t>::max() - 0xD0u)
        {
            Stop(ActionMovementObservationStop::InvalidObject);
            return false;
        }
        record.actorVptr          = static_cast<std::uint32_t>(vptr);
        record.gateTarget         = static_cast<std::uint32_t>(Read<std::uintptr_t>(vptr, 0x268u));
        record.owner              = static_cast<std::uint32_t>(owner);
        record.ownerActor         = static_cast<std::uint32_t>(Read<std::uintptr_t>(owner, 0xD0u));
        record.actorWord2B70      = Read<std::uint32_t>(actor, 0x2B70u);
        record.controllerFlags    = Read<std::uint8_t>(controller, 4u);
        record.timerPrimaryBits   = Read<std::uint32_t>(controller, 0x1CCu);
        record.timerAlternateBits = Read<std::uint32_t>(controller, 0x1D4u);
        record.actionTimerBits    = Read<std::uint32_t>(actor, 0x2858u + 0x22Cu);
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        readFaults_.fetch_add(1, std::memory_order_relaxed);
        Stop(ActionMovementObservationStop::ReadFault);
        return false;
    }
}

void ActionMovementObserver::Record(ActionMovementObservationKind kind, void* object, std::int32_t duration, std::uintptr_t callerReturn)
{
    // A second producer or collection reentry permanently stops rather than spins.
    if (collecting_.exchange(true, std::memory_order_acquire))
    {
        Stop(ActionMovementObservationStop::ReentrantCollection);
        return;
    }
    if (StopReason() != ActionMovementObservationStop::Recording)
    {
        collecting_.store(false, std::memory_order_release);
        return;
    }
    const auto    thread   = sources_.thread();
    std::uint32_t expected = 0;
    (void)producerThread_.compare_exchange_strong(expected, thread, std::memory_order_acq_rel);
    if (thread == 0 || producerThread_.load(std::memory_order_acquire) != thread)
    {
        Stop(ActionMovementObservationStop::ThreadChange);
        collecting_.store(false, std::memory_order_release);
        return;
    }
    if (StopReason() != ActionMovementObservationStop::Recording)
    {
        collecting_.store(false, std::memory_order_release);
        return;
    }
    const auto index = stored_.load(std::memory_order_relaxed);
    if (index == storage_.size())
    {
        Stop(ActionMovementObservationStop::Capacity);
        collecting_.store(false, std::memory_order_release);
        return;
    }
    ActionMovementObservation record;
    record.kind         = kind;
    record.ordinal      = index + 1u;
    record.thread       = thread;
    record.callerReturn = static_cast<std::uint32_t>(callerReturn);
    record.duration     = duration;
    if (kind == ActionMovementObservationKind::WrapperEntry)
        record.actor = static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(object));
    else
        record.controller = static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(object));
    const bool clockValid = sources_.clock(record.qpc);
    if (clockValid && StopReason() == ActionMovementObservationStop::Recording)
        record.readsComplete = Capture(kind, object, record);
    else if (!clockValid)
        Stop(ActionMovementObservationStop::ClockFailure);
    storage_[index] = record;
    stored_.store(index + 1u, std::memory_order_release);
    collecting_.store(false, std::memory_order_release);
}

void ActionMovementObserver::ForwardWrapper(void* actor, std::int32_t duration, std::uintptr_t callerReturn, ActionMovementWrapper original)
{
    if (StopReason() == ActionMovementObservationStop::Recording && callerReturn == calls_.producerReturn)
    {
        NativeState state;
        SaveNativeState(state);
        Record(ActionMovementObservationKind::WrapperEntry, actor, duration, callerReturn);
        RestoreNativeState(state);
    }
    original(actor, duration);
}

void ActionMovementObserver::ForwardSetter(void* controller, std::int32_t duration, std::uintptr_t callerReturn, ActionMovementSetter original)
{
    if (StopReason() == ActionMovementObservationStop::Recording && callerReturn == calls_.setterReturn)
    {
        NativeState state;
        SaveNativeState(state);
        Record(ActionMovementObservationKind::SetterEntry, controller, duration, callerReturn);
        RestoreNativeState(state);
    }
    original(controller, duration);
}

} // namespace bahamut_client
