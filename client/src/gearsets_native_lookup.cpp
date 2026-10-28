#include "gearsets_native_lookup.h"

namespace
{

using namespace bahamut_client;

// Static actor lookup: client/README.md#client-evidence.
constexpr std::uint32_t kCategoryGlobal       = 0x0130D426u;
constexpr std::uint32_t kInvalidActorGlobal   = 0x0130C778u;
constexpr std::uint32_t kEngineActorIds       = 0x1C4u;
constexpr std::uint32_t kEngineActorTables    = 0x17Cu;
constexpr std::uint32_t kCategoryTableOffset  = 0x60u;
constexpr std::uint32_t kReferenceMask        = 0xF0F00000u;
constexpr std::uint32_t kMaximumCategoryNodes = 64u;

// Category list layout: client/README.md#client-evidence.
constexpr std::uint32_t kListHead     = 0x0Cu;
constexpr std::uint32_t kNodeNext     = 0x00u;
constexpr std::uint32_t kNodeCategory = 0x08u;
constexpr std::uint32_t kNodeBase     = 0x0Cu;

// Dense table layout and probe: client/README.md#client-evidence.
constexpr std::uint32_t kTableDeletedCount = 0x04u;
constexpr std::uint32_t kTableUseDeleted   = 0x08u;
constexpr std::uint32_t kTableDeletedKey   = 0x0Cu;
constexpr std::uint32_t kTableEmptyKey     = 0x14u;
constexpr std::uint32_t kTableBuckets      = 0x1Cu;
constexpr std::uint32_t kTableBucketCount  = 0x20u;
constexpr std::uint32_t kTableElements     = 0x24u;
constexpr std::uint32_t kMaximumBuckets    = 1u << 20;

using Lookup = GearsetsStaticActorLookup;
using State  = GearsetsStaticActorState;

// The category entry's base, or empty when the list has none. unreadable is
// set when the walk could not finish.
std::optional<std::uint32_t> CategoryBase(const GearsetsNativeMemory& memory, std::uint32_t actorIds, std::uint8_t category, bool& unreadable)
{
    const auto head = memory.Get<std::uint32_t>(actorIds + kListHead);
    if (!head)
    {
        unreadable = true;
        return std::nullopt;
    }
    auto node = memory.Get<std::uint32_t>(*head + kNodeNext);
    for (std::uint32_t count = 0; node && *node != *head; ++count)
    {
        if (count >= kMaximumCategoryNodes)
        {
            unreadable = true;
            return std::nullopt;
        }
        const auto key = memory.Get<std::uint8_t>(*node + kNodeCategory);
        if (!key)
        {
            break;
        }
        if (*key == category)
        {
            const auto base = memory.Get<std::uint32_t>(*node + kNodeBase);
            if (!base)
            {
                break;
            }
            return base;
        }
        node = memory.Get<std::uint32_t>(*node + kNodeNext);
    }
    unreadable = !node || *node != *head;
    return std::nullopt;
}

Lookup FindInTable(const GearsetsNativeMemory& memory, std::uint32_t table, std::uint32_t reference)
{
    Lookup     result{ State::Unreadable, reference, 0u };
    const auto deletedCount = memory.Get<std::uint32_t>(table + kTableDeletedCount);
    const auto useDeleted   = memory.Get<std::uint8_t>(table + kTableUseDeleted);
    const auto deletedKey   = memory.Get<std::uint32_t>(table + kTableDeletedKey);
    const auto emptyKey     = memory.Get<std::uint32_t>(table + kTableEmptyKey);
    const auto buckets      = memory.Get<std::uint32_t>(table + kTableBuckets);
    const auto count        = memory.Get<std::uint32_t>(table + kTableBucketCount);
    const auto elements     = memory.Get<std::uint32_t>(table + kTableElements);
    if (!deletedCount || !useDeleted || !deletedKey || !emptyKey || !buckets || !count || !elements)
    {
        return result;
    }
    if (*elements == *deletedCount)
    {
        result.state = State::NotLoaded;
        return result;
    }
    if (*count == 0u || *count > kMaximumBuckets || (*count & (*count - 1u)) != 0u)
    {
        result.state = State::Unmodeled;
        return result;
    }
    const std::uint32_t mask   = *count - 1u;
    std::uint32_t       bucket = GearsetsNativeHashBucket(reference, mask);
    for (std::uint32_t step = 0; step < *count; ++step)
    {
        const auto key = memory.Get<std::uint32_t>(*buckets + bucket * 8u);
        if (!key)
        {
            return result;
        }
        if (*key == *emptyKey)
        {
            result.state = State::NotLoaded;
            return result;
        }
        const bool deleted = *useDeleted != 0u && *deletedCount != 0u && *key == *deletedKey;
        if (!deleted && *key == reference)
        {
            const auto value = memory.Get<std::uint32_t>(*buckets + bucket * 8u + 4u);
            if (!value)
            {
                return result;
            }
            if (*value == 0u)
            {
                result.state = State::NotLoaded;
                return result;
            }
            const auto control = memory.Get<std::uint32_t>(*value);
            if (!control)
            {
                return result;
            }
            result.state   = *control != 0u ? State::Loaded : State::NotLoaded;
            result.control = *control;
            return result;
        }
        bucket = (bucket + step + 1u) & mask;
    }
    // A table without an empty bucket on this probe never ends the game's
    // loop; nothing here models that.
    result.state = State::Unmodeled;
    return result;
}

} // namespace

