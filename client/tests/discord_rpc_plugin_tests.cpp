#include "../src/discord_rpc_plugin.cpp"

#include <chrono>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <string>
#include <thread>

namespace
{

void Require(bool condition, const char* message)
{
    if (!condition)
    {
        std::cerr << message << '\n';
        std::exit(1);
    }
}

void StalledPayloadHonorsDeadline()
{
    const std::wstring pipeName = L"\\\\.\\pipe\\bahamut-discord-test-" + std::to_wstring(GetCurrentProcessId());
    HANDLE             server   = CreateNamedPipeW(pipeName.c_str(), PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, nullptr);
    Require(server != INVALID_HANDLE_VALUE, "could not create test pipe");

    HANDLE releaseServer = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    HANDLE headerWritten = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    Require(releaseServer != nullptr && headerWritten != nullptr,
            "could not create test events");

    std::thread peer([&]()
                     {
                         const BOOL connected = ConnectNamedPipe(server, nullptr);
                         Require(connected != FALSE || GetLastError() == ERROR_PIPE_CONNECTED,
                                 "test pipe connection failed");
                         const std::array<std::uint32_t, 2> header{ 1, 4 };
                         DWORD                              written = 0;
                         Require(WriteFile(server, header.data(), sizeof(header), &written, nullptr) != FALSE && written == sizeof(header),
                                 "test pipe header write failed");
                         SetEvent(headerWritten);
                         WaitForSingleObject(releaseServer, INFINITE);
                     });

    HANDLE client = CreateFileW(pipeName.c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, nullptr);
    Require(client != INVALID_HANDLE_VALUE, "could not open test pipe");
    Require(WaitForSingleObject(headerWritten, 1000) == WAIT_OBJECT_0,
            "test pipe header was not written");

    const auto startedAt = std::chrono::steady_clock::now();
    const bool read      = ReadFrame(client, StartIpcDeadline());
    const auto elapsed   = std::chrono::steady_clock::now() - startedAt;

    SetEvent(releaseServer);
    CloseHandle(client);
    peer.join();
    CloseHandle(headerWritten);
    CloseHandle(releaseServer);
    CloseHandle(server);

    Require(!read, "stalled payload unexpectedly completed");
    Require(elapsed < std::chrono::milliseconds(1500),
            "stalled payload exceeded the IPC deadline allowance");
}

void StalledIdlePollDoesNotBlockStatePublication()
{
    const std::wstring pipeName = L"\\\\.\\pipe\\bahamut-discord-idle-test-" + std::to_wstring(GetCurrentProcessId());
    HANDLE             server   = CreateNamedPipeW(pipeName.c_str(), PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, nullptr);
    Require(server != INVALID_HANDLE_VALUE, "could not create idle test pipe");
    HANDLE client = CreateFileW(pipeName.c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, nullptr);
    Require(client != INVALID_HANDLE_VALUE, "could not open idle test pipe");
    Require(ConnectNamedPipe(server, nullptr) != FALSE || GetLastError() == ERROR_PIPE_CONNECTED,
            "idle test pipe connection failed");
    const std::array<std::uint32_t, 2> header{ 1, 4 };
    DWORD                              written = 0;
    Require(WriteFile(server, header.data(), sizeof(header), &written, nullptr) != FALSE && written == sizeof(header),
            "idle test could not send a partial frame");

    DiscordRpcPlugin plugin;
    plugin.pipe        = client;
    plugin.hasPresence = true;
    plugin.dirty       = true;
    StartWorker(plugin);
    const auto waitUntil = std::chrono::steady_clock::now() + std::chrono::seconds(1);
    DWORD      available = sizeof(header);
    while (available != 0 && std::chrono::steady_clock::now() < waitUntil)
    {
        Require(PeekNamedPipe(client, nullptr, 0, nullptr, &available, nullptr) != FALSE,
                "idle test pipe closed before the worker consumed its header");
        std::this_thread::yield();
    }
    Require(available == 0, "worker did not begin reading the stalled payload");

    BahamutNativePluginPlayerStateV1 state{};
    state.structSize     = sizeof(state);
    const auto startedAt = std::chrono::steady_clock::now();
    const BOOL published = SetDiscordPlayerState(&plugin, &state);
    const auto elapsed   = std::chrono::steady_clock::now() - startedAt;
    StopWorker(plugin);
    CloseHandle(server);
    Require(published != FALSE, "state publication failed during idle polling");
    Require(elapsed < std::chrono::milliseconds(kIpcTimeoutMilliseconds / 2),
            "idle pipe polling blocked render-facing state publication");
}

void IdlePollSharesOneDeadlineAcrossFrames()
{
    const std::wstring pipeName = L"\\\\.\\pipe\\bahamut-discord-drain-test-" + std::to_wstring(GetCurrentProcessId());
    HANDLE             server   = CreateNamedPipeW(pipeName.c_str(), PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, nullptr);
    Require(server != INVALID_HANDLE_VALUE, "could not create drain test pipe");
    HANDLE client = CreateFileW(pipeName.c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, nullptr);
    Require(client != INVALID_HANDLE_VALUE, "could not open drain test pipe");
    Require(ConnectNamedPipe(server, nullptr) != FALSE || GetLastError() == ERROR_PIPE_CONNECTED,
            "drain test pipe connection failed");
    const std::array<std::uint32_t, 2> header{ 4, 4 };
    DWORD                              written = 0;
    Require(WriteFile(server, header.data(), sizeof(header), &written, nullptr) != FALSE && written == sizeof(header),
            "drain test could not write its first header");

    std::thread      peer([&]()
                          {
                         const auto delay = std::chrono::milliseconds(kIpcTimeoutMilliseconds * 3 / 5);
                         std::this_thread::sleep_for(delay);
                         // Complete the first frame and queue the next header without an idle gap.
                         const std::array<unsigned char, 12> next{ 'p', 'o', 'n', 'g', 4, 0, 0, 0, 4, 0, 0, 0 };
                         DWORD                               bytes = 0;
                         Require(WriteFile(server, next.data(), sizeof(next), &bytes, nullptr) != FALSE && bytes == sizeof(next),
                                 "drain test could not write its next frame");
                         std::this_thread::sleep_for(delay);
                         // The shared deadline closes the client before this second payload arrives.
                         (void)WriteFile(server, next.data(), 4, &bytes, nullptr);
                          });
    DiscordRpcPlugin plugin;
    plugin.pipe        = client;
    plugin.hasPresence = true;
    const bool alive   = PipeIsAlive(plugin);
    peer.join();
    ClosePipe(plugin);
    CloseHandle(server);
    Require(!alive && !plugin.hasPresence, "idle polling renewed the deadline for each frame");
}

void PresenceUsesVerifiedJobMapping()
{
    DiscordRpcPlugin plugin;
    plugin.startedAt = 123;
    BahamutNativePluginPlayerStateV1 state{};
    state.structSize  = sizeof(state);
    state.flags       = BahamutNativePluginPlayerStateHasCharacter |
                        BahamutNativePluginPlayerStateHasArea |
                        BahamutNativePluginPlayerStateHasClassJob;
    state.jobId       = 19;
    state.baseClassId = 8;
    state.level       = 50;
    std::memcpy(state.displayName, "Name\\\"Test", sizeof("Name\\\"Test"));
    std::memcpy(state.areaName, "Ul'dah", sizeof("Ul'dah"));
    const std::string activity = BuildActivity(plugin, state);
    Require(activity.find("\"name\":\"BahamutXIV\"") != std::string::npos,
            "presence should include the application name");
    Require(activity.find("\"details\":\"Name\\\\\\\"Test\"") != std::string::npos,
            "presence should escape character names");
    Require(activity.find("\"state\":\"Ul'dah\"") != std::string::npos,
            "presence should include the copied area");
    Require(activity.find("\"small_image\":\"job_drg\"") != std::string::npos,
            "job should take precedence over the base class");
    Require(activity.find("DRG - Level 50") != std::string::npos,
            "presence should include the job and level tooltip");
}

void PresenceOmitsUnknownPartialState()
{
    DiscordRpcPlugin plugin;
    plugin.startedAt = 456;
    BahamutNativePluginPlayerStateV1 state{};
    state.structSize  = sizeof(state);
    state.flags       = BahamutNativePluginPlayerStateHasCharacter |
                        BahamutNativePluginPlayerStateHasClassJob;
    state.baseClassId = 999;
    state.level       = 0;
    std::memcpy(state.displayName, "Player", sizeof("Player"));
    const std::string activity = BuildActivity(plugin, state);
    Require(activity.find("\"details\":\"Player\"") != std::string::npos,
            "partial state should retain the character name");
    Require(activity.find("assets") == std::string::npos,
            "unknown class mapping should omit stale artwork");
    Require(activity.find("\"state\"") == std::string::npos,
            "unavailable area should be omitted");
    state.baseClassId              = 3;
    const std::string missingLevel = BuildActivity(plugin, state);
    Require(missingLevel.find("assets") == std::string::npos,
            "known class without a level should not display Level 0");
}

void PresenceUsesClassAssetKeys()
{
    DiscordRpcPlugin plugin;
    plugin.startedAt = 789;
    BahamutNativePluginPlayerStateV1 state{};
    state.structSize           = sizeof(state);
    state.flags                = BahamutNativePluginPlayerStateHasClassJob;
    state.baseClassId          = 3;
    state.level                = 12;
    const std::string activity = BuildActivity(plugin, state);
    Require(activity.find("\"small_image\":\"class_gld\"") != std::string::npos,
            "class should use the confirmed GLD artwork key");
    Require(activity.find("GLD - Level 12") != std::string::npos,
            "class presence should include the class and level tooltip");
}

void PresenceMapsAllSelectedIds()
{
    struct Mapping
    {
        std::uint16_t id;
        const char*   abbreviation;
        const char*   assetKey;
    };

    // XIVLegacy/xivl-client-data:csv/xtx_text_jobName.csv supplies the IDs
    // and names; the prepared artwork roster supplies the asset keys.
    constexpr std::array classes{
        Mapping{ 2, "PGL", "class_pgl" },
        Mapping{ 3, "GLD", "class_gld" },
        Mapping{ 4, "MRD", "class_mrd" },
        Mapping{ 7, "ARC", "class_arc" },
        Mapping{ 8, "LNC", "class_lnc" },
        Mapping{ 22, "THM", "class_thm" },
        Mapping{ 23, "CNJ", "class_cnj" },
        Mapping{ 29, "CRP", "class_crp" },
        Mapping{ 30, "BSM", "class_bsm" },
        Mapping{ 31, "ARM", "class_arm" },
        Mapping{ 32, "GSM", "class_gsm" },
        Mapping{ 33, "LTW", "class_ltw" },
        Mapping{ 34, "WVR", "class_wvr" },
        Mapping{ 35, "ALC", "class_alc" },
        Mapping{ 36, "CUL", "class_cul" },
        Mapping{ 39, "MIN", "class_min" },
        Mapping{ 40, "BTN", "class_btn" },
        Mapping{ 41, "FSH", "class_fsh" },
    };
    constexpr std::array jobs{
        Mapping{ 15, "MNK", "job_mnk" },
        Mapping{ 16, "PLD", "job_pld" },
        Mapping{ 17, "WAR", "job_war" },
        Mapping{ 18, "BRD", "job_brd" },
        Mapping{ 19, "DRG", "job_drg" },
        Mapping{ 26, "BLM", "job_blm" },
        Mapping{ 27, "WHM", "job_whm" },
    };
    BahamutNativePluginPlayerStateV1 state{};
    for (const Mapping& expected : classes)
    {
        state.baseClassId        = expected.id;
        state.jobId              = 0;
        const PresenceJob actual = ResolveJob(state);
        Require(actual.abbreviation == expected.abbreviation &&
                    actual.assetKey == expected.assetKey,
                "class ID should select its artwork key");
    }
    for (const Mapping& expected : jobs)
    {
        state.baseClassId        = 3;
        state.jobId              = expected.id;
        const PresenceJob actual = ResolveJob(state);
        Require(actual.abbreviation == expected.abbreviation &&
                    actual.assetKey == expected.assetKey,
                "job ID should override the class artwork key");
    }
}

void PresenceRetainsGenericSessionWhenStateMissing()
{
    DiscordRpcPlugin plugin;
    plugin.startedAt = 999;
    BahamutNativePluginPlayerStateV1 state{};
    state.structSize           = sizeof(state);
    const std::string activity = BuildActivity(plugin, state);
    Require(activity.find("\"name\":\"BahamutXIV\"") != std::string::npos,
            "generic presence should retain the application name");
    Require(activity.find("\"timestamps\":{\"start\":999}") != std::string::npos,
            "generic presence should retain the session timer");
    Require(activity.find("details") == std::string::npos &&
                activity.find("\"state\"") == std::string::npos &&
                activity.find("assets") == std::string::npos,
            "generic presence should omit character-specific fields");
}

void RpcRepliesMustConfirmTheRequestedActivity()
{
    Require(IsRpcReply("{ \"cmd\": \"DISPATCH\", \"evt\": \"READY\" }",
                       "DISPATCH",
                       JsonString("READY")),
            "READY handshake response was rejected");
    Require(IsRpcReply("{\"nonce\":\"7\",\"cmd\":\"SET_ACTIVITY\",\"data\":{},\"evt\":null}",
                       "SET_ACTIVITY",
                       "",
                       "7"),
            "accepted presence response was rejected");
    Require(IsRpcReply("{\"nonce\":\"7\",\"cmd\":\"SET_ACTIVITY\",\"data\":{}}",
                       "SET_ACTIVITY",
                       "",
                       "7"),
            "accepted presence response without evt was rejected");
    Require(!IsRpcReply("{\"nonce\":\"7\",\"cmd\":\"SET_ACTIVITY\",\"evt\":\"ERROR\"}",
                        "SET_ACTIVITY",
                        "",
                        "7"),
            "Discord error was accepted as active presence");
    Require(!IsRpcReply("{\"nonce\":\"8\",\"cmd\":\"SET_ACTIVITY\",\"evt\":null}",
                        "SET_ACTIVITY",
                        "",
                        "7"),
            "unrelated presence response was accepted");
}

void PresenceReusesPipeAndChecksDiscordReplies()
{
    const std::wstring pipeName = L"\\\\.\\pipe\\bahamut-discord-activity-test-" +
                                  std::to_wstring(GetCurrentProcessId());
    HANDLE             server   = CreateNamedPipeW(pipeName.c_str(), PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, nullptr);
    Require(server != INVALID_HANDLE_VALUE, "could not create activity test pipe");
    HANDLE idlePingWritten = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    Require(idlePingWritten != nullptr, "could not create activity test event");

    std::thread peer([&]()
                     {
                         const BOOL connected = ConnectNamedPipe(server, nullptr);
                         Require(connected != FALSE || GetLastError() == ERROR_PIPE_CONNECTED,
                                 "activity test pipe connection failed");
                         const auto pingAndExpectPong = [&](bool signalIdle)
                         {
                             const std::string                  ping = "{}";
                             const std::array<std::uint32_t, 2> pingHeader{
                                 3, static_cast<std::uint32_t>(ping.size())
                             };
                             DWORD bytes = 0;
                             Require(WriteFile(server, pingHeader.data(), sizeof(pingHeader), &bytes, nullptr) != FALSE &&
                                         bytes == sizeof(pingHeader) &&
                                         WriteFile(server, ping.data(), static_cast<DWORD>(ping.size()), &bytes, nullptr) != FALSE &&
                                         bytes == ping.size(),
                                     "activity test could not send a ping");
                             if (signalIdle)
                                 SetEvent(idlePingWritten);
                             std::array<std::uint32_t, 2> pongHeader{};
                             Require(ReadFile(server, pongHeader.data(), sizeof(pongHeader), &bytes, nullptr) != FALSE &&
                                         bytes == sizeof(pongHeader) && pongHeader[0] == 4 &&
                                         pongHeader[1] == ping.size(),
                                     "activity test did not receive a pong");
                             std::string pong(pongHeader[1], '\0');
                             Require(ReadFile(server, pong.data(), pongHeader[1], &bytes, nullptr) != FALSE &&
                                         bytes == pongHeader[1] && pong == ping,
                                     "activity test received the wrong pong payload");
                         };
                         for (int index = 1; index <= 3; ++index)
                         {
                             std::array<std::uint32_t, 2> header{};
                             DWORD                        bytes = 0;
                             Require(ReadFile(server, header.data(), sizeof(header), &bytes, nullptr) != FALSE &&
                                         bytes == sizeof(header) && header[0] == 1 && header[1] < 4096,
                                     "activity test did not receive a request frame");
                             std::string request(header[1], '\0');
                             Require(ReadFile(server, request.data(), header[1], &bytes, nullptr) != FALSE &&
                                         bytes == header[1] &&
                                         request.find("\"nonce\":\"" + std::to_string(index) + "\"") != std::string::npos,
                                     "activity test received the wrong request");
                             if (index == 1)
                                 pingAndExpectPong(false);
                             std::string response =
                                 "{\"cmd\":\"SET_ACTIVITY\",\"nonce\":\"" +
                                 std::to_string(index) + "\"";
                             if (index == 1)
                                 response += ",\"evt\":null";
                             else if (index == 3)
                                 response += ",\"evt\":\"ERROR\"";
                             response += "}";
                             const std::array<std::uint32_t, 2> replyHeader{
                                 1, static_cast<std::uint32_t>(response.size())
                             };
                             Require(WriteFile(server, replyHeader.data(), sizeof(replyHeader), &bytes, nullptr) != FALSE &&
                                         bytes == sizeof(replyHeader) &&
                                         WriteFile(server, response.data(), static_cast<DWORD>(response.size()), &bytes, nullptr) != FALSE &&
                                         bytes == response.size(),
                                     "activity test could not send a reply");
                             if (index == 2)
                                 pingAndExpectPong(true);
                         }
                     });

    HANDLE client = CreateFileW(pipeName.c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, nullptr);
    Require(client != INVALID_HANDLE_VALUE, "could not open activity test pipe");
    DiscordRpcPlugin plugin;
    plugin.pipe      = client;
    plugin.startedAt = 123;
    BahamutNativePluginPlayerStateV1 state{};
    state.structSize = sizeof(state);
    Require(SendPresence(plugin, state) && plugin.hasPresence && plugin.pipe == client,
            "first accepted activity did not retain the connection");
    Require(SendPresence(plugin, state) && plugin.hasPresence && plugin.pipe == client,
            "updated activity did not reuse the connection");
    Require(WaitForSingleObject(idlePingWritten, 1000) == WAIT_OBJECT_0 &&
                PipeIsAlive(plugin),
            "idle connection did not respond to Discord ping");
    Require(SendPresence(plugin, state) && !plugin.hasPresence &&
                plugin.pipe == INVALID_HANDLE_VALUE,
            "rejected activity was marked active");
    peer.join();
    CloseHandle(idlePingWritten);
    CloseHandle(server);
}

} // namespace

int main()
{
    StalledPayloadHonorsDeadline();
    StalledIdlePollDoesNotBlockStatePublication();
    IdlePollSharesOneDeadlineAcrossFrames();
    PresenceUsesVerifiedJobMapping();
    PresenceOmitsUnknownPartialState();
    PresenceUsesClassAssetKeys();
    PresenceMapsAllSelectedIds();
    PresenceRetainsGenericSessionWhenStateMissing();
    RpcRepliesMustConfirmTheRequestedActivity();
    PresenceReusesPipeAndChecksDiscordReplies();
    return 0;
}
