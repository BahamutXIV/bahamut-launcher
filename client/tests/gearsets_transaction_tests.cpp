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
    return { view.session, view.inventory.equipment.slots, std::nullopt };
}

class FakeExecutor final : public GearsetsExecutor
{
public:
    std::vector<GearsetsOperation> sent;
    std::vector<std::uint8_t>      jobs;
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

    GearsetsSubmission SubmitJob(std::uint8_t mode) override
    {
        if (busy)
            return GearsetsSubmission::Deferred;
        jobs.push_back(mode);
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
    ok = Expect(machine.Result().phase == GearsetsPhase::Complete && view.classId == 4 && view.jobId == 0 && executor.jobs.empty(), "the set completes in the observed class; a set naming no job never requests a job change") && ok;

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

// Class 4 wears nothing; the set's main hand 11 is class 3's, with body 22.
struct JobCase
{
    GearsetsView        view = EmptyView();
    GearsetIntent       intent;
    FakeExecutor        executor;
    GearsetsTransaction machine;

    explicit JobCase(std::optional<std::uint8_t> job)
    {
        view.classId          = 4;
        view.inventory.bag[1] = { EquipmentSlotState::Item, 11, 100 };
        view.inventory.bag[2] = { EquipmentSlotState::Item, 22, 200 };
        intent                = Intent(view);
        intent.slots[0]       = { 0, EquipmentSlotState::Item, 11, 100 };
        intent.slots[7]       = { 10, EquipmentSlotState::Item, 22, 200 };
        intent.job            = job;
        executor.settleMs     = 20;
    }

    void MainHand()
    {
        machine.Begin(intent, view, executor);
        machine.Step(view, executor);
        view.classId = 3;
        executor.Confirm(view);
        machine.Step(view, executor);
        view.nowMs += 20;
    }

    void Job(std::uint8_t job)
    {
        view.jobId = job;
        ++view.nowMs;
    }

    void Body()
    {
        executor.Confirm(view);
        machine.Step(view, executor);
        machine.Step(view, executor);
    }
};

bool TestJob()
{
    bool ok = true;
    {
        JobCase take(16);
        take.MainHand();
        take.machine.Step(take.view, take.executor);
        ok = Expect(take.executor.jobs == std::vector<std::uint8_t>{ 1 } && take.executor.sent.size() == 1 && take.machine.Result().reason == "awaiting-job",
                    "a job is taken after the main hand settles and before the other slots") &&
             ok;
        take.Job(16);
        take.machine.Step(take.view, take.executor);
        take.machine.Step(take.view, take.executor);
        ok = Expect(take.executor.sent.size() == 2 && take.executor.sent.back().target.slot == 10, "the other slots follow the confirmed job") && ok;
        take.Body();
        ok = Expect(take.machine.Result().phase == GearsetsPhase::Complete && take.executor.jobs.size() == 1, "the set completes in its job") && ok;
    }
    {
        JobCase retry(16);
        retry.MainHand();
        retry.machine.Step(retry.view, retry.executor);
        retry.view.nowMs += 50;
        retry.machine.Step(retry.view, retry.executor);
        ok = Expect(retry.machine.Active() && retry.executor.jobs.size() == 1, "a refused first attempt does not stop the set") && ok;
        retry.machine.Step(retry.view, retry.executor);
        ok = Expect(retry.executor.sent.size() == 2 && retry.executor.sent.back().target.slot == 10, "the slots follow a refused first attempt") && ok;
        retry.Body();
        ok = Expect(retry.executor.jobs == std::vector<std::uint8_t>{ 1, 1 } && retry.machine.Result().reason == "awaiting-job", "the job is tried again once the slots are worn") && ok;
        retry.Job(16);
        retry.machine.Step(retry.view, retry.executor);
        retry.machine.Step(retry.view, retry.executor);
        ok = Expect(retry.machine.Result().phase == GearsetsPhase::Complete, "the second attempt completes the set") && ok;
    }
    {
        JobCase refused(16);
        refused.MainHand();
        refused.machine.Step(refused.view, refused.executor);
        refused.view.nowMs += 50;
        refused.machine.Step(refused.view, refused.executor);
        refused.machine.Step(refused.view, refused.executor);
        refused.Body();
        refused.view.nowMs += 50;
        refused.machine.Step(refused.view, refused.executor);
        ok               = Expect(refused.machine.Result().phase == GearsetsPhase::Partial && refused.machine.Result().reason == "job-refused" && refused.executor.jobs.size() == 2,
                                  "two refusals stop with the slots already changed") &&
                           ok;
        const auto count = refused.executor.jobs.size();
        refused.machine.Step(refused.view, refused.executor);
        ok = Expect(refused.executor.jobs.size() == count, "a refused job is not retried again") && ok;
    }
    {
        JobCase late(16);
        late.MainHand();
        late.machine.Step(late.view, late.executor);
        late.view.nowMs += 50;
        late.machine.Step(late.view, late.executor);
        late.machine.Step(late.view, late.executor);
        late.Job(16);
        late.Body();
        ok = Expect(late.machine.Result().phase == GearsetsPhase::Complete && late.executor.jobs.size() == 1, "a late job confirmation completes without another attempt") && ok;
    }
    for (const bool cancelled : { false, true })
    {
        JobCase leave(0);
        leave.view.classId                      = 3;
        leave.view.jobId                        = 16;
        leave.view.inventory.equipment.slots[0] = leave.intent.slots[0];
        leave.view.inventory.equipment.slots[7] = leave.intent.slots[7];
        leave.view.inventory.bag[1]             = { EquipmentSlotState::Empty, 0, 0 };
        leave.view.inventory.bag[2]             = { EquipmentSlotState::Empty, 0, 0 };
        leave.machine.Begin(leave.intent, leave.view, leave.executor);
        leave.machine.Step(leave.view, leave.executor);
        ok = Expect(leave.executor.jobs == std::vector<std::uint8_t>{ 2 } && leave.executor.sent.empty(), "a class set leaves the job with mode 2") && ok;
        if (cancelled)
            leave.machine.Cancel();
        leave.Job(0);
        leave.machine.Step(leave.view, leave.executor);
        leave.machine.Step(leave.view, leave.executor);
        ok = Expect(leave.machine.Result().phase == (cancelled ? GearsetsPhase::Partial : GearsetsPhase::Complete),
                    cancelled ? "a cancel after the only change, a job's, is partial" : "leaving the job completes the worn set") &&
             ok;
    }
    {
        JobCase worn(16);
        worn.view.inventory.equipment.slots[7] = worn.intent.slots[7];
        worn.view.inventory.bag[2]             = { EquipmentSlotState::Empty, 0, 0 };
        worn.MainHand();
        worn.machine.Step(worn.view, worn.executor);
        worn.view.nowMs += 50;
        worn.machine.Step(worn.view, worn.executor);
        worn.machine.Step(worn.view, worn.executor);
        ok = Expect(worn.machine.Result().phase == GearsetsPhase::Partial && worn.machine.Result().reason == "job-refused" && worn.executor.jobs.size() == 1,
                    "a refusal with no slot left to replace is not retried") &&
             ok;
    }
    for (const bool arrives : { true, false })
    {
        JobCase lateClass(16);
        lateClass.machine.Begin(lateClass.intent, lateClass.view, lateClass.executor);
        lateClass.machine.Step(lateClass.view, lateClass.executor);
        lateClass.executor.Confirm(lateClass.view);
        lateClass.machine.Step(lateClass.view, lateClass.executor);
        lateClass.view.nowMs += 20;
        lateClass.machine.Step(lateClass.view, lateClass.executor);
        ok = Expect(lateClass.machine.Active() && lateClass.machine.Result().reason == "waiting-class" && lateClass.executor.jobs.empty(), "a job waits for the main hand's class") && ok;
        if (!arrives)
        {
            lateClass.view.nowMs += 50;
            lateClass.machine.Step(lateClass.view, lateClass.executor);
            ok = Expect(lateClass.machine.Result().phase == GearsetsPhase::Partial && lateClass.machine.Result().reason == "job-class-mismatch" && lateClass.executor.jobs.empty(),
                        "a class that never arrives stops without a job request") &&
                 ok;
            continue;
        }
        lateClass.view.classId = 3;
        lateClass.view.nowMs += 5;
        lateClass.machine.Step(lateClass.view, lateClass.executor);
        ok = Expect(lateClass.machine.Active() && lateClass.machine.Result().reason == "settling", "the late class settles like a main hand") && ok;
        lateClass.view.nowMs += 20;
        lateClass.machine.Step(lateClass.view, lateClass.executor);
        ok = Expect(lateClass.executor.jobs == std::vector<std::uint8_t>{ 1 } && lateClass.executor.sent.size() == 1, "the job follows the late class") && ok;
        lateClass.Job(16);
        lateClass.machine.Step(lateClass.view, lateClass.executor);
        lateClass.machine.Step(lateClass.view, lateClass.executor);
        lateClass.Body();
        ok = Expect(lateClass.machine.Result().phase == GearsetsPhase::Complete, "the set completes after a late class") && ok;
    }
    {
        JobCase mismatch(27);
        mismatch.view.inventory.equipment.slots[0] = mismatch.intent.slots[0];
        mismatch.view.inventory.bag[1]             = { EquipmentSlotState::Empty, 0, 0 };
        mismatch.machine.Begin(mismatch.intent, mismatch.view, mismatch.executor);
        mismatch.machine.Step(mismatch.view, mismatch.executor);
        ok = Expect(mismatch.machine.Result().phase == GearsetsPhase::Rejected && mismatch.machine.Result().reason == "job-class-mismatch" && mismatch.executor.jobs.empty(),
                    "a job of another class sends nothing") &&
             ok;
        JobCase invalid(5);
        invalid.machine.Begin(invalid.intent, invalid.view, invalid.executor);
        ok = Expect(invalid.machine.Result().reason == "invalid-intent", "an ID that is not a job is refused") && ok;
    }
    {
        JobCase cancel(16);
        cancel.MainHand();
        cancel.machine.Step(cancel.view, cancel.executor);
        cancel.machine.Cancel();
        ok = Expect(cancel.machine.Active() && cancel.machine.Result().reason == "cancelling", "a cancel waits for a pending job") && ok;
        cancel.Job(16);
        cancel.machine.Step(cancel.view, cancel.executor);
        ok = Expect(cancel.machine.Result().phase == GearsetsPhase::Partial && cancel.machine.Result().reason == "cancelled" && cancel.executor.sent.size() == 1,
                    "the cancelled set accounts for the job and sends nothing more") &&
             ok;
    }
    return ok;
}

} // namespace

bool RunGearsetsTransactionTests()
{
    return TestPreflight() && TestOrderMovementAndImmutableIntent() && TestClassTransition() && TestLateClassChange() && TestDeferred() && TestCancelWhileSettling() && TestStops() &&
           TestJob();
}
