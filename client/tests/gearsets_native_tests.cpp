#include "equipment_memory_fixture.h"
#include "equipment_state.h"
#include "gearsets_native_executor.h"
#include "gearsets_native_lookup.h"
#include "gearsets_runtime.h"

#include <cstring>
#include <iostream>
#include <map>
#include <vector>

namespace
{

using namespace bahamut_client;

constexpr std::uint32_t kEngine      = 0x10000000u;
constexpr std::uint32_t kActorIds    = 0x11000000u;
constexpr std::uint32_t kHead        = 0x11000100u;
constexpr std::uint32_t kNodeF       = 0x11000200u;
constexpr std::uint32_t kNode3       = 0x11000300u;
constexpr std::uint32_t kTables      = 0x12000000u;
constexpr std::uint32_t kTable       = kTables + 0x60u;
constexpr std::uint32_t kBuckets     = 0x13000000u;
constexpr std::uint32_t kValue       = 0x14000000u;
constexpr std::uint32_t kControl     = 0x15000000u;
constexpr std::uint32_t kEmptyKey    = 0xFFFFFFFFu;
constexpr std::uint32_t kDeletedKey  = 0xFFFFFFFEu;
constexpr std::uint32_t kReference   = 0xA0F02EE9u; // 0xA0F00000 + 12009
constexpr std::uint32_t kBucketCount = 16u;

bool Expect(bool condition, const char* message)
{
    if (!condition)
    {
        std::cerr << message << '\n';
        return false;
    }
    return true;
}

// Sparse little-endian memory; unwritten bytes are unreadable.
struct FakeMemory
{
    std::map<std::uint32_t, std::uint8_t> bytes;

    void Put(std::uint32_t address, const void* data, std::size_t size)
    {
        const auto* source = static_cast<const std::uint8_t*>(data);
        for (std::size_t index = 0; index < size; ++index)
        {
            bytes[address + static_cast<std::uint32_t>(index)] = source[index];
        }
    }

    void Word(std::uint32_t address, std::uint32_t value)
    {
        Put(address, &value, sizeof(value));
    }

    void Byte(std::uint32_t address, std::uint8_t value)
    {
        Put(address, &value, sizeof(value));
    }

    void Erase(std::uint32_t address, std::size_t size)
    {
        for (std::size_t index = 0; index < size; ++index)
        {
            bytes.erase(address + static_cast<std::uint32_t>(index));
        }
    }

    static bool Read(void* context, std::uint32_t address, void* out, std::size_t size)
    {
        auto* self   = static_cast<FakeMemory*>(context);
        auto* target = static_cast<std::uint8_t*>(out);
        for (std::size_t index = 0; index < size; ++index)
        {
            const auto found = self->bytes.find(address + static_cast<std::uint32_t>(index));
            if (found == self->bytes.end())
            {
                return false;
            }
            target[index] = found->second;
        }
        return true;
    }

    GearsetsNativeMemory View()
    {
        return { &FakeMemory::Read, this };
    }

