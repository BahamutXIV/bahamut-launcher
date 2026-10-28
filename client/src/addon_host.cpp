#include "addon_host.h"

#include "action_movement.h"
#include "actor_name.h"
#include "area_names.h"
#include "custom_timer.h"
#include "enemy_cast.h"
#include "player_state.h"
#include "servicing_lock.h"
#include "target_distance.h"

extern "C"
{
#include <lauxlib.h>
#include <lua.h>
#include <lualib.h>
}

#include <windows.h>

#include <algorithm>
#include <array>
#include <cctype>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <ctime>
#include <fstream>
#include <map>
#include <optional>
#include <sstream>
#include <string>
#include <utility>

namespace
{

constexpr std::size_t      kMaximumAddonIdLength         = 64;
constexpr std::size_t      kMaximumSettingLength         = 1024;
constexpr std::size_t      kMaximumAddonUrlBytes         = 2048;
constexpr std::size_t      kMaximumChatLogBytes          = 8192;
constexpr std::size_t      kMaximumQueuedChatLines       = 256;
constexpr std::size_t      kMaximumQueuedCombatEvents    = 256;
constexpr std::size_t      kMaximumCombatMeterRows       = 64;
constexpr std::size_t      kMaximumCombatMeterNameBytes  = 128;
constexpr std::size_t      kMaximumCombatMeterLabelBytes = 16;
constexpr std::uint16_t    kCombatSingleOpcode           = 0x0139u;
constexpr std::uint16_t    kCombatMultiOpcode            = 0x013Au;
constexpr std::uint16_t    kCombatLargeOpcode            = 0x013Bu;
constexpr std::uint16_t    kDamageTextId                 = 30301u;
constexpr std::uint16_t    kCriticalDamageTextId         = 30302u;
constexpr std::uint16_t    kMissTextId                   = 30311u;
constexpr std::uint16_t    kEvadeTextId                  = 30310u;
constexpr std::uint16_t    kParryTextId                  = 30308u;
constexpr std::uint16_t    kBlockTextId                  = 30306u;
constexpr std::uint16_t    kHealTextId                   = 30320u;
constexpr std::string_view kBahamutWikiOrigin            = "https://bahamut.miraheze.org";
// SetMap opcode and payload zone offset match PlayerStateService::Observe.
constexpr std::uint16_t kSetMapOpcode = 0x0005u;

constexpr std::size_t kMaximumCommandWordLength = 32;

struct ParsedManifest
{
    std::string              id;
    std::string              entry;
    std::vector<std::string> commandWords;
};

std::string Trim(std::string value)
{
    const auto notSpace = [](unsigned char character)
    {
        return !std::isspace(character);
    };
    value.erase(value.begin(), std::find_if(value.begin(), value.end(), notSpace));
    value.erase(std::find_if(value.rbegin(), value.rend(), notSpace).base(), value.end());
    return value;
}

std::optional<std::string> QuotedValue(const std::string& value)
{
    const std::string trimmed = Trim(value);
    if (trimmed.size() < 2 || trimmed.front() != '"' || trimmed.back() != '"')
    {
        return std::nullopt;
    }
    const std::string result = trimmed.substr(1, trimmed.size() - 2);
    if (result.find_first_of("\r\n") != std::string::npos || result.find('\0') != std::string::npos)
    {
        return std::nullopt;
    }
    return result;
}

bool IsIdentifier(std::string_view value)
{
    if (value.empty() || value.size() > kMaximumAddonIdLength)
    {
        return false;
    }
    return std::all_of(value.begin(), value.end(), [](unsigned char character)
                       {
                           return (character >= 'a' && character <= 'z') || (character >= '0' && character <= '9') || character == '-';
                       });
}

bool IsSettingKey(std::string_view value)
{
    if (value.empty() || value.size() > kMaximumAddonIdLength)
    {
        return false;
    }
    return std::all_of(value.begin(), value.end(), [](unsigned char character)
                       {
                           return (character >= 'a' && character <= 'z') || (character >= 'A' && character <= 'Z') || (character >= '0' && character <= '9') || character == '-' || character == '_' || character == '.';
                       });
}

bool IsCommandWord(std::string_view value)
{
    if (value.size() < 2 || value.size() > kMaximumCommandWordLength || value.front() != '/')
    {
        return false;
    }
    return std::all_of(value.begin() + 1, value.end(), [](unsigned char character)
                       {
                           return (character >= 'a' && character <= 'z') || (character >= '0' && character <= '9') || character == '-' || character == '_';
                       });
}

// Returns the position of a '#' that starts a comment, skipping quoted text
// such as usage = "/fps color #RRGGBB".
std::size_t FindComment(std::string_view line)
{
    char quote = '\0';
    for (std::size_t index = 0; index != line.size(); ++index)
    {
        const char character = line[index];
        if (quote != '\0')
        {
            quote = character == quote ? '\0' : quote;
        }
        else if (character == '"' || character == '\'')
        {
            quote = character;
        }
        else if (character == '#')
        {
            return index;
        }
    }
    return std::string_view::npos;
}

// Command records are inline tables such as { name = "/fps lock", ... } or
// [[commands]] tables. The first word of each name is claimed; malformed
// words are ignored so they cannot invalidate an otherwise loadable package.
void CollectCommandWords(std::string_view line, bool inCommandsTable, std::vector<std::string>& words)
{
    const auto skipBlanks = [&](std::size_t cursor)
    {
        while (cursor < line.size() && (line[cursor] == ' ' || line[cursor] == '\t'))
        {
            ++cursor;
        }
        return cursor;
    };
    for (std::size_t at = line.find("name"); at != std::string_view::npos; at = line.find("name", at + 4))
    {
        // Only keys inside a command table; the top-level display name is not a command.
        const std::string_view before    = line.substr(0, at);
        const bool             tableKey  = inCommandsTable && before.find_first_not_of(" \t") == std::string_view::npos;
        const bool             inlineKey = before.find('{') != std::string_view::npos &&
                                           (before.back() == '{' || before.back() == ',' || before.back() == ' ' || before.back() == '\t');
        if (!tableKey && !inlineKey)
        {
            continue;
        }
        std::size_t cursor = skipBlanks(at + 4);
        if (cursor >= line.size() || line[cursor] != '=')
        {
            continue;
        }
        cursor = skipBlanks(cursor + 1);
        if (cursor + 1 >= line.size() || (line[cursor] != '"' && line[cursor] != '\'') || line[cursor + 1] != '/')
        {
            continue;
        }
        const char        quote = line[cursor];
        const std::size_t begin = cursor + 1;
        std::size_t       end   = begin;
        while (end < line.size() && line[end] != quote && line[end] != ' ')
        {
            ++end;
        }
        const std::string_view word = line.substr(begin, end - begin);
        if (IsCommandWord(word) && std::find(words.begin(), words.end(), word) == words.end())
        {
            words.emplace_back(word);
        }
    }
}

std::optional<ParsedManifest> ParseManifest(const std::filesystem::path& path)
{
    std::ifstream file(path);
    if (!file)
    {
        return std::nullopt;
    }
    ParsedManifest manifest;
    std::string    line;
    bool           inCommandsTable = false;
    while (std::getline(file, line))
    {
        const std::size_t comment = FindComment(line);
        if (comment != std::string::npos)
        {
            line.resize(comment);
        }
        const std::string header = Trim(line);
        if (!header.empty() && header.front() == '[')
        {
            inCommandsTable = header == "[[commands]]";
            continue;
        }
        CollectCommandWords(line, inCommandsTable, manifest.commandWords);
        const std::size_t equals = line.find('=');
        if (equals == std::string::npos)
        {
            continue;
        }
        const std::string key   = Trim(line.substr(0, equals));
        const std::string value = Trim(line.substr(equals + 1));
        if (key == "id")
        {
            const auto parsed = QuotedValue(value);
            if (!parsed)
                return std::nullopt;
            manifest.id = *parsed;
        }
        else if (key == "entry")
        {
            const auto parsed = QuotedValue(value);
            if (!parsed)
                return std::nullopt;
            manifest.entry = *parsed;
        }
    }
    const std::filesystem::path entry(manifest.entry);
    if (!IsIdentifier(manifest.id) || entry.empty() || entry.has_parent_path() || entry.extension() != L".lua")
    {
        return std::nullopt;
    }
    return manifest;
}

std::optional<std::string> ReadScript(const std::filesystem::path& path)
{
    std::ifstream file(path, std::ios::binary);
    if (!file)
    {
        return std::nullopt;
    }
    std::ostringstream contents;
    contents << file.rdbuf();
    if (!file.good() && !file.eof())
    {
        return std::nullopt;
    }
    return contents.str();
}

void RemoveGlobal(lua_State* state, const char* name)
{
    lua_pushnil(state);
    lua_setglobal(state, name);
}

} // namespace

struct AddonHost::Instance
{
    std::filesystem::path                  manifestPath;
    std::filesystem::path                  settingsPath;
    std::filesystem::path                  chatLogDirectory;
    std::filesystem::path                  packetLogDirectory;
    std::string                            id;
    AddonHost*                             owner = nullptr;
    std::map<std::string, std::string>     settings;
    lua_State*                             state                 = nullptr;
    const AddonWindowSink*                 drawSink              = nullptr;
    bahamut_client::PlayerStateService*    playerState           = nullptr;
    bahamut_client::TargetDistanceService* targetDistance        = nullptr;
    bahamut_client::CustomTimerService*    customTimer           = nullptr;
    bahamut_client::EnemyCastService*      enemyCast             = nullptr;
    const AddonClipboardSink*              clipboardSink         = nullptr;
    const AddonChatSink*                   chatSink              = nullptr;
    const AddonUrlSink*                    urlSink               = nullptr;
    bahamut_client::ActionMovementService* actionMovement        = nullptr;
    std::uint64_t                          actionMovementOwner   = 0;
    std::uint64_t                          actionMovementSession = 0;
    bool                                   faulted               = false;
    bool                                   closing               = false;
};

