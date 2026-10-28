#include "equipment_memory_fixture.h"
#include "equipment_state.h"
#include "gearsets_debug.h"
#include "gearsets_debug_service.h"
#include "runtime_contract.h"

#include <chrono>
#include <filesystem>
#include <fstream>
#include <future>
#include <iostream>
#include <optional>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

namespace
{

namespace fs = std::filesystem;
using namespace bahamut_client;
using namespace equipment_memory_fixture;

constexpr std::uint64_t kUidA = 0x1200000002ull;
constexpr std::uint64_t kUidB = 0x1200000009ull;

bool Expect(bool condition, const char* message)
{
    if (!condition)
    {
        std::cerr << message << '\n';
        return false;
    }
    return true;
}

BagItem Item(std::uint64_t uid, std::uint32_t catalog = 8030423u)
{
    return { EquipmentSlotState::Item, uid, catalog };
}

// A complete snapshot with every supported slot empty except one.
EquipmentSnapshot Holding(std::uint8_t position, std::uint64_t uid)
{
    EquipmentSnapshot snapshot;
    snapshot.complete = true;
    for (std::size_t index = 0; index < kEquipmentSlotCount; ++index)
    {
        snapshot.slots[index].slot  = kEquipmentSlots[index];
        snapshot.slots[index].state = EquipmentSlotState::Empty;
        if (kEquipmentSlots[index] == position)
        {
            snapshot.slots[index] = { position, EquipmentSlotState::Item, uid, 8030423u };
        }
    }
    return snapshot;
}

struct Frames
{
    std::optional<BagItem> bag             = Item(kUidA);
    int                    invocations     = 0;
    bool                   invokeAvailable = true;
    GearsetsInvokeResult   result          = GearsetsInvokeResult::ReturnedTrue;

