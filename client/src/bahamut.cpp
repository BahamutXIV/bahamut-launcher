#include "actor_name.h"
#include "addon_host.h"
#include "camera_zoom.h"
#include "chat_boundary.h"
#include "dat_overlay.h"
#include "input.h"
#include "native_plugin_host.h"
#include "object_distance.h"
#include "overlay.h"
#include "packet_observer.h"
#include "player_state.h"
#include "render_boundary.h"
#include "retail_identity.h"
#include "runtime_api.h"
#include "startup_script.h"
#include "target_distance.h"
#include "targetlines.h"
#include "targetlines_probe.h"

#include <algorithm>
#include <cstring>
#include <cwchar>
#include <filesystem>
#include <shellapi.h>
#include <string>
#include <unordered_map>
#include <vector>

namespace
{

constexpr wchar_t kReadyEnvironment[]          = L"BAHAMUT_RUNTIME_READY_EVENT";
constexpr wchar_t kEntryEnvironment[]          = L"BAHAMUT_RUNTIME_ENTRY_EVENT";
constexpr wchar_t kNoSignalEnvironment[]       = L"BAHAMUT_RUNTIME_NO_SIGNAL";
constexpr wchar_t kTestStubEnvironment[]       = L"BAHAMUT_RUNTIME_TEST_STUB";
constexpr wchar_t kFrameEnvironment[]          = L"BAHAMUT_RUNTIME_FRAME_EVENT";
constexpr wchar_t kTelemetryEnvironment[]      = L"BAHAMUT_RUNTIME_TELEMETRY_MAPPING";
constexpr wchar_t kAddonManifestsEnvironment[] = L"BAHAMUT_RUNTIME_ADDON_MANIFESTS";
constexpr wchar_t kAddonCatalogEnvironment[]   = L"BAHAMUT_RUNTIME_ADDON_CATALOG";
constexpr wchar_t kAddonSettingsEnvironment[]  = L"BAHAMUT_RUNTIME_ADDON_SETTINGS";
constexpr wchar_t kChatLogsEnvironment[]       = L"BAHAMUT_RUNTIME_CHAT_LOGS";
constexpr wchar_t kScreenshotPluginEnvironment[] =
    L"BAHAMUT_RUNTIME_SCREENSHOT_PLUGIN";
constexpr wchar_t kScreenshotsEnvironment[]      = L"BAHAMUT_RUNTIME_SCREENSHOTS";
constexpr wchar_t kScreenshotFormatEnvironment[] = L"BAHAMUT_RUNTIME_SCREENSHOT_FORMAT";
constexpr wchar_t kScreenshotHideOverlaysEnvironment[] =
    L"BAHAMUT_RUNTIME_SCREENSHOT_HIDE_OVERLAYS";
constexpr wchar_t kScreenshotHotkeyEnvironment[] =
    L"BAHAMUT_RUNTIME_SCREENSHOT_HOTKEY";
constexpr wchar_t kScreenshotEnabledEnvironment[] =
    L"BAHAMUT_RUNTIME_SCREENSHOT_ENABLED";
constexpr wchar_t kDiscordPluginEnvironment[]         = L"BAHAMUT_RUNTIME_DISCORD_PLUGIN";
constexpr wchar_t kDiscordEnabledEnvironment[]        = L"BAHAMUT_RUNTIME_DISCORD_ENABLED";
constexpr wchar_t kObjectDistanceEnabledEnvironment[] = L"BAHAMUT_RUNTIME_OBJECT_DISTANCE_ENABLED";
constexpr wchar_t kCameraZoomEnabledEnvironment[]     = L"BAHAMUT_RUNTIME_CAMERA_ZOOM_ENABLED";
constexpr wchar_t kStartupScriptEnvironment[]         = L"BAHAMUT_RUNTIME_STARTUP_SCRIPT";
constexpr wchar_t kDatPackageRootsEnvironment[] =
    L"BAHAMUT_RUNTIME_DAT_PACKAGE_ROOTS";
constexpr wchar_t     kBorderlessEnvironment[]    = L"BAHAMUT_RUNTIME_BORDERLESS";
constexpr std::size_t kMaximumAddonClipboardBytes = 4096;

bool SetAddonClipboardText(void*, std::string_view text)
{
    if (text.size() > kMaximumAddonClipboardBytes || text.find('\0') != std::string_view::npos)
    {
        return false;
    }
    const int wideLength     = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text.data(), static_cast<int>(text.size()), nullptr, 0);
    HWND      owner          = GetForegroundWindow();
    DWORD     ownerProcessId = 0;
    if (wideLength == 0 || owner == nullptr || GetWindowThreadProcessId(owner, &ownerProcessId) == 0 || ownerProcessId != GetCurrentProcessId() || !OpenClipboard(owner))
    {
        return false;
    }