std::optional<std::string> ReadAddonManifestId(
    const std::filesystem::path& manifestPath)
{
    const auto manifest = ParseManifest(manifestPath);
    return manifest ? std::optional<std::string>(manifest->id) : std::nullopt;
}

std::vector<std::string> ReadAddonManifestCommandWords(
    const std::filesystem::path& manifestPath)
{
    auto manifest = ParseManifest(manifestPath);
    return manifest ? std::move(manifest->commandWords) : std::vector<std::string>{};
}

AddonHost::AddonHost(bahamut_client::PlayerStateService* playerState)
: playerState_(playerState)
{
}

namespace
{

// One recursive lock serializes addon Lua across the render and command threads.
using LuaLock = ServicingLock;

} // namespace

AddonHost::~AddonHost()
{
    for (const auto& addon : addons_)
    {
        // Process teardown reclaims Lua allocations. Running addon callbacks or
        // finalizers from the DLL detach path would execute under the loader lock.
        addon->state = nullptr;
    }
}

void AddonHost::LoadManifests(
    const std::vector<std::filesystem::path>& manifests,
    const std::filesystem::path&              settingsRoot,
    const std::filesystem::path&              chatLogsRoot)
{
    const LuaLock luaLock(luaMutex_);
    settingsRoot_ = settingsRoot;
    chatLogsRoot_ = chatLogsRoot;
    for (const auto& manifest : manifests)
    {
        Enable(manifest);
    }
}

void AddonHost::SetPlayerStateService(
    bahamut_client::PlayerStateService* playerState)
{
    const LuaLock luaLock(luaMutex_);
    playerState_ = playerState;
    for (const auto& addon : addons_)
    {
        addon->playerState = playerState;
    }
}

void AddonHost::SetTargetDistanceService(
    bahamut_client::TargetDistanceService* targetDistance)
{
    const LuaLock luaLock(luaMutex_);
    targetDistance_ = targetDistance;
    for (const auto& addon : addons_)
    {
        addon->targetDistance = targetDistance;
    }
}

void AddonHost::SetCustomTimerService(
    bahamut_client::CustomTimerService* customTimer)
{
    const LuaLock luaLock(luaMutex_);
    customTimer_ = customTimer;
    for (const auto& addon : addons_)
    {
        addon->customTimer = customTimer;
    }
}

void AddonHost::SetEnemyCastService(
    bahamut_client::EnemyCastService* enemyCast)
{
    const LuaLock luaLock(luaMutex_);
    enemyCast_ = enemyCast;
    for (const auto& addon : addons_)
    {
        addon->enemyCast = enemyCast;
    }
}

void AddonHost::SetActorNameService(bahamut_client::ActorNameService* actorNames)
{
    const LuaLock luaLock(luaMutex_);
    actorNames_ = actorNames;
}

bool IsHexDigit(unsigned char value)
{
    return (value >= '0' && value <= '9') || (value >= 'a' && value <= 'f') || (value >= 'A' && value <= 'F');
}

bool IsUriAscii(unsigned char value)
{
    return (value >= 'a' && value <= 'z') || (value >= 'A' && value <= 'Z') || (value >= '0' && value <= '9') || std::string_view("-._~:/?#[]@!$&'()*+,;=%").find(static_cast<char>(value)) != std::string_view::npos;
}

bool IsValidUtf8(std::string_view value)
{
    std::size_t index = 0;
    while (index < value.size())
    {
        const unsigned char first             = static_cast<unsigned char>(value[index]);
        std::size_t         continuationCount = 0;
        unsigned char       minimum           = 0x80;
        unsigned char       maximum           = 0xbf;
        if (first <= 0x7f)
        {
            ++index;
            continue;
        }
        if (first >= 0xc2 && first <= 0xdf)
        {
            continuationCount = 1;
        }
        else if (first >= 0xe0 && first <= 0xef)
        {
            continuationCount = 2;
            if (first == 0xe0)
                minimum = 0xa0;
            if (first == 0xed)
                maximum = 0x9f;
        }
        else if (first >= 0xf0 && first <= 0xf4)
        {
            continuationCount = 3;
            if (first == 0xf0)
                minimum = 0x90;
            if (first == 0xf4)
                maximum = 0x8f;
        }
        else
        {
            return false;
        }
        if (index + continuationCount >= value.size())
        {
            return false;
        }
        const unsigned char second = static_cast<unsigned char>(value[index + 1]);
        if (second < minimum || second > maximum)
        {
            return false;
        }
        for (std::size_t offset = 2; offset <= continuationCount; ++offset)
        {
            const unsigned char continuation = static_cast<unsigned char>(
                value[index + offset]);
            if (continuation < 0x80 || continuation > 0xbf)
            {
                return false;
            }
        }
        index += continuationCount + 1;
    }
    return true;
}

bool IsAllowedAddonUrlImpl(std::string_view url)
{
    if (url.empty() || url.size() > kMaximumAddonUrlBytes || !IsValidUtf8(url) || url.size() <= kBahamutWikiOrigin.size() || url.compare(0, kBahamutWikiOrigin.size(), kBahamutWikiOrigin) != 0 || url[kBahamutWikiOrigin.size()] != '/')
    {
        return false;
    }
    for (std::size_t index = kBahamutWikiOrigin.size(); index < url.size(); ++index)
    {
        const unsigned char value = static_cast<unsigned char>(url[index]);
        if (value < 0x80 && !IsUriAscii(value))
        {
            return false;
        }
        if (value == '%')
        {
            if (index + 2 >= url.size() || !IsHexDigit(static_cast<unsigned char>(url[index + 1])) || !IsHexDigit(static_cast<unsigned char>(url[index + 2])))
            {
                return false;
            }
            index += 2;
        }
    }
    return true;
}

bool IsAllowedAddonUrl(std::string_view url)
{
    return IsAllowedAddonUrlImpl(url);
}

void AddonHost::SetClipboardSink(const AddonClipboardSink& sink)
{
    const LuaLock luaLock(luaMutex_);
    clipboardSink_ = sink;
}

void AddonHost::SetChatSink(const AddonChatSink& sink)
{
    const LuaLock luaLock(luaMutex_);
    chatSink_ = sink;
}

void AddonHost::SetUrlSink(const AddonUrlSink& sink)
{
    const LuaLock luaLock(luaMutex_);
    urlSink_ = sink;
}

bool AddonHost::AdmitActionMovementConsumer(
    bahamut_client::ActionMovementService& service,
    const std::filesystem::path&           manifestPath,
    std::uint64_t                          session)
{
    if (actionMovementShutdown_ || !manifestPath.is_absolute() || session == 0)
    {
        return false;
    }
    const auto found = std::find_if(addons_.begin(), addons_.end(), [&](const auto& addon)
                                    {
                                        return addon->id == "nolock" && !addon->faulted && !addon->closing && addon->state != nullptr &&
                                               addon->manifestPath.lexically_normal() == manifestPath.lexically_normal();
                                    });
    if (found == addons_.end())
    {
        return false;
    }
    return BindActionMovement(**found, service, session);
}

bool AddonHost::RegisterActionMovementConsumer(
    bahamut_client::ActionMovementService& service,
    const std::filesystem::path&           manifestPath,
    std::uint64_t                          session)
{
    if (actionMovementShutdown_ || !manifestPath.is_absolute() || session == 0)
    {
        return false;
    }
    actionMovementRegistration_ = ActionMovementRegistration{ &service, manifestPath, session };
    static_cast<void>(AdmitActionMovementConsumer(service, manifestPath, session));
    return true;
}

bool AddonHost::BindActionMovement(Instance& addon, bahamut_client::ActionMovementService& service, std::uint64_t session)
{
    const auto status = service.Status();
    if (status.boundary != bahamut_client::ActionMovementBoundaryState::Ready || status.session != session)
    {
        return false;
    }
    if (addon.actionMovement == &service && addon.actionMovementSession == session &&
        addon.actionMovementOwner != 0 && addon.actionMovementOwner == status.owner)
    {
        return true;
    }
    const auto owner = service.BindConsumer(session);
    if (owner == 0)
    {
        return false;
    }
    // Binding the replacement first makes old-token cleanup harmless on the same service.
    RevokeActionMovement(addon);
    addon.actionMovement        = &service;
    addon.actionMovementOwner   = owner;
    addon.actionMovementSession = session;
    return true;
}

void AddonHost::AdmitRegisteredActionMovement(Instance& addon, const std::filesystem::path& manifestPath)
{
    if (actionMovementShutdown_ || !actionMovementRegistration_ || addon.id != "nolock" || addon.faulted || addon.closing ||
        addon.state == nullptr || manifestPath.lexically_normal() != actionMovementRegistration_->manifestPath.lexically_normal())
    {
        return;
    }
    static_cast<void>(BindActionMovement(addon, *actionMovementRegistration_->service, actionMovementRegistration_->session));
}

void AddonHost::RevokeActionMovement(Instance& addon)
{
    if (addon.actionMovement != nullptr)
    {
        (void)addon.actionMovement->RevokeConsumer(addon.actionMovementOwner);
        addon.actionMovement        = nullptr;
        addon.actionMovementOwner   = 0;
        addon.actionMovementSession = 0;
    }
}