    GearsetsFrameView View(std::uint64_t frame, std::uint64_t nowMs, std::uint64_t generation, const EquipmentSnapshot* snapshot = nullptr)
    {
        GearsetsFrameView view;
        view.frame      = frame;
        view.nowMs      = nowMs;
        view.generation = generation;
        view.snapshot   = snapshot;
        view.readBag    = [this](std::uint16_t)
        {
            return bag;
        };
        if (invokeAvailable)
        {
            view.invoke = [this](std::uint16_t, std::uint8_t)
            {
                ++invocations;
                return result;
            };
        }
        return view;
    }
};

GearsetsEquipRequest Request(std::uint64_t generation, std::optional<std::uint64_t> uid = kUidA, std::uint8_t equipSlot = 11u)
{
    return { generation, 1u, equipSlot, uid };
}

bool IsFinal(const GearsetsEquipMachine& machine, GearsetsEquipPhase phase, GearsetsEquipReason reason)
{
    const auto status = machine.Status();
    return status && status->phase == phase && status->reason == reason && !machine.Active();
}

bool TestSwitchesDefaultOff()
{
    const auto none     = ParseGearsetsDebugSwitches(std::nullopt, std::nullopt);
    const auto relative = ParseGearsetsDebugSwitches(std::wstring(L"receipt.csv"), std::wstring(L"true"));
    const auto zero     = ParseGearsetsDebugSwitches(std::wstring(L""), std::wstring(L"0"));
    const auto on       = ParseGearsetsDebugSwitches(std::wstring(L"C:\\receipts\\gearsets.csv"), std::wstring(L"1"));
    return Expect(!none.Any() && none.snapshotFile.empty() && !none.equip, "no environment means every switch is off") &&
           Expect(!relative.Any(), "a relative receipt path and a non-1 equip value stay off") &&
           Expect(!zero.Any(), "empty and zero values stay off") &&
           Expect(on.equip && on.snapshotFile == fs::path(L"C:\\receipts\\gearsets.csv"), "an absolute path and 1 turn the switches on");
}

bool TestSubmitGates()
{
    GearsetsEquipMachine machine;
    const auto           disabled  = machine.Submit(Request(1), false, 0);
    const auto           slotZero  = machine.Submit(Request(1, kUidA, 0u), true, 0);
    const auto           slotThree = machine.Submit(Request(1, kUidA, 3u), true, 0); // position 2 is not supported
    const auto           zeroUid   = machine.Submit(Request(1, 0u), true, 0);
    const auto           first     = machine.Submit(Request(1), true, 0);
    const auto           second    = machine.Submit(Request(1), true, 0);
    bool                 ok        = Expect(!disabled.accepted && disabled.reason == GearsetsEquipReason::Disabled, "a disabled machine refuses") &&
                                     Expect(!slotZero.accepted && slotZero.reason == GearsetsEquipReason::InvalidSlot && !slotThree.accepted && slotThree.reason == GearsetsEquipReason::InvalidSlot, "unsupported equip slots are refused") &&
                                     Expect(!zeroUid.accepted && zeroUid.reason == GearsetsEquipReason::InvalidUid, "a zero uid is refused") &&
                                     Expect(first.accepted && first.id == 1u, "the first valid request is accepted") &&
                                     Expect(!second.accepted && second.reason == GearsetsEquipReason::Busy && machine.Status()->id == 1u, "a second request while one is active is refused, not queued");
    Frames               frames;
    frames.bag = std::nullopt;
    machine.Step(frames.View(1, 0, 1));
    const auto third = machine.Submit(Request(1), true, 2);
    return Expect(ok, "submit gates") && Expect(third.accepted && third.id == 2u, "a finished transaction frees the slot for the next request");
}

bool TestStaleGenerationRefused()
{
    GearsetsEquipMachine machine;
    Frames               frames;
    machine.Submit(Request(1), true, 0);
    machine.Step(frames.View(1, 0, 2));
    return Expect(IsFinal(machine, GearsetsEquipPhase::Rejected, GearsetsEquipReason::StaleGeneration), "a request from an older generation is rejected") &&
           Expect(frames.invocations == 0 && machine.Status()->invocations == 0, "a stale request never invokes");
}

bool TestUidMismatchRefused()
{
    GearsetsEquipMachine machine;
    Frames               frames;
    frames.bag = Item(kUidB);
    machine.Submit(Request(1, kUidA), true, 0);
    machine.Step(frames.View(1, 0, 1));
    return Expect(IsFinal(machine, GearsetsEquipPhase::Rejected, GearsetsEquipReason::UidMismatch), "a different item in the bag slot is rejected") &&
           Expect(frames.invocations == 0 && machine.Status()->invocations == 0, "a uid mismatch never invokes");
}

bool TestCaptureThenRecheck()
{
    bool ok = true;
    {
        GearsetsEquipMachine machine;
        Frames               frames;
        machine.Submit(Request(1, std::nullopt), true, 0);
        machine.Step(frames.View(1, 0, 1));
        ok         = Expect(machine.Status()->phase == GearsetsEquipPhase::Resolved && machine.Status()->expectedUid == kUidA && frames.invocations == 0, "without a uid the first frame only captures it") && ok;
        frames.bag = Item(kUidB);
        machine.Step(frames.View(2, 16, 1));
        ok = Expect(IsFinal(machine, GearsetsEquipPhase::Rejected, GearsetsEquipReason::UidMismatch) && frames.invocations == 0, "an item moved between capture and recheck is rejected without invoking") && ok;
    }
    {
        GearsetsEquipMachine machine;
        Frames               frames;
        machine.Submit(Request(1, std::nullopt), true, 0);
        machine.Step(frames.View(1, 0, 1));
        machine.Step(frames.View(2, 16, 1));
        const auto status = machine.Status();
        ok                = Expect(status->phase == GearsetsEquipPhase::Invoked && frames.invocations == 1 && status->invocations == 1 && status->invokeFrame == 2 && status->returnValue == true, "a stable uid is invoked once on the recheck frame") && ok;
    }
    return ok;
}

bool TestConfirmationAndTimeout()
{
    bool ok = true;
    {
        GearsetsEquipMachine machine;
        Frames               frames;
        machine.Submit(Request(1), true, 0);
        machine.Step(frames.View(1, 1000, 1));
        const EquipmentSnapshot before = Holding(10, kUidB);
        machine.Step(frames.View(2, 1100, 1, &before));
        ok                            = Expect(machine.Active() && machine.Status()->phase == GearsetsEquipPhase::Invoked, "another item in the slot does not confirm") && ok;
        const EquipmentSnapshot after = Holding(10, kUidA);
        machine.Step(frames.View(3, 1200, 1, &after));
        ok = Expect(IsFinal(machine, GearsetsEquipPhase::Complete, GearsetsEquipReason::Confirmed) && machine.Status()->finishFrame == 3 && frames.invocations == 1, "the requested uid in the requested slot completes") && ok;
    }
    {
        GearsetsEquipMachine machine;
        Frames               frames;
        machine.Submit(Request(1), true, 0);
        machine.Step(frames.View(1, 1000, 1));
        machine.Step(frames.View(2, 1000 + GearsetsEquipMachine::kConfirmTimeoutMs - 1, 1));
        ok = Expect(machine.Active(), "confirmation waits until the deadline") && ok;
        machine.Step(frames.View(3, 1000 + GearsetsEquipMachine::kConfirmTimeoutMs, 1));
        ok = Expect(IsFinal(machine, GearsetsEquipPhase::Unconfirmed, GearsetsEquipReason::Timeout) && frames.invocations == 1, "no observed change by the deadline is unconfirmed, not success") && ok;
    }
    return ok;
}

bool TestOtherOutcomes()
{
    bool ok = true;
    {
        GearsetsEquipMachine machine;
        Frames               frames;
        frames.result = GearsetsInvokeResult::ReturnedFalse;
        machine.Submit(Request(1), true, 0);
        machine.Step(frames.View(1, 0, 1));
        ok = Expect(IsFinal(machine, GearsetsEquipPhase::Rejected, GearsetsEquipReason::BindingFalse) && machine.Status()->returnValue == false && frames.invocations == 1, "a false return is a local rejection") && ok;
    }
    {
        GearsetsEquipMachine machine;
        Frames               frames;
        frames.invokeAvailable = false;
        machine.Submit(Request(1), true, 0);
        machine.Step(frames.View(1, 0, 1));
        ok = Expect(IsFinal(machine, GearsetsEquipPhase::Refused, GearsetsEquipReason::InvokeUnavailable) && machine.Status()->invocations == 0 && !machine.Status()->returnValue, "without an invocation the request is refused after its checks") && ok;
    }
    {
        GearsetsEquipMachine    machine;
        Frames                  frames;
        const EquipmentSnapshot equipped = Holding(10, kUidA);
        machine.Submit(Request(1), true, 0);
        machine.Step(frames.View(1, 0, 1, &equipped));
        ok = Expect(IsFinal(machine, GearsetsEquipPhase::Complete, GearsetsEquipReason::AlreadyEquipped) && frames.invocations == 0, "an already matching slot completes without invoking") && ok;
    }
    {
        GearsetsEquipMachine machine;
        Frames               frames;
        frames.bag = BagItem{ EquipmentSlotState::Empty, 0, 0 };
        machine.Submit(Request(1), true, 0);
        machine.Step(frames.View(1, 0, 1));
        ok = Expect(IsFinal(machine, GearsetsEquipPhase::Rejected, GearsetsEquipReason::BagEmpty), "an empty bag slot is rejected") && ok;
        machine.Submit(Request(1), true, 2);
        frames.bag = BagItem{};
        machine.Step(frames.View(3, 0, 1));
        ok = Expect(IsFinal(machine, GearsetsEquipPhase::Rejected, GearsetsEquipReason::BagUnavailable) && frames.invocations == 0, "an unreadable bag slot is rejected") && ok;
    }
    {
        GearsetsEquipMachine machine;
        Frames               frames;
        machine.Submit(Request(1), true, 0);
        ok = Expect(machine.EndSession(1) && IsFinal(machine, GearsetsEquipPhase::Rejected, GearsetsEquipReason::SessionEnded), "an unsent request ends with the session") && ok;
        machine.Submit(Request(1), true, 2);
        machine.Step(frames.View(3, 0, 1));
        ok = Expect(machine.EndSession(4) && IsFinal(machine, GearsetsEquipPhase::Unconfirmed, GearsetsEquipReason::SessionEnded), "an invoked request ending with the session is unconfirmed") && ok;
        ok = Expect(!machine.EndSession(5), "ending again changes nothing") && ok;
        machine.Submit(Request(1), true, 6);
        machine.Step(frames.View(7, 0, 1));
        machine.Step(frames.View(8, 0, 2));
        ok = Expect(IsFinal(machine, GearsetsEquipPhase::Unconfirmed, GearsetsEquipReason::SessionEnded), "a new generation after invoking is unconfirmed") && ok;
    }
    return ok;
}

bool TestReceiptFormat()
{
    const std::string   header = FormatGearsetsReceiptHeader(bahamut_runtime_contract::kClientSha256, 42u);
    const std::string   rows   = FormatGearsetsSnapshotRecord(3, 120, Holding(10, 0xFEDCBA9876543210ull));
    GearsetsEquipStatus status;
    status.id                = 7;
    status.phase             = GearsetsEquipPhase::Refused;
    status.reason            = GearsetsEquipReason::InvokeUnavailable;
    status.generation        = 2;
    status.bagSlot           = 1;
    status.equipSlot         = 11;
    status.expectedUid       = kUidA;
    status.catalogId         = 8030423u;
    status.submitFrame       = 5;
    status.finishFrame       = 6;
    const std::string equip  = FormatGearsetsEquipRecord(4, 6, status);
    const std::string footer = FormatGearsetsReceiptFooter(4, 1, "addon-inactive");
    std::size_t       lines  = 0;
    for (const char value : rows)
    {
        lines += value == '\n' ? 1u : 0u;
    }
    return Expect(header.starts_with("# bahamut-gearsets-receipt schema=1\n# client_sha256=9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9\n# process_id=42\n"), "the header names the schema, client, and process") &&
           Expect(lines == kEquipmentSlotCount, "a snapshot record has one row per supported slot") &&
           Expect(rows.starts_with("snapshot,3,120,0,1,0,empty,,\n") && rows.find("snapshot,3,120,0,1,10,item,fedcba9876543210,8030423\n") != std::string::npos, "rows carry slot, state, 16-digit uid, and catalog") &&
           Expect(equip == "equip,4,6,7,refused,invoke-unavailable,2,1,11,0000001200000002,8030423,0,,5,0,6\n", "an equip record carries the transaction fields") &&
           Expect(footer == "# end records=4 dropped=1 reason=addon-inactive\n", "the footer carries counts and the reason") &&
           Expect(FormatGearsetsUnavailableRecord(9, 10) == "unavailable,9,10\n", "an unavailable record");
}

std::string ReadText(const fs::path& path)
{
    std::ifstream      file(path, std::ios::binary);
    std::ostringstream text;
    text << file.rdbuf();
    return text.str();
}

std::size_t CountPrefix(const std::string& text, std::string_view prefix)
{
    std::size_t        count = 0;
    std::istringstream lines(text);
    std::string        line;
    while (std::getline(lines, line))
    {
        count += line.starts_with(prefix) ? 1u : 0u;
    }
    return count;
}

const void* PlayerOf(const Fixture& fixture)
{
    return reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player));
}

