#include "equipment_state.h"

#include "fault_guard.h"

#include <windows.h>

#include <algorithm>
#include <cstdint>
#include <cstring>

namespace
{

using namespace bahamut_client;

// Item store layout: client/README.md#client-evidence.
constexpr std::size_t   kItemStoreOffset      = 0xD8u;
constexpr std::size_t   kStorePackageMapField = 0x08u;
constexpr std::size_t   kMapHeadOffset        = 0x04u;
constexpr std::size_t   kNodeLeftOffset       = 0x00u;
constexpr std::size_t   kNodeParentOffset     = 0x04u;
constexpr std::size_t   kNodeRightOffset      = 0x08u;
constexpr std::size_t   kNodeKeyOffset        = 0x0Cu;
constexpr std::size_t   kNodeValueOffset      = 0x10u;
constexpr std::size_t   kNodeIsNilOffset      = 0x15u;
constexpr std::size_t   kPackageVectorBegin   = 0x14u;
constexpr std::size_t   kPackageVectorEnd     = 0x18u;
constexpr std::size_t   kEntryItemOffset      = 0x08u;
constexpr std::size_t   kEntryRedirectPackage = 0x0Cu;
constexpr std::size_t   kEntryRedirectSlot    = 0x10u;
constexpr std::size_t   kItemUidLowOffset     = 0x10u;
constexpr std::size_t   kItemUidHighOffset    = 0x14u;
constexpr std::size_t   kItemCatalogOffset    = 0x24u;
constexpr std::uint32_t kEquipmentPackage     = 0xFEu;
constexpr std::uint32_t kEquipmentPositions   = 35u;
constexpr std::uint32_t kBagPackage           = 0u;
constexpr std::uint32_t kUnsetRedirect        = 0xFFFFFFFFu;
constexpr int           kMaximumRedirectDepth = 4;
constexpr int           kMaximumTreeDepth     = 64;

// MyPlayer lookup: client/README.md#client-evidence.
constexpr std::size_t   kWaitingContainerOffset = 0x518u;
constexpr std::size_t   kWaitingEngineOffset    = 0x04u;
constexpr std::size_t   kEngineHolderOffset     = 0x220u;
constexpr std::size_t   kHolderStateOffset      = 0x04u;
constexpr std::size_t   kHolderPlayerOffset     = 0x0Cu;
constexpr std::uint32_t kInvalidPointer         = 0xFFFFFFFFu;
constexpr std::uint32_t kUninitializedPointer   = 0xCDCDCDCDu;

bool ReadWord(std::uint32_t address, std::uint32_t& value)
{
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(&value, reinterpret_cast<const void*>(static_cast<std::uintptr_t>(address)), sizeof(value));
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool ReadByte(std::uint32_t address, std::uint8_t& value)
{
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(&value, reinterpret_cast<const void*>(static_cast<std::uintptr_t>(address)), sizeof(value));
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool IsSentinel(std::uint32_t value)
{
    return value == 0 || value == kInvalidPointer || value == kUninitializedPointer;
}

// Reject null pointers and debug fill patterns.
bool ReadPointer(std::uint32_t base, std::size_t offset, std::uint32_t& value)
{
    return !IsSentinel(base) && ReadWord(base + static_cast<std::uint32_t>(offset), value) && !IsSentinel(value);
}

struct PackageView
{
    std::uint32_t begin = 0;
    std::uint32_t count = 0;
};

// std::map lower_bound walk without insertion. Depth is bounded and the walk
// must end on the nil head node, so a corrupted tree cannot loop or match.
bool FindPackage(std::uint32_t head, std::uint32_t code, std::uint32_t& package)
{
    std::uint32_t node = 0;
    if (!ReadPointer(head, kNodeParentOffset, node))
    {
        return false;
    }
    std::uint32_t candidate  = 0;
    bool          reachedNil = false;
    for (int depth = 0; depth < kMaximumTreeDepth && !reachedNil; ++depth)
    {
        std::uint8_t isNil = 0;
        if (IsSentinel(node) || !ReadByte(node + static_cast<std::uint32_t>(kNodeIsNilOffset), isNil))
        {
            return false;
        }
        if (isNil != 0)
        {
            reachedNil = true;
            continue;
        }
        std::uint32_t key  = 0;
        std::uint32_t next = 0;
        if (!ReadWord(node + static_cast<std::uint32_t>(kNodeKeyOffset), key))
        {
            return false;
        }
        const bool goLeft = key >= code;
        if (goLeft)
        {
            candidate = node;
        }
        if (!ReadWord(node + static_cast<std::uint32_t>(goLeft ? kNodeLeftOffset : kNodeRightOffset), next))
        {
            return false;
        }
        node = next;
    }
    std::uint32_t foundKey = 0;
    if (!reachedNil || candidate == 0 || !ReadWord(candidate + static_cast<std::uint32_t>(kNodeKeyOffset), foundKey) || foundKey != code)
    {
        return false;
    }
    return ReadPointer(candidate, kNodeValueOffset, package);
}

bool OpenPackage(std::uint32_t head, std::uint32_t code, PackageView& view)
{
    std::uint32_t package = 0;
    std::uint32_t begin   = 0;
    std::uint32_t end     = 0;
    if (!FindPackage(head, code, package) || !ReadPointer(package, kPackageVectorBegin, begin) || !ReadWord(package + static_cast<std::uint32_t>(kPackageVectorEnd), end) || end < begin ||
        (end - begin) % sizeof(std::uint32_t) != 0)
    {
        return false;
    }
    view.begin = begin;
    view.count = (end - begin) / sizeof(std::uint32_t);
    return true;
}

bool ReadEntry(const PackageView& view, std::uint32_t index, std::uint32_t& entry)
{
    return index < view.count && ReadWord(view.begin + index * static_cast<std::uint32_t>(sizeof(std::uint32_t)), entry);
}

// Resolves one equipment position. Unknown covers every case where a link
// claims an item that cannot be read, so such a position never saves as Empty.
EquipmentSlot ResolveSlot(std::uint32_t head, const PackageView& equipment, std::uint8_t slot)
{
    EquipmentSlot result;
    result.slot = slot;

    PackageView   current = equipment;
    std::uint32_t index   = slot;
    for (int depth = 0; depth <= kMaximumRedirectDepth; ++depth)
    {
        std::uint32_t entry = 0;
        if (!ReadEntry(current, index, entry))
        {
            return result;
        }
        if (entry == 0)
        {
            // A null position in the equipment package is empty; a null
            // position behind a link means the link dangles.
            if (depth == 0)
            {
                result.state = EquipmentSlotState::Empty;
            }
            return result;
        }
        std::uint32_t item = 0;
        if (!ReadWord(entry + static_cast<std::uint32_t>(kEntryItemOffset), item))
        {
            return result;
        }
        if (item != 0)
        {
            std::uint32_t low     = 0;
            std::uint32_t high    = 0;
            std::uint32_t catalog = 0;
            if (IsSentinel(item) || !ReadWord(item + static_cast<std::uint32_t>(kItemUidLowOffset), low) || !ReadWord(item + static_cast<std::uint32_t>(kItemUidHighOffset), high) || !ReadWord(item + static_cast<std::uint32_t>(kItemCatalogOffset), catalog))
            {
                return result;
            }
            const std::uint64_t uid = (static_cast<std::uint64_t>(high) << 32u) | low;
            if (uid == 0)
            {
                return result;
            }
            result.state     = EquipmentSlotState::Item;
            result.uid       = uid;
            result.catalogId = catalog;
            return result;
        }
        std::uint32_t redirectPackage = 0;
        std::uint32_t redirectSlot    = 0;
        if (!ReadWord(entry + static_cast<std::uint32_t>(kEntryRedirectPackage), redirectPackage) || !ReadWord(entry + static_cast<std::uint32_t>(kEntryRedirectSlot), redirectSlot))
        {
            return result;
        }
        if (redirectPackage == kUnsetRedirect && redirectSlot == kUnsetRedirect)
        {
            if (depth == 0)
            {
                result.state = EquipmentSlotState::Empty;
            }
            return result;
        }
        if (redirectPackage == kUnsetRedirect || redirectSlot == kUnsetRedirect || !OpenPackage(head, redirectPackage, current))
        {
            return result;
        }
        index = redirectSlot;
    }
    return result;
}

std::optional<EquipmentSnapshot> ReadOnce(const void* player)
{
    const std::uint32_t playerAddress = static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(player));
    std::uint32_t       store         = 0;
    std::uint32_t       head          = 0;
    if (!ReadPointer(playerAddress, kItemStoreOffset, store) || !ReadPointer(store, kStorePackageMapField + kMapHeadOffset, head))
    {
        return std::nullopt;
    }
    PackageView equipment;
    PackageView bag;
    // Both packages must exist and the equipment package must have its full
    // position count. A store that exists but has not received the login
    // inventory must not read as an all-empty character.
    if (!OpenPackage(head, kEquipmentPackage, equipment) || equipment.count != kEquipmentPositions || !OpenPackage(head, kBagPackage, bag) || bag.count == 0)
    {
        return std::nullopt;
    }

    EquipmentSnapshot snapshot;
    snapshot.complete = true;
    for (std::size_t index = 0; index < kEquipmentSlotCount; ++index)
    {
        snapshot.slots[index] = ResolveSlot(head, equipment, kEquipmentSlots[index]);
        if (snapshot.slots[index].state == EquipmentSlotState::Unknown)
        {
            snapshot.complete = false;
        }
    }
    return snapshot;
}

// Bag positions hold items directly; a redirect there is not an observed
// shape, so it reads as Unknown rather than being followed.
BagItem ReadBagSlot(const PackageView& bag, std::uint16_t bagSlot)
{
    BagItem       result;
    std::uint32_t entry = 0;
    if (!ReadEntry(bag, bagSlot, entry))
    {
        return result;
    }
    if (entry == 0)
    {
        result.state = EquipmentSlotState::Empty;
        return result;
    }
    std::uint32_t item            = 0;
    std::uint32_t redirectPackage = 0;
    std::uint32_t redirectSlot    = 0;
    if (!ReadWord(entry + static_cast<std::uint32_t>(kEntryItemOffset), item))
    {
        return result;
    }
    if (item == 0)
    {
        if (ReadWord(entry + static_cast<std::uint32_t>(kEntryRedirectPackage), redirectPackage) && ReadWord(entry + static_cast<std::uint32_t>(kEntryRedirectSlot), redirectSlot) && redirectPackage == kUnsetRedirect && redirectSlot == kUnsetRedirect)
        {
            result.state = EquipmentSlotState::Empty;
        }
        return result;
    }
    std::uint32_t low     = 0;
    std::uint32_t high    = 0;
    std::uint32_t catalog = 0;
    if (IsSentinel(item) || !ReadWord(item + static_cast<std::uint32_t>(kItemUidLowOffset), low) || !ReadWord(item + static_cast<std::uint32_t>(kItemUidHighOffset), high) || !ReadWord(item + static_cast<std::uint32_t>(kItemCatalogOffset), catalog))
    {
        return result;
    }
    const std::uint64_t uid = (static_cast<std::uint64_t>(high) << 32u) | low;
    if (uid != 0)
    {
        result.state     = EquipmentSlotState::Item;
        result.uid       = uid;
        result.catalogId = catalog;
    }
    return result;
}

bool OpenBag(const void* player, PackageView& bag)
{
    const auto    address = static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(player));
    std::uint32_t store   = 0;
    std::uint32_t head    = 0;
    return ReadPointer(address, kItemStoreOffset, store) && ReadPointer(store, kStorePackageMapField + kMapHeadOffset, head) &&
           OpenPackage(head, kBagPackage, bag) && bag.count != 0;
}

std::optional<BagItem> ReadBagOnce(const void* player, std::uint16_t bagSlot)
{
    PackageView bag;
    return OpenBag(player, bag) ? std::optional<BagItem>(ReadBagSlot(bag, bagSlot)) : std::nullopt;
}

std::optional<EquipmentInventorySnapshot> ReadInventoryOnce(const void* player)
{
    const auto  equipment = ReadOnce(player);
    PackageView bag;
    if (!equipment || !OpenBag(player, bag) || bag.count > kMaximumAccessibleBagSlots)
    {
        return std::nullopt;
    }
    EquipmentInventorySnapshot result{ *equipment, {} };
    result.bag.reserve(bag.count);
    for (std::uint16_t slot = 0; slot < bag.count; ++slot)
    {
        result.bag.push_back(ReadBagSlot(bag, slot));
    }
    return result;
}

bool SameContent(const EquipmentSnapshot& left, const EquipmentSnapshot& right)
{
    return left.complete == right.complete && left.slots == right.slots && left.classId == right.classId && left.jobId == right.jobId &&
           left.characterId == right.characterId;
}

} // namespace