void AddonHost::ShutdownActionMovement()
{
    actionMovementShutdown_ = true;
    actionMovementRegistration_.reset();
    for (const auto& addon : addons_)
    {
        RevokeActionMovement(*addon);
    }
}

void AddonHost::Fault(Instance& addon)
{
    RevokeActionMovement(addon);
    addon.faulted  = true;
    addon.drawSink = nullptr;
}

bool AddonHost::Enable(const std::filesystem::path& manifestPath)
{
    const LuaLock luaLock(luaMutex_);
    const auto    id = ReadAddonManifestId(manifestPath);
    if (!id)
    {
        return false;
    }
    const bool duplicate = std::any_of(addons_.begin(), addons_.end(), [&](const auto& existing)
                                       {
                                           return existing->id == *id;
                                       });
    if (duplicate)
    {
        return true;
    }
    auto addon = LoadOne(manifestPath, settingsRoot_, chatLogsRoot_, this, playerState_, targetDistance_, customTimer_, enemyCast_, &clipboardSink_, &chatSink_, &urlSink_);
    if (addon == nullptr)
    {
        return false;
    }
    addons_.push_back(std::move(addon));
    if (*id == "zonename")
    {
        ResetAreaTransition();
    }
    if (*id == "packetlogger")
    {
        packetLoggerFaults_ = 0;
        ReplacePacketWriter(*addons_.back());
        packetLoggerEnabled_.store(!addons_.back()->faulted,
                                   std::memory_order_release);
        if (addons_.back()->faulted || !StartPacketCapture(*addons_.back()))
        {
            packetLoggerEnabled_.store(false, std::memory_order_release);
            Close(*addons_.back());
            addons_.pop_back();
            return false;
        }
    }
    if (*id == "combatparser")
    {
        std::lock_guard lock(combatQueueMutex_);
        combatQueue_.clear();
        combatDropped_ = 0;
        combatParserEnabled_.store(!addons_.back()->faulted, std::memory_order_release);
    }
    return true;
}

bool AddonHost::Reload(std::string_view addonId)
{
    const LuaLock luaLock(luaMutex_);
    if (addonId == "packetlogger")
    {
        packetLoggerFaults_ = 0;
    }
    return ReloadAddon(addonId, packet_capture::EndReason::Disable);
}

bool AddonHost::ReloadAddon(std::string_view addonId, packet_capture::EndReason reason)
{
    const auto found = std::find_if(addons_.begin(), addons_.end(), [&](const auto& addon)
                                    {
                                        return addon->id == addonId;
                                    });
    if (found == addons_.end())
    {
        return false;
    }
    const std::filesystem::path manifest = (*found)->manifestPath;
    if (addonId == "zonename")
    {
        ResetAreaTransition();
    }
    if (addonId == "packetlogger")
    {
        packetLoggerEnabled_.store(false, std::memory_order_release);
        StopPacketCaptureAsync(reason);
    }
    if (addonId == "combatparser")
    {
        combatParserEnabled_.store(false, std::memory_order_release);
        std::lock_guard lock(combatQueueMutex_);
        combatQueue_.clear();
        combatDropped_ = 0;
    }
    Close(**found);
    auto replacement = LoadOne(manifest, settingsRoot_, chatLogsRoot_, this, playerState_, targetDistance_, customTimer_, enemyCast_, &clipboardSink_, &chatSink_, &urlSink_);
    if (replacement == nullptr)
    {
        addons_.erase(found);
        return false;
    }
    *found = std::move(replacement);
    if (addonId == "packetlogger")
    {
        ReplacePacketWriter(**found);
        packetLoggerEnabled_.store(!(*found)->faulted,
                                   std::memory_order_release);
        if ((*found)->faulted || !StartPacketCapture(**found))
        {
            packetLoggerEnabled_.store(false, std::memory_order_release);
            Close(**found);
            addons_.erase(found);
            return false;
        }
    }
    if (addonId == "combatparser")
    {
        combatParserEnabled_.store(!(*found)->faulted, std::memory_order_release);
    }
    return true;
}

bool AddonHost::Disable(std::string_view addonId)
{
    const LuaLock luaLock(luaMutex_);
    const auto    found = std::find_if(addons_.begin(), addons_.end(), [&](const auto& addon)
                                       {
                                        return addon->id == addonId;
                                       });
    if (found == addons_.end())
    {
        return false;
    }
    if (addonId == "zonename")
    {
        ResetAreaTransition();
    }
    if (addonId == "packetlogger")
    {
        packetLoggerFaults_ = 0;
        packetLoggerEnabled_.store(false, std::memory_order_release);
        StopPacketCaptureAsync(packet_capture::EndReason::Disable);
    }
    if (addonId == "combatparser")
    {
        combatParserEnabled_.store(false, std::memory_order_release);
        std::lock_guard lock(combatQueueMutex_);
        combatQueue_.clear();
        combatDropped_ = 0;
    }
    Close(**found);
    addons_.erase(found);
    return true;
}

bool AddonHost::IsLoaded(std::string_view addonId) const
{
    const LuaLock luaLock(luaMutex_);
    return std::any_of(addons_.begin(), addons_.end(), [&](const auto& addon)
                       {
                           return addon->id == addonId;
                       });
}

bool AddonHost::TargetlinesEnabled() const
{
    const LuaLock luaLock(luaMutex_);
    return std::any_of(addons_.begin(), addons_.end(), [](const auto& addon)
                       {
                           return addon->id == "targetlines" && !addon->faulted && addon->state != nullptr;
                       });
}

bool AddonHost::IsFaulted(std::string_view addonId) const
{
    const LuaLock luaLock(luaMutex_);
    return std::any_of(addons_.begin(), addons_.end(), [&](const auto& addon)
                       {
                           return addon->id == addonId && addon->faulted;
                       });
}

std::vector<std::string> AddonHost::LoadedIds() const
{
    const LuaLock            luaLock(luaMutex_);
    std::vector<std::string> ids;
    ids.reserve(addons_.size());
    for (const auto& addon : addons_)
    {
        ids.push_back(addon->id);
    }
    return ids;
}

bool AddonHost::DispatchCommand(std::string_view command)
{
    const LuaLock luaLock(luaMutex_);
    for (const auto& addon : addons_)
    {
        if (addon->faulted || addon->state == nullptr)
        {
            continue;
        }
        bool       handled = false;
        const bool invoked = InvokeCommand(*addon, command, handled);
        StopFaultedCombatParser();
        if (!invoked)
        {
            continue;
        }
        if (handled)
        {
            return true;
        }
    }
    return false;
}

void AddonHost::QueueChat(std::string_view source, std::string_view message)
{
    const std::size_t separatorSize = source.empty() ? 0u : 2u;
    if (message.empty() || source.size() + separatorSize + message.size() > kMaximumChatLogBytes)
    {
        return;
    }
    std::string combined(source);
    if (!source.empty())
    {
        combined.append(": ");
    }
    combined.append(message);
    std::lock_guard<std::mutex> lock(chatQueueMutex_);
    if (chatQueue_.size() == kMaximumQueuedChatLines)
    {
        chatQueue_.erase(chatQueue_.begin());
    }
    chatQueue_.push_back(std::move(combined));
}

void AddonHost::QueueAreaTransition(const packet_observer::GameMessage& message)
{
    if (message.direction != packet_observer::Direction::Incoming ||
        message.opcode != kSetMapOpcode || message.sourceId == 0u ||
        message.sourceId == 0xffffffffu || message.sourceId == 0xcdcdcdcdu ||
        message.sourceId == 0xc0000000u || message.payload.size() != 16u)
    {
        return;
    }
    std::uint32_t zoneId = 0;
    std::memcpy(&zoneId, message.payload.data() + 4u, sizeof(zoneId));
    if (zoneId == 0u || zoneId == 0xffffffffu || zoneId == 0xcdcdcdcdu ||
        zoneId == 0xc0000000u)
    {
        return;
    }
    if (packetLoggerEnabled_.load(std::memory_order_acquire))
    {
        if (const auto writer = PacketWriter(); writer != nullptr)
        {
            writer->EnqueueZone(std::chrono::system_clock::now(), zoneId, bahamut_client::AreaNameForZone(zoneId));
        }
    }
    std::lock_guard lock(areaQueueMutex_);
    queuedAreaZoneId_ = zoneId;
}

void AddonHost::SetRuntimeIdentity(std::string launcherVersion, std::uint32_t runtimeApiVersion)
{
    launcherVersion_   = std::move(launcherVersion);
    runtimeApiVersion_ = runtimeApiVersion;
}

bool AddonHost::QueuePacketFrame(const packet_observer::FrameInfo& frame)
{
    if (!packetLoggerEnabled_.load(std::memory_order_acquire) ||
        frame.bytes.empty())
    {
        return false;
    }
    const auto writer = PacketWriter();
    if (writer == nullptr)
    {
        return false;
    }
    const auto decision = packet_capture::Classify(
        static_cast<packet_capture::Scope>(packetScope_.load(std::memory_order_acquire)), frame);
    if (!decision.write)
    {
        writer->CountSkipped();
        return false;
    }
    const auto now = std::chrono::system_clock::now();
    if (!decision.stub.empty())
    {
        return writer->EnqueueStub(frame.direction, now, frame.bytes.size(), decision.stub);
    }
    return writer->Enqueue(frame.direction, now, frame.bytes);
}