std::optional<std::uint64_t> UidAt(const std::optional<EquipmentSnapshot>& snapshot, std::uint8_t position)
{
    if (!snapshot)
    {
        return std::nullopt;
    }
    for (const auto& slot : snapshot->slots)
    {
        if (slot.slot == position && slot.state == EquipmentSlotState::Item)
        {
            return slot.uid;
        }
    }
    return std::nullopt;
}

struct FakeHost
{
    const EquipmentStateService* equipment = nullptr;
    GearsetsDebugPort*           port      = nullptr;

    void SetEquipmentStateService(const EquipmentStateService* service)
    {
        equipment = service;
    }

    void SetGearsetsDebugPort(GearsetsDebugPort* value)
    {
        port = value;
    }
};

struct FakePlayerState
{
    void (*observer)(void*, const void*) = nullptr;
    void* context                        = nullptr;

    void SetGameFrameObserver(void (*value)(void*, const void*), void* valueContext)
    {
        observer = value;
        context  = valueContext;
    }
};

bool TestInstallOnlyWithSwitch()
{
    bool ok = true;
    {
        EquipmentStateService equipment;
        GearsetsDebugService  service(&equipment);
        FakeHost              host;
        FakePlayerState       playerState;
        const bool            installed = InstallGearsetsDebug(GearsetsDebugSwitches{}, service, equipment, host, playerState);
        ok                              = Expect(!installed && host.equipment == nullptr && host.port == nullptr && playerState.observer == nullptr, "without a switch nothing is installed") && ok;
    }
    {
        EquipmentStateService equipment;
        GearsetsDebugService  service(&equipment);
        FakeHost              host;
        FakePlayerState       playerState;
        const bool            installed = InstallGearsetsDebug({ {}, true }, service, equipment, host, playerState);
        ok                              = Expect(installed && host.equipment == &equipment && host.port == &service && playerState.observer == &GearsetsDebugService::ObserveGameFrame && playerState.context == &service && service.EquipEnabled(), "a switch installs the service") && ok;
    }
    return ok;
}

