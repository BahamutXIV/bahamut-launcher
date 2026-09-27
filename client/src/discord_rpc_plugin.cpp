#include "native_plugin_api.h"

#include <array>
#include <cctype>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <new>
#include <string>
#include <thread>
#include <vector>

namespace
{

constexpr wchar_t kApplicationIdEnvironment[] =
    L"BAHAMUT_RUNTIME_DISCORD_APPLICATION_ID";
constexpr DWORD kIpcTimeoutMilliseconds = 500;

struct IpcDeadline
{
    ULONGLONG expiresAt = 0;
};

struct DiscordRpcPlugin
{
    HANDLE                           pipe = INVALID_HANDLE_VALUE;
    std::string                      applicationId;
    std::int64_t                     startedAt = 0;
    std::uint32_t                    nonce     = 0;
    BahamutNativePluginPlayerStateV1 state{};
    std::mutex                       mutex;
    std::condition_variable          wake;
    std::thread                      worker;
    bool                             enabled        = false;
    bool                             dirty          = false;
    bool                             clearRequested = false;
    bool                             stop           = false;
    bool                             hasPresence    = false;
};

std::wstring EnvironmentValue(const wchar_t* name)
{
    const DWORD needed = GetEnvironmentVariableW(name, nullptr, 0);
    if (needed == 0)
        return {};
    std::wstring value(needed, L'\0');
    const DWORD  copied = GetEnvironmentVariableW(name, value.data(), needed);
    if (copied == 0 || copied >= needed)
        return {};
    value.resize(copied);
    return value;
}

std::string Utf8(const std::wstring& value)
{
    if (value.empty())
        return {};
    const int needed = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
    if (needed <= 0)
        return {};
    std::string output(static_cast<std::size_t>(needed), '\0');
    if (WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), output.data(), needed, nullptr, nullptr) != needed)
    {
        return {};
    }
    return output;
}

bool IsApplicationId(const std::string& value)
{
    if (value.size() < 17 || value.size() > 20)
        return false;
    for (const char character : value)
    {
        if (character < '0' || character > '9')
            return false;
    }
    return true;
}

std::string JsonString(const std::string& value)
{
    std::string escaped;
    escaped.reserve(value.size() + 2);
    escaped.push_back('"');
    for (const unsigned char character : value)
    {
        switch (character)
        {
            case '"':
                escaped += "\\\"";
                break;
            case '\\':
                escaped += "\\\\";
                break;
            case '\b':
                escaped += "\\b";
                break;
            case '\f':
                escaped += "\\f";
                break;
            case '\n':
                escaped += "\\n";
                break;
            case '\r':
                escaped += "\\r";
                break;
            case '\t':
                escaped += "\\t";
                break;
            default:
                if (character >= 0x20)
                    escaped.push_back(static_cast<char>(character));
                break;
        }
    }
    escaped.push_back('"');
    return escaped;
}

// IDs and names: XIVLegacy/xivl-client-data:csv/xtx_text_jobName.csv,
// SHA-256 61535798445DDB716CD16E8B68B06D9C6A67F76C321FE0DD5E9840C143DE8B57.
// Abbreviations select this application's Discord asset keys.
const char* ClassAbbreviation(std::uint16_t id)
{
    switch (id)
    {
        case 2:
            return "PGL";
        case 3:
            return "GLD";
        case 4:
            return "MRD";
        case 7:
            return "ARC";
        case 8:
            return "LNC";
        case 22:
            return "THM";
        case 23:
            return "CNJ";
        case 29:
            return "CRP";
        case 30:
            return "BSM";
        case 31:
            return "ARM";
        case 32:
            return "GSM";
        case 33:
            return "LTW";
        case 34:
            return "WVR";
        case 35:
            return "ALC";
        case 36:
            return "CUL";
        case 39:
            return "MIN";
        case 40:
            return "BTN";
        case 41:
            return "FSH";
        default:
            return nullptr;
    }
}

