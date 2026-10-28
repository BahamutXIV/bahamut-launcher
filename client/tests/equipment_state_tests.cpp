#include "equipment_memory_fixture.h"
#include "equipment_state.h"

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <deque>
#include <iostream>
#include <vector>

namespace
{

using namespace bahamut_client;
using namespace equipment_memory_fixture;

bool Expect(bool condition, const char* message)
{
    if (!condition)
    {
        std::cerr << message << '\n';
        return false;
    }
    return true;
}

const EquipmentSlot* Find(const EquipmentSnapshot& snapshot, std::uint8_t slot)
{
    for (const auto& entry : snapshot.slots)
    {
        if (entry.slot == slot)
        {
            return &entry;
        }
    }
    return nullptr;
}

bool TestEquippedEmptyAndDuplicateItems()
{
    Fixture fixture;
    fixture.Bag(3, 0x07AF7850u, 4030013u);
    fixture.Bag(5, 0x06D4CCD1u, 9050052u);
    fixture.Bag(6, 0x06D4CCD2u, 9050052u);
    fixture.Link(0, 0, 3);
    fixture.Link(14, 0, 5);
    fixture.Link(15, 0, 6);
    // A direct item in the equipment package needs no redirect.
    fixture.SetEntry(fixture.equipment, 8, fixture.MakeEntry(fixture.MakeItem(0xFEDCBA9876543210ull, 8011608u), kUnset, kUnset));
    // A present entry with no item and no redirect is an empty position.
    fixture.SetEntry(fixture.equipment, 1, fixture.MakeEntry(0, kUnset, kUnset));

    const auto snapshot = ReadEquipmentFromPlayer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player)));
    if (!Expect(snapshot.has_value() && snapshot->complete, "a fully readable store should give a complete snapshot"))
    {
        return false;
    }
    const EquipmentSlot* mainHand   = Find(*snapshot, 0);
    const EquipmentSlot* offHand    = Find(*snapshot, 1);
    const EquipmentSlot* head       = Find(*snapshot, 8);
    const EquipmentSlot* firstRing  = Find(*snapshot, 21);
    const EquipmentSlot* secondRing = Find(*snapshot, 22);
    const EquipmentSlot* bodySlot   = Find(*snapshot, 10);
    // Slots 14 and 15 hold two copies of one item type.
    const EquipmentSlot* feet  = Find(*snapshot, 14);
    const EquipmentSlot* waist = Find(*snapshot, 15);
    return Expect(snapshot->slots.size() == 18, "the snapshot should cover 18 supported slots") && Expect(mainHand && mainHand->state == EquipmentSlotState::Item && mainHand->uid == 0x07AF7850u && mainHand->catalogId == 4030013u, "a linked item should resolve through the bag package") && Expect(offHand && offHand->state == EquipmentSlotState::Empty, "an entry without item or link is empty") && Expect(bodySlot && bodySlot->state == EquipmentSlotState::Empty, "a null entry is empty") && Expect(head && head->state == EquipmentSlotState::Item && head->uid == 0xFEDCBA9876543210ull, "a direct item should keep all 64 uid bits") && Expect(feet && waist && feet->catalogId == waist->catalogId && feet->uid == 0x06D4CCD1u && waist->uid == 0x06D4CCD2u, "duplicate item types should keep distinct instance ids") && Expect(firstRing && secondRing && firstRing->state == EquipmentSlotState::Empty && secondRing->state == EquipmentSlotState::Empty, "unlinked positions are empty");
}

bool TestUnresolvedLinksAreUnknownNeverEmpty()
{
    Fixture fixture;
    fixture.Bag(3, 0x11u, 100u);
    fixture.Link(0, 0, 3);
    fixture.Link(4, 0, 9);      // the bag position is empty
    fixture.Link(5, 7, 0);      // no such package
    fixture.Link(6, 0, 999);    // beyond the bag vector
    fixture.Link(8, kUnset, 3); // half-set redirect
    fixture.Link(9, 0xFE, 9);   // links to itself until the depth limit
    fixture.Bag(2, 0, 100u);    // an item with a zero instance id
    fixture.Link(10, 0, 2);
    fixture.SetEntry(fixture.equipment, 11, 0xFFFFFFF0u); // unreadable entry pointer

    const auto snapshot = ReadEquipmentFromPlayer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player)));
    if (!Expect(snapshot.has_value() && !snapshot->complete, "unresolved positions should leave the snapshot incomplete"))
    {
        return false;
    }
    bool ok = Expect(Find(*snapshot, 0)->state == EquipmentSlotState::Item, "readable positions should still resolve");
    for (const int slot : { 4, 5, 6, 8, 9, 10, 11 })
    {
        ok = Expect(Find(*snapshot, static_cast<std::uint8_t>(slot))->state == EquipmentSlotState::Unknown, "an unresolved link must be unknown, not empty") && ok;
    }
    return ok;
}

