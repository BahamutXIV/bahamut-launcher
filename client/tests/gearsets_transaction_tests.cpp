#include "gearsets_transaction.h"

#include <iostream>

namespace
{

using namespace bahamut_client;

bool Expect(bool condition, const char* message)
{
    if (!condition)
        std::cerr << "transaction: " << message << '\n';
    return condition;
}

GearsetsView EmptyView()
{
    GearsetsView view;
    view.session                      = 1;
    view.classId                      = 3;
    view.inventory.equipment.complete = true;
    for (std::size_t index = 0; index < kEquipmentSlotCount; ++index)
    {
        view.inventory.equipment.slots[index] = { kEquipmentSlots[index], EquipmentSlotState::Empty, 0, 0 };
    }
    view.inventory.bag.resize(16, { EquipmentSlotState::Empty, 0, 0 });
    return view;
}

GearsetIntent Intent(const GearsetsView& view)
{
    return { view.session, view.inventory.equipment.slots, view.classId, view.jobId };
}

class FakeExecutor final : public GearsetsExecutor
{
public:
    std::vector<GearsetsOperation> sent;
    bool                           reject       = false;
    bool                           restricted   = false;
    bool                           allowUnequip = true;
    bool                           busy         = false;
    std::uint64_t                  settleMs     = 0;
    mutable int                    checks       = 0;

    std::optional<GearsetsPlan> Plan(const GearsetIntent&) const override
    {
        return GearsetsPlan{ { kEquipmentSlots.begin(), kEquipmentSlots.end() }, 50, settleMs };
    }

    std::optional<GearsetsFailure> Preflight(const GearsetIntent& intent, const GearsetsView& view) const override
    {
        ++checks;
        if (restricted)
            return GearsetsFailure{ "restricted", 10 };
        for (std::size_t index = 0; index < kEquipmentSlotCount; ++index)
        {
            if (!allowUnequip && intent.slots[index].state == EquipmentSlotState::Empty && view.inventory.equipment.slots[index].state == EquipmentSlotState::Item)
                return GearsetsFailure{ "unequip-unavailable", intent.slots[index].slot };
        }
        return {};
    }

    GearsetsSubmission Submit(const GearsetsOperation& operation) override
    {
        if (busy)
            return GearsetsSubmission::Deferred;
        sent.push_back(operation);
        return reject ? GearsetsSubmission::Rejected : GearsetsSubmission::Accepted;
    }

