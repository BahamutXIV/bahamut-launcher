#include "target_distance.h"

#include "actor_name.h"

#include <cmath>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string_view>
#include <utility>
#include <vector>

namespace
{

constexpr std::uint16_t kSetActorTarget         = 0x00DBu;
constexpr std::uint16_t kSetActorTargetAnimated = 0x00D3u;
constexpr std::uint16_t kMoveActor              = 0x00CFu;
constexpr std::uint16_t kSetActorPosition       = 0x00CEu;
constexpr std::uint16_t kSetMap                 = 0x0005u;
constexpr std::uint16_t kAddActor               = 0x00CAu;
constexpr std::uint16_t kActorDespawn           = 0x0114u;
constexpr std::uint16_t kRemoveActor            = 0x00CBu;
constexpr std::uint16_t kDeleteAllActors        = 0x0007u;
constexpr std::uint16_t kActorProperty          = 0x0137u;
constexpr std::size_t   kMaximumTrackedActors   = 2048u;
// MurmurHash2 ids for charaWork.parameterSave.hp[0] and hpMax[0].
constexpr std::uint32_t kCurrentHpProperty = 0x4232BCAAu;
constexpr std::uint32_t kMaxHpProperty     = 0x7BCDFB69u;

bool IsActorId(std::uint32_t id)
{
    return id != 0u && id != 0xFFFFFFFFu && id != 0xC0000000u && id != 0xE0000000u;
}

bool ReadU32(const std::vector<std::uint8_t>& data, std::size_t offset, std::uint32_t& value)
{
    if (offset > data.size() || data.size() - offset < sizeof(value))
    {
        return false;
    }
    std::memcpy(&value, data.data() + offset, sizeof(value));
    return true;
}

std::optional<std::pair<std::uint16_t, std::uint16_t>> ReadHealth(
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
    for (const std::string_view group : { std::string_view("/_init"),
                                          std::string_view("charaWork/stateAtQuicklyForAll") })
    {
        if (end >= group.size() + 2u &&
            data[end - group.size() - 1u] == 0x82u + group.size() &&
            std::memcmp(data.data() + end - group.size(), group.data(), group.size()) == 0)
        {
            valuesEnd = end - group.size() - 1u;
            break;
        }
    }
    if (valuesEnd == 0)
    {
        return std::nullopt;
    }

    std::optional<std::uint16_t> current;
    std::optional<std::uint16_t> maximum;
    for (std::size_t offset = 1u; offset < valuesEnd;)
    {
        if (valuesEnd - offset < 5u)
        {
            return std::nullopt;
        }
        const std::size_t width      = data[offset];
        std::uint32_t     propertyId = 0;
        if (width > valuesEnd - offset - 5u || !ReadU32(data, offset + 1u, propertyId))
        {
            return std::nullopt;
        }
        if (propertyId == kCurrentHpProperty || propertyId == kMaxHpProperty)
        {
            if (width != 2u)
            {
                return std::nullopt;
            }
            const std::uint16_t value = static_cast<std::uint16_t>(data[offset + 5u]) |
                                        static_cast<std::uint16_t>(data[offset + 6u] << 8u);
            auto&               field = propertyId == kCurrentHpProperty ? current : maximum;
            if (field)
            {
                return std::nullopt;
            }
            field = value;
        }
        offset += 5u + width;
    }
    // A partial update cannot be joined to an older actor value safely.
    if (!current || !maximum || *maximum == 0u || *current > *maximum)
    {
        return std::nullopt;
    }
    return std::pair{ *current, *maximum };
}

bool ReadFloat(const std::vector<std::uint8_t>& data, std::size_t offset, float& value)
{
    if (offset > data.size() || data.size() - offset < sizeof(value))
    {
        return false;
    }
    std::memcpy(&value, data.data() + offset, sizeof(value));
    return std::isfinite(value);
}

bool ReadActorPosition(const packet_observer::GameMessage& message,
                       std::uint32_t&                      actorId,
                       bahamut_client::WorldPosition&      position)
{
    actorId = message.sourceId;
    if (message.opcode == kSetActorPosition)
    {
        std::uint32_t payloadActorId = 0;
        if (!ReadU32(message.payload, 4u, payloadActorId))
        {
            return false;
        }
        if (IsActorId(payloadActorId))
        {
            actorId = payloadActorId;
        }
    }
    else if (message.opcode != kMoveActor)
    {
        return false;
    }

    // The wire offsets and binary observation are recorded in
    // target_distance.h.
    return IsActorId(actorId) && ReadFloat(message.payload, 8u, position.x) &&
           ReadFloat(message.payload, 12u, position.y) &&
           ReadFloat(message.payload, 16u, position.z);
}

} // namespace