    HGLOBAL memory  = GlobalAlloc(GMEM_MOVEABLE,
                                  static_cast<SIZE_T>(wideLength + 1) * sizeof(wchar_t));
    auto*   output  = memory == nullptr
                          ? nullptr
                          : static_cast<wchar_t*>(GlobalLock(memory));
    bool    written = false;
    if (output != nullptr && MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text.data(), static_cast<int>(text.size()), output, wideLength) == wideLength)
    {
        output[wideLength] = L'\0';
        GlobalUnlock(memory);
        output = nullptr;
        if (EmptyClipboard() && SetClipboardData(CF_UNICODETEXT, memory) != nullptr)
        {
            written = true;
            memory  = nullptr;
        }
    }
    if (output != nullptr)
    {
        GlobalUnlock(memory);
    }
    if (memory != nullptr)
    {
        GlobalFree(memory);
    }
    CloseClipboard();
    return written;
}

bool OpenAddonUrl(void*, std::string_view url)
{
    const int wideLength = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, url.data(), static_cast<int>(url.size()), nullptr, 0);
    if (wideLength == 0)
    {
        return false;
    }
    std::wstring wideUrl(static_cast<std::size_t>(wideLength), L'\0');
    if (MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, url.data(), static_cast<int>(url.size()), wideUrl.data(), wideLength) != wideLength)
    {
        return false;
    }
    return reinterpret_cast<INT_PTR>(ShellExecuteW(nullptr, L"open", wideUrl.c_str(), nullptr, nullptr, SW_SHOWNORMAL)) > 32;
}

bool HasValue(const wchar_t* name)
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(name, value, ARRAYSIZE(value));
    return count != 0 && count < ARRAYSIZE(value) && value[0] != L'0';
}

void SignalNamedEvent(const wchar_t* environmentName)
{
    wchar_t     eventName[256]{};
    const DWORD count = GetEnvironmentVariableW(environmentName, eventName, ARRAYSIZE(eventName));
    if (count == 0 || count >= ARRAYSIZE(eventName))
    {
        return;
    }

    HANDLE eventHandle = OpenEventW(EVENT_MODIFY_STATE, FALSE, eventName);
    if (eventHandle != nullptr)
    {
        SetEvent(eventHandle);
        CloseHandle(eventHandle);
    }
}

HANDLE OpenNamedEvent(const wchar_t* environmentName)
{
    wchar_t     eventName[256]{};
    const DWORD count = GetEnvironmentVariableW(environmentName, eventName, ARRAYSIZE(eventName));
    if (count == 0 || count >= ARRAYSIZE(eventName))
    {
        return nullptr;
    }
    return OpenEventW(EVENT_MODIFY_STATE, FALSE, eventName);
}

std::wstring EnvironmentValue(const wchar_t* name)
{
    const DWORD needed = GetEnvironmentVariableW(name, nullptr, 0);
    if (needed == 0)
    {
        return {};
    }
    std::wstring value(needed, L'\0');
    const DWORD  copied = GetEnvironmentVariableW(name, value.data(), needed);
    if (copied == 0 || copied >= needed)
    {
        return {};
    }
    value.resize(copied);
    return value;
}