bool TestServiceDefaultOff(const fs::path& scratch)
{
    Fixture fixture;
    fixture.Bag(1, kUidA, 8030423u);
    fixture.Link(10, 0, 1);
    EquipmentStateService equipment;
    GearsetsDebugService  service(&equipment);
    service.Configure(ReadGearsetsDebugSwitchesFromEnvironment());
    service.OnAddonStateChanged(true);
    service.OnRenderFrame();
    service.OnGameFrame(PlayerOf(fixture), 0);
    const auto refused = service.SubmitDebugEquip(1, 11, kUidA);
    return Expect(!service.EquipEnabled(), "the equip switch is off without the environment") &&
           Expect(!equipment.Snapshot().has_value(), "without a switch the game frame reads nothing") &&
           Expect(!refused.accepted && refused.reason == GearsetsEquipReason::Disabled && !service.DebugEquipStatus(), "without a switch equip is refused") &&
           Expect(fs::is_empty(scratch), "without a switch no receipt is written");
}

bool TestServiceRequiresAddon(const fs::path& scratch)
{
    Fixture fixture;
    fixture.Bag(1, kUidA, 8030423u);
    const fs::path        receipt = scratch / "inactive.csv";
    EquipmentStateService equipment;
    GearsetsDebugService  service(&equipment);
    service.Configure({ receipt, true });
    service.OnRenderFrame();
    service.OnGameFrame(PlayerOf(fixture), 0);
    const auto refused = service.SubmitDebugEquip(1, 11, kUidA);
    service.OnRenderFrame();
    return Expect(service.EquipEnabled(), "the switch is configured") &&
           Expect(!equipment.Snapshot().has_value(), "without the addon the game frame reads nothing") &&
           Expect(!refused.accepted && refused.reason == GearsetsEquipReason::Disabled, "without the addon equip is refused") &&
           Expect(!fs::exists(receipt), "without the addon no receipt is opened");
}

