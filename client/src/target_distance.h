#pragma once

#include "packet_observer.h"
#include "player_state.h"
#include "world_projection.h"

#include <cstdint>
#include <mutex>
#include <optional>
#include <string>
#include <unordered_map>

namespace bahamut_client
{

class ActorNameService;

// Retail 1.23b ffxivgame.exe (SHA-256
// 9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9):
// FUN_0058CCA0 loads position floats from [esi+0x18], [esi+0x1c], and
// [esi+0x20] for 0x00CE at VAs 0x0058CE37, 0x0058CE42, 0x0058CE4D and for
// 0x00CF at VAs 0x0058CF31, 0x0058CF46, 0x0058CF54. The launcher's 16-byte
// GameMessage header maps these packet offsets to payload +8, +12, +16.
// Observation: Capstone 5.0.7 disassembly with pefile 2024.8.26.
struct TargetPositionSnapshot
{
    std::uint32_t sourceActorId = 0;
    std::uint32_t targetActorId = 0;
    std::uint32_t zoneId        = 0;
    WorldPosition source;
    WorldPosition target;
};

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
    [[nodiscard]] std::optional<TargetPositionSnapshot> PositionSnapshot(
        std::optional<std::uint32_t> actorId = std::nullopt);
    void Clear();

private:
    using Position = WorldPosition;

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