std::vector<std::filesystem::path> PathList(const wchar_t* environmentName)
{
    const std::wstring                 value = EnvironmentValue(environmentName);
    std::vector<std::filesystem::path> manifests;
    std::size_t                        start = 0;
    while (start < value.size())
    {
        const std::size_t  end  = value.find(L'\n', start);
        const std::wstring path = value.substr(start,
                                               end == std::wstring::npos ? std::wstring::npos : end - start);
        if (!path.empty())
        {
            manifests.emplace_back(path);
        }
        if (end == std::wstring::npos)
            break;
        start = end + 1;
    }
    return manifests;
}

UINT ScreenshotHotkey()
{
    const std::wstring value = EnvironmentValue(kScreenshotHotkeyEnvironment);
    if (value == L"print_screen")
        return VK_SNAPSHOT;
    if (value == L"insert")
        return VK_INSERT;
    if (value.size() == 2 && value[0] == L'f' && value[1] >= L'1' && value[1] <= L'9')
    {
        return VK_F1 + static_cast<UINT>(value[1] - L'1');
    }
    return 0;
}

class RuntimeHost
{
public:
    DWORD Initialize()
    {
        if (!VerifyRuntimeHostIdentity())
        {
            return 10;
        }
        if (!OpenTelemetry())
        {
            ReleaseHandles();
            return 11;
        }
        frameEvent_                                                      = OpenNamedEvent(kFrameEnvironment);
        const auto                                             manifests = PathList(kAddonManifestsEnvironment);
        const auto                                             catalog   = PathList(kAddonCatalogEnvironment);
        StartupScriptPlan                                      startupPlan;
        std::unordered_map<std::string, std::filesystem::path> manifestsById;
        if (!PrepareStartupScript(catalog, startupPlan, manifestsById))
        {
            ReleaseHandles();
            return 13;
        }
        const std::wstring settings = EnvironmentValue(kAddonSettingsEnvironment);
        const std::wstring chatLogs = EnvironmentValue(kChatLogsEnvironment);
        screenshotPluginPath_       = std::filesystem::path(
            EnvironmentValue(kScreenshotPluginEnvironment));
        const std::wstring screenshots      = EnvironmentValue(kScreenshotsEnvironment);
        const std::wstring screenshotFormat = EnvironmentValue(kScreenshotFormatEnvironment);
        const std::wstring hideOverlays     = EnvironmentValue(
            kScreenshotHideOverlaysEnvironment);
        const std::wstring screenshotEnabled = EnvironmentValue(
            kScreenshotEnabledEnvironment);
        screenshotConfiguration_.storagePath  = std::filesystem::path(screenshots);
        screenshotConfiguration_.format       = screenshotFormat == L"bmp"
                                                    ? BahamutNativePluginFormatBmp
                                                    : BahamutNativePluginFormatPng;
        screenshotConfiguration_.hideOverlays = hideOverlays != L"false";
        screenshotConfiguration_.hotkey       = ScreenshotHotkey();
        screenshotConfiguration_.enabled      = screenshotEnabled.empty()
                                                    ? screenshotConfiguration_.hotkey != 0
                                                    : screenshotEnabled == L"true";
        discordPluginPath_                    = std::filesystem::path(
            EnvironmentValue(kDiscordPluginEnvironment));
        discordConfiguration_.enabled =
            EnvironmentValue(kDiscordEnabledEnvironment) == L"true";
        if (screenshotConfiguration_.enabled && !LoadScreenshotPlugin())
        {
            RecordPluginError("screenshot", nativePluginHost_.LastError());
            ReleaseHandles();
            return 15;
        }
        if (discordConfiguration_.enabled && !discordPluginHost_.Load(
                                                 discordPluginPath_, "discord-rpc", discordConfiguration_))
        {
            RecordPluginError("discord-rpc", discordPluginHost_.LastError());
            ReleaseHandles();
            return 15;
        }
        addonHost_.SetPlayerStateService(&playerState_);
        addonHost_.SetTargetDistanceService(&targetDistance_);
        addonHost_.SetActorNameService(&actorNames_);
        addonHost_.SetClipboardSink({ nullptr, &SetAddonClipboardText });
        addonHost_.SetChatSink({ nullptr, &PrintAddonChatText });
        addonHost_.SetUrlSink({ nullptr, &OpenAddonUrl });
        if (!settings.empty() && !chatLogs.empty())
        {
            ConfigureOverlayLayout(
                std::filesystem::path(settings) / L"layout.ini");
            addonHost_.LoadManifests(
                manifests, std::filesystem::path(settings), std::filesystem::path(chatLogs));
        }
        if (!ApplyStartupScript(startupPlan, manifestsById))
        {
            ReleaseHandles();
            return nativePluginFailed_ ? 15 : 13;
        }
        ConfigureBuiltInHotkeys(fillModeHotkeyEnabled_, fpsHotkeyEnabled_);
        packetObserver_.SetSink([this](const packet_observer::GameMessage& message)
                                {
                                    playerState_.Observe(message);
                                    addonHost_.QueueAreaTransition(message);
                                    actorNames_.Observe(message);
                                    targetDistance_.Observe(message);
                                    targetlines_.Observe(message);
                                    addonHost_.QueueCombatResult(message);
                                });
        packetObserver_.SetFrameSink([this](packet_observer::Direction    direction,
                                            std::span<const std::uint8_t> frame)
                                     {
                                         addonHost_.QueuePacketFrame(direction, frame);
                                     });
        if (!InstallRenderBoundary(telemetry_, &sharedState_->bootstrap, frameEvent_, &addonHost_, &nativePluginHost_, &discordPluginHost_, &playerState_, &packetObserver_, &objectDistance_, EnvironmentValue(kObjectDistanceEnabledEnvironment) == L"true", &cameraZoom_, EnvironmentValue(kCameraZoomEnabledEnvironment) == L"true", EnvironmentValue(kBorderlessEnvironment) == L"true"))
        {
            ReleaseHandles();
            return 12;
        }
        bahamut_client::StartTargetlinesRenderer(&targetDistance_, &targetlines_, addonHost_.TargetlinesEnabled());
        if (!datOverlay_.Install(PathList(kDatPackageRootsEnvironment),
                                 !HasValue(kTestStubEnvironment)))
        {
            ReleaseHandles();
            return 14;
        }
        if (!HasValue(kNoSignalEnvironment))
        {
            SignalNamedEvent(kReadyEnvironment);
        }
        return 1;
    }

private:
    bool PrepareStartupScript(const std::vector<std::filesystem::path>&               catalog,
                              StartupScriptPlan&                                      plan,
                              std::unordered_map<std::string, std::filesystem::path>& manifestsById)
    {
        std::vector<std::string> addonIds;
        for (const auto& manifest : catalog)
        {
            const auto id = ReadAddonManifestId(manifest);
            if (!id || !manifestsById.emplace(*id, manifest).second)
            {
                RecordStartupError(0, "addon catalog contains an invalid or duplicate manifest");
                return false;
            }
            addonIds.push_back(*id);
        }

        StartupScriptError          error;
        const std::filesystem::path path(EnvironmentValue(kStartupScriptEnvironment));
        if (!ParseStartupScript(path, addonIds, plan, error))
        {
            RecordStartupError(error.line, error.message);
            return false;
        }
        return true;
    }

