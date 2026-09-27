#include "player_state.h"
#include "fault_guard.h"

#include "area_names.h"
#include "packet_observer.h"
#include "runtime_contract.h"

#include <MinHook.h>

#include <windows.h>

#include <algorithm>
#include <array>
#include <bit>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string>
#include <string_view>
#include <vector>

namespace
{

using namespace bahamut_client;

// Client references: XIVLegacy/xivl-client-structs:manifests/symbols.json
// BCS-Y-1711; manifests/structs.json RaptureElementContainer/MapLayoutElement;
// BCS-Y-0659 SetMap packet+0x14 zone argument.
// Zone lifecycle: ffxivgame.exe SHA-256
// 9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9;
// Ghidra 12.1.3 read-only ghidra/DumpVAs.java at VAs 0x0059E3C0,
// 0x0059E6D0, 0x0059C8F0, 0x0059D0D0: pending +0xAC commits to map
// state +0x94; reset clears both. The SetMap zone argument is at +0x98.
// manifests/control_class_napi_field_access_recursive.json `_getPosition`.
constexpr std::size_t kActiveActorPointerOffset = 0x17838u;
constexpr std::size_t kActiveActorIdOffset      = 0x1783Cu;
constexpr std::size_t kMapLayoutCacheOffset     = 0x4E0u;
constexpr std::size_t kMapStateIdOffset         = 0x94u;
constexpr std::size_t kZoneIdOffset             = 0x98u;
constexpr std::size_t kPendingMapStateIdOffset  = 0xACu;
constexpr std::size_t kActorXOffset             = 0x9Cu;
constexpr std::size_t kActorYOffset             = 0xA0u;
constexpr std::size_t kActorZOffset             = 0xA4u;
constexpr std::size_t kActorParentOffset        = 0x98u;
constexpr std::size_t kActorFacingOffset        = 0xACu;
// XIVLegacy/xivl-client-structs:manifests/local_player_display_name.json.
constexpr std::size_t   kWaitingContainerOffset  = 0x518u;
constexpr std::size_t   kWaitingEngineOffset     = 0x04u;
constexpr std::size_t   kEngineHolderOffset      = 0x220u;
constexpr std::size_t   kHolderStateOffset       = 0x04u;
constexpr std::size_t   kHolderPlayerOffset      = 0x0Cu;
constexpr std::size_t   kPlayerNameStateOffset   = 0x60u;
constexpr std::size_t   kNameDataOffset          = 0x04u;
constexpr std::size_t   kNameCapacityOffset      = 0x08u;
constexpr std::size_t   kNameCountOffset         = 0x0Cu;
constexpr std::size_t   kNameOwnershipOffset     = 0x15u;
constexpr std::size_t   kNameInlineBufferOffset  = 0x16u;
constexpr std::size_t   kNameKindOffset          = 0x58u;
constexpr std::size_t   kNameHasNoIdOffset       = 0x59u;
constexpr std::size_t   kMaximumDisplayNameBytes = 32u;
constexpr std::size_t   kMaximumAreaNameBytes    = kPlayerStateAreaNameBytes - 1u;
constexpr std::uint32_t kInvalidPointer          = 0xFFFFFFFFu;
constexpr std::uint32_t kInvalidActorId          = 0xFFFFFFFFu;
constexpr std::uint32_t kUninitializedPointer    = 0xCDCDCDCDu;
constexpr std::uint32_t kActiveActorSentinel     = 0xC0000000u;
constexpr std::size_t   kMaxPositionParentDepth  = 32u;
// XIVLegacy/xivl-client-structs:manifests/property_stream_hash_catalog.json
// and manifests/pcap_opcode_coverage_matrix.json (0x0137 source actor).
constexpr std::uint16_t kActorPropertyOpcode    = 0x0137u;
constexpr std::uint16_t kSetCurrentJobOpcode    = 0x01A4u;
constexpr std::uint16_t kSetMapOpcode           = 0x0005u;
constexpr std::uint32_t kMainSkillProperty      = 0x7532CE24u;
constexpr std::uint32_t kMainSkillLevelProperty = 0x96063588u;
constexpr std::size_t   kMaximumObservedActors  = 2048u;
constexpr double        kPi                     = 3.1415926535897932384626433832795;
constexpr double        kTwoPi                  = 2.0 * kPi;

constexpr std::array<std::uint8_t, kPlayerStateFunctionPrologueLength>
    kPlayerStateFunctionPrologue = {
        0x6A,
        0xFF,
        0x68,
        0xEE,
        0xD6,
        0xE5,
        0x00,
        0x64,
        0xA1,
        0x00,
        0x00,
        0x00,
        0x00,
        0x50,
        0x83,
        0xEC,
        0x60,
    };

using PlayerStateUpdateFunction = bool(__thiscall*)(void* container);

std::atomic<PlayerStateService*> gPlayerStateService        = nullptr;
PlayerStateUpdateFunction        gOriginalPlayerStateUpdate = nullptr;

bool HasValue(const wchar_t* name)
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(name, value, ARRAYSIZE(value));
    return count != 0 && count < ARRAYSIZE(value) && value[0] != L'0';
}