bool TestServiceReceipt(const fs::path& scratch)
{
    Fixture fixture;
    fixture.Bag(1, kUidA, 8030423u);
    fixture.Link(10, 0, 1);
    const fs::path        receipt = scratch / "receipt.csv";
    EquipmentStateService equipment;
    {
        GearsetsDebugService service(&equipment);
        service.Configure({ receipt, false });
        service.OnAddonStateChanged(true);
        service.OnRenderFrame();
        service.OnGameFrame(PlayerOf(fixture), 0);
        service.OnRenderFrame();
        service.OnGameFrame(PlayerOf(fixture), 50);  // inside the read interval
        service.OnGameFrame(PlayerOf(fixture), 200); // same content
        fixture.Link(0, 0, 1);
        service.OnGameFrame(PlayerOf(fixture), 400); // changed content
        service.OnGameFrame(nullptr, 600);           // player gone
        service.OnRenderFrame();
        const auto refused = service.SubmitDebugEquip(1, 11, kUidA);
        if (!Expect(!refused.accepted && refused.reason == GearsetsEquipReason::Disabled, "the snapshot switch alone does not enable equip"))
        {
            return false;
        }
        // A reload ends the receipt; the next session is not recorded.
        service.OnAddonStateChanged(false);
        service.OnAddonStateChanged(true);
        service.OnGameFrame(PlayerOf(fixture), 800);
        service.OnRenderFrame();
        service.OnGameFrame(PlayerOf(fixture), 1000);
        service.OnRenderFrame();
    }
    const std::string text = ReadText(receipt);
    bool              ok   = Expect(text.starts_with("# bahamut-gearsets-receipt schema=1\n"), "the receipt starts with its header") &&
                             Expect(CountPrefix(text, "snapshot,1,") == kEquipmentSlotCount && CountPrefix(text, "snapshot,2,") == kEquipmentSlotCount && CountPrefix(text, "snapshot,") == 2 * kEquipmentSlotCount, "only distinct snapshots are recorded") &&
                             Expect(text.find("snapshot,1,1,1,1,10,item,0000001200000002,8030423\n") != std::string::npos && text.find("snapshot,2,4,2,1,0,item,0000001200000002,8030423\n") != std::string::npos, "rows carry the frame, generation, and item") &&
                             Expect(CountPrefix(text, "unavailable,3,5") == 1, "losing the player records one unavailable row") &&
                             Expect(text.ends_with("# end records=3 dropped=0 reason=addon-inactive\n") && CountPrefix(text, "# end") == 1, "the receipt ends once with its footer at the first unload");

    // An existing file is never replaced.
    {
        GearsetsDebugService service(&equipment);
        service.Configure({ receipt, false });
        service.OnAddonStateChanged(true);
        service.OnRenderFrame();
        service.OnGameFrame(PlayerOf(fixture), 0);
        service.OnAddonStateChanged(false);
        service.OnRenderFrame();
    }
    return Expect(ReadText(receipt) == text, "an existing receipt is left untouched") && ok;
}

