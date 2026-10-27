#pragma once

#include "packet_capture.h"
#include "packet_observer.h"

#include <atomic>
#include <chrono>
#include <cstdint>
#include <deque>
#include <filesystem>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace bahamut_client
{

class PlayerStateService;
class TargetDistanceService;
class CustomTimerService;
class EnemyCastService;
class ActorNameService;

} // namespace bahamut_client

struct lua_State;

struct AddonCombatMeterRow
{
    std::string   name;
    std::uint64_t amount = 0;
    double        rate   = 0.0;
    std::string   accuracy;
    std::string   color;
};

struct AddonWindowSink
{
    void* context                                                                                                                                                  = nullptr;
    void (*window)(void* context, const char* addonId, const char* title, const char* text, bool locked)                                                           = nullptr;
    void (*rawText)(void* context, const char* addonId, const char* text, float red, float green, float blue, float alpha, bool locked, float fontSize)            = nullptr;
    void (*combatMeter)(void* context, const char* addonId, const char* mode, const AddonCombatMeterRow* rows, std::size_t rowCount, bool locked, bool incomplete) = nullptr;
    void (*targetlines)(void* context, const char* addonId)                                                                                                        = nullptr;
};

struct AddonClipboardSink
{
    void* context                                         = nullptr;
    bool (*setText)(void* context, std::string_view text) = nullptr;
};

struct AddonChatSink
{
    void* context                                       = nullptr;
    bool (*print)(void* context, std::string_view text) = nullptr;
};

struct AddonUrlSink
{
    void* context                                     = nullptr;
    bool (*open)(void* context, std::string_view url) = nullptr;
};

std::optional<std::string> ReadAddonManifestId(
    const std::filesystem::path& manifestPath);
bool IsAllowedAddonUrl(std::string_view url);

class AddonHost
{
public:
    explicit AddonHost(bahamut_client::PlayerStateService* playerState = nullptr);
    ~AddonHost();

    AddonHost(const AddonHost&)            = delete;
    AddonHost& operator=(const AddonHost&) = delete;

    void SetPlayerStateService(bahamut_client::PlayerStateService* playerState);
    void SetTargetDistanceService(bahamut_client::TargetDistanceService* targetDistance);
    void SetCustomTimerService(bahamut_client::CustomTimerService* customTimer);
    void SetEnemyCastService(bahamut_client::EnemyCastService* enemyCast);
    void SetActorNameService(bahamut_client::ActorNameService* actorNames);
    void SetClipboardSink(const AddonClipboardSink& sink);
    void SetChatSink(const AddonChatSink& sink);
    void SetUrlSink(const AddonUrlSink& sink);

    void LoadManifests(const std::vector<std::filesystem::path>& manifests,
                       const std::filesystem::path&              settingsRoot,
                       const std::filesystem::path&              chatLogsRoot);
    bool Enable(const std::filesystem::path& manifestPath);
    bool Reload(std::string_view addonId);
    bool Disable(std::string_view addonId);
    bool IsLoaded(std::string_view addonId) const;
    bool TargetlinesEnabled() const;
    bool HasCommandAddons() const;
    bool DispatchCommand(std::string_view command);
    void QueueChat(std::string_view source, std::string_view message);
    void QueueAreaTransition(const packet_observer::GameMessage& message);
    void QueueCombatResult(const packet_observer::GameMessage& message);
    bool QueuePacketFrame(const packet_observer::FrameInfo& frame);
    // Launcher version (empty means unknown) and runtime API version for capture metadata.
    void SetRuntimeIdentity(std::string launcherVersion, std::uint32_t runtimeApiVersion);
    void Update(double frameDeltaSeconds);
    void FailNextPacketWritesForTest(int count);
    // Blocks until queued packet frames are written; for tests.
    bool WaitForPacketCaptureIdle(std::chrono::milliseconds timeout);
    void Draw(const AddonWindowSink& sink);

    std::size_t LoadedCount() const;
    std::size_t FaultedCount() const;

private:
    struct Instance;

    static int LuaSettingsGet(lua_State* state);
    static int LuaSettingsSet(lua_State* state);
    static int LuaPlayerState(lua_State* state);
    static int LuaTargetDistance(lua_State* state);
    static int LuaTargetHp(lua_State* state);
    static int LuaCustomTimer(lua_State* state);
    static int LuaEnemyCasts(lua_State* state);
    static int LuaWindow(lua_State* state);
    static int LuaRawText(lua_State* state);
    static int LuaClipboardSet(lua_State* state);
    static int LuaChatPrint(lua_State* state);
    static int LuaChatLogWrite(lua_State* state);
    static int LuaPacketLogStart(lua_State* state);
    static int LuaPacketLogStop(lua_State* state);
    static int LuaPacketLogSplit(lua_State* state);
    static int LuaPacketLogStatus(lua_State* state);
    static int LuaCombatEvents(lua_State* state);
    static int LuaCombatMeter(lua_State* state);
    static int LuaTargetlines(lua_State* state);
    static int LuaOpenUrl(lua_State* state);

