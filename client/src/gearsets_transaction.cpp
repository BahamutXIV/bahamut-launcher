#include "gearsets_transaction.h"

#include <algorithm>

namespace bahamut_client
{
namespace
{

bool Matches(const EquipmentSlot& current, const EquipmentSlot& wanted)
{
    return current.slot == wanted.slot && current.state == wanted.state &&
           (wanted.state == EquipmentSlotState::Empty || (wanted.state == EquipmentSlotState::Item && current.uid == wanted.uid));
}

std::size_t IndexOf(std::uint8_t slot)
{
    return static_cast<std::size_t>(std::find(kEquipmentSlots.begin(), kEquipmentSlots.end(), slot) - kEquipmentSlots.begin());
}

bool Complete(const EquipmentSnapshot& snapshot)
{
    if (!snapshot.complete)
        return false;
    for (std::size_t index = 0; index < kEquipmentSlotCount; ++index)
    {
        const auto& slot = snapshot.slots[index];
        if (slot.slot != kEquipmentSlots[index] ||
            (slot.state != EquipmentSlotState::Empty && slot.state != EquipmentSlotState::Item) ||
            (slot.state == EquipmentSlotState::Item && slot.uid == 0) ||
            (slot.state == EquipmentSlotState::Empty && slot.uid != 0))
            return false;
    }
    return true;
}

} // namespace

std::string_view GearsetsPhaseName(GearsetsPhase phase)
{
    switch (phase)
    {
        case GearsetsPhase::Checking:
            return "checking";
        case GearsetsPhase::Applying:
            return "applying";
        case GearsetsPhase::Complete:
            return "complete";
        case GearsetsPhase::Rejected:
            return "rejected";
        case GearsetsPhase::Partial:
            return "partial";
        case GearsetsPhase::Unconfirmed:
            return "unconfirmed";
        case GearsetsPhase::Cancelled:
            return "cancelled";
    }
    return "rejected";
}

bool GearsetsTransaction::Active() const
{
    return result_.phase == GearsetsPhase::Checking || result_.phase == GearsetsPhase::Applying;
}

void GearsetsTransaction::ObserveChanges(const GearsetsView& view)
{
    if (view.session != intent_.session || !Complete(view.inventory.equipment))
        return;
    for (std::size_t index = 0; index < kEquipmentSlotCount; ++index)
    {
        result_.changed[index] = !Matches(view.inventory.equipment.slots[index], baseline_.inventory.equipment.slots[index]);
    }
}

void GearsetsTransaction::Stop(std::string reason, std::optional<std::uint8_t> slot)
{
    const bool changed = std::any_of(result_.changed.begin(), result_.changed.end(), [](bool value)
                                     {
                                         return value;
                                     });
    result_.phase      = pending_ ? GearsetsPhase::Unconfirmed : changed        ? GearsetsPhase::Partial
                                                             : cancelRequested_ ? GearsetsPhase::Cancelled
                                                                                : GearsetsPhase::Rejected;
    result_.reason     = std::move(reason);
    result_.slot       = slot;
    pending_.reset();
}

std::optional<GearsetsFailure> GearsetsTransaction::Preflight(const GearsetsView& view, const GearsetsExecutor& executor) const
{
    if (view.session == 0 || view.session != intent_.session)
        return GearsetsFailure{ "session-changed", {} };
    if (!Complete(view.inventory.equipment))
        return GearsetsFailure{ "equipment-unavailable", {} };
    if (view.inventory.bag.empty() || view.inventory.bag.size() > kMaximumAccessibleBagSlots ||
        std::any_of(view.inventory.bag.begin(), view.inventory.bag.end(), [](const BagItem& item)
                    {
                        return item.state == EquipmentSlotState::Unknown || (item.state == EquipmentSlotState::Item && item.uid == 0);
                    }))
        return GearsetsFailure{ "unavailable", {} };
    for (const auto& wanted : intent_.slots)
    {
        if (wanted.state != EquipmentSlotState::Item)
            continue;
        const auto resolved = ResolveEquipmentUid(view.inventory, wanted.uid);
        if (resolved.state != ItemAvailability::Available)
        {
            return GearsetsFailure{ std::string(ItemAvailabilityName(resolved.state)), wanted.slot };
        }
        if (!Matches(view.inventory.equipment.slots[IndexOf(wanted.slot)], wanted) && !resolved.bagSlot)
        {
            return GearsetsFailure{ "source-unavailable", wanted.slot };
        }
    }
    return executor.Preflight(intent_, view);
}

bool GearsetsTransaction::Begin(const GearsetIntent& intent, const GearsetsView& view, GearsetsExecutor& executor)
{
    if (Active())
        return false;
    intent_   = intent;
    baseline_ = expected_ = view;
    result_               = {};
    result_.phase         = GearsetsPhase::Checking;
    pending_.reset();
    settleUntilMs_.reset();
    deferredSinceMs_.reset();
    cancelRequested_ = false;
    lateSettle_      = false;
    for (std::size_t index = 0; index < kEquipmentSlotCount; ++index)
    {
        const auto& slot = intent_.slots[index];
        if (slot.slot != kEquipmentSlots[index] ||
            (slot.state != EquipmentSlotState::Empty && slot.state != EquipmentSlotState::Item) ||
            (slot.state == EquipmentSlotState::Item && slot.uid == 0) ||
            (slot.state == EquipmentSlotState::Empty && slot.uid != 0))
        {
            Stop("invalid-intent", slot.slot);
            return true;
        }
        for (std::size_t previous = 0; previous < index; ++previous)
        {
            if (slot.state == EquipmentSlotState::Item && slot.uid == intent_.slots[previous].uid)
            {
                Stop("duplicate-intent", slot.slot);
                return true;
            }
        }
    }
    const auto plan = executor.Plan(intent_);
    if (!plan)
    {
        Stop("executor-unavailable");
        return true;
    }
    auto sorted = plan->order;
    std::sort(sorted.begin(), sorted.end());
    if (sorted.size() != kEquipmentSlotCount || !std::equal(sorted.begin(), sorted.end(), kEquipmentSlots.begin()) || plan->confirmationMs == 0)
    {
        Stop("invalid-plan");
        return true;
    }
    plan_ = *plan;
    if (const auto failure = Preflight(view, executor))
        Stop(failure->reason, failure->slot);
    return true;
}

bool GearsetsTransaction::LateClassChange(const GearsetsView& view) const
{
    // The game can report the class of a new main hand after the settling
    // window; it is still this transaction's change while that main hand is
    // the one it equipped.
    return result_.changed[0] && Matches(view.inventory.equipment.slots[0], intent_.slots[0]) &&
           (view.classId != expected_.classId || view.jobId != expected_.jobId);
}

bool GearsetsTransaction::WearsIntent(const GearsetsView& view) const
{
    for (std::size_t index = 0; index < kEquipmentSlotCount; ++index)
    {
        if (!Matches(view.inventory.equipment.slots[index], intent_.slots[index]))
            return false;
    }
    return true;
}

bool GearsetsTransaction::ExpectedChanges(const GearsetsView& view, std::optional<std::uint8_t>& unexpected) const
{
    // Only this transaction's main-hand operation may explain class/job changes
    // or dependent-slot clears, including a late class change while the equipped
    // main hand still matches.
    const bool mainHand = (pending_ && pending_->target.slot == 0) || (settleUntilMs_ && view.nowMs < *settleUntilMs_) || LateClassChange(view);
    if (!mainHand && (view.classId != expected_.classId || view.jobId != expected_.jobId))
        return false;
    for (std::size_t index = 0; index < kEquipmentSlotCount; ++index)
    {
        const auto& slot = view.inventory.equipment.slots[index];
        if (Matches(slot, expected_.inventory.equipment.slots[index]))
            continue;
        if (pending_ && slot.slot == pending_->target.slot && Matches(slot, pending_->target))
            continue;
        if (pending_ && pending_->sourceSlot == slot.slot && slot.state == EquipmentSlotState::Empty)
            continue;
        if (mainHand && slot.slot != 0 && slot.state == EquipmentSlotState::Empty)
            continue;
        unexpected = slot.slot;
        return false;
    }
    return true;
}

void GearsetsTransaction::Step(const GearsetsView& view, GearsetsExecutor& executor)
{
    if (!Active())
        return;
    ObserveChanges(view);
    if (view.session != intent_.session)
    {
        Stop("session-changed");
        return;
    }
    if (!Complete(view.inventory.equipment))
    {
        Stop("equipment-unavailable");
        return;
    }
    std::optional<std::uint8_t> unexpected;
    const bool                  lateClass = LateClassChange(view);
    if (!ExpectedChanges(view, unexpected))
    {
        if (!WearsIntent(view))
        {
            if (pending_ && Matches(view.inventory.equipment.slots[IndexOf(pending_->target.slot)], pending_->target))
                pending_.reset();
            Stop("external-change", unexpected);
            return;
        }
        // However it got there, the requested set is worn. Accept it and finish
        // through the usual settling and completion checks.
        const bool mainHand = (pending_ && pending_->target.slot == 0) || lateClass || lateSettle_;
        lateSettle_         = false;
        pending_.reset();
        expected_ = view;
        if (mainHand && plan_.settleMs != 0)
        {
            settleUntilMs_ = view.nowMs + plan_.settleMs;
            result_.reason = cancelRequested_ ? "cancelling" : "settling";
        }
        return;
    }
    // Late class changes may clear dependent slots on later frames, so they
    // settle like a main-hand confirmation before anything else is sent.
    if (lateClass && plan_.settleMs != 0)
    {
        if (pending_)
            lateSettle_ = true;
        else if (!settleUntilMs_ || view.nowMs >= *settleUntilMs_)
            settleUntilMs_ = view.nowMs + plan_.settleMs;
    }
    if (pending_)
    {
        if (!Matches(view.inventory.equipment.slots[IndexOf(pending_->target.slot)], pending_->target))
        {
            if (view.nowMs < submittedMs_ || view.nowMs - submittedMs_ >= plan_.confirmationMs)
                Stop("confirmation-timeout", pending_->target.slot);
            return;
        }
        if ((pending_->target.slot == 0 || lateSettle_) && plan_.settleMs != 0)
            settleUntilMs_ = view.nowMs + plan_.settleMs;
        lateSettle_ = false;
        pending_.reset();
        expected_ = view;
        if (cancelRequested_ && !settleUntilMs_)
            Stop("cancelled");
        // Observe confirmation on its own frame before sending another request.
        return;
    }
    if (settleUntilMs_)
    {
        // Accept the main hand's class change and cleared slots seen so far.
        // A cancel still waits, so late native changes are accounted for.
        expected_ = view;
        if (view.nowMs < *settleUntilMs_)
        {
            result_.reason = cancelRequested_ ? "cancelling" : "settling";
            return;
        }
        settleUntilMs_.reset();
    }
    if (cancelRequested_)
    {
        Stop("cancelled");
        return;
    }
    if (const auto failure = Preflight(view, executor))
    {
        Stop(failure->reason, failure->slot);
        return;
    }
    for (const auto slot : plan_.order)
    {
        const auto  index  = IndexOf(slot);
        const auto& wanted = intent_.slots[index];
        if (Matches(view.inventory.equipment.slots[index], wanted))
            continue;
        GearsetsOperation operation{ wanted, {}, {} };
        if (wanted.state == EquipmentSlotState::Item)
        {
            const auto resolved  = ResolveEquipmentUid(view.inventory, wanted.uid);
            operation.bagSlot    = resolved.bagSlot;
            operation.sourceSlot = resolved.equippedSlot;
        }
        // Checked before Submit, so a route that reopens late sends nothing.
        if (deferredSinceMs_ && (view.nowMs < *deferredSinceMs_ || view.nowMs - *deferredSinceMs_ >= plan_.confirmationMs))
        {
            Stop("native-not-ready", slot);
            return;
        }
        const auto submitted = executor.Submit(operation);
        if (submitted == GearsetsSubmission::Deferred)
        {
            if (!deferredSinceMs_)
                deferredSinceMs_ = view.nowMs;
            result_.reason = "waiting-native";
            result_.slot   = slot;
            return;
        }
        deferredSinceMs_.reset();
        ++result_.submissions;
        if (submitted != GearsetsSubmission::Accepted)
        {
            Stop(submitted == GearsetsSubmission::Rejected ? "native-rejected" : "executor-unavailable", slot);
            return;
        }
        pending_       = operation;
        expected_      = view;
        submittedMs_   = view.nowMs;
        result_.phase  = GearsetsPhase::Applying;
        result_.reason = "awaiting-confirmation";
        result_.slot   = slot;
        return;
    }
    result_.phase  = GearsetsPhase::Complete;
    result_.reason = "observed-match";
    result_.slot.reset();
}

void GearsetsTransaction::Cancel()
{
    if (!Active())
        return;
    cancelRequested_ = true;
    if (!pending_ && !settleUntilMs_)
        Stop("cancelled");
    else
        result_.reason = "cancelling";
}

void GearsetsTransaction::EndSession()
{
    if (Active())
        Stop("session-ended");
}

} // namespace bahamut_client