bool TestServicePlayerChange()
{
    Fixture first;
    first.Bag(1, kUidA, 8030423u);
    first.Link(10, 0, 1);
    Fixture second;
    second.Bag(1, kUidB, 4030013u);
    second.Link(10, 0, 1);
    EquipmentStateService equipment;
    GearsetsDebugService  service(&equipment);
    service.Configure({ {}, true });
    service.OnAddonStateChanged(true);
    // Every frame below is inside the 100 ms read interval.
    service.OnGameFrame(PlayerOf(first), 0);
    bool ok = Expect(UidAt(equipment.Snapshot(), 10) == kUidA, "the first player's snapshot is published");
    service.OnGameFrame(nullptr, 10);
    ok = Expect(!equipment.Snapshot().has_value(), "losing the player clears the snapshot at once") && ok;
    service.OnGameFrame(PlayerOf(second), 20);
    ok = Expect(UidAt(equipment.Snapshot(), 10) == kUidB, "a new player's snapshot is published at once") && ok;
    service.OnGameFrame(PlayerOf(first), 30);
    ok = Expect(UidAt(equipment.Snapshot(), 10) == kUidA, "a replaced player's snapshot is published at once") && ok;
    return ok;
}

struct LinkingInvoker
{
    Fixture* fixture = nullptr;
    int      calls   = 0;

    static GearsetsInvokeResult Invoke(void* context, const void*, std::uint16_t bagSlot, std::uint8_t equipSlot)
    {
        auto* self = static_cast<LinkingInvoker*>(context);
        ++self->calls;
        self->fixture->Link(static_cast<std::size_t>(equipSlot - 1u), 0, bagSlot);
        return GearsetsInvokeResult::ReturnedTrue;
    }
};

