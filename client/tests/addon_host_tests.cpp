#include "addon_host.h"
#include "packet_observer.h"
#include "player_state.h"
#include "target_distance.h"

#include <algorithm>
#include <array>
#include <cstring>
#include <ctime>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <set>
#include <string>
#include <string_view>
#include <vector>

namespace
{

struct DrawCapture
{
    int                              windows = 0;
    std::string                      addonId;
    std::string                      title;
    std::string                      text;
    bool                             windowLocked = false;
    int                              rawTexts     = 0;
    std::string                      rawAddonId;
    std::string                      rawText;
    float                            rawRed       = 0.0F;
    float                            rawGreen     = 0.0F;
    float                            rawBlue      = 0.0F;
    float                            rawAlpha     = 0.0F;
    bool                             rawLocked    = false;
    float                            rawSize      = 0.0F;
    int                              combatMeters = 0;
    std::string                      combatAddonId;
    std::string                      combatMode;
    std::vector<AddonCombatMeterRow> combatRows;
    bool                             combatLocked     = false;
    bool                             combatIncomplete = false;
    int                              targetlines      = 0;
    std::string                      targetlinesAddonId;
    std::string                      posText;
    std::vector<std::string>         chatLines;
    std::string                      clipboardText;
    int                              clipboardWrites = 0;
    std::vector<std::string>         openedUrls;
};

void CaptureWindow(void* context, const char* addonId, const char* title, const char* text, bool locked)
{
    auto& capture = *static_cast<DrawCapture*>(context);
    ++capture.windows;
    capture.addonId      = addonId;
    capture.title        = title;
    capture.text         = text;
    capture.windowLocked = locked;
    if (std::string_view(addonId) == "pos")
    {
        capture.posText = text;
    }
}

void CaptureRawText(void* context, const char* addonId, const char* text, float red, float green, float blue, float alpha, bool locked, float fontSize)
{
    auto& capture = *static_cast<DrawCapture*>(context);
    ++capture.rawTexts;
    capture.rawAddonId = addonId;
    capture.rawText    = text;
    capture.rawRed     = red;
    capture.rawGreen   = green;
    capture.rawBlue    = blue;
    capture.rawAlpha   = alpha;
    capture.rawLocked  = locked;
    capture.rawSize    = fontSize;
}

void CaptureCombatMeter(void* context, const char* addonId, const char* mode, const AddonCombatMeterRow* rows, std::size_t rowCount, bool locked, bool incomplete)
{
    auto& capture = *static_cast<DrawCapture*>(context);
    ++capture.combatMeters;
    capture.combatAddonId = addonId;
    capture.combatMode    = mode;
    capture.combatRows.clear();
    if (rowCount != 0)
    {
        capture.combatRows.assign(rows, rows + rowCount);
    }
    capture.combatLocked     = locked;
    capture.combatIncomplete = incomplete;
}

void CaptureTargetlines(void* context, const char* addonId)
{
    auto& capture = *static_cast<DrawCapture*>(context);
    ++capture.targetlines;
    capture.targetlinesAddonId = addonId;
}

bool CaptureClipboard(void* context, std::string_view text)
{
    auto& capture = *static_cast<DrawCapture*>(context);
    capture.clipboardText.assign(text);
    ++capture.clipboardWrites;
    return true;
}

bool CaptureChat(void* context, std::string_view text)
{
    auto& capture = *static_cast<DrawCapture*>(context);
    capture.chatLines.emplace_back(text);
    return true;
}

bool CaptureUrl(void* context, std::string_view url)
{
    auto& capture = *static_cast<DrawCapture*>(context);
    capture.openedUrls.emplace_back(url);
    return true;
}

void ResetDrawCapture(DrawCapture& capture)
{
    capture.windows = 0;
    capture.addonId.clear();
    capture.title.clear();
    capture.text.clear();
    capture.windowLocked = false;
    capture.rawTexts     = 0;
    capture.rawAddonId.clear();
    capture.rawText.clear();
    capture.rawRed       = 0.0F;
    capture.rawGreen     = 0.0F;
    capture.rawBlue      = 0.0F;
    capture.rawAlpha     = 0.0F;
    capture.rawLocked    = false;
    capture.rawSize      = 0.0F;
    capture.combatMeters = 0;
    capture.combatAddonId.clear();
    capture.combatMode.clear();
    capture.combatRows.clear();
    capture.combatLocked     = false;
    capture.combatIncomplete = false;
    capture.targetlines      = 0;
    capture.targetlinesAddonId.clear();
    capture.posText.clear();
}

} // namespace