    void Confirm(GearsetsView& view) const
    {
        const auto& operation = sent.back();
        for (auto& slot : view.inventory.equipment.slots)
        {
            if (slot.slot == operation.target.slot)
                slot = operation.target;
            else if (operation.sourceSlot == slot.slot)
                slot = { slot.slot, EquipmentSlotState::Empty, 0, 0 };
        }
        ++view.nowMs;
    }
};

bool TestPreflight()
{
    auto view             = EmptyView();
    auto intent           = Intent(view);
    intent.slots[0]       = { 0, EquipmentSlotState::Item, 0xFEDCBA9876543210ull, 100 };
    view.inventory.bag[1] = { EquipmentSlotState::Item, 2, 100 };
    GearsetsTransaction machine;
    FakeExecutor        executor;
    bool                ok = Expect(machine.Begin(intent, view, executor) && machine.Result().reason == "missing" && executor.sent.empty(), "full preflight refuses another copy without submission");
    view.inventory.bag[2]  = { EquipmentSlotState::Item, intent.slots[0].uid, 100 };
    view.inventory.bag[3]  = view.inventory.bag[2];
    machine.Begin(intent, view, executor);
    ok                    = Expect(machine.Result().reason == "ambiguous" && executor.sent.empty(), "ambiguous UID sends nothing") && ok;
    view.inventory.bag[3] = { EquipmentSlotState::Unknown, 0, 0 };
    machine.Begin(intent, view, executor);
    ok                    = Expect(machine.Result().reason == "unavailable" && executor.sent.empty(), "unknown bag data sends nothing") && ok;
    view.inventory.bag[3] = { EquipmentSlotState::Empty, 0, 0 };
    executor.restricted   = true;
    machine.Begin(intent, view, executor);
    ok                   = Expect(machine.Result().reason == "restricted" && executor.sent.empty(), "known native restriction sends nothing") && ok;
    executor.restricted  = false;
    intent.slots[1]      = intent.slots[0];
    intent.slots[1].slot = 1;
    machine.Begin(intent, view, executor);
    ok                                = Expect(machine.Result().reason == "duplicate-intent" && executor.sent.empty(), "a UID cannot occupy two destinations") && ok;
    intent                            = Intent(view);
    view.inventory.equipment.slots[1] = { 1, EquipmentSlotState::Item, 2, 100 };
    executor.allowUnequip             = false;
    machine.Begin(intent, view, executor);
    ok                    = Expect(machine.Result().reason == "unequip-unavailable" && executor.sent.empty(), "preflight includes explicit unequip support") && ok;
    view                  = EmptyView();
    intent                = Intent(view);
    view.inventory.bag[1] = { EquipmentSlotState::Item, 11, 100 };
    intent.slots[0]       = { 0, EquipmentSlotState::Item, 11, 100 };
    intent.slots[7]       = { 10, EquipmentSlotState::Item, 22, 200 };
    machine.Begin(intent, view, executor);
    machine.Step(view, executor);
    return Expect(machine.Result().reason == "missing" && machine.Result().slot == 10 && executor.sent.empty(), "a later missing slot prevents the available main hand from submitting") && ok;
}

bool TestOrderMovementAndImmutableIntent()
{
    auto view                         = EmptyView();
    view.inventory.bag[1]             = { EquipmentSlotState::Item, 11, 100 };
    view.inventory.bag[2]             = { EquipmentSlotState::Item, 22, 200 };
    view.inventory.equipment.slots[1] = { 1, EquipmentSlotState::Item, 22, 200 };
    auto intent                       = Intent(EmptyView());
    intent.slots[0]                   = { 0, EquipmentSlotState::Item, 11, 100 };
    intent.slots[7]                   = { 10, EquipmentSlotState::Item, 22, 200 };
    GearsetsTransaction machine;
    FakeExecutor        executor;
    machine.Begin(intent, view, executor);
    bool ok             = Expect(!machine.Begin(Intent(view), view, executor), "another click cannot replace active intent");
    intent.slots[0].uid = 99;
    machine.Step(view, executor);
    ok = Expect(executor.sent.size() == 1 && executor.sent[0].target.uid == 11 && executor.sent[0].target.slot == 0, "immutable request sends main hand first") && ok;
    machine.Step(view, executor);
    ok = Expect(executor.sent.size() == 1 && machine.Result().phase == GearsetsPhase::Applying, "submission does not count as confirmation") && ok;
    executor.Confirm(view);
    machine.Step(view, executor);
    view.inventory.bag[9] = view.inventory.bag[2];
    view.inventory.bag[2] = { EquipmentSlotState::Empty, 0, 0 };
    machine.Step(view, executor);
    ok = Expect(executor.sent.size() == 2 && executor.sent.back().target.slot == 1 && executor.sent.back().target.state == EquipmentSlotState::Empty && !executor.sent.back().bagSlot, "explicit Empty submits a sequential unequip") && ok;
    executor.Confirm(view);
    machine.Step(view, executor);
    machine.Step(view, executor);
    ok = Expect(executor.sent.size() == 3 && executor.sent.back().bagSlot == 9, "later operation resolves moved UID again") && ok;
    executor.Confirm(view);
    machine.Step(view, executor);
    machine.Step(view, executor);
    return Expect(machine.Result().phase == GearsetsPhase::Complete && machine.Result().changed[0] && machine.Result().changed[1] && machine.Result().changed[7], "only fresh full observed equipment completes") && ok;
}

bool TestClassTransition()
{
    auto view                         = EmptyView();
    view.jobId                        = 16;
    view.inventory.bag[1]             = { EquipmentSlotState::Item, 11, 100 };
    view.inventory.bag[2]             = { EquipmentSlotState::Item, 22, 200 };
    view.inventory.equipment.slots[7] = { 10, EquipmentSlotState::Item, 22, 200 };
    auto intent                       = Intent(view);
    intent.slots[0]                   = { 0, EquipmentSlotState::Item, 11, 100 };
    FakeExecutor executor;
    executor.settleMs = 20;
    GearsetsTransaction machine;
    machine.Begin(intent, view, executor);
    machine.Step(view, executor);
    view.classId                      = 4;
    view.jobId                        = 0;
    view.inventory.equipment.slots[7] = { 10, EquipmentSlotState::Empty, 0, 0 };
    machine.Step(view, executor);
    bool ok = Expect(machine.Active() && executor.sent.size() == 1, "an unpredicted class change and cleared slot may precede the main hand");
    executor.Confirm(view);
    machine.Step(view, executor);
    machine.Step(view, executor);
    ok                    = Expect(machine.Active() && executor.sent.size() == 1 && machine.Result().reason == "settling", "a confirmed main hand settles before anything else is sent") && ok;
    view.inventory.bag[8] = view.inventory.bag[2];
    view.inventory.bag[2] = { EquipmentSlotState::Empty, 0, 0 };
    view.nowMs += 20;
    machine.Step(view, executor);
    ok = Expect(executor.sent.size() == 2 && executor.sent.back().target.slot == 10 && executor.sent.back().bagSlot == 8 && executor.checks >= 3, "the observed class is accepted and remaining slots, including a cleared one, are re-evaluated") && ok;
    executor.Confirm(view);
    machine.Step(view, executor);
    machine.Step(view, executor);
    ok = Expect(machine.Result().phase == GearsetsPhase::Complete && view.classId == 4 && view.jobId == 0, "the set completes in the observed class; saved job metadata never requests a job change") && ok;

    view                  = EmptyView();
    view.inventory.bag[2] = { EquipmentSlotState::Item, 22, 200 };
    intent                = Intent(view);
    intent.slots[7]       = { 10, EquipmentSlotState::Item, 22, 200 };
    GearsetsTransaction other;
    other.Begin(intent, view, executor);
    other.Step(view, executor);
    view.classId = 5;
    other.Step(view, executor);
    return Expect(other.Result().phase == GearsetsPhase::Unconfirmed && other.Result().reason == "external-change", "a class change without a main-hand operation stops the transaction") && ok;
}

bool TestLateClassChange()
{
    auto view             = EmptyView();
    view.inventory.bag[1] = { EquipmentSlotState::Item, 11, 100 };
    view.inventory.bag[2] = { EquipmentSlotState::Item, 22, 200 };
    auto intent           = Intent(view);
    intent.slots[0]       = { 0, EquipmentSlotState::Item, 11, 100 };
    intent.slots[7]       = { 10, EquipmentSlotState::Item, 22, 200 };
    FakeExecutor executor;
    executor.settleMs = 20;
    GearsetsTransaction machine;
    machine.Begin(intent, view, executor);
    machine.Step(view, executor);
    executor.Confirm(view);
    machine.Step(view, executor);
    view.nowMs += 20;
    machine.Step(view, executor);
    bool ok = Expect(executor.sent.size() == 2 && executor.sent.back().target.slot == 10, "the next slot is sent once the main hand settles");
    // The class of the new main hand arrives after the settling window,
    // while another request is pending, and clears that slot afterward.
    view.classId = 4;
    executor.Confirm(view);
    machine.Step(view, executor);
    ok = Expect(machine.Active(), "a late class change from this transaction's main hand is not an external change") && ok;
    view.nowMs += 5;
    view.inventory.equipment.slots[7] = { 10, EquipmentSlotState::Empty, 0, 0 };
    machine.Step(view, executor);
    ok = Expect(machine.Active() && machine.Result().reason == "settling" && executor.sent.size() == 2, "a late class change settles once the pending request confirms") && ok;
    view.nowMs += 20;
    machine.Step(view, executor);
    ok = Expect(executor.sent.size() == 3 && executor.sent.back().target.slot == 10, "a slot cleared by the late class change is equipped again") && ok;
    executor.Confirm(view);
    machine.Step(view, executor);
    machine.Step(view, executor);
    ok = Expect(machine.Result().phase == GearsetsPhase::Complete && machine.Result().reason == "observed-match", "the set completes after a late class change") && ok;

    view                  = EmptyView();
    view.inventory.bag[1] = { EquipmentSlotState::Item, 11, 100 };
    view.inventory.bag[2] = { EquipmentSlotState::Item, 22, 200 };
    GearsetsTransaction other;
    other.Begin(intent, view, executor);
    other.Step(view, executor);
    executor.Confirm(view);
    view.inventory.equipment.slots[7] = { 10, EquipmentSlotState::Item, 22, 200 };
    other.Step(view, executor);
    ok = Expect(other.Active() && other.Result().reason == "settling", "an unexplained change that leaves the whole set worn still settles the main hand") && ok;
    view.nowMs += 20;
    other.Step(view, executor);
    ok = Expect(other.Result().phase == GearsetsPhase::Complete && other.Result().reason == "observed-match", "the whole set worn then completes") && ok;

    view                  = EmptyView();
    view.inventory.bag[1] = { EquipmentSlotState::Item, 11, 100 };
    view.inventory.bag[2] = { EquipmentSlotState::Item, 22, 200 };
    GearsetsTransaction third;
    third.Begin(intent, view, executor);
    third.Step(view, executor);
    executor.Confirm(view);
    view.inventory.equipment.slots[1] = { 1, EquipmentSlotState::Item, 22, 200 };
    third.Step(view, executor);
    return Expect(third.Result().phase == GearsetsPhase::Partial && third.Result().reason == "external-change" && third.Result().slot == std::optional<std::uint8_t>(1), "an unexplained change names its slot") && ok;
}

bool TestDeferred()
{
    auto view             = EmptyView();
    view.inventory.bag[1] = { EquipmentSlotState::Item, 11, 100 };
    auto intent           = Intent(view);
    intent.slots[0]       = { 0, EquipmentSlotState::Item, 11, 100 };
    FakeExecutor executor;
    executor.busy = true;
    GearsetsTransaction machine;
    machine.Begin(intent, view, executor);
    machine.Step(view, executor);
    view.nowMs = 49;
    machine.Step(view, executor);
    bool ok       = Expect(machine.Active() && machine.Result().reason == "waiting-native" && machine.Result().submissions == 0, "a deferred submission waits without counting");
    executor.busy = false;
    machine.Step(view, executor);
    ok = Expect(executor.sent.size() == 1 && machine.Result().submissions == 1 && machine.Result().phase == GearsetsPhase::Applying, "a ready route then submits") && ok;

    view.nowMs    = 0;
    executor.busy = true;
    GearsetsTransaction stuck;
    stuck.Begin(intent, view, executor);
    stuck.Step(view, executor);
    view.nowMs    = 50;
    executor.busy = false;
    stuck.Step(view, executor);
    return Expect(stuck.Result().phase == GearsetsPhase::Rejected && stuck.Result().reason == "native-not-ready" && stuck.Result().submissions == 0 && executor.sent.size() == 1, "a route that reopens only after the deadline sends nothing") && ok;
}

bool TestCancelWhileSettling()
{
    bool ok = true;
    for (const bool beforeConfirmation : { true, false })
    {
        auto view                         = EmptyView();
        view.inventory.bag[1]             = { EquipmentSlotState::Item, 11, 100 };
        view.inventory.bag[2]             = { EquipmentSlotState::Item, 22, 200 };
        view.inventory.equipment.slots[7] = { 10, EquipmentSlotState::Item, 22, 200 };
        auto intent                       = Intent(view);
        intent.slots[0]                   = { 0, EquipmentSlotState::Item, 11, 100 };
        FakeExecutor executor;
        executor.settleMs = 20;
        GearsetsTransaction machine;
        machine.Begin(intent, view, executor);
        machine.Step(view, executor);
        if (beforeConfirmation)
            machine.Cancel();
        executor.Confirm(view);
        machine.Step(view, executor);
        if (!beforeConfirmation)
            machine.Cancel();
        ok                                = Expect(machine.Active() && machine.Result().reason == "cancelling", "a cancel waits for the main hand to settle") && ok;
        ok                                = Expect(!machine.Begin(intent, view, executor), "no other Equip begins while settling") && ok;
        view.classId                      = 4;
        view.inventory.equipment.slots[7] = { 10, EquipmentSlotState::Empty, 0, 0 };
        machine.Step(view, executor);
        view.nowMs += 20;
        machine.Step(view, executor);
        ok = Expect(machine.Result().phase == GearsetsPhase::Partial && machine.Result().reason == "cancelled" && machine.Result().changed[0] && machine.Result().changed[7] && executor.sent.size() == 1,
                    "the cancelled transaction accounts for a late dependent clear and sends nothing more") &&
             ok;
    }
    return ok;
}

bool TestStops()
{
    bool ok = true;
    for (int mode = 0; mode < 10; ++mode)
    {
        auto view             = EmptyView();
        view.inventory.bag[1] = { EquipmentSlotState::Item, 11, 100 };
        view.inventory.bag[2] = { EquipmentSlotState::Item, 22, 200 };
        auto intent           = Intent(view);
        intent.slots[0]       = { 0, EquipmentSlotState::Item, 11, 100 };
        intent.slots[7]       = { 10, EquipmentSlotState::Item, 22, 200 };
        FakeExecutor        executor;
        GearsetsTransaction machine;
        machine.Begin(intent, view, executor);
        if (mode == 0)
        {
            machine.Cancel();
            machine.Step(view, executor);
            ok = Expect(machine.Result().phase == GearsetsPhase::Cancelled && executor.sent.empty(), "cancel before submit sends nothing") && ok;
            continue;
        }
        machine.Step(view, executor);
        if (mode == 1)
        {
            machine.Cancel();
            executor.Confirm(view);
            machine.Step(view, executor);
            ok = Expect(machine.Result().phase == GearsetsPhase::Partial && machine.Result().changed[0] && executor.sent.size() == 1, "cancel accounts for the accepted request and stops unsent operations") && ok;
        }
        else if (mode == 2)
        {
            view.nowMs = 50;
            machine.Step(view, executor);
            ok = Expect(machine.Result().phase == GearsetsPhase::Unconfirmed && executor.sent.size() == 1, "missing confirmation is unconfirmed, never complete") && ok;
        }
        else if (mode == 3)
        {
            ++view.session;
            machine.Step(view, executor);
            ok = Expect(machine.Result().phase == GearsetsPhase::Unconfirmed && executor.sent.size() == 1, "session replacement stops an accepted request") && ok;
        }
        else if (mode == 7)
        {
            executor.Confirm(view);
            machine.Step(view, executor);
            machine.Step(view, executor);
            view.nowMs += 50;
            machine.Step(view, executor);
            ok = Expect(machine.Result().phase == GearsetsPhase::Unconfirmed && machine.Result().changed[0] && executor.sent.size() == 2, "unconfirmed later operation retains observed partial progress") && ok;
        }
        else if (mode == 8)
        {
            executor.Confirm(view);
            view.inventory.equipment.slots[1] = { 1, EquipmentSlotState::Item, 22, 200 };
            machine.Step(view, executor);
            ok = Expect(machine.Result().phase == GearsetsPhase::Partial && machine.Result().reason == "external-change" && executor.sent.size() == 1, "observed confirmation plus conflicting changes is partial") && ok;
        }
        else if (mode == 9)
        {
            machine.EndSession();
            ok = Expect(machine.Result().phase == GearsetsPhase::Unconfirmed && executor.sent.size() == 1, "unload or fault stops an in-flight transaction") && ok;
        }
        else
        {
            executor.Confirm(view);
            machine.Step(view, executor);
            if (mode == 4)
                executor.reject = true;
            if (mode == 5)
                view.inventory.equipment.slots[1] = { 1, EquipmentSlotState::Item, 22, 200 };
            if (mode == 6)
                executor.restricted = true;
            machine.Step(view, executor);
            ok = Expect(machine.Result().phase == GearsetsPhase::Partial && machine.Result().changed[0] &&
                            machine.Result().reason == (mode == 4 ? "native-rejected" : mode == 5 ? "external-change"
                                                                                                  : "restricted"),
                        "failure after progress preserves partial accounting") &&
                 ok;
        }
        const auto count = executor.sent.size();
        machine.Step(view, executor);
        ok = Expect(executor.sent.size() == count, "terminal outcomes never retry") && ok;
    }
    auto                        view = EmptyView();
    GearsetsTransaction         machine;
    UnavailableGearsetsExecutor executor;
    machine.Begin(Intent(view), view, executor);
    machine.Step(view, executor);
    return Expect(machine.Result().phase == GearsetsPhase::Rejected && machine.Result().reason == "executor-unavailable" && machine.Result().submissions == 0, "production executor stays unavailable even for a matching set") && ok;
}

} // namespace

bool RunGearsetsTransactionTests()
{
    return TestPreflight() && TestOrderMovementAndImmutableIntent() && TestClassTransition() && TestLateClassChange() && TestDeferred() && TestCancelWhileSettling() && TestStops();
}