bool IsSentinel(std::uint32_t value)
{
    return value == 0 || value == kInvalidPointer || value == kUninitializedPointer || value == kActiveActorSentinel;
}

struct ClassLevelUpdate
{
    std::optional<std::uint16_t> baseClassId;
    std::optional<std::uint16_t> level;
};

std::optional<ClassLevelUpdate> ReadClassLevel(
    const std::vector<std::uint8_t>& data)
{
    if (data.empty())
    {
        return std::nullopt;
    }
    const std::size_t end = static_cast<std::size_t>(data[0]) + 1u;
    if (end > data.size())
    {
        return std::nullopt;
    }
    std::size_t valuesEnd = 0;
    for (std::string_view target : { std::string_view("/_init"),
                                     std::string_view("charaWork/stateForAll"),
                                     std::string_view("charaWork/stateAtQuicklyForAll") })
    {
        if (end < target.size() + 2u)
        {
            continue;
        }
        const std::size_t  markerOffset = end - target.size() - 1u;
        const std::uint8_t marker       = data[markerOffset];
        if ((marker == 0x82u + target.size() ||
             marker == 0x60u + target.size() ||
             marker == 0xA4u + target.size()) &&
            std::memcmp(data.data() + markerOffset + 1u,
                        target.data(),
                        target.size()) == 0)
        {
            valuesEnd = markerOffset;
            break;
        }
    }
    if (valuesEnd == 0u)
    {
        return std::nullopt;
    }
    ClassLevelUpdate update;
    for (std::size_t offset = 1u; offset < valuesEnd;)
    {
        if (valuesEnd - offset < 5u)
        {
            return std::nullopt;
        }
        const std::size_t width = data[offset];
        if (width > valuesEnd - offset - 5u)
        {
            return std::nullopt;
        }
        std::uint32_t propertyId = 0;
        std::memcpy(&propertyId, data.data() + offset + 1u, sizeof(propertyId));
        if (propertyId == kMainSkillProperty)
        {
            if (width != 1u || update.baseClassId)
            {
                return std::nullopt;
            }
            update.baseClassId = static_cast<std::uint16_t>(data[offset + 5u]);
        }
        else if (propertyId == kMainSkillLevelProperty)
        {
            if (width != 2u || update.level)
            {
                return std::nullopt;
            }
            update.level = static_cast<std::uint16_t>(
                static_cast<std::uint16_t>(data[offset + 5u]) |
                static_cast<std::uint16_t>(data[offset + 6u] << 8u));
        }
        offset += 5u + width;
    }
    if (!update.baseClassId && !update.level)
    {
        return std::nullopt;
    }
    return update;
}

