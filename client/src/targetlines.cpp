#include "targetlines.h"
#include "targetlines_mobs.h"

#include <algorithm>
#include <cstring>
#include <span>
#include <string_view>

namespace
{

using Clock                          = bahamut_client::TargetlinesService::Clock;
constexpr std::size_t kMaximumActors = 2048u;
constexpr auto        kEndingFade    = std::chrono::milliseconds(250);

struct FriendlyCast
{
    std::uint16_t command;
    std::uint32_t castMilliseconds;
};

// bahamut:sql/actions.sql, CREATE TABLE column names and the listed command IDs.
// These heal and buff rows allow friendly targets. Bahamut's
// map_runtime.cpp MapRuntime::CastBattleCommand validates that relationship before
// emitting AppendNpcCastStart. This matches the server contract.
constexpr FriendlyCast kFriendlyCasts[]{
    { 27149u, 2000u }, { 27346u, 2000u }, { 27347u, 2000u }, { 27348u, 3000u }, { 27350u, 3000u }, { 27351u, 3000u }, { 27358u, 2000u }, { 28666u, 2000u }, { 28667u, 2000u }, { 28668u, 2000u }, { 28669u, 2000u }, { 28929u, 3000u }, { 29010u, 2000u }, { 29011u, 2000u }, { 29012u, 2000u }, { 29013u, 2000u }, { 29015u, 3000u }, { 29016u, 3000u }, { 29017u, 3000u }, { 29019u, 3000u }, { 29020u, 3000u }, { 29021u, 3000u }, { 29023u, 3000u }, { 29025u, 3000u }, { 29046u, 3000u }, { 29047u, 3000u }
};

std::uint32_t ReadU32(std::span<const std::uint8_t> data, std::size_t offset)
{
    std::uint32_t value = 0;
    std::memcpy(&value, data.data() + offset, sizeof(value));
    return value;
}

std::uint16_t ReadU16(std::span<const std::uint8_t> data, std::size_t offset)
{
    std::uint16_t value = 0;
    std::memcpy(&value, data.data() + offset, sizeof(value));
    return value;
}

bool ValidActor(std::uint32_t actorId)
{
    return actorId != 0u && actorId != 0xFFFFFFFFu && actorId != 0xC0000000u && actorId != 0xE0000000u;
}

const FriendlyCast* FindFriendlyCast(std::uint16_t command)
{
    const auto found = std::find_if(std::begin(kFriendlyCasts), std::end(kFriendlyCasts), [command](const auto& entry)
                                    {
                                        return entry.command == command;
                                    });
    return found != std::end(kFriendlyCasts) ? found : nullptr;
}

struct Result
{
    std::uint16_t command     = 0;
    std::uint32_t target      = 0;
    bool          start       = false;
    bool          interrupted = false;
};

std::optional<Result> ReadResult(const packet_observer::GameMessage& message)
{
    std::size_t   size     = 0;
    std::uint32_t capacity = 0;
    switch (message.opcode)
    {
        case 0x0139u:
            size     = 0x38u;
            capacity = 1u;
            break;
        case 0x013Au:
            size     = 0xB8u;
            capacity = 10u;
            break;
        case 0x013Bu:
            size     = 0x128u;
            capacity = 18u;
            break;
        case 0x013Cu:
            size = 0x28u;
            break;
        default:
            return std::nullopt;
    }
    const auto& data = message.payload;
    if (data.size() != size || ReadU32(data, 0u) != message.sourceId)
    {
        return std::nullopt;
    }
    const auto count = ReadU32(data, 0x20u);
    if ((capacity == 0u && count != 0u) || (capacity != 0u && (count == 0u || count > capacity)))
    {
        return std::nullopt;
    }
    Result result;
    result.command = ReadU16(data, 0x24u);
    if (message.opcode == 0x0139u)
    {
        const auto animation = ReadU32(data, 4u);
        const auto text      = ReadU16(data, 0x2Eu);
        result.target        = ReadU32(data, 0x28u);
        // bahamut:src/map/map_runtime.cpp AppendNpcCastStart and
        // TryInterruptPlayerCastFromDamage; command_results.h owns offsets.
        result.start       = text == 30128u && data[0x34u] == 1u && data[0x35u] == 1u &&
                             (animation & 0xFF000000u) == 0x6F000000u;
        result.interrupted = text == 30201u && animation == 0x7F000002u;
    }
    return result;
}

struct CastDeadline
{
    std::uint32_t command = 0;
    std::uint32_t end     = 0;
};

std::optional<CastDeadline> ReadCastDeadline(std::span<const std::uint8_t> data)
{
    constexpr std::string_view target = "playerWork/castState";
    if (data.size() != 0xA8u || data[0] == 0u || data[0] >= data.size())
    {
        return std::nullopt;
    }
    const std::size_t end    = 1u + data[0];
    std::size_t       offset = 1u;
    CastDeadline      result;
    bool              haveCommand = false;
    bool              haveEnd     = false;
    while (offset < end)
    {
        const auto length = data[offset++];
        if (length >= 0x82u)
        {
            if (length != 0x82u + target.size() || end - offset != target.size() ||
                std::memcmp(data.data() + offset, target.data(), target.size()) != 0)
            {
                return std::nullopt;
            }
            return haveCommand && haveEnd ? std::optional{ result } : std::nullopt;
        }
        if (end - offset < 4u || length > end - offset - 4u)
        {
            return std::nullopt;
        }
        const auto hash = ReadU32(data, offset);
        offset += 4u;
        if (hash == 0xF683A451u || hash == 0x59C40D5Du)
        {
            if (length != 4u)
            {
                return std::nullopt;
            }
            const auto value = ReadU32(data, offset);
            if (hash == 0xF683A451u)
            {
                if (haveCommand)
                {
                    return std::nullopt;
                }
                result.command = value;
                haveCommand    = true;
            }
            else
            {
                if (haveEnd)
                {
                    return std::nullopt;
                }
                result.end = value;
                haveEnd    = true;
            }
        }
        offset += length;
    }
    return std::nullopt;
}

} // namespace