void AddonHost::QueueCombatResult(const packet_observer::GameMessage& message)
{
    if (!combatParserEnabled_.load(std::memory_order_acquire) ||
        message.direction != packet_observer::Direction::Incoming ||
        playerState_ == nullptr)
    {
        return;
    }
    // Layouts match Bahamut's command_results.h for opcodes 0x0139-0x013b.
    std::size_t capacity     = 0;
    std::size_t payloadSize  = 0;
    std::size_t amountOffset = 0;
    std::size_t textOffset   = 0;
    switch (message.opcode)
    {
        case kCombatSingleOpcode:
            capacity     = 1;
            payloadSize  = 0x38u;
            amountOffset = 0x2Cu;
            textOffset   = 0x2Eu;
            break;
        case kCombatMultiOpcode:
            capacity     = 10;
            payloadSize  = 0xB8u;
            amountOffset = 0x50u;
            textOffset   = 0x64u;
            break;
        case kCombatLargeOpcode:
            capacity     = 18;
            payloadSize  = 0x128u;
            amountOffset = 0x70u;
            textOffset   = 0x94u;
            break;
        default:
            return;
    }
    const auto& payload = message.payload;
    if (payload.size() != payloadSize)
    {
        return;
    }
    const auto readU16 = [&](std::size_t offset)
    {
        return static_cast<std::uint16_t>(payload[offset] |
                                          (static_cast<std::uint16_t>(payload[offset + 1u]) << 8u));
    };
    const auto readU32 = [&](std::size_t offset)
    {
        return static_cast<std::uint32_t>(payload[offset]) |
               (static_cast<std::uint32_t>(payload[offset + 1u]) << 8u) |
               (static_cast<std::uint32_t>(payload[offset + 2u]) << 16u) |
               (static_cast<std::uint32_t>(payload[offset + 3u]) << 24u);
    };
    const auto          player   = playerState_->Snapshot();
    const std::uint32_t sourceId = readU32(0);
    if (!player || player->actorId == 0u || sourceId == 0u ||
        message.sourceId != sourceId)
    {
        return;
    }
    const std::uint32_t count = readU32(0x20u);
    if (count == 0u || count > capacity)
    {
        return;
    }
    const std::uint16_t commandId   = readU16(0x24u);
    const bool          sourceEnemy = (sourceId >> 28u) == 4u;
    // These IDs have healing action metadata in Bahamut's sql/battle_commands.sql.
    constexpr std::array<std::uint16_t, 18> healingCommands = {
        26502u, 26503u, 26504u, 27100u, 27149u, 27345u, 27346u, 27347u, 27348u, 28666u, 28667u, 28668u, 28669u, 28828u, 29010u, 29011u, 29012u, 29013u
    };
    const bool knownHealingCommand =
        std::binary_search(healingCommands.begin(), healingCommands.end(), commandId);
    std::lock_guard lock(combatQueueMutex_);
    if (!combatParserEnabled_.load(std::memory_order_relaxed))
    {
        return;
    }
    for (std::size_t index = 0; index < count; ++index)
    {
        const std::uint32_t target = readU32(0x28u + 4u * index);
        const std::uint16_t amount = readU16(amountOffset + 2u * index);
        const std::uint16_t textId = readU16(textOffset + 2u * index);
        if (target == 0u)
        {
            continue;
        }
        const bool targetEnemy = (target >> 28u) == 4u;
        // Bahamut's spell result path leaves text ID at zero for positive rows.
        const bool   hit     = (amount > 0u && commandId != 0u && textId == 0u) ||
                               (amount > 0u &&
                                (textId == kDamageTextId || textId == kCriticalDamageTextId)) ||
                               textId == kParryTextId || textId == kBlockTextId;
        const bool   avoided = amount == 0u &&
                               (textId == kMissTextId || textId == kEvadeTextId);
        std::uint8_t kind    = 0u;
        if (!sourceEnemy && targetEnemy)
        {
            kind = hit ? 1u : avoided ? 3u
                                      : 0u;
        }
        else if (!sourceEnemy && !targetEnemy && knownHealingCommand &&
                 amount > 0u && (hit || textId == kHealTextId))
        {
            kind = 2u;
        }
        if (kind == 0u)
        {
            continue;
        }
        if (combatQueue_.size() >= kMaximumQueuedCombatEvents)
        {
            ++combatDropped_;
            continue;
        }
        combatQueue_.push_back(CombatEvent{ sourceId, amount, kind });
    }
}

void AddonHost::Update(double frameDeltaSeconds)
{
    const LuaLock luaLock(luaMutex_);
    if (const auto writer = PacketWriter(); writer != nullptr)
    {
        DrainPacketNotices(*writer);
    }
    // A replaced writer may still publish a notice while it finishes its queue.
    for (auto retired = retiredPacketWriters_.begin(); retired != retiredPacketWriters_.end();)
    {
        const bool idle = !(*retired)->Status().recording && (*retired)->WaitIdle(std::chrono::milliseconds(0));
        DrainPacketNotices(**retired);
        retired = idle ? retiredPacketWriters_.erase(retired) : retired + 1;
    }
    while (!packetNotices_.empty() && chatSink_.print != nullptr &&
           chatSink_.print(chatSink_.context, packetNotices_.front()))
    {
        packetNotices_.pop_front();
    }
    std::vector<std::string> queued;
    {
        std::lock_guard<std::mutex> lock(chatQueueMutex_);
        queued.swap(chatQueue_);
    }
    for (const std::string& message : queued)
    {
        for (const auto& addon : addons_)
        {
            if (!addon->faulted && addon->state != nullptr)
            {
                InvokeChat(*addon, message);
            }
        }
    }
    for (const auto& addon : addons_)
    {
        if (addon->faulted || addon->state == nullptr)
        {
            continue;
        }
        lua_pushnumber(addon->state, frameDeltaSeconds);
        Invoke(*addon, "update", 1);
    }
    StopFaultedCombatParser();
    DispatchAreaTransition();
    // Last, so a restart never runs inside the addon loop above.
    HandlePacketLoggerFault();
}

void AddonHost::DispatchAreaTransition()
{
    std::optional<std::uint32_t> queuedZone;
    {
        std::lock_guard lock(areaQueueMutex_);
        queuedZone.swap(queuedAreaZoneId_);
    }
    if (!queuedZone)
    {
        return;
    }
    const auto found = std::find_if(addons_.begin(), addons_.end(), [](const auto& addon)
                                    {
                                        return addon->id == "zonename";
                                    });
    if (found == addons_.end() || (*found)->faulted || (*found)->state == nullptr)
    {
        return;
    }
    const std::uint32_t zoneId = *queuedZone;
    if (zoneId == lastAreaZoneId_)
    {
        return;
    }
    const std::string_view areaName   = bahamut_client::AreaNameForZone(zoneId);
    const std::string_view regionName = bahamut_client::AreaRegionNameForZone(zoneId);
    const bool             sameLabel  = !areaName.empty() && areaName == lastAreaName_ &&
                                        regionName == lastRegionName_;
    lastAreaZoneId_                   = zoneId;
    lastAreaName_                     = areaName;
    lastRegionName_                   = regionName;
    if (!sameLabel)
    {
        InvokeAreaChanged(**found, zoneId, areaName, regionName);
    }
}

void AddonHost::ResetAreaTransition()
{
    std::lock_guard lock(areaQueueMutex_);
    queuedAreaZoneId_.reset();
    lastAreaZoneId_ = 0;
    lastAreaName_.clear();
    lastRegionName_.clear();
}

void AddonHost::Draw(const AddonWindowSink& sink)
{
    const LuaLock luaLock(luaMutex_);
    for (const auto& addon : addons_)
    {
        if (addon->faulted || addon->state == nullptr)
        {
            continue;
        }
        addon->drawSink = &sink;
        Invoke(*addon, "draw");
        addon->drawSink = nullptr;
    }
    StopFaultedCombatParser();
}

// First fault: restart the addon once in a new session. Second fault: stop.
// Runs on the update thread only; the hook thread never calls into Lua.
void AddonHost::HandlePacketLoggerFault()
{
    if (!packetLoggerEnabled_.load(std::memory_order_acquire))
    {
        return;
    }
    const bool faulted = std::any_of(addons_.begin(), addons_.end(), [](const auto& addon)
                                     {
                                         return addon->id == "packetlogger" &&
                                                addon->faulted;
                                     });
    if (!faulted)
    {
        return;
    }
    if (++packetLoggerFaults_ == 1 && ReloadAddon("packetlogger", packet_capture::EndReason::Fault))
    {
        PushPacketNotice("Packet capture: recording again after an addon error");
        return;
    }
    packetLoggerEnabled_.store(false, std::memory_order_release);
    StopPacketCaptureAsync(packet_capture::EndReason::Fault);
    PushPacketNotice("Packet capture: stopped after an addon error");
}

void AddonHost::PushPacketNotice(std::string notice)
{
    constexpr std::size_t kMaximumPendingNotices = 8;
    if (packetNotices_.size() >= kMaximumPendingNotices)
    {
        packetNotices_.pop_front();
    }
    packetNotices_.push_back(std::move(notice));
}

bool AddonHost::StartPacketCapture(Instance& addon)
{
    if (addon.id != "packetlogger" || addon.faulted)
    {
        return false;
    }
    auto writer = PacketWriter();
    if (writer == nullptr)
    {
        ReplacePacketWriter(addon);
        writer = PacketWriter();
    }
    return writer != nullptr && writer->Start();
}

bool AddonHost::StopPacketCapture()
{
    const auto writer = PacketWriter();
    return writer != nullptr && writer->Stop();
}

bool AddonHost::SplitPacketCapture()
{
    const auto writer = PacketWriter();
    return writer != nullptr && writer->Split();
}