bool TestStructuralFailuresGiveNoSnapshot()
{
    bool ok = Expect(!ReadEquipmentFromPlayer(nullptr).has_value(), "a null player should give no snapshot");
    {
        Fixture fixture;
        Memory::Put32(fixture.player, kPlayerStoreOffset, 0);
        ok = Expect(!ReadEquipmentFromPlayer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player))).has_value(), "a missing store should give no snapshot") && ok;
    }
    {
        Fixture fixture;
        Memory::Put32(fixture.player, kPlayerStoreOffset, 0x00000040u);
        ok = Expect(!ReadEquipmentFromPlayer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player))).has_value(), "an unreadable store should give no snapshot, not a crash") && ok;
    }
    {
        Fixture fixture(34);
        ok = Expect(!ReadEquipmentFromPlayer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player))).has_value(), "a short equipment package should give no snapshot") && ok;
    }
    {
        // A vector end that is not on an entry boundary is malformed, even when
        // whole entries would still count 35.
        Fixture fixture;
        Memory::Put32(fixture.equipment.object, 0x18, fixture.equipment.vector + 35 * 4 + 2);
        ok = Expect(!ReadEquipmentFromPlayer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player))).has_value(), "a misaligned equipment vector should give no snapshot") && ok;
    }
    {
        Fixture fixture(35, 0);
        ok = Expect(!ReadEquipmentFromPlayer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player))).has_value(), "an empty bag package should give no snapshot") && ok;
    }
    {
        // Without the bag package an all-null equipment package must not
        // read as a character wearing nothing.
        Fixture fixture(35, 16, false);
        ok = Expect(!ReadEquipmentFromPlayer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player))).has_value(), "a store without the bag package should give no snapshot") && ok;
    }
    return ok;
}

bool TestServiceGenerations()
{
    Fixture fixture;
    fixture.Bag(3, 0x11u, 100u);
    fixture.Link(0, 0, 3);
    const void* player = reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player));

    EquipmentStateService service;
    if (!Expect(!service.Snapshot().has_value(), "a new service has no snapshot"))
    {
        return false;
    }
    service.Publish(*ReadEquipmentFromPlayer(player));
    const auto first = service.Snapshot();
    service.Publish(*ReadEquipmentFromPlayer(player));
    const auto repeated = service.Snapshot();
    fixture.Link(4, 0, 3); // the same bag item now also appears in slot 4
    service.Publish(*ReadEquipmentFromPlayer(player));
    const auto changed = service.Snapshot();
    service.Clear();
    const bool cleared = !service.Snapshot().has_value();
    service.Publish(*ReadEquipmentFromPlayer(player));
    const auto republished = service.Snapshot();
    return Expect(first && first->generation == 1, "the first publish is generation 1") && Expect(repeated && repeated->generation == 1, "identical content keeps its generation") && Expect(changed && changed->generation == 2, "changed content bumps the generation") && Expect(cleared, "Clear removes the snapshot") && Expect(republished && republished->generation == 3, "generations keep increasing after Clear");
}

bool TestCharacterChangeEndsSession()
{
    Fixture fixture;
    fixture.Bag(3, 0x11u, 100u);
    fixture.Link(0, 0, 3);
    const void* player = reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player));

    EquipmentStateService service;
    auto                  snapshot = *ReadEquipmentInventoryFromPlayer(player);
    snapshot.equipment.characterId = 0x12u;
    service.PublishInventory(snapshot);
    const auto first = service.Snapshot();
    service.PublishInventory(snapshot);
    const auto repeated            = service.Snapshot();
    snapshot.equipment.characterId = 0x13u;
    service.PublishInventory(snapshot);
    const auto other = service.Snapshot();
    service.InvalidateSnapshot();
    snapshot.equipment.characterId = 0x12u;
    service.PublishInventory(snapshot);
    const auto back = service.Snapshot();
    return Expect(first && repeated && first->session == repeated->session && first->generation == repeated->generation,
                  "the same character keeps its session and generation") &&
           Expect(other && other->session != first->session && other->characterId == 0x13u && other->generation != first->generation,
                  "another character starts a new session") &&
           Expect(back && back->session != other->session,
                  "a character change after an invalidated read still starts a new session");
}

