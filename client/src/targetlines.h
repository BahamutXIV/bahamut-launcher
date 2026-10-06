#pragma once

#include "target_distance.h"

#include <array>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <mutex>
#include <optional>
#include <unordered_map>

namespace bahamut_client
{

struct TargetlineSnapshot
{
    TargetPositionSnapshot positions;
    std::uint64_t          relationshipSequence = 0;
    bool                   friendly             = false;
    float                  opacity              = 1.0F;
};

struct TargetlinesSnapshot
{
    std::array<TargetlineSnapshot, 2> arcs{};
    std::size_t                       count = 0;
};

class TargetlinesService
{
public:
    using Clock = std::chrono::steady_clock;

    TargetlinesService(PlayerStateService* player, TargetDistanceService* targets);
    void                              SetCastingEnabled(bool enabled);
    void                              Observe(const packet_observer::GameMessage& message, Clock::time_point now = Clock::now());
    [[nodiscard]] TargetlinesSnapshot Snapshot(Clock::time_point now = Clock::now());

private:
    struct Cast
    {
        std::uint32_t                    sourceId = 0;
        std::uint32_t                    targetId = 0;
        std::uint32_t                    zoneId   = 0;
        std::uint16_t                    command  = 0;
        std::uint64_t                    sequence = 0;
        Clock::time_point                expires;
        std::optional<Clock::time_point> fadeStarted;
    };

    struct Deadline
    {
        std::uint32_t     command = 0;
        Clock::time_point observed;
        Clock::time_point expires;
    };

    bool SyncPlayer(const PlayerStateSnapshot& player);
    void ClearRelationships();

    PlayerStateService*                              player_  = nullptr;
    TargetDistanceService*                           targets_ = nullptr;
    std::mutex                                       mutex_;
    std::unordered_map<std::uint32_t, std::uint64_t> mobs_;
    std::optional<Cast>                              cast_;
    std::optional<Deadline>                          deadline_;
    std::uint64_t                                    relationshipSequence_ = 0;
    std::uint32_t                                    actorId_              = 0;
    std::uint32_t                                    zoneId_               = 0;
    std::uint32_t                                    wireZoneId_           = 0;
    bool                                             castingEnabled_       = true;
};

} // namespace bahamut_client
