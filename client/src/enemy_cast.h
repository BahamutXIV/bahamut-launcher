#pragma once

#include <chrono>
#include <cstdint>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>

namespace packet_observer
{

struct GameMessage;

}

namespace bahamut_client
{

class PlayerStateService;
class ActorNameService;

struct EnemyCastSnapshot
{
    std::uint32_t actorId   = 0;
    std::uint16_t commandId = 0;
    std::string   name;
    std::string   actorName;
    double        remainingSeconds = 0.0;
    double        durationSeconds  = 0.0;
    double        progress         = 0.0;
    bool          casting          = true;
    bool          completed        = false;
};

class EnemyCastService final
{
public:
    // The player-state service must outlive this service.
    EnemyCastService(PlayerStateService* playerState, ActorNameService* actorNames);

    void                                         Observe(const packet_observer::GameMessage& message);
    [[nodiscard]] std::vector<EnemyCastSnapshot> Snapshot();

private:
    using Clock = std::chrono::steady_clock;

    struct ActiveCast
    {
        std::uint16_t             commandId = 0;
        std::string               name;
        Clock::time_point         startedAt;
        std::chrono::milliseconds duration{ 0 };
        Clock::time_point         removeAt;
        bool                      completed = false;
    };

    void SyncPlayer(std::uint32_t actorId, std::uint32_t zoneId);
    void PruneExpired(Clock::time_point now);

    PlayerStateService*                                  playerState_ = nullptr;
    ActorNameService*                                    actorNames_  = nullptr;
    std::mutex                                           mutex_;
    std::unordered_map<std::uint32_t, ActiveCast>        activeCasts_;
    std::unordered_map<std::uint32_t, Clock::time_point> engaged_;
    std::uint32_t                                        localActorId_ = 0;
    std::uint32_t                                        zoneId_       = 0;
};

} // namespace bahamut_client