namespace bahamut_client
{

std::string_view ItemAvailabilityName(ItemAvailability state)
{
    switch (state)
    {
        case ItemAvailability::Available:
            return "available";
        case ItemAvailability::Missing:
            return "missing";
        case ItemAvailability::Ambiguous:
            return "ambiguous";
        case ItemAvailability::Unavailable:
            return "unavailable";
    }
    return "unavailable";
}

ItemResolution ResolveEquipmentUid(const EquipmentInventorySnapshot& snapshot, std::uint64_t uid)
{
    ItemResolution result;
    if (uid == 0 || snapshot.bag.empty() || snapshot.bag.size() > kMaximumAccessibleBagSlots)
    {
        return result;
    }
    bool          complete     = snapshot.equipment.complete;
    std::uint32_t catalog      = 0;
    bool          found        = false;
    bool          ambiguous    = false;
    const auto    checkCatalog = [&](std::uint32_t value)
    {
        if (found && catalog != value)
            ambiguous = true;
        catalog = value;
        found   = true;
    };
    for (std::size_t index = 0; index < snapshot.bag.size(); ++index)
    {
        const auto& item = snapshot.bag[index];
        complete         = complete && item.state != EquipmentSlotState::Unknown;
        if (item.state == EquipmentSlotState::Item && item.uid == uid)
        {
            ambiguous      = ambiguous || result.bagSlot.has_value();
            result.bagSlot = static_cast<std::uint16_t>(index);
            checkCatalog(item.catalogId);
        }
    }
    for (const auto& item : snapshot.equipment.slots)
    {
        complete = complete && item.state != EquipmentSlotState::Unknown;
        if (item.state == EquipmentSlotState::Item && item.uid == uid)
        {
            ambiguous           = ambiguous || result.equippedSlot.has_value();
            result.equippedSlot = item.slot;
            checkCatalog(item.catalogId);
        }
    }
    // A worn link and its bag entry name one instance, not two copies.
    result.state = ambiguous ? ItemAvailability::Ambiguous : !complete ? ItemAvailability::Unavailable
                                                         : found       ? ItemAvailability::Available
                                                                       : ItemAvailability::Missing;
    if (result.state != ItemAvailability::Available)
    {
        result.bagSlot.reset();
        result.equippedSlot.reset();
    }
    return result;
}

CatalogResolution ResolveEquipmentCatalog(const EquipmentInventorySnapshot& snapshot, std::uint32_t catalogId)
{
    CatalogResolution result;
    if (catalogId == 0 || snapshot.bag.empty() || snapshot.bag.size() > kMaximumAccessibleBagSlots)
    {
        return result;
    }
    bool          complete  = snapshot.equipment.complete;
    std::uint64_t uid       = 0;
    bool          twoCopies = false;
    const auto    consider  = [&](EquipmentSlotState state, std::uint64_t itemUid, std::uint32_t itemCatalog)
    {
        complete = complete && state != EquipmentSlotState::Unknown;
        if (state != EquipmentSlotState::Item || itemCatalog != catalogId)
            return;
        // A worn link and its bag entry name one instance, not two copies.
        twoCopies = twoCopies || (uid != 0 && uid != itemUid);
        uid       = itemUid;
    };
    for (const auto& item : snapshot.bag)
    {
        consider(item.state, item.uid, item.catalogId);
    }
    for (const auto& item : snapshot.equipment.slots)
    {
        consider(item.state, item.uid, item.catalogId);
    }
    result.state = !complete ? ItemAvailability::Unavailable : twoCopies ? ItemAvailability::Ambiguous
                                                           : uid != 0    ? ItemAvailability::Available
                                                                         : ItemAvailability::Missing;
    result.uid   = result.state == ItemAvailability::Available ? uid : 0;
    return result;
}

std::optional<EquipmentInventorySnapshot> ReadEquipmentInventoryFromPlayer(const void* player)
{
    if (player == nullptr)
        return std::nullopt;
    const auto first  = ReadInventoryOnce(player);
    const auto second = first ? ReadInventoryOnce(player) : std::nullopt;
    if (!first || !second || !SameContent(first->equipment, second->equipment) || first->bag != second->bag)
    {
        return std::nullopt;
    }
    return first;
}

std::optional<EquipmentSnapshot> ReadEquipmentFromPlayer(const void* player)
{
    if (player == nullptr)
    {
        return std::nullopt;
    }
    // Two passes must agree to reject changes during the read.
    const auto first = ReadOnce(player);
    if (!first)
    {
        return std::nullopt;
    }
    const auto second = ReadOnce(player);
    if (!second || !SameContent(*first, *second))
    {
        return std::nullopt;
    }
    return first;
}

std::optional<BagItem> ReadBagItemFromPlayer(const void* player, std::uint16_t bagSlot)
{
    if (player == nullptr)
    {
        return std::nullopt;
    }
    const auto first = ReadBagOnce(player, bagSlot);
    if (!first)
    {
        return std::nullopt;
    }
    const auto second = ReadBagOnce(player, bagSlot);
    if (!second || *first != *second)
    {
        return std::nullopt;
    }
    return first;
}

const void* ResolvePlayerFromContainer(const void* container)
{
    if (container == nullptr)
    {
        return nullptr;
    }
    const std::uint32_t containerAddress = static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(container));
    std::uint32_t       waiting          = 0;
    std::uint32_t       engine           = 0;
    std::uint32_t       holder           = 0;
    std::uint32_t       holderState      = 0;
    std::uint32_t       player           = 0;
    if (!ReadPointer(containerAddress, kWaitingContainerOffset, waiting) || !ReadPointer(waiting, kWaitingEngineOffset, engine) || !ReadPointer(engine, kEngineHolderOffset, holder) || !ReadPointer(holder, kHolderStateOffset, holderState) || !ReadPointer(holderState, kHolderPlayerOffset, player))
    {
        return nullptr;
    }
    return reinterpret_cast<const void*>(static_cast<std::uintptr_t>(player));
}