int wmain(int argc, wchar_t** argv)
{
    if (argc != 15)
    {
        std::cerr << "usage: addon-host-tests <fps-manifest> <fault-manifest> <pos-manifest> <wiki-manifest> <chatlogs-manifest> <zonename-manifest> <packetlogger-manifest> <fault-packetlogger-manifest> <distance-manifest> <targethp-manifest> <combatparser-manifest> <targetlines-manifest> <settings> <chat-logs>\n";
        return 2;
    }
    const std::filesystem::path                fpsManifest(argv[1]);
    const std::filesystem::path                errorIsolationManifest(argv[2]);
    const std::filesystem::path                posManifest(argv[3]);
    const std::filesystem::path                wikiManifest(argv[4]);
    const std::filesystem::path                chatlogsManifest(argv[5]);
    const std::filesystem::path                zonenameManifest(argv[6]);
    const std::filesystem::path                packetloggerManifest(argv[7]);
    const std::filesystem::path                faultPacketloggerManifest(argv[8]);
    const std::array<std::filesystem::path, 2> lockManifests = {
        argv[9], argv[10]
    };
    const std::array<std::string_view, 2> lockIds = {
        "distance", "targethp"
    };
    const std::filesystem::path combatParserManifest(argv[11]);
    const std::filesystem::path targetlinesManifest(argv[12]);
    const std::filesystem::path settings(argv[13]);
    const std::filesystem::path chatLogs(argv[14]);
    const std::time_t           now = std::time(nullptr);
    std::tm                     local{};
    localtime_s(&local, &now);
    const std::filesystem::path packetLogs = chatLogs.parent_path() / "packets";
    std::filesystem::create_directories(packetLogs);
    const auto sessionFiles = [&]()
    {
        std::set<std::filesystem::path> files;
        for (const auto& entry : std::filesystem::directory_iterator(packetLogs))
        {
            if (entry.is_regular_file() &&
                entry.path().filename().string().starts_with("capture-") &&
                entry.path().extension() == ".csv")
            {
                files.insert(entry.path());
            }
        }
        return files;
    };
    const auto initialSessions = sessionFiles();
    char       date[16]{};
    std::strftime(date, sizeof(date), "%Y.%m.%d", &local);
    const std::filesystem::path dailyChatLog =
        chatLogs / (std::string(date) + ".log");
    const std::filesystem::path namedChatLog =
        chatLogs / ("Gridaniaclonea_Test_" + std::string(date) + ".log");
    const std::filesystem::path changedChatLog =
        chatLogs / ("Other_Name_" + std::string(date) + ".log");
    std::error_code cleanupError;
    for (const auto& path : {
             settings / L"fps" / L"settings.ini",
             settings / L"style" / L"fps" / L"settings.ini",
             settings / L"error-isolation-test" / L"settings.ini",
             settings / L"pos" / L"settings.ini",
             settings / L"wiki" / L"settings.ini",
             settings / L"distance" / L"settings.ini",
             settings / L"targethp" / L"settings.ini",
             settings / L"combatparser" / L"settings.ini",
             dailyChatLog,
             namedChatLog,
             changedChatLog })
    {
        std::filesystem::remove(path, cleanupError);
        cleanupError.clear();
    }

    bahamut_client::PlayerStateService playerState;
    AddonHost                          host(&playerState);
    DrawCapture                        capture;
    host.SetClipboardSink({ &capture, &CaptureClipboard });
    host.SetChatSink({ &capture, &CaptureChat });
    host.SetUrlSink({ &capture, &CaptureUrl });
    host.LoadManifests({ fpsManifest, errorIsolationManifest, posManifest, wikiManifest, chatlogsManifest, targetlinesManifest }, settings, chatLogs);
    if (host.LoadedCount() != 6 || host.FaultedCount() != 0 || !host.TargetlinesEnabled())
    {
        std::cerr << "both addon states must load independently\n";
        return 1;
    }
    for (int frame = 0; frame < 40; ++frame)
    {
        host.Update(0.02);
    }
    host.Draw({ &capture, &CaptureWindow, &CaptureRawText, nullptr, &CaptureTargetlines });
    if (host.LoadedCount() != 5 || host.FaultedCount() != 1 || !host.TargetlinesEnabled() || capture.windows != 0 || capture.rawTexts != 1 || capture.rawAddonId != "fps" || capture.rawText != "50" || capture.rawRed != 1.0F || capture.rawGreen != 0.0F || capture.rawBlue != 0.0F || capture.rawAlpha != 1.0F || capture.rawSize != 13.0F || capture.rawLocked || capture.rawText.find("FPS") != std::string::npos || capture.targetlines != 1 || capture.targetlinesAddonId != "targetlines" || !capture.posText.empty())
    {
        std::cerr << "addon error isolation or FPS draw failed\n";
        return 1;
    }

    const std::filesystem::path targetlinesFixtureRoot     = settings / "targetlines-lifecycle";
    const std::filesystem::path targetlinesFixtureManifest = targetlinesFixtureRoot / "addon.toml";
    const std::filesystem::path targetlinesOtherRoot       = settings / "targetlines-other";
    const std::filesystem::path targetlinesOtherManifest   = targetlinesOtherRoot / "addon.toml";
    std::filesystem::create_directories(targetlinesFixtureRoot);
    std::filesystem::create_directories(targetlinesOtherRoot);
    {
        std::ofstream manifest(targetlinesFixtureManifest, std::ios::trunc);
        manifest << "id = \"targetlines\"\nentry = \"targetlines.lua\"\n";
        std::ofstream script(targetlinesFixtureRoot / "targetlines.lua", std::ios::trunc);
        script << R"lua(
function load()
    bahamut.targetlines()
end
function update(_)
    bahamut.targetlines()
end
function draw()
    bahamut.targetlines(123)
    bahamut.targetlines()
end
)lua";
        std::ofstream otherManifest(targetlinesOtherManifest, std::ios::trunc);
        otherManifest << "id = \"targetlines-other\"\nentry = \"targetlines-other.lua\"\n";
        std::ofstream otherScript(targetlinesOtherRoot / "targetlines-other.lua", std::ios::trunc);
        otherScript << R"lua(
function draw()
    if bahamut.targetlines ~= nil then error("targetlines API leaked") end
end
)lua";
    }
    AddonHost targetlinesHost;
    targetlinesHost.LoadManifests({ targetlinesFixtureManifest, targetlinesOtherManifest, fpsManifest }, settings / "targetlines-state", chatLogs);
    if (targetlinesHost.LoadedCount() != 3 || targetlinesHost.FaultedCount() != 0 || !targetlinesHost.TargetlinesEnabled())
    {
        std::cerr << "targetlines fixture did not load with a healthy owner\n";
        return 1;
    }
    ResetDrawCapture(capture);
    targetlinesHost.Update(0.02);
    if (capture.targetlines != 0 || !targetlinesHost.TargetlinesEnabled())
    {
        std::cerr << "targetlines update called its sink or disabled the feature\n";
        return 1;
    }
    targetlinesHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, nullptr, &CaptureTargetlines });
    if (capture.targetlines != 1 || capture.targetlinesAddonId != "targetlines" || capture.rawTexts != 1 || targetlinesHost.FaultedCount() != 0)
    {
        std::cerr << "targetlines draw callback or addon ownership failed\n";
        return 1;
    }
    if (!targetlinesHost.Reload("targetlines") || !targetlinesHost.TargetlinesEnabled())
    {
        std::cerr << "host did not restore targetlines after a healthy reload\n";
        return 1;
    }
    ResetDrawCapture(capture);
    targetlinesHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, nullptr, &CaptureTargetlines });
    if (capture.targetlines != 1 || capture.rawTexts != 1)
    {
        std::cerr << "reloaded targetlines or another addon did not draw exactly once\n";
        return 1;
    }
    if (!targetlinesHost.Disable("targetlines") || targetlinesHost.TargetlinesEnabled())
    {
        std::cerr << "disabled targetlines remained enabled\n";
        return 1;
    }
    ResetDrawCapture(capture);
    targetlinesHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, nullptr, &CaptureTargetlines });
    if (capture.targetlines != 0 || capture.rawTexts != 1 || targetlinesHost.FaultedCount() != 0)
    {
        std::cerr << "disabled targetlines failed drawing or fault checks\n";
        return 1;
    }
    if (!targetlinesHost.Enable(targetlinesFixtureManifest) || !targetlinesHost.TargetlinesEnabled())
    {
        std::cerr << "targetlines did not recover after disable\n";
        return 1;
    }
    {
        std::ofstream script(targetlinesFixtureRoot / "targetlines.lua", std::ios::trunc);
        script << "function load() error(\"targetlines load failure\") end\n";
    }
    if (!targetlinesHost.Reload("targetlines") || targetlinesHost.TargetlinesEnabled() ||
        !targetlinesHost.IsLoaded("targetlines") || targetlinesHost.LoadedCount() != 2 || targetlinesHost.FaultedCount() != 1)
    {
        std::cerr << "targetlines load fault left incorrect addon state\n";
        return 1;
    }
    {
        std::ofstream script(targetlinesFixtureRoot / "targetlines.lua", std::ios::trunc);
        script << "function update(_) error(\"targetlines update failure\") end\n"
                  "function draw() bahamut.targetlines() end\n";
    }
    if (!targetlinesHost.Reload("targetlines") || !targetlinesHost.TargetlinesEnabled())
    {
        std::cerr << "targetlines did not recover after load fault\n";
        return 1;
    }
    ResetDrawCapture(capture);
    targetlinesHost.Update(0.02);
    targetlinesHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, nullptr, &CaptureTargetlines });
    if (capture.targetlines != 0 || capture.rawTexts != 1 || targetlinesHost.TargetlinesEnabled() ||
        !targetlinesHost.IsLoaded("targetlines") || targetlinesHost.LoadedCount() != 2 || targetlinesHost.FaultedCount() != 1)
    {
        std::cerr << "targetlines update fault failed addon state or drawing checks\n";
        return 1;
    }
    {
        std::ofstream script(targetlinesFixtureRoot / "targetlines.lua", std::ios::trunc);
        script << "function draw() bahamut.targetlines() error(\"targetlines draw failure\") end\n";
    }
    if (!targetlinesHost.Reload("targetlines") || !targetlinesHost.TargetlinesEnabled())
    {
        std::cerr << "targetlines did not recover after update fault\n";
        return 1;
    }
    ResetDrawCapture(capture);
    targetlinesHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, nullptr, &CaptureTargetlines });
    if (capture.targetlines != 1 || capture.rawTexts != 1 || targetlinesHost.TargetlinesEnabled() ||
        !targetlinesHost.IsLoaded("targetlines") || targetlinesHost.LoadedCount() != 2 || targetlinesHost.FaultedCount() != 1)
    {
        std::cerr << "targetlines draw fault was not isolated from other addons\n";
        return 1;
    }
    std::filesystem::remove(targetlinesFixtureRoot / "targetlines.lua");
    if (targetlinesHost.Reload("targetlines") || targetlinesHost.TargetlinesEnabled() ||
        targetlinesHost.IsLoaded("targetlines") || targetlinesHost.LoadedCount() != 2 || targetlinesHost.FaultedCount() != 0)
    {
        std::cerr << "failed targetlines reload left incorrect addon state\n";
        return 1;
    }
    ResetDrawCapture(capture);
    targetlinesHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, nullptr, &CaptureTargetlines });
    if (capture.targetlines != 0 || capture.rawTexts != 1)
    {
        std::cerr << "healthy addon did not survive failed targetlines reload\n";
        return 1;
    }

    capture.chatLines.clear();
    constexpr std::array<std::string_view, 4> fpsHelp = {
        "/fps", "/fps lock", "/fps color #RRGGBB", "/fps size 8-48"
    };
    if (!host.DispatchCommand("/fps help") || capture.chatLines.size() != fpsHelp.size() ||
        std::any_of(capture.chatLines.begin(), capture.chatLines.end(), [](const std::string& line)
                    {
                        return line.find("/help") != std::string::npos;
                    }) ||
        !std::all_of(fpsHelp.begin(), fpsHelp.end(), [&](std::string_view line)
                     {
                         return std::find(capture.chatLines.begin(), capture.chatLines.end(), line) != capture.chatLines.end();
                     }))
    {
        std::cerr << "FPS help omitted a supported command or included /help\n";
        return 1;
    }
    host.QueueChat("Gridaniaclonea Test", "Hello\aSecond line\r\n");
    host.Update(0.02);
    const std::string chatLog = [&]()
    {
        std::ifstream file(dailyChatLog, std::ios::binary);
        return std::string((std::istreambuf_iterator<char>(file)), {});
    }();
    if (chatLog.find("] Gridaniaclonea Test: Hello\nSecond line\n") == std::string::npos)
    {
        std::cerr << "chat callback did not append a cleaned timestamped line\n";
        return 1;
    }
    host.QueueChat("", "System message");
    host.Update(0.02);
    const std::string systemChatLog = [&]()
    {
        std::ifstream file(dailyChatLog, std::ios::binary);
        return std::string((std::istreambuf_iterator<char>(file)), {});
    }();
    if (systemChatLog.find("] System message\n") == std::string::npos)
    {
        std::cerr << "source-less chat gained a speaker separator\n";
        return 1;
    }
    const std::array<std::uint8_t, 3> packet{ 0x00, 0xA5, 0xFF };
    if (host.QueuePacketFrame(packet_observer::Direction::Incoming, packet))
    {
        std::cerr << "disabled packet logger accepted a frame\n";
        return 1;
    }
    host.Update(0.02);
    if (sessionFiles() != initialSessions || !host.Enable(packetloggerManifest))
    {
        std::cerr << "packet logger did not remain opt-in\n";
        return 1;
    }
    const auto activeSessions = sessionFiles();
    if (activeSessions.size() != initialSessions.size() + 1u ||
        !host.DispatchCommand("/packetlogger status") ||
        capture.chatLines.back().find("recording") == std::string::npos)
    {
        std::cerr << "packet logger did not start a visible capture session\n";
        return 1;
    }
    std::filesystem::path firstSession;
    for (const auto& file : activeSessions)
    {
        if (!initialSessions.contains(file))
        {
            firstSession = file;
            break;
        }
    }
    if (!host.QueuePacketFrame(packet_observer::Direction::Incoming, packet) ||
        !host.QueuePacketFrame(packet_observer::Direction::Outgoing, packet))
    {
        std::cerr << "enabled packet logger rejected a frame\n";
        return 1;
    }
    host.Update(0.02);
    const auto readPacketLog = [&](const std::filesystem::path& filePath)
    {
        std::ifstream file(filePath, std::ios::binary);
        return std::string((std::istreambuf_iterator<char>(file)), {});
    };
    const std::string packetLog  = readPacketLog(firstSession);
    const std::size_t firstRow   = packetLog.find('\n') + 1u;
    const std::size_t firstComma = packetLog.find(',', firstRow);
    if (!packetLog.starts_with("capture_unix_ms_utc,direction,size,frame_hex\n") ||
        firstComma == std::string::npos || firstComma - firstRow != 13u ||
        packetLog.find(",incoming,3,00A5FF\n") == std::string::npos ||
        packetLog.find(",outgoing,3,00A5FF\n") == std::string::npos ||
        !host.DispatchCommand("/packetlogger status") ||
        capture.chatLines.back().find("captured 2, written 2, dropped 0") == std::string::npos)
    {
        std::cerr << "packet callback did not append both directions\n";
        return 1;
    }
    if (!host.QueuePacketFrame(packet_observer::Direction::Incoming, packet) ||
        !host.DispatchCommand("/packetlogger stop") ||
        host.QueuePacketFrame(packet_observer::Direction::Incoming, packet) ||
        !host.DispatchCommand("/packetlogger start") ||
        sessionFiles().size() != activeSessions.size() + 1u ||
        !host.QueuePacketFrame(packet_observer::Direction::Incoming, packet))
    {
        std::cerr << "packet capture start/stop did not separate sessions\n";
        return 1;
    }
    host.Update(0.02);
    const std::string firstSessionAfterStop = readPacketLog(firstSession);
    if (firstSessionAfterStop == packetLog ||
        firstSessionAfterStop.find(",incoming,3,00A5FF\n", packetLog.size()) == std::string::npos)
    {
        std::cerr << "queued frames were lost at capture stop\n";
        return 1;
    }
    for (int index = 0; index < 256; ++index)
    {
        if (!host.QueuePacketFrame(packet_observer::Direction::Incoming, packet))
        {
            std::cerr << "packet queue rejected a frame before its bound\n";
            return 1;
        }
    }
    if (host.QueuePacketFrame(packet_observer::Direction::Incoming, packet))
    {
        std::cerr << "packet queue exceeded its bound\n";
        return 1;
    }
    for (int index = 0; index < 8; ++index)
    {
        host.Update(0.02);
    }
    if (!host.DispatchCommand("/packetlogger status") ||
        capture.chatLines.back().find("captured 261, written 260, dropped 1, queued 0, peak 256") == std::string::npos ||
        !host.Disable("packetlogger"))
    {
        std::cerr << "packet capture status lost frame counts\n";
        return 1;
    }
    if (host.QueuePacketFrame(packet_observer::Direction::Incoming, packet))
    {
        std::cerr << "disabled packet logger accepted another frame\n";
        return 1;
    }
    host.Update(0.02);
    if (readPacketLog(firstSession) != firstSessionAfterStop)
    {
        std::cerr << "disabled packet logger continued writing\n";
        return 1;
    }
    const auto beforeRotation = sessionFiles();
    if (!host.Enable(packetloggerManifest))
    {
        std::cerr << "rotation capture did not start\n";
        return 1;
    }
    std::filesystem::path rotationBase;
    for (const auto& file : sessionFiles())
    {
        if (!beforeRotation.contains(file))
        {
            rotationBase = file;
            break;
        }
    }
    const std::array<std::uint8_t, 65536> largePacket{};
    for (int index = 0; index < 130; ++index)
    {
        if (!host.QueuePacketFrame(packet_observer::Direction::Incoming, largePacket))
        {
            std::cerr << "rotation capture rejected a frame\n";
            return 1;
        }
        host.Update(0.02);
    }
    const std::filesystem::path secondPart = rotationBase.parent_path() /
                                             (rotationBase.stem().string() + "-part0002.csv");
    const auto                  dataRows   = [](const std::filesystem::path& filePath)
    {
        std::ifstream file(filePath, std::ios::binary);
        std::string   line;
        if (!std::getline(file, line) ||
            line != "capture_unix_ms_utc,direction,size,frame_hex")
        {
            return -1;
        }
        int rows = 0;
        while (std::getline(file, line))
        {
            ++rows;
        }
        return rows;
    };
    if (rotationBase.empty() || !std::filesystem::exists(secondPart) ||
        std::filesystem::file_size(rotationBase) > 16u * 1024u * 1024u ||
        std::filesystem::file_size(secondPart) > 16u * 1024u * 1024u ||
        dataRows(rotationBase) + dataRows(secondPart) != 130 ||
        !host.DispatchCommand("/packetlogger status") ||
        capture.chatLines.back().find("captured 130, written 130, dropped 0") == std::string::npos ||
        capture.chatLines.back().find(secondPart.filename().string()) == std::string::npos ||
        !host.Disable("packetlogger"))
    {
        std::cerr << "packet capture rotation lost rows or exceeded its file bound\n";
        return 1;
    }
    if (!host.Enable(faultPacketloggerManifest) ||
        !host.QueuePacketFrame(packet_observer::Direction::Incoming, packet))
    {
        std::cerr << "fault fixture did not start capture\n";
        return 1;
    }
    host.Update(0.02);
    if (host.QueuePacketFrame(packet_observer::Direction::Incoming, packet) ||
        capture.chatLines.back() != "Packet capture stopped: logger callback failed" ||
        !host.Disable("packetlogger"))
    {
        std::cerr << "faulted packet logger kept capture enabled\n";
        return 1;
    }
    if (!host.DispatchCommand("/fps"))
    {
        std::cerr << "fps toggle command was not handled\n";
        return 1;
    }
    ResetDrawCapture(capture);
    host.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 0 || capture.windows != 0)
    {
        std::cerr << "fps toggle did not hide the overlay\n";
        return 1;
    }
    if (!host.Reload("fps"))
    {
        std::cerr << "FPS hidden-state reload failed\n";
        return 1;
    }
    ResetDrawCapture(capture);
    host.Update(0.02);
    host.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 0 || capture.windows != 0)
    {
        std::cerr << "fps visibility did not persist across reload\n";
        return 1;
    }
    if (!host.DispatchCommand("/fps"))
    {
        std::cerr << "fps toggle command did not restore visibility\n";
        return 1;
    }
    const std::size_t fpsLockChatStart = capture.chatLines.size();
    if (!host.DispatchCommand("/fps lock") || capture.chatLines.size() != fpsLockChatStart + 1 || capture.chatLines.back() != "FPS overlay locked")
    {
        std::cerr << "fps lock command did not persist or report\n";
        return 1;
    }
    ResetDrawCapture(capture);
    host.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 1 || !capture.rawLocked)
    {
        std::cerr << "fps lock state was not submitted to the draw sink\n";
        return 1;
    }
    if (!host.Reload("fps"))
    {
        std::cerr << "FPS lock-state reload failed\n";
        return 1;
    }
    ResetDrawCapture(capture);
    host.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 1 || !capture.rawLocked)
    {
        std::cerr << "fps lock state did not persist across reload\n";
        return 1;
    }
    if (!host.DispatchCommand("/fps lock"))
    {
        std::cerr << "fps unlock command was not handled\n";
        return 1;
    }
    bahamut_client::PlayerStateSnapshot snapshot;
    snapshot.actorId     = 0x42;
    snapshot.zoneId      = 230;
    snapshot.x           = 12.5F;
    snapshot.y           = 4.25F;
    snapshot.z           = -7.75F;
    snapshot.rotation    = -1.5F;
    snapshot.displayName = "Gridaniaclonea Test";
    if (!playerState.Publish(snapshot))
    {
        std::cerr << "valid player state did not publish\n";
        return 1;
    }
    host.QueueChat("Gridaniaclonea Test", "Named log message");
    host.Update(0.02);
    const std::string namedLog = [&]()
    {
        std::ifstream file(namedChatLog, std::ios::binary);
        return std::string((std::istreambuf_iterator<char>(file)), {});
    }();
    if (namedLog.find("] Gridaniaclonea Test: Named log message\n") == std::string::npos)
    {
        std::cerr << "chatlogs did not use the current player name\n";
        return 1;
    }
    snapshot.displayName = "Other/Name";
    if (!playerState.Publish(snapshot))
    {
        std::cerr << "changed player name did not publish\n";
        return 1;
    }
    host.QueueChat("Other Name", "Rotated log message");
    host.Update(0.02);
    const std::string changedLog = [&]()
    {
        std::ifstream file(changedChatLog, std::ios::binary);
        return std::string((std::istreambuf_iterator<char>(file)), {});
    }();
    if (changedLog.find("] Other Name: Rotated log message\n") == std::string::npos)
    {
        std::cerr << "chatlogs did not sanitize and rotate the player name\n";
        return 1;
    }
    snapshot.displayName = "Gridaniaclonea Test";
    if (!playerState.Publish(snapshot))
    {
        std::cerr << "original player name did not republish\n";
        return 1;
    }
    capture.chatLines.clear();
    capture.clipboardText.clear();
    capture.clipboardWrites = 0;
    ResetDrawCapture(capture);
    host.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.windows != 1 || capture.rawTexts != 1 || capture.windowLocked || capture.posText != "X    12.500 | Y     4.250 | Z    -7.750 | R 194 | Zone 230")
    {
        std::cerr << "position state or always-visible pos window failed\n";
        return 1;
    }
    if (!host.DispatchCommand("/pos"))
    {
        std::cerr << "pos command was not handled\n";
        return 1;
    }
    ResetDrawCapture(capture);
    host.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    const std::string expectedPosition =
        "{ 12.500, -7.750, 4.250, 194 }, -- !pos 12.500 4.250 -7.750 230";
    const std::string expectedWindow =
        "X    12.500 | Y     4.250 | Z    -7.750 | R 194 | Zone 230";
    if (capture.windows != 1 || capture.rawTexts != 1 || capture.windowLocked || capture.posText != expectedWindow || capture.chatLines.size() != 1 || capture.chatLines.front() != expectedPosition || capture.clipboardWrites != 1 || capture.clipboardText != expectedPosition)
    {
        std::cerr << "pos command did not print and copy the exact position\n";
        return 1;
    }
    const std::size_t helpChatStart = capture.chatLines.size();
    if (!host.DispatchCommand("/pos help"))
    {
        std::cerr << "pos help command was not handled\n";
        return 1;
    }
    if (capture.chatLines.size() != helpChatStart + 2 || capture.chatLines[helpChatStart] != "/pos - print and copy your current position" || capture.chatLines[helpChatStart + 1] != "/pos lock - toggle position overlay dragging" || capture.clipboardWrites != 1)
    {
        std::cerr << "pos help output or copy behavior failed\n";
        return 1;
    }
    for (const std::string_view invalid : {
             "/pos nope", "/pos window", "/pos WINDOW on", "/pos window on", "/pos window off", "/pos window on extra" })
    {
        if (host.DispatchCommand(invalid))
        {
            std::cerr << "invalid pos command was handled\n";
            return 1;
        }
    }
    const std::size_t posLockChatStart = capture.chatLines.size();
    if (!host.DispatchCommand("/pos lock") || capture.chatLines.size() != posLockChatStart + 1 || capture.chatLines.back() != "Position overlay locked")
    {
        std::cerr << "pos lock command did not persist or report\n";
        return 1;
    }
    capture.openedUrls.clear();
    const std::size_t wikiChatStart = capture.chatLines.size();
    if (!host.DispatchCommand("/wiki") || capture.openedUrls.size() != 1 || capture.openedUrls.back() != "https://bahamut.miraheze.org/wiki/Main_Page" || capture.chatLines.size() != wikiChatStart + 1 || capture.chatLines.back() != "Opening Bahamut wiki")
    {
        std::cerr << "wiki home command did not open and report\n";
        return 1;
    }
    if (!host.DispatchCommand("/wiki aether quest") || capture.openedUrls.size() != 2 || capture.openedUrls.back() != "https://bahamut.miraheze.org/w/index.php?title=Special%3ASearch&search=aether%20quest" || capture.chatLines.back() != "Opening Bahamut wiki search")
    {
        std::cerr << "wiki query command did not encode, open, and report\n";
        return 1;
    }
    const std::size_t wikiHelpStart = capture.chatLines.size();
    if (!host.DispatchCommand("/wiki help") || capture.openedUrls.size() != 2 || capture.chatLines.size() != wikiHelpStart + 1 || capture.chatLines[wikiHelpStart] != "/wiki <query> - search the Bahamut wiki")
    {
        std::cerr << "wiki help command opened a URL or emitted wrong help\n";
        return 1;
    }
    std::string invalidWikiQuery = "/wiki bad-";
    invalidWikiQuery.push_back(static_cast<char>(0xff));
    const std::size_t rejectedWikiChatStart = capture.chatLines.size();
    if (!host.DispatchCommand(invalidWikiQuery) || capture.openedUrls.size() != 2 || capture.chatLines.size() != rejectedWikiChatStart + 1 || capture.chatLines.back() != "Wiki search query was rejected")
    {
        std::cerr << "wiki invalid UTF-8 query was not rejected\n";
        return 1;
    }
    std::string invalidUtf8Url = "https://bahamut.miraheze.org/wiki/";
    invalidUtf8Url.push_back(static_cast<char>(0xff));
    if (!IsAllowedAddonUrl("https://bahamut.miraheze.org/wiki/Main_Page") || IsAllowedAddonUrl("http://bahamut.miraheze.org/wiki/Main_Page") || IsAllowedAddonUrl("https://example.org/wiki/Main_Page") || IsAllowedAddonUrl("https://bahamut.miraheze.org/wiki/%ZZ") || IsAllowedAddonUrl("https://bahamut.miraheze.org/wiki/invalid quote") || IsAllowedAddonUrl(std::string("https://bahamut.miraheze.org/wiki/") + std::string(2048, 'x')) || IsAllowedAddonUrl(invalidUtf8Url))
    {
        std::cerr << "wiki URL boundary accepted an invalid URL\n";
        return 1;
    }
    ResetDrawCapture(capture);
    host.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.windows != 1 || !capture.windowLocked)
    {
        std::cerr << "pos lock state was not submitted to the draw sink\n";
        return 1;
    }
    if (!host.Reload("pos"))
    {
        std::cerr << "pos lock-state reload failed\n";
        return 1;
    }
    ResetDrawCapture(capture);
    host.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.windows != 1 || capture.title != "" || capture.rawTexts != 1 || !capture.windowLocked || capture.posText != expectedWindow)
    {
        std::cerr << "pos lock state did not persist across reload\n";
        return 1;
    }
    if (!std::filesystem::is_regular_file(settings / L"fps" / L"settings.ini"))
    {
        std::cerr << "addon-scoped settings were not persisted\n";
        return 1;
    }
    if (!host.Reload("fps"))
    {
        std::cerr << "FPS reload failed\n";
        return 1;
    }
    ResetDrawCapture(capture);
    host.Update(0.02);
    host.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.windows != 1 || capture.title != "" || capture.rawTexts != 1 || capture.rawAddonId != "fps" || capture.rawText != "0" || capture.rawText.find("FPS") != std::string::npos || capture.rawRed != 1.0F || capture.rawGreen != 0.0F || capture.rawBlue != 0.0F || capture.rawAlpha != 1.0F || capture.posText.empty() || host.FaultedCount() != 1)
    {
        std::cerr << "reload disturbed the isolated fault state\n";
        return 1;
    }
    if (!host.Disable("fps"))
    {
        std::cerr << "FPS disable failed\n";
        return 1;
    }
    ResetDrawCapture(capture);
    host.Update(0.02);
    host.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.windows != 1 || capture.rawTexts != 0 || capture.posText.empty() || host.LoadedCount() != 4 || host.FaultedCount() != 1)
    {
        std::cerr << "disabled addon retained callback activity\n";
        return 1;
    }
    AddonHost areaHost;
    areaHost.LoadManifests({ zonenameManifest }, settings, chatLogs);
    if (areaHost.LoadedCount() != 1)
    {
        std::cerr << "area-name addon did not load\n";
        return 1;
    }
    packet_observer::GameMessage invalidArea;
    invalidArea.direction = packet_observer::Direction::Outgoing;
    invalidArea.opcode    = 0x0005u;
    invalidArea.sourceId  = 11u;
    invalidArea.payload.resize(16u);
    areaHost.QueueAreaTransition(invalidArea);
    areaHost.Update(0.1);
    ResetDrawCapture(capture);
    areaHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 0)
    {
        std::cerr << "outgoing SetMap triggered area popup\n";
        return 1;
    }
    const auto queueArea = [&](std::uint32_t zoneId)
    {
        packet_observer::GameMessage message;
        message.direction = packet_observer::Direction::Incoming;
        message.opcode    = 0x0005u;
        message.sourceId  = 11u;
        message.payload.resize(16u);
        for (std::size_t index = 0; index < 4u; ++index)
        {
            message.payload[4u + index] = static_cast<std::uint8_t>(zoneId >> (index * 8u));
        }
        areaHost.QueueAreaTransition(message);
    };
    queueArea(230u);
    ResetDrawCapture(capture);
    areaHost.Update(8.0);
    areaHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 1 || capture.rawAddonId != "zonename" ||
        capture.rawText != "LA NOSCEA\nLimsa Lominsa" || capture.rawAlpha != 0.0F ||
        capture.rawLocked)
    {
        std::cerr << "incoming SetMap did not show area\n";
        return 1;
    }
    ResetDrawCapture(capture);
    areaHost.Update(0.2);
    areaHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 1 || capture.rawAlpha < 0.49F || capture.rawAlpha > 0.51F)
    {
        std::cerr << "area popup did not fade in\n";
        return 1;
    }
    ResetDrawCapture(capture);
    areaHost.Update(5.1);
    areaHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 1 || capture.rawAlpha < 0.49F || capture.rawAlpha > 0.51F)
    {
        std::cerr << "area popup did not fade out\n";
        return 1;
    }
    ResetDrawCapture(capture);
    areaHost.Update(0.7);
    areaHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 0)
    {
        std::cerr << "area popup did not expire\n";
        return 1;
    }
    queueArea(230u);
    areaHost.Update(0.1);
    areaHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 0)
    {
        std::cerr << "unchanged zone retriggered area popup\n";
        return 1;
    }
    queueArea(133u);
    areaHost.Update(0.1);
    areaHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 0)
    {
        std::cerr << "same region and map name retriggered area popup\n";
        return 1;
    }
    queueArea(130u);
    ResetDrawCapture(capture);
    areaHost.Update(0.1);
    areaHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 1 ||
        capture.rawText != "LA NOSCEA\nEastern La Noscea")
    {
        std::cerr << "observed SetMap zone did not show its area\n";
        return 1;
    }
    queueArea(231u);
    ResetDrawCapture(capture);
    areaHost.Update(0.1);
    areaHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 1 || capture.rawText != "COERTHAS\nDzemael Darkhold")
    {
        std::cerr << "SetMap zone change did not refresh popup\n";
        return 1;
    }
    queueArea(999u);
    ResetDrawCapture(capture);
    areaHost.Update(0.1);
    areaHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 0)
    {
        std::cerr << "unknown area retained popup\n";
        return 1;
    }
    queueArea(231u);
    areaHost.Update(0.1);
    areaHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 1 || capture.rawText != "COERTHAS\nDzemael Darkhold")
    {
        std::cerr << "known area did not restore popup\n";
        return 1;
    }
    queueArea(999u);
    ResetDrawCapture(capture);
    areaHost.Update(0.1);
    areaHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 0 || !areaHost.Disable("zonename"))
    {
        std::cerr << "unknown area or disable retained popup\n";
        return 1;
    }
    bahamut_client::PlayerStateService  combatPlayer;
    bahamut_client::PlayerStateSnapshot combatPlayerSnapshot;
    combatPlayerSnapshot.actorId     = 11u;
    combatPlayerSnapshot.zoneId      = 130u;
    combatPlayerSnapshot.displayName = "Sample Player";
    combatPlayerSnapshot.baseClassId = 8u;
    combatPlayerSnapshot.jobId       = 27u;
    AddonHost combatHost(&combatPlayer);
    combatHost.SetChatSink({ &capture, &CaptureChat });
    combatHost.LoadManifests({}, settings, chatLogs);
    packet_observer::GameMessage combatMessage;
    combatMessage.direction = packet_observer::Direction::Incoming;
    combatMessage.opcode    = 0x0139u;
    combatMessage.sourceId  = 11u;
    combatMessage.payload.resize(0x38u);
    const auto writeCombatU16 = [&](std::size_t offset, std::uint16_t value)
    {
        combatMessage.payload[offset]      = static_cast<std::uint8_t>(value);
        combatMessage.payload[offset + 1u] = static_cast<std::uint8_t>(value >> 8u);
    };
    const auto writeCombatU32 = [&](std::size_t offset, std::uint32_t value)
    {
        for (std::size_t index = 0; index < 4u; ++index)
        {
            combatMessage.payload[offset + index] = static_cast<std::uint8_t>(value >> (index * 8u));
        }
    };
    writeCombatU32(0, 11u);
    writeCombatU32(0x20u, 1u);
    writeCombatU32(0x28u, 0x44000BBBu);
    writeCombatU16(0x2Cu, 170u);
    writeCombatU16(0x2Eu, 30301u);
    combatHost.QueueCombatResult(combatMessage);
    if (!combatPlayer.Publish(combatPlayerSnapshot) || !combatHost.Enable(combatParserManifest))
    {
        std::cerr << "combat parser did not load with player state\n";
        return 1;
    }
    ResetDrawCapture(capture);
    combatHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, &CaptureCombatMeter });
    if (capture.windows != 0 || capture.combatMeters != 1 || capture.combatAddonId != "combatparser" ||
        capture.combatMode != "dps" || !capture.combatRows.empty() || capture.combatLocked ||
        capture.combatIncomplete)
    {
        std::cerr << "combat parser did not start with a headerless meter\n";
        return 1;
    }
    capture.chatLines.clear();
    constexpr std::array<std::string_view, 3> combatHelp = {
        "/combatparser mode", "/combatparser reset", "/combatparser lock"
    };
    if (!combatHost.DispatchCommand("/combatparser help") || capture.chatLines.size() != combatHelp.size() ||
        std::any_of(capture.chatLines.begin(), capture.chatLines.end(), [](const std::string& line)
                    {
                        return line.find("/help") != std::string::npos;
                    }) ||
        !std::all_of(combatHelp.begin(), combatHelp.end(), [&](std::string_view line)
                     {
                         return std::find(capture.chatLines.begin(), capture.chatLines.end(), line) != capture.chatLines.end();
                     }))
    {
        std::cerr << "combat parser help omitted a supported command or included /help\n";
        return 1;
    }
    capture.chatLines.clear();
    if (!combatHost.DispatchCommand("/combatparser") || capture.chatLines.size() != combatHelp.size())
    {
        std::cerr << "combat parser base command did not show usage\n";
        return 1;
    }
    combatHost.QueueCombatResult(combatMessage);
    writeCombatU16(0x2Eu, 0u);
    combatHost.QueueCombatResult(combatMessage);
    writeCombatU16(0x2Eu, 30301u);
    writeCombatU32(0x28u, 11u);
    combatHost.QueueCombatResult(combatMessage);
    writeCombatU32(0x28u, 0x44000BBBu);
    combatMessage.sourceId = 12u;
    combatHost.QueueCombatResult(combatMessage);
    combatMessage.sourceId = 11u;
    writeCombatU16(0x2Cu, 30u);
    writeCombatU16(0x2Eu, 30302u);
    combatHost.QueueCombatResult(combatMessage);
    combatHost.Update(0.1);
    combatHost.Update(1.0);
    ResetDrawCapture(capture);
    combatHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, &CaptureCombatMeter });
    if (capture.combatMeters != 1 || capture.combatMode != "dps" || capture.combatRows.size() != 1 ||
        capture.combatRows[0].name != "Sample Player" || capture.combatRows[0].amount != 200u ||
        capture.combatRows[0].rate != 200.0 || capture.combatRows[0].accuracy != "100.0%" ||
        capture.combatRows[0].color != "#F2F2F2" || capture.combatIncomplete ||
        !combatHost.DispatchCommand("/combatparser lock"))
    {
        std::cerr << "combat parser DPS row, local-job color, or lock state failed; rows="
                  << capture.combatRows.size() << " faults=" << combatHost.FaultedCount() << "\n";
        return 1;
    }

    if (!combatHost.DispatchCommand("/combatparser mode"))
    {
        std::cerr << "combat parser rejected HPS mode\n";
        return 1;
    }
    writeCombatU16(0x24u, 23001u);
    writeCombatU16(0x2Cu, 0u);
    writeCombatU16(0x2Eu, 30311u);
    combatHost.QueueCombatResult(combatMessage);
    writeCombatU16(0x24u, 27346u);
    writeCombatU32(0x28u, 11u);
    writeCombatU16(0x2Cu, 50u);
    writeCombatU16(0x2Eu, 30301u);
    combatHost.QueueCombatResult(combatMessage);
    writeCombatU16(0x24u, 27347u);
    writeCombatU16(0x2Cu, 10u);
    writeCombatU16(0x2Eu, 0u);
    combatHost.QueueCombatResult(combatMessage);
    writeCombatU16(0x24u, 29001u);
    writeCombatU32(0x28u, 0x44000BBBu);
    writeCombatU16(0x2Cu, 20u);
    combatHost.QueueCombatResult(combatMessage);
    combatMessage.sourceId = 0x44000BBBu;
    writeCombatU32(0, combatMessage.sourceId);
    writeCombatU32(0x28u, 11u);
    writeCombatU16(0x24u, 23001u);
    writeCombatU16(0x2Cu, 25u);
    writeCombatU16(0x2Eu, 30301u);
    combatHost.QueueCombatResult(combatMessage);
    combatHost.Update(0.1);
    combatHost.Update(15.0);
    ResetDrawCapture(capture);
    combatHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, &CaptureCombatMeter });
    if (capture.combatMode != "hps" || capture.combatRows.size() != 1 ||
        capture.combatRows[0].amount != 60u || capture.combatRows[0].rate <= 0.0 ||
        capture.combatRows[0].name != "Sample Player")
    {
        std::cerr << "combat parser HPS selection or healing accumulation failed\n";
        return 1;
    }
    if (!combatHost.DispatchCommand("/combatparser mode"))
    {
        std::cerr << "combat parser rejected DPS mode\n";
        return 1;
    }
    combatMessage.sourceId = 11u;
    writeCombatU32(0, 11u);
    writeCombatU16(0x24u, 23001u);
    writeCombatU32(0x28u, 0x44000BBBu);
    combatMessage.opcode = 0x013Au;
    combatMessage.payload.resize(0xB8u);
    writeCombatU32(0x20u, 2u);
    writeCombatU32(0x2Cu, 11u);
    writeCombatU16(0x50u, 40u);
    writeCombatU16(0x52u, 90u);
    writeCombatU16(0x64u, 30301u);
    writeCombatU16(0x66u, 30301u);
    combatHost.QueueCombatResult(combatMessage);
    combatHost.Update(0.1);
    ResetDrawCapture(capture);
    combatHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, &CaptureCombatMeter });
    combatHost.QueueCombatResult(combatMessage);
    if (!capture.combatLocked || capture.combatMode != "dps" || capture.combatRows.size() != 1 ||
        capture.combatRows[0].amount != 260u ||
        !combatHost.DispatchCommand("/combatparser reset"))
    {
        std::cerr << "combat parser multi-target filter, lock, or reset failed\n";
        return 1;
    }
    if (capture.chatLines.empty() || capture.chatLines.back() != "Combat Parser reset")
    {
        std::cerr << "combat parser reset command did not complete\n";
        return 1;
    }
    combatMessage.opcode = 0x0139u;
    combatMessage.payload.resize(0x38u);
    writeCombatU32(0x20u, 1u);
    writeCombatU16(0x2Cu, 10u);
    writeCombatU16(0x2Eu, 30301u);
    combatHost.QueueCombatResult(combatMessage);
    combatMessage.sourceId = 12u;
    writeCombatU32(0, 12u);
    writeCombatU16(0x2Cu, 15u);
    combatHost.QueueCombatResult(combatMessage);
    combatHost.Update(0.1);
    ResetDrawCapture(capture);
    combatHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, &CaptureCombatMeter });
    if (capture.combatRows.size() != 2 || capture.combatRows[0].name != "Actor 0000000C" ||
        capture.combatRows[0].amount != 15u ||
        capture.combatRows[0].color != "#A0A0A0" ||
        capture.combatRows[1].name != "Sample Player" || capture.combatRows[1].amount != 10u ||
        capture.combatRows[1].color != "#F2F2F2" ||
        capture.combatRows[0].rate < capture.combatRows[1].rate)
    {
        std::cerr << "combat parser rank order, unknown-player color, or local-job color failed\n";
        return 1;
    }

    const double resultRate = capture.combatRows[0].rate;
    combatHost.Update(5.0);
    ResetDrawCapture(capture);
    combatHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, &CaptureCombatMeter });
    if (capture.combatRows.size() != 2 || capture.combatRows[0].rate != resultRate)
    {
        std::cerr << "combat parser rate changed without a new result\n";
        return 1;
    }
    combatHost.QueueCombatResult(combatMessage);
    combatHost.Update(0.1);
    ResetDrawCapture(capture);
    combatHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, &CaptureCombatMeter });
    if (capture.combatRows.size() != 2 || capture.combatRows[0].amount != 30u ||
        capture.combatRows[0].rate < 5.8 || capture.combatRows[0].rate > 5.9)
    {
        std::cerr << "combat parser did not count a short gap between results\n";
        return 1;
    }
    const double shortGapRate = capture.combatRows[0].rate;
    combatHost.Update(60.0);
    combatPlayerSnapshot.zoneId = 131u;
    if (!combatPlayer.Publish(combatPlayerSnapshot))
    {
        std::cerr << "combat parser zone-change player state failed\n";
        return 1;
    }
    combatHost.Update(0.1);
    combatPlayer.Clear();
    combatHost.Update(0.1);
    ResetDrawCapture(capture);
    combatHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, &CaptureCombatMeter });
    if (capture.combatRows.size() != 2 || capture.combatRows[0].name != "Actor 0000000C" ||
        capture.combatRows[0].rate != shortGapRate)
    {
        std::cerr << "combat parser rate declined during idle time or lost its rows\n";
        return 1;
    }
    if (!combatPlayer.Publish(combatPlayerSnapshot))
    {
        std::cerr << "combat parser player state did not recover after a missing snapshot\n";
        return 1;
    }
    combatHost.QueueCombatResult(combatMessage);
    combatHost.Update(0.1);
    ResetDrawCapture(capture);
    combatHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, &CaptureCombatMeter });
    if (capture.combatRows.size() != 2 || capture.combatRows[0].amount != 45u ||
        capture.combatRows[0].rate <= shortGapRate ||
        !combatHost.DispatchCommand("/combatparser reset"))
    {
        std::cerr << "combat parser counted a long idle gap or lost retained totals; rows="
                  << capture.combatRows.size() << " amount="
                  << (capture.combatRows.empty() ? 0u : capture.combatRows[0].amount)
                  << " rate="
                  << (capture.combatRows.empty() ? 0.0 : capture.combatRows[0].rate)
                  << " previous=" << shortGapRate << '\n';
        return 1;
    }

    for (std::size_t index = 0; index < 257u; ++index)
    {
        combatHost.QueueCombatResult(combatMessage);
    }
    combatHost.Update(0.1);
    ResetDrawCapture(capture);
    combatHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, &CaptureCombatMeter });
    if (capture.combatRows.size() != 1 || capture.combatRows[0].amount != 3840u ||
        !capture.combatIncomplete)
    {
        std::cerr << "combat parser queue overflow was not bounded and reported\n";
        return 1;
    }
    if (!combatHost.DispatchCommand("/combatparser mode"))
    {
        std::cerr << "combat parser rejected HPS mode with an incomplete encounter\n";
        return 1;
    }
    ResetDrawCapture(capture);
    combatHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, &CaptureCombatMeter });
    if (capture.combatMode != "hps" || !capture.combatRows.empty() || !capture.combatIncomplete)
    {
        std::cerr << "combat parser lost incomplete status when the selected mode has no rows\n";
        return 1;
    }
    if (!combatHost.DispatchCommand("/combatparser reset"))
    {
        std::cerr << "combat parser reset command did not complete after queue overflow\n";
        return 1;
    }
    ResetDrawCapture(capture);
    combatHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, &CaptureCombatMeter });
    if (capture.combatMeters != 1 || !capture.combatRows.empty() || capture.combatIncomplete ||
        !combatHost.Disable("combatparser"))
    {
        std::cerr << "combat parser reset did not return to the empty meter\n";
        return 1;
    }

    const std::filesystem::path meterApiRoot = settings / "meter-api-test";
    std::filesystem::create_directories(meterApiRoot);
    std::filesystem::copy_file(combatParserManifest, meterApiRoot / "addon.toml", std::filesystem::copy_options::overwrite_existing);
    {
        std::ofstream script(meterApiRoot / "combatparser.lua", std::ios::trunc);
        script << R"lua(
local row = { name = "Valid", amount = 0, rate = 0, accuracy = "--", color = "#A0A0A0" }
local invalid_rows = {}
for index = 1, 65 do invalid_rows[index] = row end
function draw()
    if bahamut.combat_meter("invalid", {}, false, false) ~= false then error("invalid mode was accepted") end
    if bahamut.combat_meter("dps", {{name="Bad", amount=-1, rate=0, accuracy="--", color="#FFFFFF"}}, false, false) ~= false then error("negative amount was accepted") end
    if bahamut.combat_meter("dps", {{name="Bad", amount=0, rate=0/0, accuracy="--", color="#FFFFFF"}}, false, false) ~= false then error("NaN rate was accepted") end
    if bahamut.combat_meter("dps", {{name="Bad", amount=0, rate=0, accuracy="--", color="white"}}, false, false) ~= false then error("invalid color was accepted") end
    if bahamut.combat_meter("dps", invalid_rows, false, false) ~= false then error("oversized meter was accepted") end
    if bahamut.combat_meter("dps", {row}, false, false) ~= true then error("valid meter was rejected") end
end
)lua";
    }
    AddonHost meterApiHost;
    meterApiHost.LoadManifests({}, settings / "meter-api-settings", chatLogs);
    if (!meterApiHost.Enable(meterApiRoot / "addon.toml"))
    {
        std::cerr << "combat meter API validation addon did not load\n";
        return 1;
    }
    ResetDrawCapture(capture);
    meterApiHost.Draw({ &capture, &CaptureWindow, &CaptureRawText, &CaptureCombatMeter });
    if (capture.combatMeters != 1 || capture.combatMode != "dps" || capture.combatRows.size() != 1 ||
        capture.combatRows[0].name != "Valid" || meterApiHost.FaultedCount() != 0)
    {
        std::cerr << "combat meter API rejected a valid row or emitted malformed rows\n";
        return 1;
    }
    bahamut_client::PlayerStateService  targetPlayer;
    bahamut_client::PlayerStateSnapshot targetPlayerSnapshot;
    targetPlayerSnapshot.actorId = 11u;
    targetPlayerSnapshot.zoneId  = 130u;
    bahamut_client::TargetDistanceService targetService(&targetPlayer, nullptr);
    AddonHost                             targetHost(&targetPlayer);
    targetHost.SetTargetDistanceService(&targetService);
    targetHost.LoadManifests({ lockManifests[0], lockManifests[1] }, settings, chatLogs);
    if (!targetPlayer.Publish(targetPlayerSnapshot) || targetHost.LoadedCount() != 2)
    {
        std::cerr << "target addons did not load with player state\n";
        return 1;
    }
    packet_observer::GameMessage targetMessage;
    targetMessage.direction = packet_observer::Direction::Incoming;
    targetMessage.sourceId  = 11u;
    targetMessage.opcode    = 0x00DBu;
    targetMessage.payload   = { 12u, 0u, 0u, 0u };
    targetService.Observe(targetMessage);
    targetMessage.sourceId = 12u;
    targetMessage.opcode   = 0x00CFu;
    targetMessage.payload.assign(20u, 0u);
    const float targetX = 3.0F;
    const float targetZ = 4.0F;
    std::memcpy(targetMessage.payload.data() + 8u, &targetX, sizeof(targetX));
    std::memcpy(targetMessage.payload.data() + 16u, &targetZ, sizeof(targetZ));
    targetService.Observe(targetMessage);
    targetMessage.opcode    = 0x0137u;
    targetMessage.payload   = { 0u };
    const auto appendHealth = [&](std::uint32_t propertyId)
    {
        targetMessage.payload.push_back(2u);
        for (std::size_t index = 0; index < 4u; ++index)
        {
            targetMessage.payload.push_back(static_cast<std::uint8_t>(propertyId >> (index * 8u)));
        }
        targetMessage.payload.push_back(27u);
        targetMessage.payload.push_back(0u);
    };
    appendHealth(0x4232BCAAu);
    appendHealth(0x7BCDFB69u);
    targetMessage.payload.push_back(0x88u);
    for (const char character : std::string_view("/_init"))
    {
        targetMessage.payload.push_back(static_cast<std::uint8_t>(character));
    }
    targetMessage.payload[0] = static_cast<std::uint8_t>(targetMessage.payload.size() - 1u);
    targetService.Observe(targetMessage);
    ResetDrawCapture(capture);
    targetHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 2 || capture.rawAddonId != "targethp" ||
        capture.rawText != "HP 27/27 (100%)" ||
        capture.rawRed != 1.0F || capture.rawGreen != 1.0F || capture.rawBlue != 1.0F ||
        capture.rawSize != 13.0F || capture.windows != 0 || capture.rawLocked)
    {
        std::cerr << "target HP did not show unboxed target values\n";
        return 1;
    }
    targetMessage.sourceId = 11u;
    targetService.Observe(targetMessage);
    targetMessage.opcode = 0x00CEu;
    targetMessage.payload.assign(40u, 0u);
    targetMessage.payload[4] = 11u;
    targetService.Observe(targetMessage);
    targetPlayerSnapshot.x = 130.0F;
    if (!targetPlayer.Publish(targetPlayerSnapshot))
    {
        std::cerr << "moved player state for target addons was rejected\n";
        return 1;
    }
    targetMessage.opcode  = 0x00DBu;
    targetMessage.payload = { 11u, 0u, 0u, 0u };
    targetService.Observe(targetMessage);
    ResetDrawCapture(capture);
    targetHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 1 || capture.rawAddonId != "targethp" ||
        capture.rawText != "HP 27/27 (100%)")
    {
        std::cerr << "self-target did not hide distance while retaining target HP\n";
        return 1;
    }
    targetPlayerSnapshot.x = 0.0F;
    if (!targetPlayer.Publish(targetPlayerSnapshot))
    {
        std::cerr << "restored player state for target addons was rejected\n";
        return 1;
    }
    targetMessage.payload = { 12u, 0u, 0u, 0u };
    targetService.Observe(targetMessage);
    if (!targetHost.DispatchCommand("/distance color #00FF00") ||
        !targetHost.DispatchCommand("/distance size 20") ||
        !targetHost.DispatchCommand("/targethp color #0000FF") ||
        !targetHost.DispatchCommand("/targethp size 22") ||
        !targetHost.Reload("distance") || !targetHost.Reload("targethp"))
    {
        std::cerr << "target text style commands did not persist\n";
        return 1;
    }
    ResetDrawCapture(capture);
    targetHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawAddonId != "targethp" || capture.rawRed != 0.0F ||
        capture.rawGreen != 0.0F || capture.rawBlue != 1.0F ||
        capture.rawSize != 22.0F || capture.windows != 0)
    {
        std::cerr << "target HP color or size did not survive reload\n";
        return 1;
    }
    if (!targetHost.Disable("targethp"))
    {
        std::cerr << "target HP did not disable\n";
        return 1;
    }
    ResetDrawCapture(capture);
    targetHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 1 || capture.rawAddonId != "distance" ||
        capture.rawText != "5.0" || capture.rawRed != 0.0F ||
        capture.rawGreen != 1.0F || capture.rawBlue != 0.0F ||
        capture.rawSize != 20.0F || capture.windows != 0)
    {
        std::cerr << "distance text changed with target HP presentation\n";
        return 1;
    }
    for (std::size_t index = 0; index < lockManifests.size(); ++index)
    {
        const std::string id(lockIds[index]);
        const std::string lockCommand = "/" + id + " lock";
        AddonHost         lockHost(&playerState);
        lockHost.SetChatSink({ &capture, &CaptureChat });
        lockHost.LoadManifests({ lockManifests[index] }, settings, chatLogs);
        if (lockHost.LoadedCount() != 1 || lockHost.DispatchCommand("/" + id))
        {
            std::cerr << id << " did not load or accepted an unsupported command\n";
            return 1;
        }
        if (id == "distance" || id == "targethp")
        {
            const std::array<std::string_view, 3> help = id == "distance"
                                                             ? std::array<std::string_view, 3>{ "/distance lock", "/distance color #RRGGBB", "/distance size 8-48" }
                                                             : std::array<std::string_view, 3>{ "/targethp lock", "/targethp color #RRGGBB", "/targethp size 8-48" };
            capture.chatLines.clear();
            if (!lockHost.DispatchCommand("/" + id + " help") ||
                capture.chatLines.size() != help.size() ||
                std::any_of(capture.chatLines.begin(), capture.chatLines.end(), [](const std::string& line)
                            {
                                return line.find("/help") != std::string::npos;
                            }) ||
                !std::all_of(help.begin(), help.end(), [&](std::string_view line)
                             {
                                 return std::find(capture.chatLines.begin(), capture.chatLines.end(), line) != capture.chatLines.end();
                             }))
            {
                std::cerr << id << " help omitted a supported command or included /help\n";
                return 1;
            }
        }
        else if (lockHost.DispatchCommand("/" + id + " help"))
        {
            std::cerr << id << " accepted an unsupported help command\n";
            return 1;
        }
        capture.chatLines.clear();
        if (!lockHost.DispatchCommand(lockCommand) || capture.chatLines.size() != 1 ||
            capture.chatLines.front().find("overlay locked") == std::string::npos)
        {
            std::cerr << id << " lock command did not report the locked state\n";
            return 1;
        }
        const auto        lockSettings = settings / id / "settings.ini";
        std::ifstream     settingsFile(lockSettings);
        const std::string saved((std::istreambuf_iterator<char>(settingsFile)), {});
        if (saved.find("locked=true") == std::string::npos || !lockHost.Reload(id))
        {
            std::cerr << id << " lock setting did not persist across reload\n";
            return 1;
        }
        if (id == "distance" || id == "targethp")
        {
            ResetDrawCapture(capture);
            lockHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
            if (capture.windows != 0 || capture.rawTexts != 0)
            {
                std::cerr << id << " showed a window without a target\n";
                return 1;
            }
        }
        capture.chatLines.clear();
        if (!lockHost.DispatchCommand(lockCommand) || capture.chatLines.size() != 1 ||
            capture.chatLines.front().find("overlay unlocked") == std::string::npos)
        {
            std::cerr << id << " did not reload its locked state\n";
            return 1;
        }
    }
    AddonHost styleHost(&playerState);
    styleHost.LoadManifests({ fpsManifest }, settings / L"style", chatLogs);
    if (styleHost.LoadedCount() != 1 ||
        !styleHost.DispatchCommand("/fps color #00FFFF") ||
        !styleHost.DispatchCommand("/fps size 24") ||
        !styleHost.DispatchCommand("/fps size 100") ||
        !styleHost.Reload("fps"))
    {
        std::cerr << "FPS text style commands did not persist\n";
        return 1;
    }
    ResetDrawCapture(capture);
    styleHost.Draw({ &capture, &CaptureWindow, &CaptureRawText });
    if (capture.rawTexts != 1 || capture.rawAddonId != "fps" ||
        capture.rawRed != 0.0F || capture.rawGreen != 1.0F ||
        capture.rawBlue != 1.0F || capture.rawSize != 24.0F)
    {
        std::cerr << "FPS color or size did not survive reload\n";
        return 1;
    }
    AddonHost   packetOnlyHost;
    DrawCapture packetOnlyCapture;
    packetOnlyHost.SetChatSink({ &packetOnlyCapture, &CaptureChat });
    if (packetOnlyHost.HasCommandAddons())
    {
        std::cerr << "empty addon selection required command hooks\n";
        return 1;
    }
    packetOnlyHost.LoadManifests({ packetloggerManifest }, settings / L"packet-only", chatLogs);
    if (packetOnlyHost.LoadedCount() != 1 || !packetOnlyHost.HasCommandAddons() ||
        !packetOnlyHost.DispatchCommand("/packetlogger start") ||
        !packetOnlyHost.DispatchCommand("/packetlogger status") ||
        !packetOnlyHost.DispatchCommand("/packetlogger stop") ||
        packetOnlyCapture.chatLines.size() != 3 ||
        !packetOnlyHost.Disable("packetlogger") || packetOnlyHost.HasCommandAddons())
    {
        std::cerr << "Packetlogger-only selection did not activate its commands\n";
        return 1;
    }
    std::cout << "addon-host-tests: PASS\n";
    return 0;
}
