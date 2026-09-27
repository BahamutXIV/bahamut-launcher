#pragma once

#include "packet_observer.h"
#include "player_state.h"

#include <cstdint>
#include <mutex>
#include <optional>
#include <string>
#include <unordered_map>

namespace bahamut_client
{

class ActorNameService;

struct TargetDistanceSnapshot
{
    std::uint32_t                actorId = 0;
    std::optional<float>         yalms;
    std::string                  name;
    std::optional<std::uint16_t> currentHp;
    std::optional<std::uint16_t> maxHp;
};

class TargetDistanceService
{
public:
    TargetDistanceService(PlayerStateService* playerState, ActorNameService* actorNames);

    void                                                Observe(const packet_observer::GameMessage& message);
    [[nodiscard]] std::optional<TargetDistanceSnapshot> Snapshot();
    void                                                Clear();

private:
    struct Position
    {
        float x = 0.0F;
        float z = 0.0F;
    };

    struct Health
    {
        std::uint16_t current = 0;
        std::uint16_t maximum = 0;
    };

    void SyncPlayer(const PlayerStateSnapshot& player);
    void ClearActive();
    void StorePosition(std::unordered_map<std::uint32_t, Position>& positions,
                       std::uint32_t                                actorId,
                       const Position&                              position);

    PlayerStateService*                         playerState_ = nullptr;
    ActorNameService*                           actorNames_  = nullptr;
    std::mutex                                  mutex_;
    std::unordered_map<std::uint32_t, Position> positions_;
    std::unordered_map<std::uint32_t, Position> pendingPositions_;
    std::unordered_map<std::uint32_t, Health>   health_;
    std::uint32_t                               localActorId_            = 0;
    std::uint32_t                               zoneId_                  = 0;
    std::uint32_t                               targetActorId_           = 0;
    std::uint32_t                               pendingZoneId_           = 0;
    bool                                        hasSynchronizedPlayer_   = false;
    bool                                        pendingPositionsAllowed_ = true;
};

} // namespace bahamut_client