    void Bucket(std::uint32_t index, std::uint32_t key, std::uint32_t value = 0u)
    {
        Word(kBuckets + index * 8u, key);
        Word(kBuckets + index * 8u + 4u, value);
    }
};

// Category 0x0F current, categories 0x0F and 3 registered, and the command
// reference in its home bucket of an otherwise empty 16-bucket table.
FakeMemory World()
{
    FakeMemory memory;
    memory.Byte(0x0130D426u, 0x0Fu);
    memory.Word(0x0130C778u, 0xE0000000u);
    memory.Word(kEngine + 0x1C4u, kActorIds);
    memory.Word(kEngine + 0x17Cu, kTables);
    memory.Word(kActorIds + 0x0Cu, kHead);
    memory.Word(kHead, kNode3);
    memory.Word(kNode3, kNodeF);
    memory.Byte(kNode3 + 8u, 3u);
    memory.Word(kNode3 + 0x0Cu, 0xA0300000u);
    memory.Word(kNodeF, kHead);
    memory.Byte(kNodeF + 8u, 0x0Fu);
    memory.Word(kNodeF + 0x0Cu, 0xA0F00000u);

    memory.Word(kTable + 0x04u, 0u);
    memory.Byte(kTable + 0x08u, 1u);
    memory.Word(kTable + 0x0Cu, kDeletedKey);
    memory.Word(kTable + 0x14u, kEmptyKey);
    memory.Word(kTable + 0x1Cu, kBuckets);
    memory.Word(kTable + 0x20u, kBucketCount);
    memory.Word(kTable + 0x24u, 1u);
    for (std::uint32_t index = 0; index < kBucketCount; ++index)
    {
        memory.Bucket(index, kEmptyKey);
    }
    memory.Bucket(GearsetsNativeHashBucket(kReference, kBucketCount - 1u), kReference, kValue);
    memory.Word(kValue, kControl);
    return memory;
}

GearsetsStaticActorLookup Find(FakeMemory& memory)
{
    return FindLoadedStaticActor(memory.View(), kEngine, 12009u);
}

bool TestHash()
{
    // Independent reimplementation of 0x009D6E98 and 0x00CEB9C0's mixing,
    // including keys whose mixed value is negative.
    return Expect(GearsetsNativeHashBucket(0xA0F02EE9u, 0xFu) == 10u && GearsetsNativeHashBucket(0xA0F02EE9u, 0xFFFFu) == 47802u, "the command reference hashes like the game") &&
           Expect(GearsetsNativeHashBucket(0u, 0xFFFFu) == 11217u && GearsetsNativeHashBucket(0x12345678u, 0xFFFFu) == 63268u && GearsetsNativeHashBucket(0xFFFFFFFFu, 0xFFFFu) == 37511u, "other keys hash like the game");
}

bool TestLoaded()
{
    bool ok = true;
    {
        FakeMemory memory = World();
        const auto result = Find(memory);
        ok                = Expect(result.state == GearsetsStaticActorState::Loaded && result.reference == kReference && result.control == kControl, "a reference in its home bucket is loaded") && ok;
    }
    {
        // Two collisions: the probe visits home, home + 1, home + 3.
        FakeMemory          memory = World();
        const std::uint32_t home   = GearsetsNativeHashBucket(kReference, kBucketCount - 1u);
        memory.Bucket(home, 0x1234u, 0x99u);
        memory.Bucket((home + 1u) & 15u, kDeletedKey);
        memory.Word(kTable + 0x04u, 1u);
        memory.Word(kTable + 0x24u, 3u);
        memory.Bucket((home + 3u) & 15u, kReference, kValue);
        const auto result = Find(memory);
        ok                = Expect(result.state == GearsetsStaticActorState::Loaded && result.control == kControl, "the probe passes other and deleted keys") && ok;
    }
    return ok;
}

bool TestNotLoaded()
{
    bool ok = true;
    {
        FakeMemory memory = World();
        memory.Bucket(GearsetsNativeHashBucket(kReference, kBucketCount - 1u), kEmptyKey);
        ok = Expect(Find(memory).state == GearsetsStaticActorState::NotLoaded, "an empty home bucket is not loaded") && ok;
    }
    {
        // With deleted entries in use, a bucket holding the deleted key is
        // skipped even when that key is the reference.
        FakeMemory memory = World();
        memory.Word(kTable + 0x0Cu, kReference);
        memory.Word(kTable + 0x04u, 1u);
        memory.Word(kTable + 0x24u, 2u);
        ok = Expect(Find(memory).state == GearsetsStaticActorState::NotLoaded, "a deleted bucket is not a match") && ok;
        memory.Byte(kTable + 0x08u, 0u);
        ok = Expect(Find(memory).state == GearsetsStaticActorState::Loaded, "without deleted entries in use the key matches") && ok;
    }
    {
        FakeMemory memory = World();
        memory.Word(kTable + 0x24u, 0u);
        ok = Expect(Find(memory).state == GearsetsStaticActorState::NotLoaded, "an empty table is not loaded") && ok;
    }
    {
        FakeMemory memory = World();
        memory.Word(kValue, 0u);
        ok = Expect(Find(memory).state == GearsetsStaticActorState::NotLoaded, "a zero control is not loaded") && ok;
        memory.Bucket(GearsetsNativeHashBucket(kReference, kBucketCount - 1u), kReference, 0u);
        ok = Expect(Find(memory).state == GearsetsStaticActorState::NotLoaded, "a zero value is not loaded") && ok;
    }
    {
        FakeMemory memory = World();
        memory.Word(kNode3, kHead);
        ok = Expect(Find(memory).state == GearsetsStaticActorState::NotLoaded, "an unregistered category is not loaded") && ok;
    }
    return ok;
}

bool TestRefusals()
{
    bool ok = true;
    {
        FakeMemory memory = World();
        memory.Word(kNodeF + 0x0Cu, 0xA0F00010u);
        ok = Expect(Find(memory).state == GearsetsStaticActorState::Unmodeled, "a base the reference mask does not reproduce takes the unmodeled table") && ok;
    }
    {
        FakeMemory memory = World();
        memory.Word(kTable + 0x20u, 12u);
        ok = Expect(Find(memory).state == GearsetsStaticActorState::Unmodeled, "a bucket count that is not a power of two is unmodeled") && ok;
    }
    {
        FakeMemory memory = World();
        for (std::uint32_t index = 0; index < kBucketCount; ++index)
        {
            memory.Bucket(index, 0x1000u + index, 1u);
        }
        ok = Expect(Find(memory).state == GearsetsStaticActorState::Unmodeled, "a full probe without an empty bucket is unmodeled") && ok;
    }
    {
        FakeMemory memory = World();
        memory.Erase(kBuckets, kBucketCount * 8u);
        ok = Expect(Find(memory).state == GearsetsStaticActorState::Unreadable, "unreadable buckets are unreadable") && ok;
    }
    {
        FakeMemory memory = World();
        memory.Word(kNodeF, kNodeF);
        memory.Byte(kNodeF + 8u, 4u);
        ok = Expect(Find(memory).state == GearsetsStaticActorState::Unreadable, "a list that never returns to its head is unreadable") && ok;
    }
    {
        FakeMemory memory = World();
        ok                = Expect(FindLoadedStaticActor(memory.View(), 0u, 12009u).state == GearsetsStaticActorState::Unreadable, "no engine is unreadable") && ok;
    }
    return ok;
}

constexpr std::uint32_t kContainer  = 0x30000000u;
constexpr std::uint32_t kWaiting    = 0x30000100u;
constexpr std::uint32_t kReporter   = 0x30000200u;
constexpr std::uint32_t kSender     = 0x30000300u;
constexpr std::uint32_t kState      = 0x30000400u;
constexpr std::uint32_t kNetHolder  = 0x30000500u;
constexpr std::uint32_t kNetwork    = 0x31000000u;
constexpr std::uint32_t kSession    = 0x32000000u;
constexpr std::uint32_t kLink       = 0x33000000u;
constexpr std::uint32_t kConnection = 0x34000000u;
constexpr std::uint32_t kPlayer     = 0x35000000u;
constexpr std::uint32_t kManager    = 0x36000000u;

// World() plus an established connection and an idle player.
FakeMemory Ready()
{
    FakeMemory memory = World();
    memory.Word(kContainer + 0x518u, kWaiting);
    memory.Word(kWaiting + 4u, kEngine);
    memory.Word(kEngine + 0x1CCu, kReporter);
    memory.Byte(kReporter + 8u, 0u);
    memory.Word(kEngine + 0x220u, kSender);
    memory.Word(kSender + 4u, kState);
    memory.Word(kState, kNetHolder);
    memory.Word(kNetHolder, kNetwork);
    memory.Word(kState + 0x10u, 0x48u);
    memory.Word(kState + 0x14u, 0x30400000u);
    memory.Word(kNetwork + 0x174ECu, kSession);
    memory.Word(kSession + 0x234u, kLink);
    memory.Word(kLink + 0x70u, kConnection);
    memory.Word(kConnection + 0x8Cu, 3u);
    memory.Word(kPlayer + 0x128u, 0xE0000000u);
    memory.Word(kPlayer + 0x12Cu, 0xE0000000u);
    memory.Word(kPlayer + 0xF8u, kManager);
    memory.Byte(kManager + 0x1Eu, 0u);
    memory.Word(kManager + 0x08u, 0u);
    memory.Word(kManager + 0x0Cu, 0u);
    memory.Byte(0x0134C3FFu, 0u);
    return memory;
}

bool TestRoute()
{
    FakeMemory                                                       memory = Ready();
    const auto                                                       route  = ReadGearsetsNativeRoute(memory.View(), kContainer, kPlayer);
    bool                                                             ok     = Expect(route && route->network == kNetwork && route->actor == 0x48u && route->owner == kReference && route->stateWord == 0x30400000u && route->eventType == 0u, "an idle connected player has a route");
    const std::vector<std::pair<const char*, void (*)(FakeMemory&)>> closed = {
        { "a halted engine has no route", [](FakeMemory& value)
          {
              value.Byte(kReporter + 8u, 1u);
          } },
        { "an unestablished connection has no route", [](FakeMemory& value)
          {
              value.Word(kConnection + 0x8Cu, 2u);
          } },
        { "no sending actor has no route", [](FakeMemory& value)
          {
              value.Word(kState + 0x10u, 0u);
          } },
        { "an event actor has no route", [](FakeMemory& value)
          {
              value.Word(kPlayer + 0x12Cu, 0x48u);
          } },
        { "a busy event manager has no route", [](FakeMemory& value)
          {
              value.Byte(kManager + 0x1Eu, 1u);
          } },
        { "a client-side event has no route", [](FakeMemory& value)
          {
              value.Word(kManager + 0x08u, 0x10u);
          } },
        { "a server-order event has no route", [](FakeMemory& value)
          {
              value.Word(kManager + 0x0Cu, 0x10u);
          } },
        { "another event type has no route", [](FakeMemory& value)
          {
              value.Byte(0x0134C3FFu, 1u);
          } },
        { "an unresolved command reference has no route", [](FakeMemory& value)
          {
              value.Erase(0x0130D426u, 1u);
          } },
        { "an unreadable session has no route", [](FakeMemory& value)
          {
              value.Erase(kNetwork + 0x174ECu, 4u);
          } },
    };
    for (const auto& [message, close] : closed)
    {
        FakeMemory changed = Ready();
        close(changed);
        ok = Expect(!ReadGearsetsNativeRoute(changed.View(), kContainer, kPlayer), message) && ok;
    }
    return Expect(!ReadGearsetsNativeRoute(memory.View(), kContainer, 0u) && !ReadGearsetsNativeRoute(memory.View(), 0u, kPlayer), "no container or player has no route") && ok;
}

bool TestRequestAndPlan()
{
    const GearsetsNativeRoute route{ kNetwork, 0x12u, kReference, 0x30400000u, 0u };
    GearsetsOperation         equip{ { 10, EquipmentSlotState::Item, 0x1200000002ull, 8030423u }, std::uint16_t{ 1 }, {} };
    const auto                request = GearsetsNativeRequest(equip, route);
    bool                      ok      = Expect(request && !request->unequip && request->equipSlot == 11u && request->bagSlot == 1u && request->uid == 0x1200000002ull && request->actor == 0x12u && request->owner == kReference && request->stateWord == 0x30400000u,
                                               "an equip names the 1-based slot, bag position and unique ID");
    const auto                unequip = GearsetsNativeRequest({ { 0, EquipmentSlotState::Empty, 0, 0 }, {}, {} }, route);
    ok                                = Expect(unequip && unequip->unequip && unequip->equipSlot == 1u, "an Empty target is an unequip") && ok;
    equip.bagSlot.reset();
    ok = Expect(!GearsetsNativeRequest(equip, route), "an item without a bag position has no request") && ok;

    GearsetsNativeExecutor executor;
    const auto             plan = executor.Plan({});
    ok                          = Expect(plan && plan->order.size() == kEquipmentSlotCount && plan->order.front() == 0u && plan->confirmationMs == GearsetsNativeExecutor::kConfirmationMs && plan->settleMs == GearsetsNativeExecutor::kSettleMs, "the main hand goes first, with bounded confirmation and settling") && ok;
    GearsetIntent intent;
    for (std::size_t index = 0; index < kEquipmentSlotCount; ++index)
        intent.slots[index] = { kEquipmentSlots[index], EquipmentSlotState::Empty, 0, 0 };
    const auto failure = executor.Preflight(intent, {});
    ok                 = Expect(failure && failure->reason == "main-hand-required" && failure->slot == 0u, "an empty main hand is refused before anything is sent") && ok;
    intent.slots[0]    = { 0, EquipmentSlotState::Item, 7, 0 };
    return Expect(!executor.Preflight(intent, {}), "a set with a main hand passes") && ok;
}

class RecordingExecutor final : public GearsetsExecutor
{
public:
    std::vector<GearsetsOperation> sent;
    const void*                    player = nullptr;