    bool ApplyStartupScript(const StartupScriptPlan&                                      plan,
                            const std::unordered_map<std::string, std::filesystem::path>& manifestsById)
    {
        for (const auto& action : plan.actions)
        {
            if (!ApplyStartupAction(action, manifestsById))
            {
                RecordStartupError(action.line,
                                   "startup action could not be applied: " + action.addonId);
                return false;
            }
        }
        return true;
    }

    bool ApplyStartupAction(const StartupAction&                                          action,
                            const std::unordered_map<std::string, std::filesystem::path>& manifestsById)
    {
        switch (action.kind)
        {
            case StartupActionKind::ScreenshotLoad:
                screenshotConfiguration_.enabled = true;
                if (!LoadScreenshotPlugin())
                {
                    RecordPluginError("screenshot", nativePluginHost_.LastError());
                    return false;
                }
                return true;
            case StartupActionKind::ScreenshotUnload:
                screenshotConfiguration_.enabled = false;
                if (!nativePluginHost_.SetEnabled(false))
                {
                    RecordPluginError("screenshot", nativePluginHost_.LastError());
                    return false;
                }
                return true;
            case StartupActionKind::ScreenshotBind:
                screenshotConfiguration_.hotkey       = action.virtualKey;
                screenshotConfiguration_.hideOverlays = action.hideOverlays;
                if (!nativePluginHost_.Configure(screenshotConfiguration_))
                {
                    RecordPluginError("screenshot", nativePluginHost_.LastError());
                    return false;
                }
                return true;
            case StartupActionKind::FillModeBind:
                fillModeHotkeyEnabled_ = true;
                return true;
            case StartupActionKind::FpsBind:
                fpsHotkeyEnabled_ = true;
                return true;
            case StartupActionKind::AddonLoad:
            {
                if (addonHost_.IsLoaded(action.addonId))
                {
                    return true;
                }
                const auto found = manifestsById.find(action.addonId);
                return found != manifestsById.end() && addonHost_.Enable(found->second);
            }
            case StartupActionKind::AddonUnload:
                return !addonHost_.IsLoaded(action.addonId) || addonHost_.Disable(action.addonId);
            case StartupActionKind::AddonReload:
            {
                if (addonHost_.IsLoaded(action.addonId))
                {
                    return addonHost_.Reload(action.addonId);
                }
                const auto found = manifestsById.find(action.addonId);
                return found != manifestsById.end() && addonHost_.Enable(found->second);
            }
        }
        return false;
    }

