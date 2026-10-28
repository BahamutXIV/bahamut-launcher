#pragma once

#include "gearsets_transaction.h"

#include <atomic>
#include <cstdint>
#include <functional>
#include <mutex>
#include <optional>

namespace bahamut_client
{

class EquipmentStateService;

// A healthy loaded addon enables equipment and bag reads after the hooked
// player-state update. Reads and native submissions run on the game thread;
// the mutex serializes transaction access from addon callbacks.
class GearsetsRuntime
{
public:
    static constexpr std::uint64_t kSnapshotIntervalMs = 100u;
    // A read fails while its two passes disagree, as during an inventory
    // update; an active transaction stops only when none succeeds this long.
    static constexpr std::uint64_t kReadFailureMs = 1000u;

    GearsetsRuntime(EquipmentStateService* equipment, GearsetsExecutor& executor);

    GearsetsRuntime(const GearsetsRuntime&)            = delete;
    GearsetsRuntime& operator=(const GearsetsRuntime&) = delete;

    // Setup, before any hook can reach the runtime.
    void SetMetadataProvider(std::function<void(EquipmentSnapshot&)> provider);
    // Another game-frame observer, run after this one.
    void SetGameFrameObserver(void (*observer)(void* context, const void* container), void* context);

    // PlayerStateService game-frame observer; context is the runtime.
    static void ObserveGameFrame(void* context, const void* container);
    // Game window thread. player is the resolved MyPlayer control or null.
    void OnGameFrame(const void* container, const void* player, std::uint64_t nowMs);

    // Every addon transition ends the transaction; deactivation also stops
    // reading and clears published equipment.
    void SetActive(bool active);
    // nullopt while another transaction is active.
    std::optional<GearsetsResult> Begin(const GearsetIntent& intent, const GearsetsView& view);
    void                          Cancel();
    GearsetsResult                Result() const;

private:
    void FrameLocked(const void* container, const void* player, std::uint64_t nowMs);

    EquipmentStateService* equipment_ = nullptr;
    GearsetsExecutor&      executor_;
    std::atomic<bool>      active_{ false };

    mutable std::mutex                      mutex_;
    GearsetsTransaction                     transaction_;
    std::function<void(EquipmentSnapshot&)> metadata_;
    const void*                             lastPlayer_ = nullptr;
    std::optional<std::uint64_t>            lastReadMs_;
    std::optional<std::uint64_t>            lastGoodReadMs_;

    void (*next_)(void*, const void*) = nullptr;
    void* nextContext_                = nullptr;
};

} // namespace bahamut_client