void AddonHost::StopPacketCaptureAsync(packet_capture::EndReason reason)
{
    if (const auto writer = PacketWriter(); writer != nullptr)
    {
        writer->Stop(reason, std::chrono::milliseconds(0));
    }
}

void AddonHost::ReplacePacketWriter(const Instance& addon)
{
    const auto setting = addon.settings.find("capture_scope");
    const auto scope   = packet_capture::ParseScope(setting == addon.settings.end() ? std::string_view{} : std::string_view(setting->second));
    packetScope_.store(static_cast<std::uint8_t>(scope), std::memory_order_release);
    if (auto old = PacketWriter(); old != nullptr)
    {
        // A Start that timed out may still open a session; close it before retiring.
        old->Stop(packet_capture::EndReason::Disable, std::chrono::milliseconds(0));
        DrainPacketNotices(*old);
        retiredPacketWriters_.push_back(std::move(old));
    }
    auto writer = std::make_shared<packet_capture::PacketCaptureWriter>(
        addon.packetLogDirectory, packet_capture::PacketCaptureLimits{}, packet_capture::PacketCaptureIdentity{ launcherVersion_, runtimeApiVersion_ });
    writer->SetScope(scope);
    std::lock_guard lock(packetWriterMutex_);
    packetWriter_ = std::move(writer);
}

void AddonHost::DrainPacketNotices(packet_capture::PacketCaptureWriter& writer)
{
    while (auto notice = writer.TakeNotice())
    {
        PushPacketNotice(std::move(*notice));
    }
}

void AddonHost::FailNextPacketWritesForTest(int count)
{
    if (const auto writer = PacketWriter(); writer != nullptr)
    {
        writer->FailNextWritesForTest(count);
    }
}

std::shared_ptr<packet_capture::PacketCaptureWriter> AddonHost::PacketWriter() const
{
    std::lock_guard lock(packetWriterMutex_);
    return packetWriter_;
}

bool AddonHost::WaitForPacketCaptureIdle(std::chrono::milliseconds timeout)
{
    const auto writer = PacketWriter();
    return writer == nullptr || writer->WaitIdle(timeout);
}

std::size_t AddonHost::LoadedCount() const
{
    const LuaLock luaLock(luaMutex_);
    return static_cast<std::size_t>(std::count_if(addons_.begin(), addons_.end(), [](const auto& addon)
                                                  {
                                                      return !addon->faulted && addon->state != nullptr;
                                                  }));
}

std::size_t AddonHost::FaultedCount() const
{
    const LuaLock luaLock(luaMutex_);
    return static_cast<std::size_t>(std::count_if(addons_.begin(), addons_.end(), [](const auto& addon)
                                                  {
                                                      return addon->faulted;
                                                  }));
}

std::unique_ptr<AddonHost::Instance> AddonHost::LoadOne(
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
    const AddonUrlSink*                    urlSink)
{
    const auto manifest = ParseManifest(manifestPath);
    if (!manifest)
    {
        return nullptr;
    }
    const auto script = ReadScript(manifestPath.parent_path() / manifest->entry);
    if (!script)
    {
        return nullptr;
    }

    auto addon            = std::make_unique<Instance>();
    addon->manifestPath   = manifestPath;
    addon->id             = manifest->id;
    addon->owner          = owner;
    addon->playerState    = playerState;
    addon->targetDistance = targetDistance;
    addon->customTimer    = customTimer;
    addon->enemyCast      = enemyCast;
    addon->clipboardSink  = clipboardSink;
    addon->chatSink       = chatSink;
    addon->urlSink        = urlSink;
    const std::filesystem::path addonId(addon->id);
    const std::filesystem::path settingsDirectory = settingsRoot / addonId;
    addon->settingsPath                           = settingsDirectory / L"settings.ini";
    addon->chatLogDirectory                       = chatLogsRoot;
    addon->packetLogDirectory                     = chatLogsRoot.parent_path() / L"packets";
    std::error_code directoryError;
    std::filesystem::create_directories(settingsDirectory, directoryError);
    if (directoryError)
    {
        return nullptr;
    }
    if (addon->id == "chatlogs")
    {
        std::filesystem::create_directories(addon->chatLogDirectory,
                                            directoryError);
        if (directoryError)
        {
            return nullptr;
        }
    }
    if (addon->id == "packetlogger")
    {
        std::filesystem::create_directories(addon->packetLogDirectory,
                                            directoryError);
        if (directoryError)
        {
            return nullptr;
        }
    }

    std::ifstream settingsFile(addon->settingsPath);
    std::string   settingLine;
    while (std::getline(settingsFile, settingLine))
    {
        const std::size_t equals = settingLine.find('=');
        if (equals == std::string::npos)
            continue;
        const std::string key   = settingLine.substr(0, equals);
        const std::string value = settingLine.substr(equals + 1);
        if (IsSettingKey(key) && value.size() <= kMaximumSettingLength)
        {
            addon->settings[key] = value;
        }
    }

    addon->state = luaL_newstate();
    if (addon->state == nullptr)
    {
        return nullptr;
    }
    lua_State* state = addon->state;
    luaopen_base(state);
    lua_settop(state, 0);
    luaopen_table(state);
    lua_settop(state, 0);
    luaopen_string(state);
    lua_settop(state, 0);
    luaopen_math(state);
    lua_settop(state, 0);
    for (const char* name : { "dofile", "loadfile", "load", "loadstring", "print", "require" })
    {
        RemoveGlobal(state, name);
    }

    lua_newtable(state);
    const auto registerFunction = [&](const char* name, lua_CFunction function)
    {
        lua_pushlightuserdata(state, addon.get());
        lua_pushcclosure(state, function, 1);
        lua_setfield(state, -2, name);
    };
    registerFunction("settings_get", &LuaSettingsGet);
    registerFunction("settings_set", &LuaSettingsSet);
    registerFunction("player_state", &LuaPlayerState);
    if (addon->id == "nolock")
    {
        registerFunction("action_movement_configure", &LuaActionMovementConfigure);
        registerFunction("action_movement_status", &LuaActionMovementStatus);
    }
    if (addon->id == "distance")
    {
        registerFunction("target_distance", &LuaTargetDistance);
    }
    if (addon->id == "targethp")
    {
        registerFunction("target_hp", &LuaTargetHp);
    }
    if (addon->id == "customtimer")
    {
        registerFunction("custom_timer", &LuaCustomTimer);
    }
    if (addon->id == "enemycast")
    {
        registerFunction("enemy_casts", &LuaEnemyCasts);
    }
    registerFunction("window", &LuaWindow);
    registerFunction("raw_text", &LuaRawText);
    registerFunction("clipboard_set", &LuaClipboardSet);
    registerFunction("chat_print", &LuaChatPrint);
    if (addon->id == "chatlogs")
    {
        registerFunction("chatlog_write", &LuaChatLogWrite);
    }
    if (addon->id == "packetlogger")
    {
        registerFunction("packetlog_start", &LuaPacketLogStart);
        registerFunction("packetlog_stop", &LuaPacketLogStop);
        registerFunction("packetlog_split", &LuaPacketLogSplit);
        registerFunction("packetlog_status", &LuaPacketLogStatus);
    }
    if (addon->id == "combatparser")
    {
        registerFunction("combat_events", &LuaCombatEvents);
        registerFunction("combat_meter", &LuaCombatMeter);
    }
    if (addon->id == "targetlines")
    {
        registerFunction("targetlines", &LuaTargetlines);
    }
    if (addon->id == "wiki")
    {
        registerFunction("open_url", &LuaOpenUrl);
    }
    lua_setglobal(state, "bahamut");

    if (luaL_loadbuffer(state, script->data(), script->size(), manifest->entry.c_str()) != 0 || lua_pcall(state, 0, 0, 0) != 0)
    {
        lua_pop(state, 1);
        Fault(*addon);
        return addon;
    }
    // Admit before the load callback so it can apply policy to the new owner.
    if (owner != nullptr)
    {
        owner->AdmitRegisteredActionMovement(*addon, manifestPath);
    }
    Invoke(*addon, "load");
    return addon;
}

void AddonHost::Close(Instance& addon)
{
    addon.closing = true;
    RevokeActionMovement(addon);
    if (addon.state == nullptr)
    {
        return;
    }
    if (!addon.faulted)
    {
        Invoke(addon, "unload");
    }
    lua_close(addon.state);
    addon.state    = nullptr;
    addon.drawSink = nullptr;
}

bool AddonHost::Invoke(Instance& addon, const char* callback, int argumentCount)
{
    lua_State* state = addon.state;
    lua_getglobal(state, callback);
    if (!lua_isfunction(state, -1))
    {
        lua_pop(state, 1 + argumentCount);
        return true;
    }
    if (argumentCount != 0)
    {
        lua_insert(state, -1 - argumentCount);
    }
    if (lua_pcall(state, argumentCount, 0, 0) == 0)
    {
        return true;
    }
    lua_pop(state, 1);
    Fault(addon);
    return false;
}

bool AddonHost::InvokeCommand(Instance& addon, std::string_view command, bool& handled)
{
    handled = false;
    lua_getglobal(addon.state, "command");
    if (!lua_isfunction(addon.state, -1))
    {
        lua_pop(addon.state, 1);
        return true;
    }
    lua_pushlstring(addon.state, command.data(), command.size());
    if (lua_pcall(addon.state, 1, 1, 0) != 0)
    {
        lua_pop(addon.state, 1);
        Fault(addon);
        return false;
    }
    handled = lua_toboolean(addon.state, -1) != 0;
    lua_pop(addon.state, 1);
    return true;
}