    void RecordStartupError(unsigned long line, const std::string& message)
    {
        sharedState_->bootstrap.startupScriptLine = static_cast<LONG>(line);
        const std::size_t count                   = std::min<std::size_t>(
            message.size(), ARRAYSIZE(sharedState_->bootstrap.startupScriptMessage) - 1);
        for (std::size_t index = 0; index < count; ++index)
        {
            sharedState_->bootstrap.startupScriptMessage[index] =
                static_cast<unsigned char>(message[index]);
        }
        sharedState_->bootstrap.startupScriptMessage[count] = L'\0';
    }

    void RecordPluginError(const char* id, const std::string& message)
    {
        nativePluginFailed_ = true;
        const auto copy     = [](wchar_t* destination, std::size_t capacity, const char* source, std::size_t length)
        {
            const std::size_t count = std::min(length, capacity - 1);
            for (std::size_t index = 0; index < count; ++index)
            {
                destination[index] = static_cast<unsigned char>(source[index]);
            }
            destination[count] = L'\0';
        };
        copy(sharedState_->bootstrap.nativePluginId,
             ARRAYSIZE(sharedState_->bootstrap.nativePluginId),
             id,
             std::strlen(id));
        copy(sharedState_->bootstrap.nativePluginMessage,
             ARRAYSIZE(sharedState_->bootstrap.nativePluginMessage),
             message.c_str(),
             message.size());
    }

    bool LoadScreenshotPlugin()
    {
        if (nativePluginHost_.IsLoaded())
        {
            return nativePluginHost_.Configure(screenshotConfiguration_);
        }
        return nativePluginHost_.Load(screenshotPluginPath_, "screenshot", screenshotConfiguration_);
    }