bool TestCaptureFromContainerChain()
{
    Fixture fixture;
    fixture.Bag(3, 0x11u, 100u);
    fixture.Link(0, 0, 3);

    // container+0x518 -> +4 -> +0x220 -> +4 -> +0x0C is the player.
    const std::uint32_t container   = fixture.memory.Alloc(0x600);
    const std::uint32_t waiting     = fixture.memory.Alloc(0x10);
    const std::uint32_t engine      = fixture.memory.Alloc(0x230);
    const std::uint32_t holder      = fixture.memory.Alloc(0x10);
    const std::uint32_t holderState = fixture.memory.Alloc(0x20);
    Memory::Put32(container, 0x518, waiting);
    Memory::Put32(waiting, 0x04, engine);
    Memory::Put32(engine, 0x220, holder);
    Memory::Put32(holder, 0x04, holderState);
    Memory::Put32(holderState, 0x0C, fixture.player);

    EquipmentStateService service;
    bool                  ok       = Expect(service.CaptureFromContainer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(container))), "a valid container chain should capture");
    const auto            snapshot = service.Snapshot();
    ok                             = Expect(snapshot && Find(*snapshot, 0)->uid == 0x11u, "the captured snapshot should hold the linked item") && ok;
    Memory::Put32(holderState, 0x0C, 0);
    ok = Expect(!service.CaptureFromContainer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(container))) && !service.Snapshot().has_value(), "a broken chain should clear the snapshot") && ok;
    ok = Expect(!service.CaptureFromContainer(nullptr), "a null container should be rejected") && ok;
    return ok;
}

bool TestBagItems()
{
    Fixture fixture;
    fixture.Bag(1, 0x1200000002ull, 8030423u);
    fixture.SetEntry(fixture.bag, 2, fixture.MakeEntry(0, kUnset, kUnset));
    fixture.SetEntry(fixture.bag, 3, fixture.MakeEntry(0, 0xFE, 1)); // a redirect is not a bag shape
    fixture.Bag(4, 0, 100u);                                         // zero instance id
    const void* player = reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player));

    const auto item       = ReadBagItemFromPlayer(player, 1);
    const auto emptyEntry = ReadBagItemFromPlayer(player, 2);
    const auto nullEntry  = ReadBagItemFromPlayer(player, 0);
    const auto redirect   = ReadBagItemFromPlayer(player, 3);
    const auto zeroUid    = ReadBagItemFromPlayer(player, 4);
    const auto outside    = ReadBagItemFromPlayer(player, 16);
    bool       ok         = Expect(item && item->state == EquipmentSlotState::Item && item->uid == 0x1200000002ull && item->catalogId == 8030423u, "a bag item should keep its 64-bit uid and catalog") && Expect(emptyEntry && emptyEntry->state == EquipmentSlotState::Empty && nullEntry && nullEntry->state == EquipmentSlotState::Empty, "an entry without item or link, or a null entry, is empty") && Expect(redirect && redirect->state == EquipmentSlotState::Unknown && zeroUid && zeroUid->state == EquipmentSlotState::Unknown, "a redirect or a zero uid in the bag is unknown") && Expect(outside && outside->state == EquipmentSlotState::Unknown, "a position beyond the bag is unknown");
    ok                    = Expect(!ReadBagItemFromPlayer(nullptr, 1).has_value(), "a null player gives no bag read") && ok;
    Fixture noBag(35, 16, false);
    ok = Expect(!ReadBagItemFromPlayer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(noBag.player)), 1).has_value(), "a store without the bag package gives no bag read") && ok;
    return ok;
}