namespace bahamut_client
{

TargetDistanceService::TargetDistanceService(PlayerStateService* playerState, ActorNameService* actorNames)
: playerState_(playerState)
, actorNames_(actorNames)
{
}

void TargetDistanceService::SyncPlayer(const PlayerStateSnapshot& player)
{
    if (localActorId_ == player.actorId && zoneId_ == player.zoneId)
    {
        return;
    }
    const bool usePendingPositions =
        pendingPositionsAllowed_ &&
        ((!hasSynchronizedPlayer_ && pendingZoneId_ == 0u) ||
         pendingZoneId_ == player.zoneId);
    positions_.clear();
    health_.clear();
    targetActorId_ = 0;
    if (usePendingPositions)
    {
        positions_.swap(pendingPositions_);
    }
    pendingPositions_.clear();
    pendingZoneId_           = 0;
    pendingPositionsAllowed_ = false;
    hasSynchronizedPlayer_   = true;
    localActorId_            = player.actorId;
    zoneId_                  = player.zoneId;
}

void TargetDistanceService::ClearActive()
{
    positions_.clear();
    health_.clear();
    localActorId_  = 0;
    zoneId_        = 0;
    targetActorId_ = 0;
}

void TargetDistanceService::StorePosition(
    std::unordered_map<std::uint32_t, Position>& positions,
    std::uint32_t                                actorId,
    const Position&                              position)
{
    if (positions.size() >= kMaximumTrackedActors && !positions.contains(actorId))
    {
        positions.clear();
    }
    positions[actorId] = position;
}

void TargetDistanceService::Observe(const packet_observer::GameMessage& message)
{
    if (message.direction != packet_observer::Direction::Incoming || playerState_ == nullptr)
    {
        return;
    }

    if (message.opcode == kDeleteAllActors)
    {
        std::lock_guard<std::mutex> lock(mutex_);
        ClearActive();
        pendingPositions_.clear();
        return;
    }

    if (message.opcode == kSetMap)
    {
        std::uint32_t nextZoneId = 0;
        if (message.payload.size() != 16u || !ReadU32(message.payload, 4u, nextZoneId) || nextZoneId == 0u)
        {
            return;
        }
        std::lock_guard<std::mutex> lock(mutex_);
        const std::uint32_t         trackedZoneId =
            pendingPositionsAllowed_ && pendingZoneId_ != 0u ? pendingZoneId_ : zoneId_;
        if (nextZoneId != trackedZoneId)
        {
            ClearActive();
            pendingPositions_.clear();
            pendingZoneId_           = nextZoneId;
            pendingPositionsAllowed_ = true;
        }
        return;
    }

    // Bahamut's spawn writer emits AddActor before CE position and CC init.
    // Clear reused IDs before the subsequent fresh position update.
    if (message.opcode == kAddActor && message.payload.size() != 8u)
    {
        return;
    }
    if (message.opcode == kActorDespawn || message.opcode == kRemoveActor || message.opcode == kAddActor)
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (IsActorId(localActorId_) && message.sourceId == localActorId_)
        {
            ClearActive();
            pendingPositions_.clear();
            pendingZoneId_           = 0;
            pendingPositionsAllowed_ = false;
        }
        else
        {
            positions_.erase(message.sourceId);
            pendingPositions_.erase(message.sourceId);
            health_.erase(message.sourceId);
            if (targetActorId_ == message.sourceId)
            {
                targetActorId_ = 0;
            }
        }
        return;
    }

    const auto player = playerState_->Snapshot();
    if (!player || !IsActorId(player->actorId) || player->zoneId == 0u)
    {
        std::lock_guard<std::mutex> lock(mutex_);
        ClearActive();
        if (hasSynchronizedPlayer_ && !pendingPositionsAllowed_)
        {
            pendingPositions_.clear();
            pendingZoneId_ = 0;
        }
        std::uint32_t actorId = 0;
        Position      position;
        if (pendingPositionsAllowed_ && ReadActorPosition(message, actorId, position))
        {
            StorePosition(pendingPositions_, actorId, position);
        }
        return;
    }

    std::lock_guard<std::mutex> lock(mutex_);
    SyncPlayer(*player);
    const auto& data = message.payload;