bool TestServiceEquip(const fs::path& scratch)
{
    Fixture fixture;
    fixture.Bag(1, kUidA, 8030423u);
    fixture.Bag(2, kUidB, 4030013u);
    const fs::path        receipt = scratch / "equip.csv";
    EquipmentStateService equipment;
    LinkingInvoker        invoker{ &fixture };
    GearsetsDebugService  service(&equipment);
    service.Configure({ receipt, true });
    service.SetInvoker(&LinkingInvoker::Invoke, &invoker);
    service.OnAddonStateChanged(true);
    service.OnRenderFrame();
    service.OnGameFrame(PlayerOf(fixture), 0);

    bool ok = Expect(service.SubmitDebugEquip(1, 11, kUidB).accepted, "a request is accepted while the addon is active");
    service.OnGameFrame(PlayerOf(fixture), 10);
    ok = Expect(service.DebugEquipStatus()->reason == GearsetsEquipReason::UidMismatch && invoker.calls == 0, "the service rechecks the bag uid and never invokes on a mismatch") && ok;

    ok = Expect(service.SubmitDebugEquip(1, 11, kUidA).accepted, "a matching request is accepted") && ok;
    service.OnGameFrame(PlayerOf(fixture), 20);
    ok = Expect(service.DebugEquipStatus()->phase == GearsetsEquipPhase::Invoked && invoker.calls == 1, "a matching uid is invoked once") && ok;
    service.OnGameFrame(PlayerOf(fixture), 30);
    ok = Expect(service.DebugEquipStatus()->phase == GearsetsEquipPhase::Complete && service.DebugEquipStatus()->reason == GearsetsEquipReason::Confirmed, "a fresh snapshot with the uid in the slot completes") && ok;

    service.SetInvoker(nullptr, nullptr);
    ok = Expect(service.SubmitDebugEquip(2, 1, std::nullopt).accepted, "a request without a uid is accepted") && ok;
    service.OnGameFrame(PlayerOf(fixture), 40);
    service.OnGameFrame(PlayerOf(fixture), 50);
    const auto refused = service.DebugEquipStatus();
    ok                 = Expect(refused->phase == GearsetsEquipPhase::Refused && refused->reason == GearsetsEquipReason::InvokeUnavailable && refused->invocations == 0 && refused->expectedUid == kUidB && invoker.calls == 1, "without an invoker the request is refused after its recheck") && ok;

    ok = Expect(service.SubmitDebugEquip(2, 1, kUidB).accepted, "another request is accepted") && ok;
    service.OnGameFrame(nullptr, 60); // the player changed before the request ran
    ok = Expect(service.DebugEquipStatus()->reason == GearsetsEquipReason::StaleGeneration, "a request from before a player change is stale") && ok;

    ok = Expect(service.SubmitDebugEquip(2, 1, kUidB).accepted, "a request before unload is accepted") && ok;
    service.OnAddonStateChanged(false);
    const auto ended = service.DebugEquipStatus();
    ok               = Expect(ended->phase == GearsetsEquipPhase::Rejected && ended->reason == GearsetsEquipReason::SessionEnded, "unloading the addon ends the request at once") && ok;
    ok               = Expect(!service.SubmitDebugEquip(2, 1, kUidB).accepted, "an inactive addon cannot submit") && ok;

    // A reload with no update between its halves still ends a queued request
    // before any frame can run it.
    service.SetInvoker(&LinkingInvoker::Invoke, &invoker);
    service.OnAddonStateChanged(true);
    ok = Expect(service.SubmitDebugEquip(2, 1, kUidB).accepted, "a request after the load is accepted") && ok;
    service.OnAddonStateChanged(false);
    service.OnAddonStateChanged(true);
    const auto reloaded = service.DebugEquipStatus();
    ok                  = Expect(reloaded->phase == GearsetsEquipPhase::Rejected && reloaded->reason == GearsetsEquipReason::SessionEnded && reloaded->invocations == 0, "a reload ends the previous instance's request") && ok;
    service.OnGameFrame(PlayerOf(fixture), 70);
    ok = Expect(invoker.calls == 1, "the ended request is never invoked") && ok;
    service.OnRenderFrame();

    const std::string text = ReadText(receipt);
    return Expect(CountPrefix(text, "equip,") == 12 && text.find(",complete,confirmed,") != std::string::npos && text.find(",refused,invoke-unavailable,") != std::string::npos && text.ends_with("reason=addon-inactive\n"), "every transaction change of the first session is in the receipt") && ok;
}