const char* JobAbbreviation(std::uint16_t id)
{
    switch (id)
    {
        case 15:
            return "MNK";
        case 16:
            return "PLD";
        case 17:
            return "WAR";
        case 18:
            return "BRD";
        case 19:
            return "DRG";
        case 26:
            return "BLM";
        case 27:
            return "WHM";
        default:
            return nullptr;
    }
}

struct PresenceJob
{
    std::string abbreviation;
    std::string assetKey;
};

PresenceJob ResolveJob(const BahamutNativePluginPlayerStateV1& state)
{
    const auto keyFor = [](const char* prefix, const char* abbreviation)
    {
        std::string key = std::string(prefix) + abbreviation;
        for (char& character : key)
        {
            character = static_cast<char>(
                std::tolower(static_cast<unsigned char>(character)));
        }
        return key;
    };
    if (const char* job = JobAbbreviation(state.jobId); job != nullptr)
    {
        return { job, keyFor("job_", job) };
    }
    if (const char* base = ClassAbbreviation(state.baseClassId); base != nullptr)
    {
        return { base, keyFor("class_", base) };
    }
    return {};
}

std::string BuildActivity(const DiscordRpcPlugin&                 plugin,
                          const BahamutNativePluginPlayerStateV1& state)
{
    std::string activity = "{\"name\":\"BahamutXIV\",\"timestamps\":{\"start\":" +
                           std::to_string(plugin.startedAt) + "}";
    if ((state.flags & BahamutNativePluginPlayerStateHasCharacter) != 0u &&
        state.displayName[0] != '\0')
    {
        activity += ",\"details\":" + JsonString(state.displayName);
    }
    if ((state.flags & BahamutNativePluginPlayerStateHasArea) != 0u &&
        state.areaName[0] != '\0')
    {
        activity += ",\"state\":" + JsonString(state.areaName);
    }
    if ((state.flags & BahamutNativePluginPlayerStateHasClassJob) != 0u &&
        state.level != 0u)
    {
        const PresenceJob job = ResolveJob(state);
        if (!job.abbreviation.empty() && !job.assetKey.empty())
        {
            activity += ",\"assets\":{\"small_image\":" +
                        JsonString(job.assetKey) + ",\"small_text\":" +
                        JsonString(std::string(job.abbreviation) + " - Level " +
                                   std::to_string(state.level)) +
                        "}";
        }
    }
    activity += "}";
    return activity;
}

std::int64_t UnixTime()
{
    FILETIME time{};
    GetSystemTimeAsFileTime(&time);
    ULARGE_INTEGER ticks{};
    ticks.LowPart                               = time.dwLowDateTime;
    ticks.HighPart                              = time.dwHighDateTime;
    constexpr std::uint64_t kWindowsToUnixEpoch = 116444736000000000ULL;
    return static_cast<std::int64_t>((ticks.QuadPart - kWindowsToUnixEpoch) / 10000000ULL);
}

IpcDeadline StartIpcDeadline()
{
    return { GetTickCount64() + kIpcTimeoutMilliseconds };
}

DWORD RemainingMilliseconds(const IpcDeadline& deadline)
{
    const ULONGLONG now = GetTickCount64();
    if (now >= deadline.expiresAt)
        return 0;
    return static_cast<DWORD>(deadline.expiresAt - now);
}

bool FinishIo(HANDLE pipe, OVERLAPPED& operation, BOOL started, DWORD& transferred, const IpcDeadline& deadline)
{
    if (started != FALSE)
        return true;
    if (GetLastError() != ERROR_IO_PENDING)
        return false;

    const DWORD remaining = RemainingMilliseconds(deadline);
    if (remaining != 0 && WaitForSingleObject(operation.hEvent, remaining) == WAIT_OBJECT_0)
    {
        return GetOverlappedResult(pipe, &operation, &transferred, FALSE) != FALSE;
    }

    (void)CancelIoEx(pipe, &operation);
    (void)GetOverlappedResult(pipe, &operation, &transferred, TRUE);
    return false;
}