bool TestExactResolution()
{
    constexpr std::uint64_t uid = 0xFEDCBA9876543210ull;
    Fixture                 fixture;
    fixture.Bag(1, uid, 100);
    fixture.Bag(2, 2, 100);
    fixture.Link(10, 0, 1);
    const void* player   = reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player));
    auto        snapshot = ReadEquipmentInventoryFromPlayer(player);
    bool        ok       = Expect(snapshot.has_value(), "full inventory reads over the bounded bag");
    if (!snapshot)
        return false;
    auto resolved = ResolveEquipmentUid(*snapshot, uid);
    ok            = Expect(resolved.state == ItemAvailability::Available && resolved.bagSlot == 1 && resolved.equippedSlot == 10, "worn and bag links resolve one wide UID") && ok;
    ok            = Expect(ResolveEquipmentUid(*snapshot, 3).state == ItemAvailability::Missing, "another copy of a type cannot replace a missing UID") && ok;
    fixture.SetEntry(fixture.bag, 1, 0);
    fixture.Bag(7, uid, 100);
    fixture.Link(10, 0, 7);
    snapshot = ReadEquipmentInventoryFromPlayer(player);
    ok       = Expect(snapshot && ResolveEquipmentUid(*snapshot, uid).bagSlot == 7, "a moved item resolves to its current position") && ok;
    fixture.Bag(8, uid, 100);
    snapshot = ReadEquipmentInventoryFromPlayer(player);
    ok       = Expect(snapshot && ResolveEquipmentUid(*snapshot, uid).state == ItemAvailability::Ambiguous, "duplicate bag UID is ambiguous") && ok;
    fixture.SetEntry(fixture.bag, 8, 0);
    fixture.SetEntry(fixture.bag, 9, fixture.MakeEntry(0, 0xFE, 0));
    snapshot = ReadEquipmentInventoryFromPlayer(player);
    ok       = Expect(snapshot && ResolveEquipmentUid(*snapshot, uid).state == ItemAvailability::Unavailable && ResolveEquipmentUid(*snapshot, 3).state == ItemAvailability::Unavailable, "unreadable positions prevent available and missing claims") && ok;
    Fixture oversized(35, kMaximumAccessibleBagSlots + 1);
    ok = Expect(!ReadEquipmentInventoryFromPlayer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(oversized.player))), "an oversized bag is refused before traversal") && ok;
    EquipmentStateService service;
    service.PublishInventory(*snapshot);
    service.Clear();
    return Expect(!service.Inventory(), "clearing equipment also clears inventory") && ok;
}

EquipmentInventorySnapshot CatalogInventory()
{
    EquipmentInventorySnapshot inventory;
    inventory.equipment.complete = true;
    for (std::size_t index = 0; index < kEquipmentSlotCount; ++index)
    {
        inventory.equipment.slots[index].slot  = kEquipmentSlots[index];
        inventory.equipment.slots[index].state = EquipmentSlotState::Empty;
    }
    inventory.bag.push_back({ EquipmentSlotState::Empty, 0, 0 });
    return inventory;
}

void Wear(EquipmentInventorySnapshot& inventory, std::uint8_t slot, std::uint64_t uid, std::uint32_t catalog)
{
    for (auto& entry : inventory.equipment.slots)
    {
        if (entry.slot == slot)
        {
            entry.state     = EquipmentSlotState::Item;
            entry.uid       = uid;
            entry.catalogId = catalog;
        }
    }
}