bool TestShutdownFooter(const fs::path& scratch)
{
    Fixture fixture;
    fixture.Bag(1, kUidA, 8030423u);
    fixture.Link(10, 0, 1);
    const fs::path        receipt = scratch / "shutdown.csv";
    EquipmentStateService equipment;
    {
        GearsetsDebugService service(&equipment);
        service.Configure({ receipt, false });
        service.OnAddonStateChanged(true);
        service.OnRenderFrame();
        service.OnGameFrame(PlayerOf(fixture), 0);
    }
    const std::string text = ReadText(receipt);
    return Expect(CountPrefix(text, "snapshot,1,") == kEquipmentSlotCount && text.ends_with("# end records=1 dropped=0 reason=shutdown\n"), "shutdown writes pending records and the footer");
}

struct BlockingInvoker
{
    std::promise<void>       entered;
    std::shared_future<void> release;

    static GearsetsInvokeResult Invoke(void* context, const void*, std::uint16_t, std::uint8_t)
    {
        auto* self = static_cast<BlockingInvoker*>(context);
        self->entered.set_value();
        self->release.wait();
        return GearsetsInvokeResult::ReturnedFalse;
    }
};

bool TestShutdownDoesNotWait(const fs::path& scratch)
{
    Fixture fixture;
    fixture.Bag(1, kUidA, 8030423u);
    const fs::path        receipt = scratch / "contended.csv";
    EquipmentStateService equipment;
    std::promise<void>    release;
    BlockingInvoker       invoker;
    invoker.release              = release.get_future().share();
    auto                 entered = invoker.entered.get_future();
    GearsetsDebugService service(&equipment);
    service.Configure({ receipt, true });
    service.SetInvoker(&BlockingInvoker::Invoke, &invoker);
    service.OnAddonStateChanged(true);
    service.OnRenderFrame();
    service.OnGameFrame(PlayerOf(fixture), 0);
    bool ok = Expect(service.SubmitDebugEquip(1, 11, kUidA).accepted, "the blocking request is accepted");

    // The game thread holds the service lock inside the invocation, as a
    // thread that ExitProcess terminated could.
    std::thread game([&]
                     {
                         service.OnGameFrame(PlayerOf(fixture), 10);
                     });
    entered.wait();
    auto       shutdown = std::async(std::launch::async, [&]
                                     {
                                   service.FinishAtShutdown();
                                     });
    const bool returned = shutdown.wait_for(std::chrono::seconds(2)) == std::future_status::ready;
    release.set_value();
    game.join();
    shutdown.get();
    const std::string text = ReadText(receipt);
    ok                     = Expect(returned, "shutdown must not wait for a lock another thread holds") && ok;
    ok                     = Expect(text.starts_with("# bahamut-gearsets-receipt schema=1\n") && text.find("# end") == std::string::npos, "a contended shutdown closes the receipt without a footer") && ok;
    return ok;
}

} // namespace

int wmain(int argc, wchar_t** argv)
{
    if (argc != 2)
    {
        std::cerr << "usage: gearsets-debug-tests <scratch-directory>\n";
        return 2;
    }
    const fs::path  scratch = fs::path(argv[1]);
    std::error_code error;
    fs::remove_all(scratch, error);
    fs::create_directories(scratch / "off");
    fs::create_directories(scratch / "service");
    const bool ok = TestSwitchesDefaultOff() && TestSubmitGates() && TestStaleGenerationRefused() && TestUidMismatchRefused() &&
                    TestCaptureThenRecheck() && TestConfirmationAndTimeout() && TestOtherOutcomes() && TestReceiptFormat() &&
                    TestInstallOnlyWithSwitch() && TestServiceDefaultOff(scratch / "off") && TestServiceRequiresAddon(scratch / "service") &&
                    TestServiceReceipt(scratch / "service") && TestServicePlayerChange() && TestServiceEquip(scratch / "service") &&
                    TestShutdownFooter(scratch / "service") && TestShutdownDoesNotWait(scratch / "service");
    if (ok)
    {
        std::cout << "gearsets-debug-tests: PASS\n";
    }
    return ok ? 0 : 1;
}
