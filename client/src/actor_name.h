#pragma once

#include <cstdint>
#include <mutex>
#include <string>
#include <unordered_map>

namespace packet_observer
{

struct GameMessage;

}

namespace bahamut_client
{

class PlayerStateService;

class ActorNameService final
{
public:
    // The player-state service must outlive this service.
    explicit ActorNameService(PlayerStateService* playerState);

    void                      Observe(const packet_observer::GameMessage& message);
    [[nodiscard]] std::string NameFor(std::uint32_t actorId);
    void                      Clear();

private:
    void SyncPlayer(std::uint32_t actorId, std::uint32_t zoneId);

    PlayerStateService*                            playerState_ = nullptr;
    std::mutex                                     mutex_;
    std::unordered_map<std::uint32_t, std::string> names_;
    std::uint32_t                                  localActorId_ = 0;
    std::uint32_t                                  zoneId_       = 0;
};

} // namespace bahamut_client