std::string SanitizeChatLogName(std::string_view value)
{
    std::string result(value);
    for (char& character : result)
    {
        const unsigned char byte = static_cast<unsigned char>(character);
        if (character == ' ')
        {
            character = '_';
        }
        else if (byte < 0x20 || std::string_view("<>:\"/\\|?*").find(character) != std::string_view::npos)
        {
            character = '_';
        }
    }
    const std::size_t firstPeriod = result.find('.');
    std::string       deviceName  = result.substr(0, firstPeriod);
    std::transform(deviceName.begin(), deviceName.end(), deviceName.begin(), [](unsigned char byte)
                   {
                       return static_cast<char>(std::toupper(byte));
                   });
    const bool numberedDevice = deviceName.size() == 4u && (deviceName.starts_with("COM") || deviceName.starts_with("LPT")) && deviceName[3] >= '1' && deviceName[3] <= '9';
    if (deviceName == "CON" || deviceName == "PRN" || deviceName == "AUX" || deviceName == "NUL" || numberedDevice)
    {
        result.insert(result.begin(), '_');
    }
    return result;
}

bool AppendChatLog(const std::filesystem::path& path, std::string_view time, std::string_view message)
{
    std::ofstream log(path, std::ios::app | std::ios::binary);
    if (!log)
    {
        return false;
    }
    log << '[' << time << "] " << message << '\n';
    return log.good();
}

bool AddonHost::InvokeChat(Instance& addon, std::string_view message)
{
    lua_getglobal(addon.state, "chat");
    if (!lua_isfunction(addon.state, -1))
    {
        lua_pop(addon.state, 1);
        return true;
    }
    lua_pushlstring(addon.state, message.data(), message.size());
    if (lua_pcall(addon.state, 1, 0, 0) == 0)
    {
        return true;
    }
    lua_pop(addon.state, 1);
    Fault(addon);
    return false;
}

bool AddonHost::InvokeAreaChanged(Instance& addon, std::uint32_t zoneId, std::string_view areaName, std::string_view regionName)
{
    lua_pushnumber(addon.state, zoneId);
    lua_pushlstring(addon.state, areaName.empty() ? "" : areaName.data(), areaName.size());
    lua_pushlstring(addon.state, regionName.empty() ? "" : regionName.data(), regionName.size());
    return Invoke(addon, "area_changed", 3);
}

int AddonHost::LuaPacketLogStart(lua_State* state)
{
    auto*      addon   = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    const bool started = addon != nullptr && addon->id == "packetlogger" &&
                         addon->owner != nullptr &&
                         addon->owner->packetLoggerEnabled_.load(std::memory_order_acquire) &&
                         addon->owner->StartPacketCapture(*addon);
    lua_pushboolean(state, started);
    return 1;
}

int AddonHost::LuaPacketLogStop(lua_State* state)
{
    auto*      addon   = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    const bool stopped = addon != nullptr && addon->id == "packetlogger" &&
                         addon->owner != nullptr && addon->owner->StopPacketCapture();
    lua_pushboolean(state, stopped);
    return 1;
}

int AddonHost::LuaPacketLogSplit(lua_State* state)
{
    auto*      addon = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    const bool split = addon != nullptr && addon->id == "packetlogger" &&
                       addon->owner != nullptr && addon->owner->SplitPacketCapture();
    lua_pushboolean(state, split);
    return 1;
}

int AddonHost::LuaPacketLogStatus(lua_State* state)
{
    auto* addon = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    if (addon == nullptr || addon->id != "packetlogger" || addon->owner == nullptr)
    {
        lua_pushnil(state);
        return 1;
    }
    packet_capture::PacketCaptureStatus status;
    if (const auto writer = addon->owner->PacketWriter(); writer != nullptr)
    {
        status = writer->Status();
    }
    const auto setNumber = [&](const char* name, double value)
    {
        lua_pushnumber(state, static_cast<lua_Number>(value));
        lua_setfield(state, -2, name);
    };
    lua_createtable(state, 0, 11);
    lua_pushboolean(state, status.recording);
    lua_setfield(state, -2, "recording");
    lua_pushlstring(state, status.file.data(), status.file.size());
    lua_setfield(state, -2, "file");
    setNumber("part", static_cast<double>(status.part));
    setNumber("captured", static_cast<double>(status.captured));
    setNumber("written", static_cast<double>(status.written));
    setNumber("dropped", static_cast<double>(status.dropped));
    setNumber("skipped", static_cast<double>(status.skipped));
    setNumber("queued", static_cast<double>(status.queued));
    setNumber("queue_high_water", static_cast<double>(status.queue_high_water));
    setNumber("bytes_on_disk", static_cast<double>(status.bytes_on_disk));
    lua_pushlstring(state, status.error.data(), status.error.size());
    lua_setfield(state, -2, "error");
    return 1;
}

void AddonHost::StopFaultedCombatParser()
{
    if (!combatParserEnabled_.load(std::memory_order_acquire))
    {
        return;
    }
    const bool faulted = std::any_of(addons_.begin(), addons_.end(), [](const auto& addon)
                                     {
                                         return addon->id == "combatparser" && addon->faulted;
                                     });
    if (faulted)
    {
        combatParserEnabled_.store(false, std::memory_order_release);
        std::lock_guard lock(combatQueueMutex_);
        combatQueue_.clear();
    }
}

int AddonHost::LuaCombatEvents(lua_State* state)
{
    auto* addon = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    if (addon == nullptr || addon->id != "combatparser" || addon->owner == nullptr)
    {
        lua_pushnil(state);
        lua_pushinteger(state, 0);
        lua_pushinteger(state, 0);
        return 3;
    }
    std::vector<CombatEvent> events;
    std::uint64_t            dropped = 0;
    {
        std::lock_guard lock(addon->owner->combatQueueMutex_);
        events.swap(addon->owner->combatQueue_);
        dropped = addon->owner->combatDropped_;
    }
    lua_createtable(state, static_cast<int>(events.size()), 0);
    const auto                                player = addon->owner->playerState_ != nullptr
                                                           ? addon->owner->playerState_->Snapshot()
                                                           : std::nullopt;
    constexpr std::array<std::string_view, 4> kinds  = {
        "", "damage", "healing", "miss"
    };
    for (std::size_t index = 0; index < events.size(); ++index)
    {
        const CombatEvent& event = events[index];
        lua_createtable(state, 0, 6);
        lua_pushinteger(state, static_cast<lua_Integer>(event.sourceId));
        lua_setfield(state, -2, "source_id");
        lua_pushinteger(state, static_cast<lua_Integer>(event.amount));
        lua_setfield(state, -2, "amount");
        const std::string_view kind = kinds[event.kind];
        lua_pushlstring(state, kind.data(), kind.size());
        lua_setfield(state, -2, "kind");
        std::string sourceName = addon->owner->actorNames_ != nullptr
                                     ? addon->owner->actorNames_->NameFor(event.sourceId)
                                     : std::string{};
        if (player && event.sourceId == player->actorId && !player->displayName.empty())
        {
            sourceName = player->displayName;
        }
        lua_pushlstring(state, sourceName.data(), sourceName.size());
        lua_setfield(state, -2, "source_name");
        const std::uint16_t sourceClassId = player && event.sourceId == player->actorId
                                                ? player->baseClassId
                                                : 0u;
        const std::uint16_t sourceJobId   = player && event.sourceId == player->actorId
                                                ? player->jobId
                                                : 0u;
        lua_pushinteger(state, static_cast<lua_Integer>(sourceClassId));
        lua_setfield(state, -2, "source_class_id");
        lua_pushinteger(state, static_cast<lua_Integer>(sourceJobId));
        lua_setfield(state, -2, "source_job_id");
        lua_rawseti(state, -2, static_cast<int>(index + 1u));
    }
    lua_pushnumber(state, static_cast<lua_Number>(dropped));
    lua_pushinteger(state, player ? static_cast<lua_Integer>(player->actorId) : 0);
    return 3;
}

int AddonHost::LuaTargetlines(lua_State* state)
{
    auto* addon = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    if (addon == nullptr || addon->id != "targetlines" || addon->drawSink == nullptr ||
        addon->drawSink->targetlines == nullptr || lua_gettop(state) != 0)
    {
        return 0;
    }
    addon->drawSink->targetlines(addon->drawSink->context, addon->id.c_str());
    return 0;
}

