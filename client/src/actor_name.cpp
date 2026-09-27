#include "actor_name.h"

#include "packet_observer.h"
#include "player_state.h"

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <iterator>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace
{

constexpr std::uint16_t kActorNameOpcode    = 0x013Du;
constexpr std::uint16_t kActorDespawnOpcode = 0x0114u;
constexpr std::uint16_t kRemoveActorOpcode  = 0x00CBu;
constexpr std::uint16_t kDeleteAllActors    = 0x0007u;
constexpr std::size_t   kMaximumActorNames  = 2048u;
constexpr std::size_t   kNameBytes          = 32u;

struct DisplayName
{
    std::uint32_t    id;
    std::string_view name;
};

constexpr DisplayName kDisplayNames[] = {
#include "actor_display_names.inc"
};

bool IsActorId(std::uint32_t actorId)
{
    return actorId != 0u && actorId != 0xFFFFFFFFu &&
           actorId != 0xC0000000u && actorId != 0xE0000000u;
}

std::uint32_t ReadU32(const std::vector<std::uint8_t>& bytes)
{
    return static_cast<std::uint32_t>(bytes[0]) |
           (static_cast<std::uint32_t>(bytes[1]) << 8u) |
           (static_cast<std::uint32_t>(bytes[2]) << 16u) |
           (static_cast<std::uint32_t>(bytes[3]) << 24u);
}

std::string ReadCustomName(const std::vector<std::uint8_t>& bytes)
{
    if (bytes.size() <= 4u)
    {
        return {};
    }
    std::string       name;
    const std::size_t end = std::min(bytes.size(), 4u + kNameBytes);
    for (std::size_t index = 4u; index < end && bytes[index] != 0u; ++index)
    {
        // The packet field is fixed ASCII, not a UTF-8 string.
        if (bytes[index] < 0x20u || bytes[index] > 0x7Eu)
        {
            return {};
        }
        name.push_back(static_cast<char>(bytes[index]));
    }
    const auto first = name.find_first_not_of(' ');
    if (first == std::string::npos)
    {
        return {};
    }
    const auto last = name.find_last_not_of(' ');
    return name.substr(first, last - first + 1u);
}

std::string_view LookupDisplayName(std::uint32_t displayNameId)
{
    const auto found = std::lower_bound(
        std::begin(kDisplayNames), std::end(kDisplayNames), displayNameId, [](const DisplayName& item, std::uint32_t id)
        {
            return item.id < id;
        });
    return found != std::end(kDisplayNames) && found->id == displayNameId
               ? found->name
               : std::string_view{};
}

} // namespace

namespace bahamut_client
{

ActorNameService::ActorNameService(PlayerStateService* playerState)
: playerState_(playerState)
{
}

void ActorNameService::Observe(const packet_observer::GameMessage& message)
{
    if (message.direction != packet_observer::Direction::Incoming)
    {
        return;
    }

    if (message.opcode == kDeleteAllActors)
    {
        Clear();
        return;
    }

    if (message.opcode != kActorNameOpcode &&
        message.opcode != kActorDespawnOpcode &&
        message.opcode != kRemoveActorOpcode)
    {
        return;
    }

    if (message.opcode == kActorDespawnOpcode || message.opcode == kRemoveActorOpcode)
    {
        std::lock_guard lock(mutex_);
        if (IsActorId(localActorId_) && message.sourceId == localActorId_)
        {
            names_.clear();
            localActorId_ = 0;
            zoneId_       = 0;
        }
        else
        {
            names_.erase(message.sourceId);
        }
        return;
    }

    if (!IsActorId(message.sourceId) || message.payload.size() < 4u)
    {
        return;
    }
    const auto player = playerState_ != nullptr ? playerState_->Snapshot() : std::nullopt;
    if (!player || !IsActorId(player->actorId) || player->zoneId == 0u)
    {
        Clear();
        return;
    }

    std::string name = ReadCustomName(message.payload);
    if (name.empty())
    {
        const std::uint32_t displayNameId = ReadU32(message.payload);
        name.assign(LookupDisplayName(displayNameId));
    }
    if (name.empty())
    {
        return;
    }

    std::lock_guard lock(mutex_);
    SyncPlayer(player->actorId, player->zoneId);
    if (names_.size() >= kMaximumActorNames && !names_.contains(message.sourceId))
    {
        names_.erase(names_.begin());
    }
    names_[message.sourceId] = std::move(name);
}

std::string ActorNameService::NameFor(std::uint32_t actorId)
{
    if (!IsActorId(actorId))
    {
        return {};
    }
    const auto player = playerState_ != nullptr ? playerState_->Snapshot() : std::nullopt;
    if (!player || !IsActorId(player->actorId) || player->zoneId == 0u)
    {
        Clear();
        return {};
    }

    std::lock_guard lock(mutex_);
    SyncPlayer(player->actorId, player->zoneId);
    const auto found = names_.find(actorId);
    return found != names_.end() ? found->second : std::string{};
}

void ActorNameService::Clear()
{
    std::lock_guard lock(mutex_);
    names_.clear();
    localActorId_ = 0;
    zoneId_       = 0;
}

void ActorNameService::SyncPlayer(std::uint32_t actorId, std::uint32_t zoneId)
{
    if (actorId != localActorId_ || zoneId != zoneId_)
    {
        names_.clear();
        localActorId_ = actorId;
        zoneId_       = zoneId;
    }
}

} // namespace bahamut_client
