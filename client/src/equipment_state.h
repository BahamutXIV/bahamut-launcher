#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <mutex>
#include <optional>
#include <string_view>
#include <vector>

namespace bahamut_client
{

// Read-only; nothing here equips. Slot evidence: client/README.md#client-evidence.
inline constexpr std::size_t                                   kEquipmentSlotCount = 18;
inline constexpr std::array<std::uint8_t, kEquipmentSlotCount> kEquipmentSlots     = {
    0, 1, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 19, 21, 22
};

enum class EquipmentSlotState : std::uint8_t
{
    // The read could not establish the slot. Never equivalent to Empty.
    Unknown,
    Empty,
    Item,
};

struct EquipmentSlot
{
    std::uint8_t       slot      = 0;
    EquipmentSlotState state     = EquipmentSlotState::Unknown;
    std::uint64_t      uid       = 0; // item instance identity; 0 unless Item
    std::uint32_t      catalogId = 0; // item type, advisory; 0 unless Item

    bool operator==(const EquipmentSlot&) const = default;
};

struct EquipmentSnapshot
{
    // Assigned by EquipmentStateService; bumps when the content changes.
    std::uint64_t                                  generation = 0;
    bool                                           complete   = false; // no slot is Unknown
    std::array<EquipmentSlot, kEquipmentSlotCount> slots{};
    std::optional<std::uint16_t>                   classId;
    std::optional<std::uint16_t>                   jobId;
    // Copied with class/job on the same game-thread observation. Bahamut uses
    // the character's database ID as the player actor ID.
    std::optional<std::uint32_t> characterId;
    std::uint64_t                session = 0;
};

// Game window thread. nullopt when the item store, equipment package or bag
// package is missing or the read is unstable; unresolved slots are Unknown.
// The returned generation is 0.
std::optional<EquipmentSnapshot> ReadEquipmentFromPlayer(const void* player);

struct BagItem
{
    EquipmentSlotState state     = EquipmentSlotState::Unknown;
    std::uint64_t      uid       = 0; // 0 unless Item
    std::uint32_t      catalogId = 0; // 0 unless Item

    bool operator==(const BagItem&) const = default;
};

// Game window thread; 0-based position in the bag package (code 0). nullopt when
// the store or bag is missing or two passes disagree; out-of-range or unreadable is Unknown.
std::optional<BagItem> ReadBagItemFromPlayer(const void* player, std::uint16_t bagSlot);

// A safety budget for copied accessible bag data, not a claimed client capacity.
inline constexpr std::size_t kMaximumAccessibleBagSlots = 1024;

struct EquipmentInventorySnapshot
{
    EquipmentSnapshot    equipment;
    std::vector<BagItem> bag;
};

enum class ItemAvailability
{
    Available,
    Missing,
    Ambiguous,
    Unavailable,
};

struct ItemResolution
{
    ItemAvailability             state = ItemAvailability::Unavailable;
    std::optional<std::uint16_t> bagSlot;
    std::optional<std::uint8_t>  equippedSlot;
};

std::string_view ItemAvailabilityName(ItemAvailability state);
ItemResolution   ResolveEquipmentUid(const EquipmentInventorySnapshot& snapshot, std::uint64_t uid);

struct CatalogResolution
{
    ItemAvailability state = ItemAvailability::Unavailable;
    std::uint64_t    uid   = 0; // nonzero only when Available
};

// The single instance of an item type across equipment and accessible bag
// data. Two instances are Ambiguous; any unread slot leaves the answer
// Unavailable, since it could hold another copy.
CatalogResolution ResolveEquipmentCatalog(const EquipmentInventorySnapshot& snapshot, std::uint32_t catalogId);
// Both complete passes must agree across equipment and every accessible bag slot.
std::optional<EquipmentInventorySnapshot> ReadEquipmentInventoryFromPlayer(const void* player);

// Resolves the MyPlayer control through the player-state container chain;
// nullptr when any link is missing.
const void* ResolvePlayerFromContainer(const void* container);

// Holds the latest copied snapshot for render-thread readers.
class EquipmentStateService
{
public:
    EquipmentStateService()  = default;
    ~EquipmentStateService() = default;

    EquipmentStateService(const EquipmentStateService&)            = delete;
    EquipmentStateService& operator=(const EquipmentStateService&) = delete;

    // Stores a copy and assigns a generation; an identical snapshot keeps the
    // previous generation.
    void Publish(EquipmentSnapshot snapshot);
    void PublishInventory(EquipmentInventorySnapshot snapshot);
    // A failed read invalidates copied state without replacing the session.
    void InvalidateSnapshot();
    // Ends the session even if its last inventory read failed.
    void Clear();
    // A snapshot whose character differs from the session's earlier one also
    // ends that session before it is stored.

    [[nodiscard]] std::optional<EquipmentSnapshot>          Snapshot() const;
    [[nodiscard]] std::optional<EquipmentInventorySnapshot> Inventory() const;
    [[nodiscard]] std::uint64_t                             SessionGeneration() const;

    // Resolves MyPlayer through the player-state container chain and publishes
    // ReadEquipmentFromPlayer. Failure clears the published state.
    bool CaptureFromContainer(const void* container);

private:
    void BindCharacterLocked(const EquipmentSnapshot& snapshot);

    mutable std::mutex                        mutex_;
    std::optional<EquipmentSnapshot>          current_;
    std::optional<EquipmentInventorySnapshot> inventory_;
    std::optional<std::uint32_t>              sessionCharacter_;
    std::uint64_t                             generation_        = 0;
    std::uint64_t                             sessionGeneration_ = 1;
};

} // namespace bahamut_client
