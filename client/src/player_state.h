#pragma once

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <mutex>
#include <optional>
#include <string>
#include <unordered_map>

namespace packet_observer
{

struct GameMessage;

}

namespace bahamut_client
{

inline constexpr std::uintptr_t kPlayerStateFunctionVa             = 0x004DA680u;
inline constexpr std::uintptr_t kPlayerStateFunctionRva            = 0x000DA680u;
inline constexpr std::size_t    kPlayerStateFunctionPrologueLength = 17u;
inline constexpr std::size_t    kPlayerStateAreaNameBytes          = 64u;

struct PlayerStateSnapshot
{
    std::uint32_t actorId  = 0;
    std::uint32_t zoneId   = 0;
    float         x        = 0.0F;
    float         y        = 0.0F;
    float         z        = 0.0F;
    float         rotation = 0.0F;
    std::string   displayName;
    std::string   areaName;
    std::uint16_t baseClassId = 0;
    std::uint16_t jobId       = 0;
    std::uint16_t level       = 0;
};

class PlayerStateService
{
public:
    PlayerStateService()  = default;
    ~PlayerStateService() = default;

    PlayerStateService(const PlayerStateService&)            = delete;
    PlayerStateService& operator=(const PlayerStateService&) = delete;

    // Publishes a copy for render-thread readers. Invalid values clear the
    // published state and return false.
    bool Publish(const PlayerStateSnapshot& snapshot);
    void Clear();

    [[nodiscard]] std::optional<PlayerStateSnapshot> Snapshot() const;

    // Reads only fields pinned for the supported client build from the
    // game-thread container.
    // The copy is published after the caller's original function returns.
    bool CaptureFromContainer(const void* container);
    void Observe(const packet_observer::GameMessage& message);

    // MinHook is owned by the render boundary. These methods only create,
    // enable, disable, and remove this hook after that owner is initialized.
    bool               InstallHook();
    bool               Shutdown();
    [[nodiscard]] bool IsInstalled() const;

private:
    void PublishUnchecked(const PlayerStateSnapshot& snapshot);
    void MergeObserved(PlayerStateSnapshot& snapshot, bool identityChanged);

    struct ObservedClassJob
    {
        std::uint32_t zoneId      = 0;
        std::uint16_t baseClassId = 0;
        std::uint16_t jobId       = 0;
        std::uint16_t level       = 0;
    };

    std::mutex                                          observedMutex_;
    std::unordered_map<std::uint32_t, ObservedClassJob> observed_;
    std::uint32_t                                       wireZoneId_ = 0;
    std::atomic<std::uint32_t>                          observedEpoch_{ 0 };

    mutable std::mutex                         lastSnapshotMutex_;
    PlayerStateSnapshot                        lastSnapshot_{};
    bool                                       hasLastSnapshot_ = false;
    std::atomic<std::uint32_t>                 sequence_{ 0 };
    std::atomic<std::uint32_t>                 actorId_{ 0 };
    std::atomic<std::uint32_t>                 zoneId_{ 0 };
    std::atomic<std::uint32_t>                 xBits_{ 0 };
    std::atomic<std::uint32_t>                 yBits_{ 0 };
    std::atomic<std::uint32_t>                 zBits_{ 0 };
    std::atomic<std::uint32_t>                 rotationBits_{ 0 };
    std::atomic<std::uint32_t>                 displayNameLength_{ 0 };
    std::array<std::atomic<unsigned char>, 32> displayNameBytes_{};
    std::atomic<std::uint32_t>                 areaNameLength_{ 0 };
    std::array<std::atomic<unsigned char>, kPlayerStateAreaNameBytes>
                               areaNameBytes_{};
    std::atomic<std::uint32_t> baseClassId_{ 0 };
    std::atomic<std::uint32_t> jobId_{ 0 };
    std::atomic<std::uint32_t> level_{ 0 };
    void*                      hookTarget_ = nullptr;
    bool                       installed_  = false;
};

} // namespace bahamut_client