    std::optional<GearsetsPlan> Plan(const GearsetIntent&) const override
    {
        return GearsetsPlan{ { kEquipmentSlots.begin(), kEquipmentSlots.end() }, 50, 0 };
    }

    std::optional<GearsetsFailure> Preflight(const GearsetIntent&, const GearsetsView&) const override
    {
        return std::nullopt;
    }

    void BeginFrame(const void*, const void* value) override
    {
        player = value;
    }

    GearsetsSubmission Submit(const GearsetsOperation& operation) override
    {
        sent.push_back(operation);
        return GearsetsSubmission::Accepted;
    }
};

const void* PlayerOf(const equipment_memory_fixture::Fixture& fixture)
{
    return reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player));
}

// As the addon builds it from the published inventory.
std::pair<GearsetIntent, GearsetsView> Current(const EquipmentStateService& equipment)
{
    GearsetsView view;
    if (const auto inventory = equipment.Inventory())
    {
        view.inventory = *inventory;
        view.session   = inventory->equipment.session;
    }
    return { GearsetIntent{ view.session, view.inventory.equipment.slots, 0, 0 }, view };
}

void CountFrame(void* context, const void*)
{
    ++*static_cast<int*>(context);
}

bool TestRuntime()
{
    equipment_memory_fixture::Fixture fixture;
    fixture.Bag(1, 0x4800000002ull, 8030423u);
    fixture.Bag(2, 0x4800000001ull, 4020001u);
    fixture.Bag(3, 0x4800000003ull, 4020002u);
    fixture.Link(10, 0, 1);
    fixture.Link(0, 0, 2);
    EquipmentStateService equipment;
    RecordingExecutor     executor;
    GearsetsRuntime       runtime(&equipment, executor);
    int                   forwarded = 0;
    runtime.SetGameFrameObserver(&CountFrame, &forwarded);
    const void* player = PlayerOf(fixture);

    runtime.OnGameFrame(nullptr, player, 0);
    bool ok = Expect(!equipment.Inventory() && forwarded == 1, "an inactive runtime reads nothing but forwards the frame");
    runtime.SetActive(true);
    runtime.OnGameFrame(nullptr, player, 0);
    ok = Expect(equipment.Inventory() && equipment.Inventory()->equipment.complete && forwarded == 2, "an active runtime publishes the inventory") && ok;

    auto  current    = Current(equipment);
    auto& intent     = current.first;
    auto& view       = current.second;
    intent.slots[0]  = { 0, EquipmentSlotState::Item, 0x4800000003ull, 0 };
    const auto begun = runtime.Begin(intent, view);
    ok               = Expect(begun && begun->phase == GearsetsPhase::Checking && !runtime.Begin(intent, view), "one transaction begins and blocks another") && ok;
    runtime.OnGameFrame(nullptr, player, 10);
    ok = Expect(executor.sent.size() == 1 && executor.sent[0].target.slot == 0 && executor.sent[0].bagSlot == 3 && executor.player == player, "the game frame submits with the frame's player") && ok;
    fixture.Link(0, 0, 3);
    runtime.OnGameFrame(nullptr, player, 20);
    runtime.OnGameFrame(nullptr, player, 30);
    ok = Expect(runtime.Result().phase == GearsetsPhase::Complete && runtime.Result().changed[0], "reads while transacting confirm and complete the set") && ok;

    current         = Current(equipment);
    intent.slots[7] = { 10, EquipmentSlotState::Empty, 0, 0 };
    runtime.Begin(intent, view);
    runtime.OnGameFrame(nullptr, player, 40);
    runtime.SetActive(false);
    ok = Expect(runtime.Result().phase == GearsetsPhase::Unconfirmed && runtime.Result().reason == "session-ended" && !equipment.Inventory(), "deactivation ends the transaction and clears the equipment") && ok;

    runtime.SetActive(true);
    runtime.OnGameFrame(nullptr, player, 50);
    current         = Current(equipment);
    intent.slots[7] = { 10, EquipmentSlotState::Empty, 0, 0 };
    runtime.Begin(intent, view);
    runtime.OnGameFrame(nullptr, nullptr, 60);
    return Expect(runtime.Result().phase == GearsetsPhase::Rejected && runtime.Result().reason == "session-changed" && executor.sent.size() == 2, "losing the player stops before submitting") && ok;
}

} // namespace

int main()
{
    bool ok = TestHash();
    ok      = TestLoaded() && ok;
    ok      = TestNotLoaded() && ok;
    ok      = TestRefusals() && ok;
    ok      = TestRoute() && ok;
    ok      = TestRequestAndPlan() && ok;
    ok      = TestRuntime() && ok;
    return ok ? 0 : 1;
}