bool WriteAll(HANDLE pipe, const unsigned char* bytes, DWORD size, const IpcDeadline& deadline)
{
    DWORD offset = 0;
    while (offset < size)
    {
        if (RemainingMilliseconds(deadline) == 0)
            return false;
        OVERLAPPED operation{};
        operation.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        if (operation.hEvent == nullptr)
            return false;

        DWORD      written  = 0;
        const BOOL started  = WriteFile(pipe, bytes + offset, size - offset, &written, &operation);
        const bool finished = FinishIo(pipe, operation, started, written, deadline);
        CloseHandle(operation.hEvent);
        if (!finished || written == 0)
            return false;
        offset += written;
    }
    return true;
}

bool ReadAll(HANDLE pipe, unsigned char* bytes, DWORD size, const IpcDeadline& deadline)
{
    DWORD offset = 0;
    while (offset < size)
    {
        if (RemainingMilliseconds(deadline) == 0)
            return false;
        OVERLAPPED operation{};
        operation.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        if (operation.hEvent == nullptr)
            return false;

        DWORD      read     = 0;
        const BOOL started  = ReadFile(pipe, bytes + offset, size - offset, &read, &operation);
        const bool finished = FinishIo(pipe, operation, started, read, deadline);
        CloseHandle(operation.hEvent);
        if (!finished || read == 0)
            return false;
        offset += read;
    }
    return true;
}

bool WriteFrame(HANDLE pipe, std::uint32_t opcode, const std::string& json, const IpcDeadline& deadline)
{
    const std::uint32_t        length = static_cast<std::uint32_t>(json.size());
    std::vector<unsigned char> frame(sizeof(opcode) + sizeof(length) + json.size());
    std::memcpy(frame.data(), &opcode, sizeof(opcode));
    std::memcpy(frame.data() + sizeof(opcode), &length, sizeof(length));
    std::memcpy(frame.data() + sizeof(opcode) + sizeof(length), json.data(), json.size());
    return WriteAll(pipe, frame.data(), static_cast<DWORD>(frame.size()), deadline);
}

bool ReadFrame(HANDLE pipe, const IpcDeadline& deadline, std::uint32_t* opcode = nullptr, std::string* json = nullptr)
{
    std::array<std::uint32_t, 2> header{};
    if (!ReadAll(pipe, reinterpret_cast<unsigned char*>(header.data()), sizeof(header), deadline) || header[1] > 1024 * 1024)
    {
        return false;
    }
    std::vector<unsigned char> payload(header[1]);
    if (!payload.empty() &&
        !ReadAll(pipe, payload.data(), static_cast<DWORD>(payload.size()), deadline))
        return false;
    if (opcode != nullptr)
        *opcode = header[0];
    if (json != nullptr)
        json->assign(payload.begin(), payload.end());
    return true;
}

std::string CompactJson(const std::string& json)
{
    std::string compact;
    compact.reserve(json.size());
    bool inString = false;
    bool escaped  = false;
    for (const char character : json)
    {
        if (!inString && std::isspace(static_cast<unsigned char>(character)))
            continue;
        compact.push_back(character);
        if (escaped)
            escaped = false;
        else if (inString && character == '\\')
            escaped = true;
        else if (character == '"')
            inString = !inString;
    }
    return compact;
}

bool IsRpcReply(const std::string& json, const std::string& command, const std::string& event, const std::string& nonce = {})
{
    const std::string compact      = CompactJson(json);
    const bool        eventMatches = event.empty()
                                         ? (compact.find("\"evt\":") == std::string::npos ||
                                            compact.find("\"evt\":null") != std::string::npos)
                                         : compact.find("\"evt\":" + event) != std::string::npos;
    return compact.find("\"cmd\":" + JsonString(command)) != std::string::npos &&
           eventMatches &&
           (nonce.empty() ||
            compact.find("\"nonce\":" + JsonString(nonce)) != std::string::npos);
}

