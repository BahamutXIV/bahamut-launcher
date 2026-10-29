#pragma once

#include "gearsets_debug.h"

#include <atomic>
#include <cstdint>
#include <mutex>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace bahamut_client
{

class EquipmentStateService;

GearsetsDebugSwitches ReadGearsetsDebugSwitchesFromEnvironment();

// Installed only with a debug switch; reads/steps require a healthy loaded addon.
// Run after the hooked player-state update on the game thread; only render writes receipts.
// Game-frame ordering: client/README.md#client-evidence.
class GearsetsDebugService final : public GearsetsDebugPort
{
public:
    // Production supplies no debug invoker; requests that pass their checks
    // end Refused with invoke-unavailable.
    using Invoker = GearsetsInvokeResult (*)(void* context, const void* player, std::uint16_t bagSlot, std::uint8_t equipSlot);

    static constexpr std::uint64_t kSnapshotIntervalMs     = 100u;
    static constexpr std::uint64_t kMaximumReceiptRecords  = 4096u;
    static constexpr std::size_t   kMaximumPendingRecords  = 256u;
    static constexpr std::size_t   kMaximumWritesPerUpdate = 64u;

    explicit GearsetsDebugService(EquipmentStateService* equipment);
    ~GearsetsDebugService() override;

    GearsetsDebugService(const GearsetsDebugService&)            = delete;
    GearsetsDebugService& operator=(const GearsetsDebugService&) = delete;

    // Call before any hook can reach the service.
    void Configure(const GearsetsDebugSwitches& switches);
    void SetInvoker(Invoker invoker, void* context);
    // Called on the game thread after PlayerStateService captured this frame.
    void SetMetadataProvider(std::function<void(EquipmentSnapshot&)> provider);

    // PlayerStateService game-frame observer; context is the service.
    static void ObserveGameFrame(void* context, const void* container);
    // Game window thread. player is the resolved MyPlayer control or null.
    void OnGameFrame(const void* player, std::uint64_t nowMs);

    // Writes pending records and the footer, then closes the receipt.
    // Render thread.
    void Finish(std::string_view reason);
    // DLL teardown. ExitProcess may have terminated a thread that held the
    // lock, so this never waits for it: when the lock is taken, the receipt is
    // closed without its footer and reads as truncated.
    void FinishAtShutdown();

    [[nodiscard]] bool                               EquipEnabled() const override;
    void                                             OnAddonStateChanged(bool active) override;
    void                                             OnRenderFrame() override;
    GearsetsSubmitResult                             SubmitDebugEquip(std::uint16_t bagSlot, std::uint8_t equipSlot, std::optional<std::uint64_t> uid) override;
    [[nodiscard]] std::optional<GearsetsEquipStatus> DebugEquipStatus() const override;

private:
    template <typename Format>
    void QueueRecordLocked(Format format);
    void OpenReceipt();
    // Returns true once the record limit is reached and everything is written.
    bool WritePending(std::size_t limit);
    bool WriteText(const std::string& text);

    EquipmentStateService* equipment_ = nullptr;
    GearsetsDebugSwitches  switches_;
    std::atomic<bool>      active_{ false };

    mutable std::mutex                      mutex_;
    GearsetsEquipMachine                    machine_;
    std::uint64_t                           generation_ = 1;
    std::uint64_t                           frame_      = 0;
    const void*                             lastPlayer_ = nullptr;
    std::optional<bool>                     available_;
    std::uint64_t                           recordedGeneration_ = 0;
    std::optional<std::uint64_t>            lastReadMs_;
    std::uint64_t                           records_        = 0;
    std::uint64_t                           dropped_        = 0;
    bool                                    receiptWanted_  = false;
    bool                                    receiptStarted_ = false;
    bool                                    finishPending_  = false;
    std::vector<std::string>                pending_;
    Invoker                                 invoker_        = nullptr;
    void*                                   invokerContext_ = nullptr;
    std::function<void(EquipmentSnapshot&)> metadata_;

    // Render thread only, and teardown.
    void* file_          = nullptr;
    bool  receiptOpened_ = false;
};

// Wires the service into the host only when a switch is set, so without one
// the addon host and the game-frame source keep their default paths.
template <typename Host, typename FrameSource>
bool InstallGearsetsDebug(const GearsetsDebugSwitches& switches, GearsetsDebugService& service, EquipmentStateService& equipment, Host& host, FrameSource& frames)
{
    if (!switches.Any())
    {
        return false;
    }
    service.Configure(switches);
    host.SetEquipmentStateService(&equipment);
    host.SetGearsetsDebugPort(&service);
    frames.SetGameFrameObserver(&GearsetsDebugService::ObserveGameFrame, &service);
    return true;
}

} // namespace bahamut_client