namespace bahamut_client
{

TargetlinesService::TargetlinesService(PlayerStateService* player, TargetDistanceService* targets)
: player_(player)
, targets_(targets)
{
}

void TargetlinesService::SetCastingEnabled(bool enabled)
{
    std::lock_guard lock(mutex_);
    castingEnabled_ = enabled;
    if (!enabled)
    {
        // Preserve stationary mob classifications, but never resume a cast
        // that began before the addon was disabled or faulted.
        cast_.reset();
        deadline_.reset();
    }
}

void TargetlinesService::ClearRelationships()
{
    mobs_.clear();
    cast_.reset();
    deadline_.reset();
}

bool TargetlinesService::SyncPlayer(const PlayerStateSnapshot& player)
{
    if (wireZoneId_ != 0u && player.zoneId != wireZoneId_)
    {
        cast_.reset();
        return false;
    }
    if (actorId_ != 0u && (actorId_ != player.actorId || zoneId_ != player.zoneId))
    {
        ClearRelationships();
    }
    actorId_ = player.actorId;
    zoneId_  = player.zoneId;
    return ValidActor(actorId_) && zoneId_ != 0u;
}

void TargetlinesService::Observe(const packet_observer::GameMessage& message, Clock::time_point now)
{
    if (message.direction != packet_observer::Direction::Incoming || player_ == nullptr)
    {
        return;
    }
    std::lock_guard lock(mutex_);
    if (message.opcode == 0x0007u)
    {
        ClearRelationships();
        actorId_ = 0u;
        return;
    }
    if (message.opcode == 0x0005u)
    {
        if (message.payload.size() == 16u)
        {
            const auto zone = ReadU32(message.payload, 4u);
            if (zone != 0u && zone != wireZoneId_)
            {
                ClearRelationships();
                wireZoneId_ = zone;
                actorId_    = 0u;
            }
        }
        return;
    }
    if (message.opcode == 0x00CAu && message.payload.size() != 8u)
    {
        return;
    }
    if (message.opcode == 0x0114u || message.opcode == 0x00CBu || message.opcode == 0x00CCu || message.opcode == 0x00CAu)
    {
        mobs_.erase(message.sourceId);
        if (cast_ && (message.sourceId == cast_->sourceId || message.sourceId == cast_->targetId))
        {
            cast_.reset();
        }
        if (message.sourceId == actorId_)
        {
            ClearRelationships();
            actorId_    = 0u;
            zoneId_     = 0u;
            wireZoneId_ = 0u;
        }
        if (message.opcode == 0x00CCu && IsTargetlinesMobInstantiation(message))
        {
            if (mobs_.size() >= kMaximumActors)
            {
                mobs_.clear();
            }
            // Do not join a pending scene to an earlier lifetime of the same ID.
            mobs_.emplace(message.sourceId, ++relationshipSequence_);
        }
        return;
    }
    const auto player = player_->Snapshot();
    if (!player || !SyncPlayer(*player) || message.sourceId != player->actorId || !castingEnabled_)
    {
        return;
    }
    if (message.opcode == 0x0137u)
    {
        const auto deadline = ReadCastDeadline(message.payload);
        // The header and cast-end timestamp use the same server clock. Zero
        // timestamps cannot establish a duration, so the bounded catalog fallback remains.
        if (deadline && message.timestamp != 0u && deadline->end > message.timestamp &&
            deadline->end - message.timestamp <= 30u)
        {
            deadline_ = Deadline{ deadline->command, now, now + std::chrono::seconds(deadline->end - message.timestamp) };
            if (cast_ && cast_->command == deadline_->command && !cast_->fadeStarted)
            {
                cast_->expires = deadline_->expires;
            }
        }
        return;
    }
    const auto result = ReadResult(message);
    if (!result)
    {
        return;
    }
    if (result->start)
    {
        cast_.reset();
        const auto spec = FindFriendlyCast(result->command);
        if (spec == nullptr || !ValidActor(result->target) || result->target == player->actorId || mobs_.contains(result->target))
        {
            return;
        }
        auto expires = now + std::chrono::milliseconds(spec->castMilliseconds + 1000u);
        if (deadline_ && deadline_->command == result->command &&
            now >= deadline_->observed && now - deadline_->observed <= std::chrono::seconds(1) && deadline_->expires > now)
        {
            expires = deadline_->expires;
        }
        cast_ = Cast{ player->actorId, result->target, player->zoneId, result->command, ++relationshipSequence_, expires, std::nullopt };
        deadline_.reset();
    }
    else if (cast_ && cast_->command == result->command)
    {
        if (result->interrupted)
        {
            cast_.reset();
        }
        else if (!cast_->fadeStarted)
        {
            cast_->fadeStarted = now;
            cast_->expires     = now + kEndingFade;
        }
    }
}

TargetlinesSnapshot TargetlinesService::Snapshot(Clock::time_point now)
{
    TargetlinesSnapshot result;
    const auto          player = player_ != nullptr ? player_->Snapshot() : std::nullopt;
    std::lock_guard     lock(mutex_);
    if (!player || targets_ == nullptr)
    {
        if (actorId_ != 0u)
        {
            ClearRelationships();
            actorId_    = 0u;
            zoneId_     = 0u;
            wireZoneId_ = 0u;
        }
        return result;
    }
    if (!SyncPlayer(*player))
    {
        return result;
    }
    if (const auto selected = targets_->PositionSnapshot(); selected &&
                                                            selected->sourceActorId == player->actorId && selected->zoneId == player->zoneId &&
                                                            selected->sourceActorId != selected->targetActorId && mobs_.contains(selected->targetActorId))
    {
        result.arcs[result.count++] = { *selected, mobs_.at(selected->targetActorId), false, 1.0F };
    }
    if (cast_ && (cast_->expires <= now || cast_->sourceId != player->actorId || cast_->zoneId != player->zoneId))
    {
        cast_.reset();
    }
    if (cast_)
    {
        const auto target = targets_->PositionSnapshot(cast_->targetId);
        if (target && target->sourceActorId == player->actorId && target->zoneId == player->zoneId &&
            target->sourceActorId != target->targetActorId)
        {
            const float opacity         = cast_->fadeStarted
                                              ? std::clamp(std::chrono::duration<float>(cast_->expires - now).count() /
                                                               std::chrono::duration<float>(kEndingFade).count(),
                                                           0.0F,
                                                           1.0F)
                                              : 1.0F;
            result.arcs[result.count++] = { *target, cast_->sequence, true, opacity };
        }
        else
        {
            cast_.reset();
        }
    }
    return result;
}

} // namespace bahamut_client