int AddonHost::LuaChatLogWrite(lua_State* state)
{
    auto*       addon   = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    std::size_t length  = 0;
    const char* message = lua_tolstring(state, 1, &length);
    if (addon == nullptr || addon->id != "chatlogs" || message == nullptr || length == 0 || length > kMaximumChatLogBytes || !IsValidUtf8(std::string_view(message, length)))
    {
        lua_pushboolean(state, 0);
        return 1;
    }

    std::string cleaned;
    cleaned.reserve(length);
    for (std::size_t index = 0; index < length; ++index)
    {
        const unsigned char value = static_cast<unsigned char>(message[index]);
        if (value == 0x07)
        {
            cleaned.push_back('\n');
        }
        else if (value == '\n' || value == '\t' || value >= 0x20)
        {
            cleaned.push_back(static_cast<char>(value));
        }
    }
    while (!cleaned.empty() && cleaned.back() == '\n')
    {
        cleaned.pop_back();
    }
    if (cleaned.empty())
    {
        lua_pushboolean(state, 0);
        return 1;
    }

    const std::time_t now = std::time(nullptr);
    std::tm           local{};
    if (localtime_s(&local, &now) != 0)
    {
        lua_pushboolean(state, 0);
        return 1;
    }
    char date[16]{};
    char time[16]{};
    if (std::strftime(date, sizeof(date), "%Y.%m.%d", &local) == 0 || std::strftime(time, sizeof(time), "%H:%M:%S", &local) == 0)
    {
        lua_pushboolean(state, 0);
        return 1;
    }
    const std::string fallbackFilename = std::string(date) + ".log";
    std::string       filename         = fallbackFilename;
    if (addon->playerState != nullptr)
    {
        const auto snapshot = addon->playerState->Snapshot();
        if (snapshot.has_value() && !snapshot->displayName.empty())
        {
            const std::string name = SanitizeChatLogName(snapshot->displayName);
            filename               = name + '_' + date + ".log";
        }
    }
    const auto toPath = [](std::string_view value)
    {
        const std::u8string utf8(
            reinterpret_cast<const char8_t*>(value.data()), value.size());
        return std::filesystem::path(utf8);
    };
    bool written = AppendChatLog(addon->chatLogDirectory / toPath(filename),
                                 time,
                                 cleaned);
    if (!written && filename != fallbackFilename)
    {
        written = AppendChatLog(
            addon->chatLogDirectory / toPath(fallbackFilename), time, cleaned);
    }
    lua_pushboolean(state, written);
    return 1;
}

const char* AddonHost::ActionMovementUnavailableReason(const Instance& addon, bahamut_client::ActionMovementStatus& status)
{
    if (addon.id != "nolock" || addon.faulted || addon.closing || addon.owner == nullptr ||
        addon.owner->actionMovementShutdown_ || addon.actionMovement == nullptr)
    {
        return "not_admitted";
    }
    status = addon.actionMovement->Status();
    using bahamut_client::ActionMovementBoundaryState;
    switch (status.boundary)
    {
        case ActionMovementBoundaryState::Unavailable:
            return "unavailable";
        case ActionMovementBoundaryState::TargetMismatch:
            return "target_mismatch";
        case ActionMovementBoundaryState::InstallFailed:
            return "install_failed";
        case ActionMovementBoundaryState::CleanupFailed:
            return "cleanup_failed";
        case ActionMovementBoundaryState::Ready:
            break;
    }
    if (status.session == 0 || status.session != addon.actionMovementSession)
    {
        return "session_unavailable";
    }
    if (status.owner == 0 || status.owner != addon.actionMovementOwner)
    {
        return "not_admitted";
    }
    return nullptr;
}

int AddonHost::LuaActionMovementConfigure(lua_State* state)
{
    if (lua_gettop(state) != 2 || lua_type(state, 1) != LUA_TNUMBER || lua_type(state, 2) != LUA_TBOOLEAN)
    {
        lua_pushboolean(state, false);
        lua_pushliteral(state, "invalid_request");
        return 2;
    }
    const auto percent = lua_tonumber(state, 1);
    if (!std::isfinite(percent) || percent < 0 || percent > 100 || percent != std::floor(percent))
    {
        lua_pushboolean(state, false);
        lua_pushliteral(state, "invalid_request");
        return 2;
    }
    auto*                                addon = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    bahamut_client::ActionMovementStatus status;
    const char*                          reason     = addon == nullptr ? "not_admitted" : ActionMovementUnavailableReason(*addon, status);
    const bool                           configured = reason == nullptr && addon->actionMovement->Configure(
                                                                               addon->actionMovementOwner, addon->actionMovementSession, static_cast<std::uint32_t>(percent), lua_toboolean(state, 2) != 0);
    lua_pushboolean(state, configured);
    lua_pushstring(state, configured ? "ready" : reason == nullptr ? "not_admitted"
                                                                   : reason);
    return 2;
}

int AddonHost::LuaActionMovementStatus(lua_State* state)
{
    if (lua_gettop(state) != 0)
    {
        lua_pushnil(state);
        lua_pushliteral(state, "invalid_request");
        return 2;
    }
    auto*                                addon = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    bahamut_client::ActionMovementStatus status;
    const char*                          reason    = addon == nullptr ? "not_admitted" : ActionMovementUnavailableReason(*addon, status);
    const bool                           available = reason == nullptr;
    lua_newtable(state);
    lua_pushboolean(state, available);
    lua_setfield(state, -2, "available");
    lua_pushboolean(state, available && status.enabled);
    lua_setfield(state, -2, "enabled");
    lua_pushnumber(state, available ? status.requestedPercent : 0);
    lua_setfield(state, -2, "requested_percent");
    lua_pushnumber(state, available ? status.effectivePercent : 100);
    lua_setfield(state, -2, "effective_percent");
    lua_pushstring(state, available ? "ready" : reason);
    lua_setfield(state, -2, "reason");
    return 1;
}

int AddonHost::LuaSettingsGet(lua_State* state)
{
    auto*       addon    = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    const char* key      = lua_tostring(state, 1);
    const char* fallback = lua_tostring(state, 2);
    if (addon == nullptr || key == nullptr || !IsSettingKey(key))
    {
        lua_pushnil(state);
        return 1;
    }
    const auto  found = addon->settings.find(key);
    const char* value = found == addon->settings.end()
                            ? (fallback == nullptr ? "" : fallback)
                            : found->second.c_str();
    lua_pushstring(state, value);
    return 1;
}

int AddonHost::LuaSettingsSet(lua_State* state)
{
    auto*       addon = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    const char* key   = lua_tostring(state, 1);
    const char* value = lua_tostring(state, 2);
    if (addon == nullptr || key == nullptr || value == nullptr || !IsSettingKey(key) || std::char_traits<char>::length(value) > kMaximumSettingLength || std::string_view(value).find_first_of("\r\n") != std::string_view::npos)
    {
        lua_pushboolean(state, 0);
        return 1;
    }
    addon->settings[key] = value;
    std::ofstream file(addon->settingsPath, std::ios::trunc);
    if (!file)
    {
        lua_pushboolean(state, 0);
        return 1;
    }
    for (const auto& [settingKey, settingValue] : addon->settings)
    {
        file << settingKey << '=' << settingValue << '\n';
    }
    lua_pushboolean(state, file.good() ? 1 : 0);
    return 1;
}

int AddonHost::LuaPlayerState(lua_State* state)
{
    auto* addon = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    if (addon == nullptr || addon->playerState == nullptr)
    {
        lua_pushnil(state);
        return 1;
    }
    const std::optional<bahamut_client::PlayerStateSnapshot> snapshot =
        addon->playerState->Snapshot();
    if (!snapshot.has_value())
    {
        lua_pushnil(state);
        return 1;
    }

    lua_newtable(state);
    lua_pushnumber(state, snapshot->x);
    lua_setfield(state, -2, "x");
    lua_pushnumber(state, snapshot->z);
    lua_setfield(state, -2, "z");
    lua_pushnumber(state, snapshot->y);
    lua_setfield(state, -2, "y");
    lua_pushnumber(state, snapshot->rotation);
    lua_setfield(state, -2, "rotation");
    lua_pushinteger(state, static_cast<lua_Integer>(snapshot->zoneId));
    lua_setfield(state, -2, "zone");
    return 1;
}

int AddonHost::LuaTargetDistance(lua_State* state)
{
    auto*      addon    = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    const auto snapshot = addon != nullptr && addon->targetDistance != nullptr
                              ? addon->targetDistance->Snapshot()
                              : std::nullopt;
    if (snapshot)
    {
        if (snapshot->yalms)
        {
            lua_pushnumber(state, *snapshot->yalms);
        }
        else
        {
            lua_pushnil(state);
        }
        lua_pushlstring(state, snapshot->name.data(), snapshot->name.size());
        lua_pushinteger(state, static_cast<lua_Integer>(snapshot->actorId));
    }
    else
    {
        lua_pushnil(state);
        lua_pushnil(state);
        lua_pushnil(state);
    }
    return 3;
}

int AddonHost::LuaTargetHp(lua_State* state)
{
    auto*      addon    = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    const auto snapshot = addon != nullptr && addon->targetDistance != nullptr
                              ? addon->targetDistance->Snapshot()
                              : std::nullopt;
    if (snapshot)
    {
        if (snapshot->currentHp && snapshot->maxHp)
        {
            lua_pushinteger(state, *snapshot->currentHp);
            lua_pushinteger(state, *snapshot->maxHp);
        }
        else
        {
            lua_pushnil(state);
            lua_pushnil(state);
        }
        lua_pushlstring(state, snapshot->name.data(), snapshot->name.size());
        lua_pushinteger(state, static_cast<lua_Integer>(snapshot->actorId));
    }
    else
    {
        lua_pushnil(state);
        lua_pushnil(state);
        lua_pushnil(state);
        lua_pushnil(state);
    }
    return 4;
}

int AddonHost::LuaCustomTimer(lua_State* state)
{
    auto*      addon    = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    const auto snapshot = addon != nullptr && addon->customTimer != nullptr
                              ? addon->customTimer->Snapshot()
                              : std::nullopt;
    if (!snapshot)
    {
        lua_pushnil(state);
        return 1;
    }
    lua_newtable(state);
    lua_pushlstring(state, snapshot->title.data(), snapshot->title.size());
    lua_setfield(state, -2, "instance_name");
    lua_pushlstring(state, snapshot->objective.data(), snapshot->objective.size());
    lua_setfield(state, -2, "objective");
    lua_pushinteger(state, static_cast<lua_Integer>(snapshot->remainingSeconds));
    lua_setfield(state, -2, "remaining_seconds");
    return 1;
}