bool ReadActivityReply(HANDLE pipe, const std::string& nonce)
{
    const IpcDeadline deadline = StartIpcDeadline();
    while (RemainingMilliseconds(deadline) != 0)
    {
        std::uint32_t opcode = 0;
        std::string   reply;
        if (!ReadFrame(pipe, deadline, &opcode, &reply))
            return false;
        if (opcode == 3)
        {
            if (!WriteFrame(pipe, 4, reply, deadline))
                return false;
            continue;
        }
        if (opcode != 1)
            return false;
        if (IsRpcReply(reply, "SET_ACTIVITY", "", nonce))
            return true;
        if (CompactJson(reply).find("\"nonce\":" + JsonString(nonce)) !=
            std::string::npos)
            return false;
    }
    return false;
}

HANDLE OpenDiscordPipe(const IpcDeadline& deadline)
{
    for (int index = 0; index < 10; ++index)
    {
        if (RemainingMilliseconds(deadline) == 0)
            break;
        const std::wstring path = L"\\\\.\\pipe\\discord-ipc-" + std::to_wstring(index);
        HANDLE             pipe = CreateFileW(path.c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, nullptr);
        if (pipe != INVALID_HANDLE_VALUE)
            return pipe;
    }
    return INVALID_HANDLE_VALUE;
}

void ClosePipe(DiscordRpcPlugin& plugin)
{
    if (plugin.pipe != INVALID_HANDLE_VALUE)
    {
        CloseHandle(plugin.pipe);
        plugin.pipe = INVALID_HANDLE_VALUE;
    }
}

bool SendPresence(DiscordRpcPlugin&                       plugin,
                  const BahamutNativePluginPlayerStateV1& state)
{
    if (plugin.pipe == INVALID_HANDLE_VALUE)
    {
        const IpcDeadline deadline = StartIpcDeadline();
        plugin.pipe                = OpenDiscordPipe(deadline);
        if (plugin.pipe == INVALID_HANDLE_VALUE)
        {
            plugin.hasPresence = false;
            OutputDebugStringA("Bahamut DiscordRPC: Discord is unavailable\n");
            return true;
        }
        const std::string handshake = "{\"v\":1,\"client_id\":" + JsonString(plugin.applicationId) + "}";
        std::uint32_t     opcode    = 0;
        std::string       reply;
        if (!WriteFrame(plugin.pipe, 0, handshake, deadline) ||
            !ReadFrame(plugin.pipe, StartIpcDeadline(), &opcode, &reply) ||
            opcode != 1 || !IsRpcReply(reply, "DISPATCH", JsonString("READY")))
        {
            plugin.hasPresence = false;
            ClosePipe(plugin);
            OutputDebugStringA("Bahamut DiscordRPC: handshake failed\n");
            return true;
        }
    }
    const std::string nonce = std::to_string(++plugin.nonce);
    const std::string activity =
        "{\"cmd\":\"SET_ACTIVITY\",\"args\":{\"pid\":" +
        std::to_string(GetCurrentProcessId()) + ",\"activity\":" +
        BuildActivity(plugin, state) + "},\"nonce\":" +
        JsonString(nonce) + "}";
    if (!WriteFrame(plugin.pipe, 1, activity, StartIpcDeadline()) ||
        !ReadActivityReply(plugin.pipe, nonce))
    {
        plugin.hasPresence = false;
        ClosePipe(plugin);
        OutputDebugStringA("Bahamut DiscordRPC: presence update rejected or failed\n");
        return true;
    }
    plugin.hasPresence = true;
    OutputDebugStringA("Bahamut DiscordRPC: presence active\n");
    return true;
}

