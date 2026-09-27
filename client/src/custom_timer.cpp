#include "custom_timer.h"

#include <algorithm>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace
{

// XIVLegacy/xivl-opcodes:opcodes.json, MapClientbound 0x0133 GenericDataPacket.
// This catalog entry identifies the carrier, not Bahamut's timer protocol.
constexpr std::uint16_t kGenericDataOpcode = 0x0133u;
// Bahamut-specific tags and Lua parameter encoding; see docs/extensions.md#custom-timer.
constexpr std::int32_t     kOpenOperation                  = 0x49575401;
constexpr std::int32_t     kCloseOperation                 = 0x49575402;
constexpr std::int32_t     kUpdateObjectiveOperation       = 0x49575403;
constexpr std::size_t      kMaximumGenericDataPayloadBytes = 0xC0u;
constexpr std::size_t      kMaximumTextBytes               = 64u;
constexpr std::string_view kDefaultTitle                   = "Instanced Content";
constexpr std::string_view kDefaultObjective               = "Custom Objective";

enum class TimerOperation
{
    Open,
    Close,
    UpdateObjective,
};

struct ParsedTimerOperation
{
    TimerOperation operation = TimerOperation::Close;
    std::string    text;
    std::int32_t   finishTimestamp = 0;
};

bool IsValidUtf8(std::string_view text)
{
    std::size_t index = 0;
    while (index < text.size())
    {
        const auto first = static_cast<unsigned char>(text[index]);
        if (first <= 0x7Fu)
        {
            ++index;
            continue;
        }

        std::size_t   sequenceLength = 0;
        std::uint32_t codePoint      = 0;
        std::uint32_t minimumValue   = 0;
        if (first >= 0xC2u && first <= 0xDFu)
        {
            sequenceLength = 2u;
            codePoint      = first & 0x1Fu;
            minimumValue   = 0x80u;
        }
        else if (first >= 0xE0u && first <= 0xEFu)
        {
            sequenceLength = 3u;
            codePoint      = first & 0x0Fu;
            minimumValue   = 0x800u;
        }
        else if (first >= 0xF0u && first <= 0xF4u)
        {
            sequenceLength = 4u;
            codePoint      = first & 0x07u;
            minimumValue   = 0x10000u;
        }
        else
        {
            return false;
        }

        if (text.size() - index < sequenceLength)
        {
            return false;
        }
        for (std::size_t offset = 1u; offset < sequenceLength; ++offset)
        {
            const auto continuation = static_cast<unsigned char>(text[index + offset]);
            if ((continuation & 0xC0u) != 0x80u)
            {
                return false;
            }
            codePoint = (codePoint << 6u) | (continuation & 0x3Fu);
        }
        if (codePoint < minimumValue || codePoint > 0x10FFFFu ||
            (codePoint >= 0xD800u && codePoint <= 0xDFFFu))
        {
            return false;
        }
        index += sequenceLength;
    }
    return true;
}

class LuaParamReader
{
public:
    explicit LuaParamReader(const std::vector<std::uint8_t>& payload)
    : payload_(payload)
    {
    }

    bool ReadInt32(std::int32_t& value)
    {
        if (position_ >= payload_.size() || payload_[position_] != 0x00u ||
            payload_.size() - position_ < 5u)
        {
            return false;
        }

        ++position_;
        const std::uint32_t raw =
            (static_cast<std::uint32_t>(payload_[position_]) << 24u) |
            (static_cast<std::uint32_t>(payload_[position_ + 1u]) << 16u) |
            (static_cast<std::uint32_t>(payload_[position_ + 2u]) << 8u) |
            static_cast<std::uint32_t>(payload_[position_ + 3u]);
        position_ += 4u;

        const std::int64_t signedValue = raw <= static_cast<std::uint32_t>(std::numeric_limits<std::int32_t>::max())
                                             ? static_cast<std::int64_t>(raw)
                                             : static_cast<std::int64_t>(raw) - 0x100000000LL;
        value                          = static_cast<std::int32_t>(signedValue);
        return true;
    }

    bool ReadString(std::string& value)
    {
        if (position_ >= payload_.size() || payload_[position_] != 0x02u)
        {
            return false;
        }

        ++position_;
        const std::size_t start = position_;
        while (position_ < payload_.size() && payload_[position_] != 0u &&
               position_ - start <= kMaximumTextBytes)
        {
            ++position_;
        }
        if (position_ >= payload_.size() || position_ - start > kMaximumTextBytes)
        {
            return false;
        }

        value.assign(reinterpret_cast<const char*>(payload_.data() + start), position_ - start);
        ++position_;
        return IsValidUtf8(value);
    }

    bool ReadTerminatorAndPadding()
    {
        if (position_ >= payload_.size() || payload_[position_] != 0x0Fu)
        {
            return false;
        }

        ++position_;
        return std::all_of(payload_.begin() + static_cast<std::ptrdiff_t>(position_),
                           payload_.end(),
                           [](std::uint8_t byte)
                           {
                               return byte == 0u;
                           });
    }

private:
    const std::vector<std::uint8_t>& payload_;
    std::size_t                      position_ = 0;
};

bool ParseTimerOperation(const std::vector<std::uint8_t>& payload, ParsedTimerOperation& parsed)
{
    if (payload.empty() || payload.size() > kMaximumGenericDataPayloadBytes)
    {
        return false;
    }

    LuaParamReader reader(payload);
    std::int32_t   operation = 0;
    if (!reader.ReadInt32(operation))
    {
        return false;
    }

    if (operation == kOpenOperation)
    {
        parsed.operation = TimerOperation::Open;
        return reader.ReadString(parsed.text) && reader.ReadInt32(parsed.finishTimestamp) &&
               reader.ReadTerminatorAndPadding();
    }
    if (operation == kCloseOperation)
    {
        parsed.operation = TimerOperation::Close;
        return reader.ReadTerminatorAndPadding();
    }
    if (operation == kUpdateObjectiveOperation)
    {
        parsed.operation = TimerOperation::UpdateObjective;
        return reader.ReadString(parsed.text) && reader.ReadTerminatorAndPadding();
    }
    return false;
}

std::string NormalizeText(std::string_view text)
{
    std::string normalized;
    normalized.reserve(text.size());
    bool pendingSpace = false;
    for (const char rawCharacter : text)
    {
        const auto character = static_cast<unsigned char>(rawCharacter);
        if (character <= 0x20u || character == 0x7Fu)
        {
            pendingSpace = !normalized.empty();
            continue;
        }
        if (pendingSpace)
        {
            normalized.push_back(' ');
            pendingSpace = false;
        }
        normalized.push_back(static_cast<char>(character));
    }
    return normalized;
}

std::pair<std::string, std::string> SplitTitleAndObjective(std::string_view input)
{
    const std::string normalized = NormalizeText(input);
    const std::size_t separator  = normalized.find('|');
    if (separator == std::string::npos)
    {
        return { std::string(kDefaultTitle), normalized.empty() ? std::string(kDefaultObjective) : normalized };
    }

    std::string title     = NormalizeText(std::string_view(normalized).substr(0u, separator));
    std::string objective = NormalizeText(std::string_view(normalized).substr(separator + 1u));
    if (title.empty())
    {
        title = kDefaultTitle;
    }
    if (objective.empty())
    {
        objective = kDefaultObjective;
    }
    return { std::move(title), std::move(objective) };
}

std::string NormalizeObjective(std::string_view input)
{
    std::string objective = NormalizeText(input);
    if (objective.empty())
    {
        objective = kDefaultObjective;
    }
    return objective;
}

} // namespace