    bool OpenTelemetry()
    {
        wchar_t     mappingName[256]{};
        const DWORD count = GetEnvironmentVariableW(kTelemetryEnvironment,
                                                    mappingName,
                                                    ARRAYSIZE(mappingName));
        if (count == 0 || count >= ARRAYSIZE(mappingName))
        {
            return false;
        }
        telemetryMapping_ = OpenFileMappingW(
            FILE_MAP_ALL_ACCESS, FALSE, mappingName);
        if (telemetryMapping_ == nullptr)
        {
            return false;
        }
        sharedState_ = static_cast<BahamutRuntimeSharedState*>(MapViewOfFile(
            telemetryMapping_, FILE_MAP_ALL_ACCESS, 0, 0, sizeof(BahamutRuntimeSharedState)));
        if (sharedState_ == nullptr)
        {
            return false;
        }
        telemetry_ = &sharedState_->telemetry;
        return true;
    }

    void ReleaseHandles()
    {
        nativePluginHost_.Shutdown();
        discordPluginHost_.Shutdown();
        datOverlay_.Shutdown();
        if (frameEvent_ != nullptr)
        {
            CloseHandle(frameEvent_);
            frameEvent_ = nullptr;
        }
        if (sharedState_ != nullptr)
        {
            UnmapViewOfFile(sharedState_);
            sharedState_ = nullptr;
            telemetry_   = nullptr;
        }
        if (telemetryMapping_ != nullptr)
        {
            CloseHandle(telemetryMapping_);
            telemetryMapping_ = nullptr;
        }
    }

    HANDLE                                telemetryMapping_ = nullptr;
    BahamutRuntimeSharedState*            sharedState_      = nullptr;
    BahamutRuntimeTelemetry*              telemetry_        = nullptr;
    HANDLE                                frameEvent_       = nullptr;
    bahamut_client::PlayerStateService    playerState_;
    bahamut_client::ActorNameService      actorNames_{ &playerState_ };
    bahamut_client::TargetDistanceService targetDistance_{ &playerState_, &actorNames_ };
    bahamut_client::TargetlinesService    targetlines_{ &playerState_, &targetDistance_ };
    packet_observer::PacketObserver       packetObserver_;
    bahamut_client::ObjectDistanceService objectDistance_;
    bahamut_client::CameraZoomService     cameraZoom_;
    AddonHost                             addonHost_;
    std::filesystem::path                 screenshotPluginPath_;
    NativePluginConfiguration             screenshotConfiguration_;
    NativePluginHost                      nativePluginHost_;
    std::filesystem::path                 discordPluginPath_;
    NativePluginConfiguration             discordConfiguration_;
    NativePluginHost                      discordPluginHost_;
    bool                                  fillModeHotkeyEnabled_ = false;
    bool                                  fpsHotkeyEnabled_      = false;
    bool                                  nativePluginFailed_    = false;
    bahamut_dat_overlay::DatOverlay       datOverlay_;
};

RuntimeHost gRuntimeHost;

} // namespace

extern "C" DWORD WINAPI BahamutRuntimeApiVersion(LPVOID parameter)
{
    UNREFERENCED_PARAMETER(parameter);
    return kBahamutRuntimeApiVersion;
}

extern "C" DWORD WINAPI BahamutRuntimeInitialize(LPVOID parameter)
{
    UNREFERENCED_PARAMETER(parameter);
    return gRuntimeHost.Initialize();
}

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID reserved)
{
    UNREFERENCED_PARAMETER(reserved);
    if (reason == DLL_PROCESS_ATTACH)
    {
        DisableThreadLibraryCalls(instance);
        // The helper calls the initializer only after LoadLibraryW returns,
        // keeping D3D9 and hook setup outside the loader lock.
        SignalNamedEvent(kEntryEnvironment);
    }
    return TRUE;
}