namespace bahamut_client
{

std::string_view GearsetsStaticActorStateName(GearsetsStaticActorState state)
{
    switch (state)
    {
        case GearsetsStaticActorState::Loaded:
            return "loaded";
        case GearsetsStaticActorState::NotLoaded:
            return "not-loaded";
        case GearsetsStaticActorState::Unreadable:
            return "unreadable";
        case GearsetsStaticActorState::Unmodeled:
            return "unmodeled";
    }
    return "unknown";
}

std::uint32_t GearsetsNativeHashBucket(std::uint32_t key, std::uint32_t mask)
{
    const auto   value     = static_cast<std::int32_t>(key ^ 0xDEADBEEFu);
    std::int32_t quotient  = value / 127773;
    std::int32_t remainder = value % 127773;
    // 0x009D6E98 adjusts toward floor only for a negative dividend with a
    // positive remainder, which truncating division never produces.
    if (value < 0 && remainder > 0)
    {
        ++quotient;
        remainder -= 127773;
    }
    std::uint32_t hash = static_cast<std::uint32_t>(remainder) * 16807u - static_cast<std::uint32_t>(quotient) * 2836u;
    if (static_cast<std::int32_t>(hash) < 0)
    {
        hash += 0x7FFFFFFFu;
    }
    return hash & mask;
}

GearsetsStaticActorLookup FindLoadedStaticActor(const GearsetsNativeMemory& memory, std::uint32_t engine, std::uint32_t actorId)
{
    Lookup     result;
    const auto category = memory.Get<std::uint8_t>(kCategoryGlobal);
    const auto invalid  = memory.Get<std::uint32_t>(kInvalidActorGlobal);
    const auto actorIds = memory.Get<std::uint32_t>(engine + kEngineActorIds);
    const auto tables   = memory.Get<std::uint32_t>(engine + kEngineActorTables);
    if (engine == 0u || !category || !invalid || !actorIds || !tables)
    {
        return result;
    }
    bool       unreadable = false;
    const auto base       = CategoryBase(memory, *actorIds, *category, unreadable);
    if (!base)
    {
        // Without the entry 0x00D03A50 would insert one: nothing of that
        // category can be loaded yet.
        result.state = unreadable ? State::Unreadable : State::NotLoaded;
        return result;
    }
    result.reference = *base + actorId;
    if (result.reference == *invalid)
    {
        result.state = State::NotLoaded;
        return result;
    }
    // 0x00D035D0 over the reference's own category, which must be the
    // current one for the +0x60 table.
    const auto ownCategory = static_cast<std::uint8_t>((result.reference >> 20) & 0x0Fu);
    if (ownCategory != *category || (result.reference & kReferenceMask) != *base)
    {
        result.state = State::Unmodeled;
        return result;
    }
    return FindInTable(memory, *tables + kCategoryTableOffset, result.reference);
}

} // namespace bahamut_client