namespace bahamut_client
{

void CustomTimerService::Observe(const packet_observer::GameMessage& message)
{
    if (message.direction != packet_observer::Direction::Incoming || message.opcode != kGenericDataOpcode)
    {
        return;
    }

    ParsedTimerOperation parsed;
    if (!ParseTimerOperation(message.payload, parsed))
    {
        return;
    }

    if (parsed.operation == TimerOperation::Open)
    {
        const std::int64_t packetTimestamp = static_cast<std::int64_t>(message.timestamp);
        if (packetTimestamp <= 0 || parsed.finishTimestamp <= 0)
        {
            return;
        }
        const std::int64_t remainingSeconds =
            static_cast<std::int64_t>(parsed.finishTimestamp) - packetTimestamp;
        if (remainingSeconds <= 0)
        {
            return;
        }

        auto [title, objective]              = SplitTitleAndObjective(parsed.text);
        const auto                  deadline = std::chrono::steady_clock::now() + std::chrono::seconds(remainingSeconds);
        std::lock_guard<std::mutex> lock(mutex_);
        title_     = std::move(title);
        objective_ = std::move(objective);
        deadline_  = deadline;
        active_    = true;
        return;
    }

    std::lock_guard<std::mutex> lock(mutex_);
    if (parsed.operation == TimerOperation::Close)
    {
        ClearLocked();
        return;
    }

    if (!active_)
    {
        return;
    }
    if (std::chrono::steady_clock::now() >= deadline_)
    {
        ClearLocked();
        return;
    }
    objective_ = NormalizeObjective(parsed.text);
}

std::optional<CustomTimerSnapshot> CustomTimerService::Snapshot()
{
    std::lock_guard<std::mutex> lock(mutex_);
    if (!active_)
    {
        return std::nullopt;
    }

    const auto now = std::chrono::steady_clock::now();
    if (now >= deadline_)
    {
        ClearLocked();
        return std::nullopt;
    }

    const auto remainingSeconds = std::chrono::ceil<std::chrono::seconds>(deadline_ - now).count();
    return CustomTimerSnapshot{ title_, objective_, static_cast<std::uint32_t>(remainingSeconds) };
}

void CustomTimerService::ClearLocked()
{
    active_ = false;
    title_.clear();
    objective_.clear();
    deadline_ = {};
}

} // namespace bahamut_client
