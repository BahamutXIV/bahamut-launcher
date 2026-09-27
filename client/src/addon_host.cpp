#include "addon_host.h"

#include "actor_name.h"
#include "area_names.h"
#include "player_state.h"
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
constexpr std::size_t      kMaximumQueuedPackets         = 256;
constexpr std::size_t      kMaximumQueuedPacketBytes     = 4u * 1024u * 1024u;
constexpr std::size_t      kMaximumPacketsPerUpdate      = 32;
constexpr std::size_t      kMaximumQueuedCombatEvents    = 256;
constexpr std::size_t      kMaximumCombatMeterRows       = 64;
constexpr std::size_t      kMaximumCombatMeterNameBytes  = 128;
constexpr std::size_t      kMaximumCombatMeterLabelBytes = 16;
constexpr std::size_t      kMaximumPacketLogLineBytes    = 131200;
constexpr std::uint64_t    kMaximumPacketLogFileBytes    = 16u * 1024u * 1024u;
constexpr std::string_view kPacketLogHeader              = "capture_unix_ms_utc,direction,size,frame_hex\n";
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

struct ParsedManifest
{
    std::string id;
    std::string entry;
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

std::optional<ParsedManifest> ParseManifest(const std::filesystem::path& path)
{
    std::ifstream file(path);
    if (!file)
    {
        return std::nullopt;
    }
    ParsedManifest manifest;
    std::string    line;
    while (std::getline(file, line))
    {
        const std::size_t comment = line.find('#');
        if (comment != std::string::npos)
        {
            line.resize(comment);
        }
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
    std::shared_ptr<PacketCaptureSession>  packetWriteSession;
    std::string                            id;
    AddonHost*                             owner = nullptr;
    std::map<std::string, std::string>     settings;
    lua_State*                             state                = nullptr;
    const AddonWindowSink*                 drawSink             = nullptr;
    bahamut_client::PlayerStateService*    playerState          = nullptr;
    bahamut_client::TargetDistanceService* targetDistance       = nullptr;
    const AddonClipboardSink*              clipboardSink        = nullptr;
    const AddonChatSink*                   chatSink             = nullptr;
    const AddonUrlSink*                    urlSink              = nullptr;
    bool                                   faulted              = false;
    bool                                   packetWriteCompleted = false;
};

std::optional<std::string> ReadAddonManifestId(
    const std::filesystem::path& manifestPath)
{
    const auto manifest = ParseManifest(manifestPath);
    return manifest ? std::optional<std::string>(manifest->id) : std::nullopt;
}

AddonHost::AddonHost(bahamut_client::PlayerStateService* playerState)
: playerState_(playerState)
{
}

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
    playerState_ = playerState;
    for (const auto& addon : addons_)
    {
        addon->playerState = playerState;
    }
}

void AddonHost::SetTargetDistanceService(
    bahamut_client::TargetDistanceService* targetDistance)
{
    targetDistance_ = targetDistance;
    for (const auto& addon : addons_)
    {
        addon->targetDistance = targetDistance;
    }
}

void AddonHost::SetActorNameService(bahamut_client::ActorNameService* actorNames)
{
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
    clipboardSink_ = sink;
}

void AddonHost::SetChatSink(const AddonChatSink& sink)
{
    chatSink_ = sink;
}

void AddonHost::SetUrlSink(const AddonUrlSink& sink)
{
    urlSink_ = sink;
}

bool AddonHost::Enable(const std::filesystem::path& manifestPath)
{
    const auto id = ReadAddonManifestId(manifestPath);
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
    auto addon = LoadOne(manifestPath, settingsRoot_, chatLogsRoot_, this, playerState_, targetDistance_, &clipboardSink_, &chatSink_, &urlSink_);
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
        {
            std::lock_guard lock(packetQueueMutex_);
            packetCaptureSession_.reset();
            packetCaptureSeen_    = 0;
            packetCaptureWritten_ = 0;
            packetCaptureDropped_ = 0;
            packetQueueHighWater_ = 0;
        }
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
        std::lock_guard lock(packetQueueMutex_);
        packetCaptureDropped_ += packetQueue_.size();
        packetQueue_.clear();
        packetQueueBytes_    = 0;
        packetCaptureActive_ = false;
        packetCaptureSession_.reset();
    }
    if (addonId == "combatparser")
    {
        combatParserEnabled_.store(false, std::memory_order_release);
        std::lock_guard lock(combatQueueMutex_);
        combatQueue_.clear();
        combatDropped_ = 0;
    }
    Close(**found);
    auto replacement = LoadOne(manifest, settingsRoot_, chatLogsRoot_, this, playerState_, targetDistance_, &clipboardSink_, &chatSink_, &urlSink_);
    if (replacement == nullptr)
    {
        addons_.erase(found);
        return false;
    }
    *found = std::move(replacement);
    if (addonId == "packetlogger")
    {
        {
            std::lock_guard lock(packetQueueMutex_);
            packetCaptureSession_.reset();
            packetCaptureSeen_    = 0;
            packetCaptureWritten_ = 0;
            packetCaptureDropped_ = 0;
            packetQueueHighWater_ = 0;
        }
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
    const auto found = std::find_if(addons_.begin(), addons_.end(), [&](const auto& addon)
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
        packetLoggerEnabled_.store(false, std::memory_order_release);
        std::lock_guard lock(packetQueueMutex_);
        packetCaptureDropped_ += packetQueue_.size();
        packetQueue_.clear();
        packetQueueBytes_    = 0;
        packetCaptureActive_ = false;
        packetCaptureSession_.reset();
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
    return std::any_of(addons_.begin(), addons_.end(), [&](const auto& addon)
                       {
                           return addon->id == addonId;
                       });
}

bool AddonHost::HasCommandAddons() const
{
    return IsLoaded("pos") || IsLoaded("fps") || IsLoaded("wiki") ||
           IsLoaded("distance") || IsLoaded("targethp") ||
           IsLoaded("combatparser") || IsLoaded("packetlogger");
}

bool AddonHost::DispatchCommand(std::string_view command)
{
    for (const auto& addon : addons_)
    {
        if (addon->faulted || addon->state == nullptr)
        {
            continue;
        }
        bool       handled = false;
        const bool invoked = InvokeCommand(*addon, command, handled);
        StopFaultedPacketLogger();
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
    std::lock_guard lock(areaQueueMutex_);
    queuedAreaZoneId_ = zoneId;
}

bool AddonHost::QueuePacketFrame(packet_observer::Direction    direction,
                                 std::span<const std::uint8_t> frame)
{
    if (!packetLoggerEnabled_.load(std::memory_order_acquire) ||
        frame.empty())
    {
        return false;
    }
    std::lock_guard lock(packetQueueMutex_);
    if (!packetLoggerEnabled_.load(std::memory_order_relaxed) ||
        !packetCaptureActive_)
    {
        return false;
    }
    ++packetCaptureSeen_;
    if (packetQueue_.size() >= kMaximumQueuedPackets ||
        frame.size() > kMaximumQueuedPacketBytes - packetQueueBytes_)
    {
        ++packetCaptureDropped_;
        return false;
    }
    packetQueue_.push_back(PacketFrame{ direction, std::chrono::system_clock::now(), packetCaptureSession_, { frame.begin(), frame.end() } });
    packetQueueBytes_ += frame.size();
    packetQueueHighWater_ = std::max(packetQueueHighWater_, packetQueue_.size());
    return true;
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
    if (packetLoggerEnabled_.load(std::memory_order_acquire))
    {
        std::vector<PacketFrame> packets;
        {
            std::lock_guard   lock(packetQueueMutex_);
            const std::size_t count = std::min(packetQueue_.size(), kMaximumPacketsPerUpdate);
            packets.reserve(count);
            for (std::size_t index = 0; index < count; ++index)
            {
                packetQueueBytes_ -= packetQueue_[index].bytes.size();
                packets.push_back(std::move(packetQueue_[index]));
            }
            packetQueue_.erase(packetQueue_.begin(), packetQueue_.begin() + count);
        }
        for (const auto& message : packets)
        {
            bool written = false;
            for (const auto& addon : addons_)
            {
                if (addon->id == "packetlogger" && !addon->faulted && addon->state != nullptr)
                {
                    addon->packetWriteSession   = message.session;
                    addon->packetWriteCompleted = false;
                    InvokePacket(*addon, message);
                    written = addon->packetWriteCompleted;
                    addon->packetWriteSession.reset();
                    StopFaultedPacketLogger();
                    break;
                }
            }
            if (!written)
            {
                std::lock_guard lock(packetQueueMutex_);
                ++packetCaptureDropped_;
            }
        }
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
    StopFaultedPacketLogger();
    StopFaultedCombatParser();
    DispatchAreaTransition();
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
    StopFaultedPacketLogger();
    StopFaultedCombatParser();
}

void AddonHost::StopFaultedPacketLogger()
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
    if (faulted)
    {
        packetLoggerEnabled_.store(false, std::memory_order_release);
        {
            std::lock_guard lock(packetQueueMutex_);
            packetCaptureDropped_ += packetQueue_.size();
            packetQueue_.clear();
            packetQueueBytes_    = 0;
            packetCaptureActive_ = false;
            packetCaptureSession_.reset();
        }
        if (chatSink_.print != nullptr)
        {
            chatSink_.print(chatSink_.context, "Packet capture stopped: logger callback failed");
        }
    }
}

bool AddonHost::StartPacketCapture(Instance& addon)
{
    if (addon.id != "packetlogger" || addon.faulted)
    {
        return false;
    }
    {
        std::lock_guard lock(packetQueueMutex_);
        if (packetCaptureActive_)
        {
            return true;
        }
    }
    const auto now          = std::chrono::system_clock::now();
    const auto seconds      = std::chrono::system_clock::to_time_t(now);
    const auto milliseconds = std::chrono::duration_cast<std::chrono::milliseconds>(
                                  now.time_since_epoch())
                                  .count() %
                              1000;
    std::tm    utc{};
    char       stamp[32]{};
    if (gmtime_s(&utc, &seconds) != 0 ||
        std::strftime(stamp, sizeof(stamp), "%Y-%m-%dT%H-%M-%S", &utc) == 0)
    {
        return false;
    }
    std::filesystem::path file;
    std::error_code       error;
    for (unsigned int suffix = 0; suffix < 1000; ++suffix)
    {
        char name[64]{};
        if (suffix == 0)
        {
            std::snprintf(name, sizeof(name), "capture-%s.%03lldZ.csv", stamp, milliseconds);
        }
        else
        {
            std::snprintf(name, sizeof(name), "capture-%s.%03lldZ-%u.csv", stamp, milliseconds, suffix);
        }
        file = addon.packetLogDirectory / name;
        if (!std::filesystem::exists(file, error) && !error)
        {
            break;
        }
        if (error)
        {
            return false;
        }
        file.clear();
    }
    if (file.empty())
    {
        return false;
    }
    std::ofstream output(file, std::ios::binary);
    if (!output)
    {
        return false;
    }
    output << kPacketLogHeader;
    output.close();
    if (!output.good())
    {
        return false;
    }
    std::lock_guard lock(packetQueueMutex_);
    packetCaptureSession_ = std::make_shared<PacketCaptureSession>(
        PacketCaptureSession{ file, file, kPacketLogHeader.size(), 1 });
    packetCaptureActive_ = true;
    return true;
}

bool AddonHost::StopPacketCapture()
{
    std::lock_guard lock(packetQueueMutex_);
    if (!packetCaptureActive_)
    {
        return false;
    }
    packetCaptureActive_ = false;
    return true;
}

std::size_t AddonHost::LoadedCount() const
{
    return static_cast<std::size_t>(std::count_if(addons_.begin(), addons_.end(), [](const auto& addon)
                                                  {
                                                      return !addon->faulted && addon->state != nullptr;
                                                  }));
}

std::size_t AddonHost::FaultedCount() const
{
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
    if (addon->id == "distance")
    {
        registerFunction("target_distance", &LuaTargetDistance);
    }
    if (addon->id == "targethp")
    {
        registerFunction("target_hp", &LuaTargetHp);
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
        registerFunction("packetlog_write", &LuaPacketLogWrite);
        registerFunction("packetlog_start", &LuaPacketLogStart);
        registerFunction("packetlog_stop", &LuaPacketLogStop);
        registerFunction("packetlog_status", &LuaPacketLogStatus);
    }
    if (addon->id == "combatparser")
    {
        registerFunction("combat_events", &LuaCombatEvents);
        registerFunction("combat_meter", &LuaCombatMeter);
    }
    if (addon->id == "wiki")
    {
        registerFunction("open_url", &LuaOpenUrl);
    }
    lua_setglobal(state, "bahamut");

    if (luaL_loadbuffer(state, script->data(), script->size(), manifest->entry.c_str()) != 0 || lua_pcall(state, 0, 0, 0) != 0)
    {
        lua_pop(state, 1);
        addon->faulted = true;
        return addon;
    }
    Invoke(*addon, "load");
    return addon;
}

void AddonHost::Close(Instance& addon)
{
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
    addon.faulted  = true;
    addon.drawSink = nullptr;
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
        addon.faulted  = true;
        addon.drawSink = nullptr;
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
    addon.faulted  = true;
    addon.drawSink = nullptr;
    return false;
}

bool AddonHost::InvokeAreaChanged(Instance& addon, std::uint32_t zoneId, std::string_view areaName, std::string_view regionName)
{
    lua_pushnumber(addon.state, zoneId);
    lua_pushlstring(addon.state, areaName.empty() ? "" : areaName.data(), areaName.size());
    lua_pushlstring(addon.state, regionName.empty() ? "" : regionName.data(), regionName.size());
    return Invoke(addon, "area_changed", 3);
}

bool AddonHost::InvokePacket(Instance& addon, const PacketFrame& frame)
{
    static constexpr char digits[] = "0123456789ABCDEF";
    std::string           payloadHex;
    payloadHex.reserve(frame.bytes.size() * 2u);
    for (const std::uint8_t byte : frame.bytes)
    {
        payloadHex.push_back(digits[byte >> 4u]);
        payloadHex.push_back(digits[byte & 0x0fu]);
    }
    lua_State* state = addon.state;
    lua_pushstring(state, frame.direction == packet_observer::Direction::Incoming ? "incoming" : "outgoing");
    const auto milliseconds = std::chrono::duration_cast<std::chrono::milliseconds>(
                                  frame.capturedAt.time_since_epoch())
                                  .count();
    lua_pushnumber(state, static_cast<lua_Number>(milliseconds));
    lua_pushnumber(state, static_cast<lua_Number>(frame.bytes.size()));
    lua_pushlstring(state, payloadHex.data(), payloadHex.size());
    return Invoke(addon, "packet", 4);
}

int AddonHost::LuaPacketLogWrite(lua_State* state)
{
    auto* addon = static_cast<Instance*>(
        lua_touserdata(state, lua_upvalueindex(1)));
    std::size_t length = 0;
    const char* line   = lua_tolstring(state, 1, &length);
    if (addon == nullptr || addon->id != "packetlogger" || addon->owner == nullptr ||
        addon->packetWriteSession == nullptr || addon->packetWriteCompleted || line == nullptr ||
        length == 0 || length > kMaximumPacketLogLineBytes ||
        !std::all_of(line, line + length, [](unsigned char value)
                     {
                         return value >= 0x20u && value <= 0x7eu;
                     }))
    {
        lua_pushboolean(state, 0);
        return 1;
    }
    const auto            session = addon->packetWriteSession;
    std::filesystem::path currentFile;
    std::uint64_t         currentBytes = 0;
    unsigned int          currentPart  = 0;
    {
        std::lock_guard lock(addon->owner->packetQueueMutex_);
        currentFile  = session->currentFile;
        currentBytes = session->currentBytes;
        currentPart  = session->part;
    }
    std::error_code error;
    if (!std::filesystem::exists(currentFile, error) || error)
    {
        lua_pushboolean(state, 0);
        return 1;
    }
    const std::uint64_t   rowBytes   = length + 1u;
    const bool            rotate     = currentBytes + rowBytes > kMaximumPacketLogFileBytes;
    std::filesystem::path outputFile = currentFile;
    if (rotate)
    {
        if (currentPart >= 9999u)
        {
            lua_pushboolean(state, 0);
            return 1;
        }
        char suffix[32]{};
        std::snprintf(suffix, sizeof(suffix), "-part%04u.csv", currentPart + 1u);
        outputFile = session->baseFile.parent_path() /
                     (session->baseFile.stem().string() + suffix);
        if (std::filesystem::exists(outputFile, error) || error)
        {
            lua_pushboolean(state, 0);
            return 1;
        }
    }
    std::ofstream output(outputFile, std::ios::binary | (rotate ? std::ios::trunc : std::ios::app));
    if (output)
    {
        if (rotate)
        {
            output << kPacketLogHeader;
        }
        output.write(line, static_cast<std::streamsize>(length));
        output.put('\n');
    }
    output.close();
    const bool written = output.good();
    if (written)
    {
        addon->packetWriteCompleted = true;
        std::lock_guard lock(addon->owner->packetQueueMutex_);
        session->currentFile  = std::move(outputFile);
        session->currentBytes = rotate ? kPacketLogHeader.size() + rowBytes
                                       : currentBytes + rowBytes;
        session->part         = currentPart + (rotate ? 1u : 0u);
        ++addon->owner->packetCaptureWritten_;
    }
    lua_pushboolean(state, written);
    return 1;
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

int AddonHost::LuaPacketLogStatus(lua_State* state)
{
    auto* addon = static_cast<Instance*>(lua_touserdata(state, lua_upvalueindex(1)));
    if (addon == nullptr || addon->id != "packetlogger" || addon->owner == nullptr)
    {
        lua_pushnil(state);
        return 1;
    }
    std::lock_guard lock(addon->owner->packetQueueMutex_);
    lua_createtable(state, 0, 7);
    lua_pushboolean(state, addon->owner->packetCaptureActive_);
    lua_setfield(state, -2, "recording");
    const std::string file = addon->owner->packetCaptureSession_ != nullptr
                                 ? addon->owner->packetCaptureSession_->currentFile.filename().string()
                                 : std::string();
    lua_pushlstring(state, file.data(), file.size());
    lua_setfield(state, -2, "file");
    lua_pushnumber(state, static_cast<lua_Number>(addon->owner->packetCaptureSeen_));
    lua_setfield(state, -2, "captured");
    lua_pushnumber(state, static_cast<lua_Number>(addon->owner->packetCaptureWritten_));
    lua_setfield(state, -2, "written");
    lua_pushnumber(state, static_cast<lua_Number>(addon->owner->packetCaptureDropped_));
    lua_setfield(state, -2, "dropped");
    lua_pushnumber(state, static_cast<lua_Number>(addon->owner->packetQueue_.size()));
    lua_setfield(state, -2, "queued");
    lua_pushnumber(state, static_cast<lua_Number>(addon->owner->packetQueueHighWater_));
    lua_setfield(state, -2, "queue_high_water");
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