bool TestCatalogResolution()
{
    constexpr std::uint32_t item = 100;
    auto                    one  = CatalogInventory();
    one.bag.push_back({ EquipmentSlotState::Item, 5, item });
    const auto available = ResolveEquipmentCatalog(one, item);
    bool       ok        = Expect(available.state == ItemAvailability::Available && available.uid == 5, "a single bag copy resolves to its uid");
    const auto missing   = ResolveEquipmentCatalog(one, 101);
    ok                   = Expect(missing.state == ItemAvailability::Missing && missing.uid == 0, "an item type that is not carried is missing") && ok;

    auto two = one;
    two.bag.push_back({ EquipmentSlotState::Item, 6, item });
    const auto ambiguous = ResolveEquipmentCatalog(two, item);
    ok                   = Expect(ambiguous.state == ItemAvailability::Ambiguous && ambiguous.uid == 0, "two bag copies are ambiguous") && ok;

    auto sameUid = one;
    sameUid.bag.push_back({ EquipmentSlotState::Item, 5, item });
    ok = Expect(ResolveEquipmentCatalog(sameUid, item).state == ItemAvailability::Available, "one uid listed twice is still one copy") && ok;

    auto worn = CatalogInventory();
    Wear(worn, 10, 7, item);
    worn.bag.push_back({ EquipmentSlotState::Item, 7, item });
    const auto wornOnce = ResolveEquipmentCatalog(worn, item);
    ok                  = Expect(wornOnce.state == ItemAvailability::Available && wornOnce.uid == 7, "a worn item and its bag entry with the same uid count once") && ok;
    auto wornOnly       = CatalogInventory();
    Wear(wornOnly, 10, 7, item);
    ok = Expect(ResolveEquipmentCatalog(wornOnly, item).uid == 7, "a worn item without a bag entry resolves") && ok;
    worn.bag.push_back({ EquipmentSlotState::Item, 8, item });
    ok         = Expect(ResolveEquipmentCatalog(worn, item).state == ItemAvailability::Ambiguous, "a worn item and a different bag copy are ambiguous") && ok;
    auto rings = CatalogInventory();
    Wear(rings, 21, 9, item);
    Wear(rings, 22, 10, item);
    ok = Expect(ResolveEquipmentCatalog(rings, item).state == ItemAvailability::Ambiguous, "two worn copies are ambiguous") && ok;

    auto unknownSlot                     = one;
    unknownSlot.equipment.slots[3].state = EquipmentSlotState::Unknown;
    auto incomplete                      = one;
    incomplete.equipment.complete        = false;
    auto unknownBag                      = one;
    unknownBag.bag.push_back({ EquipmentSlotState::Unknown, 0, 0 });
    ok = Expect(ResolveEquipmentCatalog(unknownSlot, item).state == ItemAvailability::Unavailable && ResolveEquipmentCatalog(unknownSlot, 101).state == ItemAvailability::Unavailable, "an unknown equipment slot makes the answer unavailable") && ok;
    ok = Expect(ResolveEquipmentCatalog(incomplete, item).state == ItemAvailability::Unavailable, "an incomplete equipment read makes the answer unavailable") && ok;
    ok = Expect(ResolveEquipmentCatalog(unknownBag, item).state == ItemAvailability::Unavailable && ResolveEquipmentCatalog(unknownBag, item).uid == 0, "an unknown bag entry makes the answer unavailable") && ok;

    ok                               = Expect(ResolveEquipmentCatalog(one, 0).state == ItemAvailability::Unavailable, "catalog id 0 is never resolved") && ok;
    ok                               = Expect(ResolveEquipmentCatalog(CatalogInventory(), item).state == ItemAvailability::Missing, "an absent item type is missing when every bag slot is readable") && ok;
    EquipmentInventorySnapshot noBag = one;
    noBag.bag.clear();
    ok             = Expect(ResolveEquipmentCatalog(noBag, item).state == ItemAvailability::Unavailable, "an inventory without bag data is unavailable") && ok;
    auto oversized = one;
    oversized.bag.resize(kMaximumAccessibleBagSlots + 1, BagItem{ EquipmentSlotState::Empty, 0, 0 });
    ok = Expect(ResolveEquipmentCatalog(oversized, item).state == ItemAvailability::Unavailable, "an oversized bag is unavailable") && ok;

    // The same answers through the real memory read.
    constexpr std::uint64_t wide = 0xFEDCBA9876543210ull;
    Fixture                 fixture;
    fixture.Bag(1, wide, item);
    fixture.Bag(2, 2, 200);
    fixture.Link(10, 0, 1);
    const auto snapshot = ReadEquipmentInventoryFromPlayer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player)));
    if (!Expect(snapshot.has_value(), "the catalog fixture reads"))
    {
        return false;
    }
    const auto wornAndBag = ResolveEquipmentCatalog(*snapshot, item);
    ok                    = Expect(wornAndBag.state == ItemAvailability::Available && wornAndBag.uid == wide, "a worn link and its bag entry resolve to one wide uid") && ok;
    ok                    = Expect(ResolveEquipmentCatalog(*snapshot, 200).uid == 2 && ResolveEquipmentCatalog(*snapshot, 300).state == ItemAvailability::Missing, "other types resolve independently") && ok;
    fixture.Bag(3, 3, item);
    const auto second = ReadEquipmentInventoryFromPlayer(reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player)));
    return Expect(second && ResolveEquipmentCatalog(*second, item).state == ItemAvailability::Ambiguous, "a second copy in the bag makes the type ambiguous") && ok;
}

} // namespace

int main()
{
    const bool ok = TestEquippedEmptyAndDuplicateItems() && TestUnresolvedLinksAreUnknownNeverEmpty() && TestStructuralFailuresGiveNoSnapshot() && TestServiceGenerations() && TestCharacterChangeEndsSession() && TestCaptureFromContainerChain() && TestBagItems();
    return ok && TestExactResolution() && TestCatalogResolution() ? 0 : 1;
}