    if (message.opcode == kSetActorTarget || message.opcode == kSetActorTargetAnimated)
    {
        std::uint32_t targetId = 0;
        if (message.sourceId == localActorId_ && ReadU32(data, 0u, targetId))
        {
            targetActorId_ = IsActorId(targetId) ? targetId : 0u;
        }
        return;
    }

    if (message.opcode == kActorProperty)
    {
        const auto health = ReadHealth(data);
        if (!IsActorId(message.sourceId) || !health)
        {
            return;
        }
        if (health_.size() >= kMaximumTrackedActors && !health_.contains(message.sourceId))
        {
            health_.clear();
        }
        health_[message.sourceId] = { health->first, health->second };
        return;
    }

    std::uint32_t actorId = 0;
    Position      position;
    if (!ReadActorPosition(message, actorId, position))
    {
        return;
    }
    StorePosition(positions_, actorId, position);
}

std::optional<TargetDistanceSnapshot> TargetDistanceService::Snapshot()
{
    const auto player = playerState_ != nullptr ? playerState_->Snapshot() : std::nullopt;
    if (!player || !IsActorId(player->actorId) || player->zoneId == 0u || !std::isfinite(player->x) || !std::isfinite(player->z))
    {
        std::lock_guard<std::mutex> lock(mutex_);
        ClearActive();
        if (hasSynchronizedPlayer_ && !pendingPositionsAllowed_)
        {
            pendingPositions_.clear();
            pendingZoneId_ = 0;
        }
        return std::nullopt;
    }

    std::lock_guard<std::mutex> lock(mutex_);
    SyncPlayer(*player);
    if (!IsActorId(targetActorId_))
    {
        return std::nullopt;
    }
    TargetDistanceSnapshot snapshot;
    snapshot.actorId = targetActorId_;
    if (actorNames_ != nullptr)
    {
        snapshot.name = actorNames_->NameFor(targetActorId_);
    }
    if (const auto health = health_.find(targetActorId_); health != health_.end())
    {
        snapshot.currentHp = health->second.current;
        snapshot.maxHp     = health->second.maximum;
    }
    // The local actor's wire position can lag its live client position.
    if (targetActorId_ == localActorId_)
    {
        return snapshot;
    }
    const auto found = positions_.find(targetActorId_);
    if (found == positions_.end())
    {
        return snapshot;
    }
    const float distance = std::hypot(player->x - found->second.x, player->z - found->second.z);
    if (!std::isfinite(distance))
    {
        return snapshot;
    }
    snapshot.yalms = distance;
    return snapshot;
}

std::optional<TargetPositionSnapshot> TargetDistanceService::PositionSnapshot(
    std::optional<std::uint32_t> actorId)
{
    const auto player = playerState_ != nullptr ? playerState_->Snapshot() : std::nullopt;
    if (!player || !IsActorId(player->actorId) || player->zoneId == 0u ||
        !std::isfinite(player->x) || !std::isfinite(player->y) ||
        !std::isfinite(player->z))
    {
        std::lock_guard<std::mutex> lock(mutex_);
        ClearActive();
        if (hasSynchronizedPlayer_ && !pendingPositionsAllowed_)
        {
            pendingPositions_.clear();
            pendingZoneId_ = 0;
        }
        return std::nullopt;
    }

    std::lock_guard<std::mutex> lock(mutex_);
    SyncPlayer(*player);
    const auto resolvedTargetId = actorId.value_or(targetActorId_);
    if (localActorId_ != player->actorId || zoneId_ != player->zoneId ||
        !IsActorId(resolvedTargetId))
    {
        return std::nullopt;
    }

    const auto found = positions_.find(resolvedTargetId);
    if (found == positions_.end() || !std::isfinite(found->second.x) ||
        !std::isfinite(found->second.y) || !std::isfinite(found->second.z))
    {
        return std::nullopt;
    }

    TargetPositionSnapshot snapshot;
    snapshot.sourceActorId = localActorId_;
    snapshot.targetActorId = resolvedTargetId;
    snapshot.zoneId        = zoneId_;
    snapshot.source        = { player->x, player->y, player->z };
    snapshot.target        = found->second;
    return snapshot;
}

void TargetDistanceService::Clear()
{
    std::lock_guard<std::mutex> lock(mutex_);
    ClearActive();
    pendingPositions_.clear();
    pendingZoneId_           = 0;
    pendingPositionsAllowed_ = !hasSynchronizedPlayer_;
}

} // namespace bahamut_client