void ClearPresence(DiscordRpcPlugin& plugin)
{
    if (plugin.hasPresence && plugin.pipe != INVALID_HANDLE_VALUE)
    {
        const IpcDeadline deadline = StartIpcDeadline();
        const std::string clear =
            "{\"cmd\":\"SET_ACTIVITY\",\"args\":{\"pid\":" + std::to_string(GetCurrentProcessId()) + ",\"activity\":null},\"nonce\":" + JsonString(std::to_string(++plugin.nonce)) + "}";
        (void)WriteFrame(plugin.pipe, 1, clear, deadline);
    }
    plugin.hasPresence = false;
    ClosePipe(plugin);
}

bool PipeIsAlive(DiscordRpcPlugin& plugin)
{
    if (plugin.pipe == INVALID_HANDLE_VALUE)
    {
        return false;
    }
    const IpcDeadline deadline  = StartIpcDeadline();
    DWORD             available = 0;
    for (;;)
    {
        if (RemainingMilliseconds(deadline) == 0)
            return true;
        if (PeekNamedPipe(plugin.pipe, nullptr, 0, nullptr, &available, nullptr) == FALSE)
            break;
        if (available < sizeof(std::uint32_t) * 2)
            return true;
        std::uint32_t opcode = 0;
        std::string   payload;
        if (!ReadFrame(plugin.pipe, deadline, &opcode, &payload) ||
            opcode == 2 ||
            (opcode == 3 &&
             !WriteFrame(plugin.pipe, 4, payload, deadline)) ||
            (opcode == 1 &&
             CompactJson(payload).find("\"evt\":\"ERROR\"") !=
                 std::string::npos))
            break;
    }
    ClosePipe(plugin);
    plugin.hasPresence = false;
    return false;
}

void WorkerMain(DiscordRpcPlugin& plugin)
{
    std::unique_lock lock(plugin.mutex);
    for (;;)
    {
        plugin.wake.wait_for(lock, std::chrono::seconds(2), [&plugin]
                             {
                                 return plugin.stop || plugin.dirty || plugin.clearRequested;
                             });
        if (!plugin.stop && plugin.hasPresence)
        {
            lock.unlock();
            const bool alive = PipeIsAlive(plugin);
            lock.lock();
            if (!alive)
                plugin.dirty = plugin.enabled;
        }
        const bool stop = plugin.stop;
        if (!plugin.dirty && !plugin.clearRequested && !stop &&
            !plugin.hasPresence)
        {
            plugin.dirty = plugin.enabled;
        }
        const bool clear      = plugin.clearRequested || stop || !plugin.enabled;
        const bool send       = plugin.enabled && !clear && plugin.dirty;
        const auto state      = plugin.state;
        plugin.dirty          = false;
        plugin.clearRequested = false;
        lock.unlock();
        if (clear)
        {
            ClearPresence(plugin);
        }
        else if (send)
        {
            (void)SendPresence(plugin, state);
        }
        lock.lock();
        if (stop)
        {
            return;
        }
    }
}

void StartWorker(DiscordRpcPlugin& plugin)
{
    plugin.worker = std::thread([&plugin]
                                {
                                    WorkerMain(plugin);
                                });
}

void StopWorker(DiscordRpcPlugin& plugin)
{
    {
        std::lock_guard lock(plugin.mutex);
        plugin.enabled        = false;
        plugin.clearRequested = true;
        plugin.stop           = true;
        plugin.wake.notify_one();
    }
    if (plugin.worker.joinable())
    {
        plugin.worker.join();
    }
}

void* WINAPI CreateDiscordRpc(const BahamutNativePluginConfigV1* config)
{
    if (config == nullptr || config->structSize < sizeof(BahamutNativePluginConfigV1))
    {
        return nullptr;
    }
    auto* plugin = new (std::nothrow) DiscordRpcPlugin();
    if (plugin == nullptr)
        return nullptr;
    plugin->applicationId = Utf8(EnvironmentValue(kApplicationIdEnvironment));
    plugin->startedAt     = UnixTime();
    plugin->enabled       = config->enabled != FALSE;
    if (!IsApplicationId(plugin->applicationId))
    {
        delete plugin;
        return nullptr;
    }
    StartWorker(*plugin);
    if (plugin->enabled)
    {
        std::lock_guard lock(plugin->mutex);
        plugin->dirty = true;
        plugin->wake.notify_one();
    }
    return plugin;
}