void EquipmentStateService::BindCharacterLocked(const EquipmentSnapshot& snapshot)
{
    if (!snapshot.characterId)
    {
        return;
    }
    if (sessionCharacter_ && *sessionCharacter_ != *snapshot.characterId)
    {
        ++sessionGeneration_;
        current_.reset();
        inventory_.reset();
    }
    sessionCharacter_ = snapshot.characterId;
}

void EquipmentStateService::Publish(EquipmentSnapshot snapshot)
{
    std::lock_guard<std::mutex> lock(mutex_);
    BindCharacterLocked(snapshot);
    inventory_.reset();
    if (current_ && SameContent(*current_, snapshot))
    {
        return;
    }
    snapshot.generation = ++generation_;
    snapshot.session    = sessionGeneration_;
    current_            = snapshot;
}

void EquipmentStateService::InvalidateSnapshot()
{
    std::lock_guard<std::mutex> lock(mutex_);
    current_.reset();
    inventory_.reset();
}

void EquipmentStateService::Clear()
{
    std::lock_guard<std::mutex> lock(mutex_);
    ++sessionGeneration_;
    current_.reset();
    inventory_.reset();
    sessionCharacter_.reset();
}

std::uint64_t EquipmentStateService::SessionGeneration() const
{
    std::lock_guard lock(mutex_);
    return sessionGeneration_;
}

void EquipmentStateService::PublishInventory(EquipmentInventorySnapshot snapshot)
{
    std::lock_guard lock(mutex_);
    BindCharacterLocked(snapshot.equipment);
    snapshot.equipment.generation = current_ && SameContent(*current_, snapshot.equipment) ? current_->generation : ++generation_;
    snapshot.equipment.session    = sessionGeneration_;
    current_                      = snapshot.equipment;
    inventory_                    = std::move(snapshot);
}

std::optional<EquipmentInventorySnapshot> EquipmentStateService::Inventory() const
{
    std::lock_guard lock(mutex_);
    return inventory_;
}

std::optional<EquipmentSnapshot> EquipmentStateService::Snapshot() const
{
    std::lock_guard<std::mutex> lock(mutex_);
    return current_;
}

bool EquipmentStateService::CaptureFromContainer(const void* container)
{
    const void* player   = ResolvePlayerFromContainer(container);
    auto        snapshot = player != nullptr ? ReadEquipmentFromPlayer(player) : std::nullopt;
    if (!snapshot)
    {
        Clear();
        return false;
    }
    Publish(*snapshot);
    return true;
}

} // namespace bahamut_client