int AddonHost::LuaEnemyCasts(lua_State* state)
{
    auto* addon = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    lua_newtable(state);
    if (addon == nullptr || addon->enemyCast == nullptr)
    {
        return 1;
    }
    const auto casts = addon->enemyCast->Snapshot();
    for (std::size_t index = 0; index < std::min<std::size_t>(casts.size(), 4u); ++index)
    {
        const auto& cast = casts[index];
        lua_newtable(state);
        lua_pushinteger(state, static_cast<lua_Integer>(cast.actorId));
        lua_setfield(state, -2, "actor_id");
        lua_pushinteger(state, static_cast<lua_Integer>(cast.commandId));
        lua_setfield(state, -2, "command_id");
        lua_pushlstring(state, cast.name.data(), cast.name.size());
        lua_setfield(state, -2, "name");
        lua_pushlstring(state, cast.actorName.data(), cast.actorName.size());
        lua_setfield(state, -2, "actor_name");
        lua_pushnumber(state, cast.remainingSeconds);
        lua_setfield(state, -2, "remaining_seconds");
        lua_pushnumber(state, cast.durationSeconds);
        lua_setfield(state, -2, "duration_seconds");
        lua_pushnumber(state, cast.progress);
        lua_setfield(state, -2, "progress");
        lua_pushboolean(state, cast.casting);
        lua_setfield(state, -2, "casting");
        lua_pushboolean(state, cast.completed);
        lua_setfield(state, -2, "completed");
        lua_rawseti(state, -2, static_cast<int>(index + 1));
    }
    return 1;
}

int AddonHost::LuaWindow(lua_State* state)
{
    auto*       addon  = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    const char* title  = lua_tostring(state, 1);
    const char* text   = lua_tostring(state, 2);
    const bool  locked = lua_isboolean(state, 3) && lua_toboolean(state, 3) != 0;
    if (addon != nullptr && addon->drawSink != nullptr && addon->drawSink->window != nullptr && title != nullptr && text != nullptr)
    {
        addon->drawSink->window(addon->drawSink->context, addon->id.c_str(), title, text, locked);
    }
    return 0;
}

int AddonHost::LuaCombatMeter(lua_State* state)
{
    const auto reject = [state]()
    {
        lua_pushboolean(state, 0);
        return 1;
    };
    auto* addon = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    if (addon == nullptr || addon->id != "combatparser" || addon->drawSink == nullptr ||
        addon->drawSink->combatMeter == nullptr || lua_gettop(state) != 4 ||
        lua_type(state, 1) != LUA_TSTRING || lua_type(state, 2) != LUA_TTABLE ||
        lua_type(state, 3) != LUA_TBOOLEAN || lua_type(state, 4) != LUA_TBOOLEAN)
    {
        return reject();
    }

    std::size_t modeLength = 0;
    const char* modeText   = lua_tolstring(state, 1, &modeLength);
    const auto  mode       = std::string_view(modeText, modeLength);
    if (mode != "dps" && mode != "hps")
    {
        return reject();
    }
    const std::size_t rowCount = lua_objlen(state, 2);
    if (rowCount > kMaximumCombatMeterRows)
    {
        return reject();
    }

    std::size_t keyCount = 0;
    lua_pushnil(state);
    while (lua_next(state, 2) != 0)
    {
        bool validKey = false;
        if (lua_type(state, -2) == LUA_TNUMBER)
        {
            const double key = lua_tonumber(state, -2);
            validKey         = std::isfinite(key) && key >= 1.0 && key <= static_cast<double>(rowCount) &&
                               std::floor(key) == key;
        }
        lua_pop(state, 1);
        if (!validKey)
        {
            lua_pop(state, 1);
            return reject();
        }
        ++keyCount;
    }
    if (keyCount != rowCount)
    {
        return reject();
    }

    const auto validLabel = [](std::string_view value, std::size_t maximumBytes, bool allowEmpty)
    {
        return (allowEmpty || !value.empty()) && value.size() <= maximumBytes &&
               IsValidUtf8(value) &&
               std::all_of(value.begin(), value.end(), [](unsigned char character)
                           {
                               return character >= 0x20u && character != 0x7fu;
                           });
    };
    std::vector<AddonCombatMeterRow> rows;
    rows.reserve(rowCount);
    for (std::size_t index = 1; index <= rowCount; ++index)
    {
        lua_rawgeti(state, 2, static_cast<int>(index));
        if (lua_type(state, -1) != LUA_TTABLE)
        {
            lua_pop(state, 1);
            return reject();
        }
        const auto rawField = [state](const char* name)
        {
            lua_pushstring(state, name);
            lua_rawget(state, -2);
        };
        const auto readStringField = [&](const char* name, std::size_t maximumBytes, bool allowEmpty, std::string& output)
        {
            rawField(name);
            if (lua_type(state, -1) != LUA_TSTRING)
            {
                lua_pop(state, 1);
                return false;
            }
            std::size_t            length = 0;
            const char*            value  = lua_tolstring(state, -1, &length);
            const std::string_view text(value, length);
            const bool             valid = validLabel(text, maximumBytes, allowEmpty);
            if (valid)
            {
                output.assign(text);
            }
            lua_pop(state, 1);
            return valid;
        };

        AddonCombatMeterRow row;
        bool                valid = readStringField("name", kMaximumCombatMeterNameBytes, false, row.name);
        rawField("amount");
        if (lua_type(state, -1) != LUA_TNUMBER)
        {
            valid = false;
        }
        else
        {
            constexpr double kMaximumExactLuaInteger = 9007199254740991.0;
            const double     amount                  = lua_tonumber(state, -1);
            if (!std::isfinite(amount) || amount < 0.0 || amount > kMaximumExactLuaInteger ||
                std::floor(amount) != amount)
            {
                valid = false;
            }
            else
            {
                row.amount = static_cast<std::uint64_t>(amount);
            }
        }
        lua_pop(state, 1);
        rawField("rate");
        if (lua_type(state, -1) != LUA_TNUMBER)
        {
            valid = false;
        }
        else
        {
            row.rate = lua_tonumber(state, -1);
            if (!std::isfinite(row.rate) || row.rate < 0.0)
            {
                valid = false;
            }
        }
        lua_pop(state, 1);
        valid = readStringField("accuracy", kMaximumCombatMeterLabelBytes, false, row.accuracy) && valid;
        valid = readStringField("color", 7, false, row.color) && valid;
        if (row.color.size() != 7 || row.color[0] != '#' ||
            !std::all_of(row.color.begin() + 1, row.color.end(), [](unsigned char character)
                         {
                             return IsHexDigit(character);
                         }))
        {
            valid = false;
        }
        lua_pop(state, 1);
        if (!valid)
        {
            return reject();
        }
        rows.push_back(std::move(row));
    }

    addon->drawSink->combatMeter(addon->drawSink->context, addon->id.c_str(), mode.data(), rows.data(), rows.size(), lua_toboolean(state, 3) != 0, lua_toboolean(state, 4) != 0);
    lua_pushboolean(state, 1);
    return 1;
}

int AddonHost::LuaRawText(lua_State* state)
{
    auto*       addon    = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    const char* text     = lua_tostring(state, 1);
    const bool  locked   = lua_isboolean(state, 6) && lua_toboolean(state, 6) != 0;
    float       fontSize = lua_isnumber(state, 7) ? static_cast<float>(lua_tonumber(state, 7)) : 0.0F;
    if (!std::isfinite(fontSize) || fontSize < 8.0F || fontSize > 48.0F)
    {
        fontSize = 0.0F;
    }
    if (addon == nullptr || addon->drawSink == nullptr || addon->drawSink->rawText == nullptr || text == nullptr || !lua_isnumber(state, 2) || !lua_isnumber(state, 3) || !lua_isnumber(state, 4) || !lua_isnumber(state, 5))
    {
        return 0;
    }
    addon->drawSink->rawText(addon->drawSink->context, addon->id.c_str(), text, static_cast<float>(lua_tonumber(state, 2)), static_cast<float>(lua_tonumber(state, 3)), static_cast<float>(lua_tonumber(state, 4)), static_cast<float>(lua_tonumber(state, 5)), locked, fontSize);
    return 0;
}

int AddonHost::LuaClipboardSet(lua_State* state)
{
    auto*       addon   = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    std::size_t length  = 0;
    const char* text    = lua_tolstring(state, 1, &length);
    const bool  written = addon != nullptr && addon->clipboardSink != nullptr && addon->clipboardSink->setText != nullptr && text != nullptr && addon->clipboardSink->setText(addon->clipboardSink->context, std::string_view(text, length));
    lua_pushboolean(state, written ? 1 : 0);
    return 1;
}

int AddonHost::LuaChatPrint(lua_State* state)
{
    auto*       addon   = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    std::size_t length  = 0;
    const char* text    = lua_tolstring(state, 1, &length);
    const bool  printed = addon != nullptr && addon->chatSink != nullptr && addon->chatSink->print != nullptr && text != nullptr && addon->chatSink->print(addon->chatSink->context, std::string_view(text, length));
    lua_pushboolean(state, printed ? 1 : 0);
    return 1;
}

int AddonHost::LuaOpenUrl(lua_State* state)
{
    auto*                  addon  = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    std::size_t            length = 0;
    const char*            value  = lua_tolstring(state, 1, &length);
    const std::string_view url(value == nullptr ? "" : value, length);
    const bool             opened = addon != nullptr && addon->urlSink != nullptr && addon->urlSink->open != nullptr && IsAllowedAddonUrl(url) && addon->urlSink->open(addon->urlSink->context, url);
    lua_pushboolean(state, opened ? 1 : 0);
    return 1;
}