BOOL WINAPI ConfigureDiscordRpc(void*                              instance,
                                const BahamutNativePluginConfigV1* config)
{
    if (instance == nullptr || config == nullptr || config->structSize < sizeof(BahamutNativePluginConfigV1))
    {
        return FALSE;
    }
    auto&      plugin  = *static_cast<DiscordRpcPlugin*>(instance);
    const bool enabled = config->enabled != FALSE;
    {
        std::lock_guard lock(plugin.mutex);
        if (plugin.enabled == enabled)
        {
            return TRUE;
        }
        plugin.enabled        = enabled;
        plugin.dirty          = enabled;
        plugin.clearRequested = !enabled;
        plugin.wake.notify_one();
    }
    return TRUE;
}

BOOL WINAPI SetDiscordPlayerState(void*                                   instance,
                                  const BahamutNativePluginPlayerStateV1* state)
{
    if (instance == nullptr || state == nullptr ||
        state->structSize < sizeof(BahamutNativePluginPlayerStateV1))
    {
        return FALSE;
    }
    auto&           plugin = *static_cast<DiscordRpcPlugin*>(instance);
    std::lock_guard lock(plugin.mutex);
    const bool      changed = std::memcmp(&plugin.state, state, sizeof(*state)) != 0;
    plugin.state            = *state;
    if (changed)
    {
        plugin.dirty = true;
        plugin.wake.notify_one();
    }
    return TRUE;
}

BOOL WINAPI EnableDiscordRpc(void* instance, BOOL enabled)
{
    if (instance == nullptr)
        return FALSE;
    auto&           plugin = *static_cast<DiscordRpcPlugin*>(instance);
    std::lock_guard lock(plugin.mutex);
    plugin.enabled        = enabled != FALSE;
    plugin.dirty          = plugin.enabled;
    plugin.clearRequested = !plugin.enabled;
    plugin.wake.notify_one();
    return TRUE;
}

BOOL WINAPI IgnoreInput(void*, const BahamutNativePluginInputV1*, std::uint32_t* flags)
{
    if (flags != nullptr)
        *flags = BahamutNativePluginEventNone;
    return TRUE;
}

BOOL WINAPI IgnoreDevice(void*, void*, std::uint32_t* flags)
{
    if (flags != nullptr)
        *flags = BahamutNativePluginEventNone;
    return TRUE;
}

void WINAPI ShutdownDiscordRpc(void* instance)
{
    if (instance != nullptr)
        StopWorker(*static_cast<DiscordRpcPlugin*>(instance));
}

void WINAPI DestroyDiscordRpc(void* instance)
{
    delete static_cast<DiscordRpcPlugin*>(instance);
}

} // namespace

extern "C" __declspec(dllexport) BOOL WINAPI BahamutNativePluginGetApi(
    std::uint32_t requestedVersion, const BahamutNativePluginHostV3* host, BahamutNativePluginApiV3* api)
{
    if (requestedVersion != kBahamutNativePluginAbiVersion || host == nullptr || host->abiVersion != kBahamutNativePluginAbiVersion || host->structSize < sizeof(BahamutNativePluginHostV3) || api == nullptr || api->structSize < sizeof(BahamutNativePluginApiV3))
    {
        return FALSE;
    }
    *api = {
        kBahamutNativePluginAbiVersion,
        sizeof(BahamutNativePluginApiV3),
        "discord-rpc",
        &CreateDiscordRpc,
        &ConfigureDiscordRpc,
        &EnableDiscordRpc,
        &IgnoreInput,
        &IgnoreDevice,
        &IgnoreDevice,
        &SetDiscordPlayerState,
        &ShutdownDiscordRpc,
        &DestroyDiscordRpc,
    };
    return TRUE;
}