    static std::unique_ptr<Instance> LoadOne(
        const std::filesystem::path&           manifestPath,
        const std::filesystem::path&           settingsRoot,
        const std::filesystem::path&           chatLogsRoot,
        AddonHost*                             owner,
        bahamut_client::PlayerStateService*    playerState,
        bahamut_client::TargetDistanceService* targetDistance,
        bahamut_client::CustomTimerService*    customTimer,
        bahamut_client::EnemyCastService*      enemyCast,
        const AddonClipboardSink*              clipboardSink,
        const AddonChatSink*                   chatSink,
        const AddonUrlSink*                    urlSink);
    static void Close(Instance& addon);
    static bool Invoke(Instance& addon, const char* callback, int argumentCount = 0);
    static bool InvokeCommand(Instance& addon, std::string_view command, bool& handled);
    static bool InvokeChat(Instance& addon, std::string_view message);
    static bool InvokeAreaChanged(Instance& addon, std::uint32_t zoneId, std::string_view areaName, std::string_view regionName);

    struct CombatEvent
    {
        std::uint32_t sourceId = 0;
        std::uint16_t amount   = 0;
        std::uint8_t  kind     = 0;
    };

    void                                                 DispatchAreaTransition();
    void                                                 ResetAreaTransition();
    void                                                 StopFaultedCombatParser();
    bool                                                 StartPacketCapture(Instance& addon);
    bool                                                 StopPacketCapture();
    bool                                                 SplitPacketCapture();
    bool                                                 ReloadAddon(std::string_view addonId, packet_capture::EndReason reason);
    void                                                 HandlePacketLoggerFault();
    void                                                 PushPacketNotice(std::string notice);
    void                                                 StopPacketCaptureAsync(packet_capture::EndReason reason);
    void                                                 ReplacePacketWriter(const Instance& addon);
    void                                                 DrainPacketNotices(packet_capture::PacketCaptureWriter& writer);
    std::shared_ptr<packet_capture::PacketCaptureWriter> PacketWriter() const;

    std::vector<std::unique_ptr<Instance>>                            addons_;
    std::filesystem::path                                             settingsRoot_;
    std::filesystem::path                                             chatLogsRoot_;
    bahamut_client::PlayerStateService*                               playerState_    = nullptr;
    bahamut_client::TargetDistanceService*                            targetDistance_ = nullptr;
    bahamut_client::CustomTimerService*                               customTimer_    = nullptr;
    bahamut_client::EnemyCastService*                                 enemyCast_      = nullptr;
    AddonClipboardSink                                                clipboardSink_;
    AddonChatSink                                                     chatSink_;
    AddonUrlSink                                                      urlSink_;
    std::mutex                                                        chatQueueMutex_;
    std::vector<std::string>                                          chatQueue_;
    std::mutex                                                        areaQueueMutex_;
    std::optional<std::uint32_t>                                      queuedAreaZoneId_;
    std::uint32_t                                                     lastAreaZoneId_ = 0;
    std::string                                                       lastAreaName_;
    std::string                                                       lastRegionName_;
    std::string                                                       launcherVersion_;
    std::uint32_t                                                     runtimeApiVersion_   = 0;
    int                                                               packetLoggerFaults_  = 0;
    std::atomic<bool>                                                 packetLoggerEnabled_ = false;
    std::atomic<std::uint8_t>                                         packetScope_{ static_cast<std::uint8_t>(packet_capture::Scope::Standard) };
    mutable std::mutex                                                packetWriterMutex_;
    std::shared_ptr<packet_capture::PacketCaptureWriter>              packetWriter_;
    std::deque<std::string>                                           packetNotices_;
    std::vector<std::shared_ptr<packet_capture::PacketCaptureWriter>> retiredPacketWriters_;
    std::atomic<bool>                                                 combatParserEnabled_ = false;
    std::mutex                                                        combatQueueMutex_;
    std::vector<CombatEvent>                                          combatQueue_;
    std::uint64_t                                                     combatDropped_ = 0;
    bahamut_client::ActorNameService*                                 actorNames_    = nullptr;
};