template <typename T>
bool ReadValue(const void* base, std::size_t offset, T& value)
{
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(&value,
                    reinterpret_cast<const std::uint8_t*>(base) + offset,
                    sizeof(value));
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool ReadBytes(const void* source, void* destination, std::size_t length)
{
    if (source == nullptr || destination == nullptr)
    {
        return false;
    }
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(destination, source, length);
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool ReadPointer(const void* base, std::size_t offset, std::uint32_t& value)
{
    return base != nullptr && ReadValue(base, offset, value) && !IsSentinel(value);
}

bool IsValidUtf8(std::string_view value)
{
    std::size_t index = 0;
    while (index < value.size())
    {
        const unsigned char first             = static_cast<unsigned char>(value[index]);
        std::size_t         continuationCount = 0;
        unsigned char       minimum           = 0x80;
        unsigned char       maximum           = 0xbf;
        if (first <= 0x7f)
        {
            ++index;
            continue;
        }
        if (first >= 0xc2 && first <= 0xdf)
        {
            continuationCount = 1;
        }
        else if (first >= 0xe0 && first <= 0xef)
        {
            continuationCount = 2;
            if (first == 0xe0)
                minimum = 0xa0;
            if (first == 0xed)
                maximum = 0x9f;
        }
        else if (first >= 0xf0 && first <= 0xf4)
        {
            continuationCount = 3;
            if (first == 0xf0)
                minimum = 0x90;
            if (first == 0xf4)
                maximum = 0x8f;
        }
        else
        {
            return false;
        }
        if (index + continuationCount >= value.size())
        {
            return false;
        }
        const unsigned char second = static_cast<unsigned char>(value[index + 1]);
        if (second < minimum || second > maximum)
        {
            return false;
        }
        for (std::size_t offset = 2; offset <= continuationCount; ++offset)
        {
            const unsigned char continuation = static_cast<unsigned char>(
                value[index + offset]);
            if (continuation < 0x80 || continuation > 0xbf)
            {
                return false;
            }
        }
        index += continuationCount + 1;
    }
    return true;
}

struct DisplayNameHeader
{
    std::int32_t  displayNameId      = 0;
    std::uint32_t data               = 0;
    std::uint32_t capacity           = 0;
    std::uint32_t count              = 0;
    std::uint8_t  ownership          = 0;
    std::uint8_t  kind               = 0;
    std::uint8_t  hasNoDisplayNameId = 0;
};

bool ReadDisplayNameHeader(const void* state, DisplayNameHeader& header)
{
    return state != nullptr && ReadValue(state, 0u, header.displayNameId) && ReadValue(state, kNameDataOffset, header.data) && ReadValue(state, kNameCapacityOffset, header.capacity) && ReadValue(state, kNameCountOffset, header.count) && ReadValue(state, kNameOwnershipOffset, header.ownership) && ReadValue(state, kNameKindOffset, header.kind) && ReadValue(state, kNameHasNoIdOffset, header.hasNoDisplayNameId);
}

bool SameDisplayNameHeader(
    const DisplayNameHeader& left, const DisplayNameHeader& right)
{
    return left.displayNameId == right.displayNameId && left.data == right.data && left.capacity == right.capacity && left.count == right.count && left.ownership == right.ownership && left.kind == right.kind && left.hasNoDisplayNameId == right.hasNoDisplayNameId;
}

bool IsUsableDisplayNameHeader(
    const void* state, const DisplayNameHeader& header)
{
    if (header.displayNameId != -1 || header.hasNoDisplayNameId != 1u || IsSentinel(header.data) || header.count == 0u || header.count > header.capacity || header.count > kMaximumDisplayNameBytes + 1u || (header.ownership != 0u && header.ownership != 1u))
    {
        return false;
    }
    const std::uint32_t inlineAddress = static_cast<std::uint32_t>(
        reinterpret_cast<std::uintptr_t>(state) + kNameInlineBufferOffset);
    return header.ownership != 1u || header.data == inlineAddress;
}

std::optional<std::string> ReadDisplayName(const void* container)
{
    struct PointerChain
    {
        std::uint32_t waiting     = 0;
        std::uint32_t engine      = 0;
        std::uint32_t holder      = 0;
        std::uint32_t holderState = 0;
        std::uint32_t player      = 0;
        std::uint32_t nameState   = 0;

        bool operator==(const PointerChain&) const = default;
    };

    const auto resolve = [container](PointerChain& chain)
    {
        return ReadPointer(container, kWaitingContainerOffset, chain.waiting) && ReadPointer(reinterpret_cast<const void*>(chain.waiting), kWaitingEngineOffset, chain.engine) && ReadPointer(reinterpret_cast<const void*>(chain.engine), kEngineHolderOffset, chain.holder) && ReadPointer(reinterpret_cast<const void*>(chain.holder), kHolderStateOffset, chain.holderState) && ReadPointer(reinterpret_cast<const void*>(chain.holderState), kHolderPlayerOffset, chain.player) && ReadPointer(reinterpret_cast<const void*>(chain.player), kPlayerNameStateOffset, chain.nameState);
    };

    PointerChain firstChain;
    PointerChain secondChain;
    if (!resolve(firstChain))
    {
        return std::nullopt;
    }

    const void*                                     state = reinterpret_cast<const void*>(firstChain.nameState);
    DisplayNameHeader                               first;
    DisplayNameHeader                               second;
    DisplayNameHeader                               third;
    std::array<char, kMaximumDisplayNameBytes + 1u> firstCopy{};
    std::array<char, kMaximumDisplayNameBytes + 1u> secondCopy{};
    if (!ReadDisplayNameHeader(state, first) || !IsUsableDisplayNameHeader(state, first) || !ReadBytes(reinterpret_cast<const void*>(first.data), firstCopy.data(), first.count) || !ReadDisplayNameHeader(state, second) || !IsUsableDisplayNameHeader(state, second) || !ReadBytes(reinterpret_cast<const void*>(second.data), secondCopy.data(), second.count) || !ReadDisplayNameHeader(state, third) || !resolve(secondChain) || firstChain != secondChain || !SameDisplayNameHeader(first, second) || !SameDisplayNameHeader(second, third) || !std::equal(firstCopy.begin(), firstCopy.begin() + first.count, secondCopy.begin()) || firstCopy[first.count - 1u] != '\0')
    {
        return std::nullopt;
    }

    std::string result(firstCopy.data(), first.count - 1u);
    if (result.empty() || result.find('\0') != std::string::npos || !IsValidUtf8(result))
    {
        return std::nullopt;
    }
    return result;
}

bool ReadPrologue(const void* address)
{
    std::array<std::uint8_t, kPlayerStateFunctionPrologueLength> bytes{};
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(bytes.data(), address, bytes.size());
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
    return bytes == kPlayerStateFunctionPrologue;
}

bool __fastcall HookedPlayerStateUpdate(void* container, void*)
{
    bool result = false;
    if (gOriginalPlayerStateUpdate != nullptr)
    {
        result = gOriginalPlayerStateUpdate(container);
    }

    PlayerStateService* service =
        gPlayerStateService.load(std::memory_order_acquire);
    if (service != nullptr)
    {
        static_cast<void>(service->CaptureFromContainer(container));
    }

    return result;
}

} // namespace

namespace bahamut_client
{

static_assert(kPlayerStateFunctionVa == bahamut_runtime_contract::kRetailImageBase + kPlayerStateFunctionRva,
              "the player-state hook VA and RVA must identify the same retail build address");

bool PlayerStateService::Publish(const PlayerStateSnapshot& snapshot)
{
    if (snapshot.actorId == 0 || snapshot.actorId == kInvalidActorId || snapshot.actorId == kActiveActorSentinel || snapshot.zoneId == 0 || snapshot.zoneId == kInvalidActorId || !std::isfinite(snapshot.x) || !std::isfinite(snapshot.y) || !std::isfinite(snapshot.z) || !std::isfinite(snapshot.rotation) || snapshot.displayName.size() > kMaximumDisplayNameBytes || snapshot.displayName.find('\0') != std::string::npos || !IsValidUtf8(snapshot.displayName) || snapshot.areaName.size() > kMaximumAreaNameBytes || snapshot.areaName.find('\0') != std::string::npos || !IsValidUtf8(snapshot.areaName))
    {
        Clear();
        return false;
    }

    PublishUnchecked(snapshot);
    return true;
}

void PlayerStateService::Clear()
{
    std::lock_guard<std::mutex> lock(observedMutex_);
    observedEpoch_.fetch_add(1u, std::memory_order_release);
    observed_.clear();
    wireZoneId_ = 0u;
    PublishUnchecked(PlayerStateSnapshot{});
}

void PlayerStateService::Observe(const packet_observer::GameMessage& message)
{
    if (message.direction != packet_observer::Direction::Incoming ||
        IsSentinel(message.sourceId))
    {
        return;
    }

    if (message.opcode == kSetMapOpcode)
    {
        if (message.payload.size() != 16u)
        {
            return;
        }
        std::uint32_t zoneId = 0;
        std::memcpy(&zoneId, message.payload.data() + 4u, sizeof(zoneId));
        if (IsSentinel(zoneId))
        {
            return;
        }
        std::lock_guard<std::mutex> lock(observedMutex_);
        const auto                  current = Snapshot();
        observedEpoch_.fetch_add(1u, std::memory_order_release);
        observed_.clear();
        wireZoneId_ = zoneId;
        if (current && current->zoneId != zoneId)
        {
            PublishUnchecked(PlayerStateSnapshot{});
        }
        return;
    }
    if (message.opcode != kActorPropertyOpcode &&
        message.opcode != kSetCurrentJobOpcode)
    {
        return;
    }

    std::optional<ClassLevelUpdate> classLevel;
    if (message.opcode == kActorPropertyOpcode)
    {
        classLevel = ReadClassLevel(message.payload);
        if (!classLevel)
        {
            return;
        }
    }
    else if (message.payload.size() != 8u)
    {
        return;
    }

    const std::uint32_t         epoch   = observedEpoch_.load(std::memory_order_acquire);
    const auto                  current = Snapshot();
    std::lock_guard<std::mutex> lock(observedMutex_);
    if (observedEpoch_.load(std::memory_order_relaxed) != epoch)
    {
        return;
    }
    const std::uint32_t zoneId = wireZoneId_ != 0u
                                     ? wireZoneId_
                                     : (current && current->actorId == message.sourceId
                                            ? current->zoneId
                                            : 0u);
    if (observed_.size() >= kMaximumObservedActors &&
        !observed_.contains(message.sourceId))
    {
        observed_.clear();
    }
    auto& observed = observed_[message.sourceId];
    if (zoneId != 0u && observed.zoneId != 0u &&
        observed.zoneId != zoneId)
    {
        observed = {};
    }
    if (zoneId != 0u)
    {
        observed.zoneId = zoneId;
    }
    if (classLevel)
    {
        if (classLevel->baseClassId)
        {
            observed.baseClassId = *classLevel->baseClassId;
        }
        if (classLevel->level)
        {
            observed.level = *classLevel->level;
        }
    }
    else
    {
        observed.jobId = message.payload[0];
    }
}

void PlayerStateService::MergeObserved(PlayerStateSnapshot& snapshot,
                                       bool                 identityChanged)
{
    // The caller holds observedMutex_ until the merged snapshot is published.
    auto found = observed_.find(snapshot.actorId);
    if (found != observed_.end() && found->second.zoneId != 0u &&
        found->second.zoneId != snapshot.zoneId)
    {
        observed_.erase(found);
        found = observed_.end();
    }
    if (identityChanged)
    {
        observedEpoch_.fetch_add(1u, std::memory_order_release);
        ObservedClassJob retained{};
        const bool       hasRetained = found != observed_.end();
        if (hasRetained)
        {
            retained = found->second;
        }
        observed_.clear();
        if (hasRetained)
        {
            found = observed_.emplace(snapshot.actorId, retained).first;
        }
    }
    if (found == observed_.end())
    {
        return;
    }
    found->second.zoneId = snapshot.zoneId;
    snapshot.baseClassId = found->second.baseClassId;
    snapshot.jobId       = found->second.jobId;
    snapshot.level       = found->second.level;
}

std::optional<PlayerStateSnapshot> PlayerStateService::Snapshot() const
{
    for (std::size_t attempt = 0; attempt != 128u; ++attempt)
    {
        const std::uint32_t before = sequence_.load(std::memory_order_acquire);
        if ((before & 1u) != 0u)
        {
            continue;
        }

        PlayerStateSnapshot copy;
        copy.actorId  = actorId_.load(std::memory_order_relaxed);
        copy.zoneId   = zoneId_.load(std::memory_order_relaxed);
        copy.x        = std::bit_cast<float>(xBits_.load(std::memory_order_relaxed));
        copy.y        = std::bit_cast<float>(yBits_.load(std::memory_order_relaxed));
        copy.z        = std::bit_cast<float>(zBits_.load(std::memory_order_relaxed));
        copy.rotation = std::bit_cast<float>(
            rotationBits_.load(std::memory_order_relaxed));
        const std::uint32_t displayNameLength =
            displayNameLength_.load(std::memory_order_relaxed);
        if (displayNameLength > kMaximumDisplayNameBytes)
        {
            continue;
        }
        copy.displayName.resize(displayNameLength);
        for (std::size_t index = 0; index < displayNameLength; ++index)
        {
            copy.displayName[index] = static_cast<char>(
                displayNameBytes_[index].load(std::memory_order_relaxed));
        }
        const std::uint32_t areaNameLength =
            areaNameLength_.load(std::memory_order_relaxed);
        if (areaNameLength > kMaximumAreaNameBytes)
        {
            continue;
        }
        copy.areaName.resize(areaNameLength);
        for (std::size_t index = 0; index < areaNameLength; ++index)
        {
            copy.areaName[index] = static_cast<char>(
                areaNameBytes_[index].load(std::memory_order_relaxed));
        }
        copy.baseClassId = static_cast<std::uint16_t>(
            baseClassId_.load(std::memory_order_relaxed));
        copy.jobId = static_cast<std::uint16_t>(
            jobId_.load(std::memory_order_relaxed));
        copy.level = static_cast<std::uint16_t>(
            level_.load(std::memory_order_relaxed));
        const std::uint32_t after = sequence_.load(std::memory_order_acquire);
        if (before != after || (after & 1u) != 0u)
        {
            continue;
        }
        if (copy.actorId == 0)
        {
            return std::nullopt;
        }
        return copy;
    }
    // Exhausting retries must not make a valid player look logged out.
    std::lock_guard<std::mutex> lock(lastSnapshotMutex_);
    if (!hasLastSnapshot_)
    {
        return std::nullopt;
    }
    return lastSnapshot_;
}

void PlayerStateService::PublishUnchecked(const PlayerStateSnapshot& snapshot)
{
    std::uint32_t sequence = sequence_.load(std::memory_order_relaxed);
    for (;;)
    {
        if ((sequence & 1u) != 0u)
        {
            sequence = sequence_.load(std::memory_order_acquire);
            continue;
        }
        if (sequence_.compare_exchange_weak(sequence, sequence + 1u, std::memory_order_acq_rel, std::memory_order_relaxed))
        {
            break;
        }
    }

    actorId_.store(snapshot.actorId, std::memory_order_relaxed);
    zoneId_.store(snapshot.zoneId, std::memory_order_relaxed);
    xBits_.store(std::bit_cast<std::uint32_t>(snapshot.x),
                 std::memory_order_relaxed);
    yBits_.store(std::bit_cast<std::uint32_t>(snapshot.y),
                 std::memory_order_relaxed);
    zBits_.store(std::bit_cast<std::uint32_t>(snapshot.z),
                 std::memory_order_relaxed);
    rotationBits_.store(std::bit_cast<std::uint32_t>(snapshot.rotation),
                        std::memory_order_relaxed);
    for (std::size_t index = 0; index < displayNameBytes_.size(); ++index)
    {
        const unsigned char value = index < snapshot.displayName.size()
                                        ? static_cast<unsigned char>(snapshot.displayName[index])
                                        : 0u;
        displayNameBytes_[index].store(value, std::memory_order_relaxed);
    }
    displayNameLength_.store(
        static_cast<std::uint32_t>(snapshot.displayName.size()),
        std::memory_order_relaxed);
    for (std::size_t index = 0; index < areaNameBytes_.size(); ++index)
    {
        const unsigned char value = index < snapshot.areaName.size()
                                        ? static_cast<unsigned char>(snapshot.areaName[index])
                                        : 0u;
        areaNameBytes_[index].store(value, std::memory_order_relaxed);
    }
    areaNameLength_.store(static_cast<std::uint32_t>(snapshot.areaName.size()),
                          std::memory_order_relaxed);
    baseClassId_.store(snapshot.baseClassId, std::memory_order_relaxed);
    jobId_.store(snapshot.jobId, std::memory_order_relaxed);
    level_.store(snapshot.level, std::memory_order_relaxed);
    {
        std::lock_guard<std::mutex> lock(lastSnapshotMutex_);
        hasLastSnapshot_ = snapshot.actorId != 0u;
        lastSnapshot_    = hasLastSnapshot_ ? snapshot : PlayerStateSnapshot{};
    }
    sequence_.store(sequence + 2u, std::memory_order_release);
}

bool ReadWorldPosition(const void* actor, std::size_t depth, float& x, float& y, float& z)
{
    if (actor == nullptr || depth >= kMaxPositionParentDepth)
    {
        return false;
    }

    std::uint32_t parentAddress = 0;
    float         localX        = 0.0F;
    float         localY        = 0.0F;
    float         localZ        = 0.0F;
    if (!ReadValue(actor, kActorParentOffset, parentAddress) || !ReadValue(actor, kActorXOffset, localX) || !ReadValue(actor, kActorYOffset, localY) || !ReadValue(actor, kActorZOffset, localZ))
    {
        return false;
    }

    if (parentAddress == 0)
    {
        x = localX;
        y = localY;
        z = localZ;
        return true;
    }
    if (IsSentinel(parentAddress))
    {
        return false;
    }

    const void* parent = reinterpret_cast<const void*>(
        static_cast<std::uintptr_t>(parentAddress));
    if (!ReadWorldPosition(parent, depth + 1u, x, y, z))
    {
        return false;
    }
    x += localX;
    y += localY;
    z += localZ;
    return true;
}

float NormalizeFacing(float facing)
{
    const double value = static_cast<double>(facing);
    if (!std::isfinite(value))
    {
        return facing;
    }
    const double adjusted = value < 0.0 ? value - kPi : value + kPi;
    return static_cast<float>(value - std::trunc(adjusted / kTwoPi) * kTwoPi);
}

bool ReadWorldFacing(const void* actor, std::size_t depth, float& facing)
{
    if (actor == nullptr || depth >= kMaxPositionParentDepth)
    {
        return false;
    }

    std::uint32_t parentAddress = 0;
    float         localFacing   = 0.0F;
    if (!ReadValue(actor, kActorParentOffset, parentAddress) || !ReadValue(actor, kActorFacingOffset, localFacing))
    {
        return false;
    }
    if (parentAddress == 0)
    {
        facing = localFacing;
        return true;
    }
    if (IsSentinel(parentAddress))
    {
        return false;
    }

    const void* parent = reinterpret_cast<const void*>(
        static_cast<std::uintptr_t>(parentAddress));
    float parentFacing = 0.0F;
    if (!ReadWorldFacing(parent, depth + 1u, parentFacing))
    {
        return false;
    }
    facing = NormalizeFacing(parentFacing + localFacing);
    return true;
}

bool PlayerStateService::CaptureFromContainer(const void* container)
{
    if (container == nullptr)
    {
        return false;
    }

    std::uint32_t actorAddress     = 0;
    std::uint32_t actorId          = 0;
    std::uint32_t mapLayoutAddress = 0;
    if (!ReadValue(container, kActiveActorPointerOffset, actorAddress) || !ReadValue(container, kActiveActorIdOffset, actorId))
    {
        return false;
    }

    if (actorId == 0u || actorId == kInvalidActorId ||
        actorId == kActiveActorSentinel || actorAddress == kActiveActorSentinel)
    {
        // Explicit invalid identity sentinels clear; failed or partial reads are transient.
        if (Snapshot())
        {
            Clear();
        }
        return false;
    }

    auto previous = Snapshot();
    if (previous && previous->actorId != actorId)
    {
        std::lock_guard<std::mutex> lock(observedMutex_);
        observedEpoch_.fetch_add(1u, std::memory_order_release);
        observed_.erase(previous->actorId);
        PublishUnchecked(PlayerStateSnapshot{});
    }
    if (IsSentinel(actorAddress) ||
        !ReadValue(container, kMapLayoutCacheOffset, mapLayoutAddress) ||
        IsSentinel(mapLayoutAddress))
    {
        return false;
    }

    const void* actor = reinterpret_cast<const void*>(
        static_cast<std::uintptr_t>(actorAddress));
    const void* mapLayout = reinterpret_cast<const void*>(
        static_cast<std::uintptr_t>(mapLayoutAddress));
    PlayerStateSnapshot snapshot;
    snapshot.actorId                = actorId;
    snapshot.displayName            = ReadDisplayName(container).value_or("");
    std::uint32_t mapStateId        = 0;
    std::uint32_t pendingMapStateId = 0;
    if (!ReadValue(mapLayout, kMapStateIdOffset, mapStateId) ||
        !ReadValue(mapLayout, kZoneIdOffset, snapshot.zoneId) ||
        !ReadValue(mapLayout, kPendingMapStateIdOffset, pendingMapStateId))
    {
        return false;
    }
    if (snapshot.zoneId == 0u || snapshot.zoneId == kInvalidActorId ||
        (pendingMapStateId != 0u && pendingMapStateId != mapStateId))
    {
        if (Snapshot())
        {
            Clear();
        }
        return false;
    }
    snapshot.areaName = AreaNameForZone(snapshot.zoneId);
    if (previous && previous->zoneId != snapshot.zoneId)
    {
        std::lock_guard<std::mutex> lock(observedMutex_);
        PublishUnchecked(PlayerStateSnapshot{});
    }
    if (!ReadWorldPosition(actor, 0u, snapshot.x, snapshot.y, snapshot.z) ||
        !ReadWorldFacing(actor, 0u, snapshot.rotation) ||
        !std::isfinite(snapshot.x) || !std::isfinite(snapshot.y) ||
        !std::isfinite(snapshot.z) || !std::isfinite(snapshot.rotation))
    {
        return false;
    }

    const bool                  identityChanged = previous &&
                                                  (previous->actorId != snapshot.actorId ||
                                                   previous->zoneId != snapshot.zoneId);
    std::lock_guard<std::mutex> lock(observedMutex_);
    if (wireZoneId_ != 0u && wireZoneId_ != snapshot.zoneId)
    {
        if (previous && previous->zoneId == wireZoneId_)
        {
            wireZoneId_ = 0u;
        }
        else
        {
            PublishUnchecked(PlayerStateSnapshot{});
            return false;
        }
    }
    MergeObserved(snapshot, identityChanged);
    PublishUnchecked(snapshot);
    return true;
}

bool PlayerStateService::InstallHook()
{
    if (installed_)
    {
        return true;
    }
    if (HasValue(L"BAHAMUT_RUNTIME_TEST_STUB"))
    {
        return true;
    }

    HMODULE image = GetModuleHandleW(nullptr);
    if (image == nullptr || reinterpret_cast<std::uintptr_t>(image) != bahamut_runtime_contract::kRetailImageBase)
    {
        return false;
    }
    void* target = reinterpret_cast<void*>(
        bahamut_runtime_contract::kRetailImageBase + kPlayerStateFunctionRva);
    if (!ReadPrologue(target))
    {
        return false;
    }

    LPVOID original = nullptr;
    if (MH_CreateHook(target, reinterpret_cast<LPVOID>(&HookedPlayerStateUpdate), &original) != MH_OK || original == nullptr)
    {
        return false;
    }
    gOriginalPlayerStateUpdate = reinterpret_cast<PlayerStateUpdateFunction>(original);
    gPlayerStateService.store(this, std::memory_order_release);
    if (MH_EnableHook(target) != MH_OK)
    {
        gPlayerStateService.store(nullptr, std::memory_order_release);
        gOriginalPlayerStateUpdate = nullptr;
        static_cast<void>(MH_RemoveHook(target));
        return false;
    }
    hookTarget_ = target;
    installed_  = true;
    return true;
}

bool PlayerStateService::Shutdown()
{
    if (!installed_)
    {
        return true;
    }

    const MH_STATUS disabled   = MH_DisableHook(hookTarget_);
    const bool      disabledOk = disabled == MH_OK || disabled == MH_ERROR_DISABLED;
    const MH_STATUS removed    = MH_RemoveHook(hookTarget_);
    const bool      removedOk  = removed == MH_OK || removed == MH_ERROR_NOT_CREATED;
    gPlayerStateService.store(nullptr, std::memory_order_release);
    gOriginalPlayerStateUpdate = nullptr;
    hookTarget_                = nullptr;
    installed_                 = false;
    return disabledOk && removedOk;
}

bool PlayerStateService::IsInstalled() const
{
    return installed_;
}

} // namespace bahamut_client
