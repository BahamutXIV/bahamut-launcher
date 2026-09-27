#include "custom_timer.h"

#include <cstdint>
#include <iostream>
#include <stdexcept>
#include <string>
#include <string_view>
#include <vector>

namespace
{

using bahamut_client::CustomTimerService;
using packet_observer::GameMessage;

void Check(bool condition, const char* message)
{
    if (!condition)
    {
        throw std::runtime_error(message);
    }
}

// Synthetic Bahamut tags and parameters, not captured retail timer traffic.
void AppendInt(std::vector<std::uint8_t>& payload, std::uint32_t value)
{
    payload.push_back(0u);
    for (int shift = 24; shift >= 0; shift -= 8)
    {
        payload.push_back(static_cast<std::uint8_t>(value >> shift));
    }
}

GameMessage MakeOperation(std::uint32_t tag, std::string_view text = {}, std::uint32_t finish = 1120u)
{
    GameMessage message;
    message.opcode    = 0x0133u;
    message.timestamp = 1000u;
    AppendInt(message.payload, tag);
    if (tag == 0x49575401u || tag == 0x49575403u)
    {
        message.payload.push_back(0x02u);
        message.payload.insert(message.payload.end(), text.begin(), text.end());
        message.payload.push_back(0u);
    }
    if (tag == 0x49575401u)
    {
        AppendInt(message.payload, finish);
    }
    message.payload.push_back(0x0Fu);
    return message;
}

void ExpectTimer(CustomTimerService& timer, std::string_view title, std::string_view objective)
{
    const auto snapshot = timer.Snapshot();
    Check(snapshot.has_value(), "active timer was lost");
    Check(snapshot->title == title && snapshot->objective == objective, "timer text changed unexpectedly");
    Check(snapshot->remainingSeconds > 0u && snapshot->remainingSeconds <= 120u,
          "timer did not use the packet-relative deadline");
}

void Lifecycle()
{
    CustomTimerService timer;
    Check(!timer.Snapshot(), "timer started active");
    timer.Observe(MakeOperation(0x49575403u, "Ignored before open"));
    Check(!timer.Snapshot(), "objective update opened a timer");

    auto open = MakeOperation(0x49575401u, " Trial\t | Defeat\n foes ");
    open.payload.resize(0xC0u, 0u);
    timer.Observe(open);
    ExpectTimer(timer, "Trial", "Defeat foes");
    const auto beforeUpdate = timer.Snapshot()->remainingSeconds;
    timer.Observe(MakeOperation(0x49575403u, " Next\t objective "));
    ExpectTimer(timer, "Trial", "Next objective");
    Check(timer.Snapshot()->remainingSeconds <= beforeUpdate, "objective update extended the deadline");

    timer.Observe(MakeOperation(0x49575401u, "Replacement"));
    ExpectTimer(timer, "Instanced Content", "Replacement");
    timer.Observe(MakeOperation(0x49575402u));
    Check(!timer.Snapshot(), "close retained a timer");
    timer.Observe(MakeOperation(0x49575403u, "Late objective"));
    Check(!timer.Snapshot(), "late objective revived a closed timer");
}

void InvalidInputAndIsolation()
{
    CustomTimerService timer;
    timer.Observe(MakeOperation(0x49575401u, "Trial|Original"));

    for (const auto& valid : { MakeOperation(0x49575401u, "Replacement"),
                               MakeOperation(0x49575403u, "Replacement"),
                               MakeOperation(0x49575402u) })
    {
        for (std::size_t length = 0; length < valid.payload.size(); ++length)
        {
            auto truncated = valid;
            truncated.payload.resize(length);
            timer.Observe(truncated);
            ExpectTimer(timer, "Trial", "Original");
        }
    }

    std::vector<GameMessage> invalid;
    auto                     wrongType = MakeOperation(0x49575402u);
    wrongType.payload[0]               = 0x02u;
    invalid.push_back(wrongType);
    auto badPadding = MakeOperation(0x49575402u);
    badPadding.payload.push_back(1u);
    invalid.push_back(badPadding);
    auto oversized = MakeOperation(0x49575402u);
    oversized.payload.resize(0xC1u, 0u);
    invalid.push_back(oversized);
    invalid.push_back(MakeOperation(0x49575403u, std::string(65u, 'x')));
    invalid.push_back(MakeOperation(0x49575403u, "\xC0\xAF"));
    invalid.push_back(MakeOperation(0x49575401u, "Expired", 1000u));
    invalid.push_back(MakeOperation(0x49575401u, "Negative", 0xFFFFFFFFu));
    auto noTimestamp      = MakeOperation(0x49575401u, "No timestamp");
    noTimestamp.timestamp = 0u;
    invalid.push_back(noTimestamp);
    for (const auto& message : invalid)
    {
        timer.Observe(message);
        ExpectTimer(timer, "Trial", "Original");
    }

    timer.Observe(MakeOperation(0x49575404u));
    timer.Observe(MakeOperation(0x12345602u));
    auto outgoing      = MakeOperation(0x49575402u);
    outgoing.direction = packet_observer::Direction::Outgoing;
    timer.Observe(outgoing);
    auto otherOpcode   = MakeOperation(0x49575402u);
    otherOpcode.opcode = 0x0134u;
    timer.Observe(otherOpcode);
    ExpectTimer(timer, "Trial", "Original");
    timer.Observe(MakeOperation(0x49575402u));
    Check(!timer.Snapshot(), "valid close failed after rejected inputs");
}

} // namespace

int main()
{
    try
    {
        Lifecycle();
        InvalidInputAndIsolation();
    }
    catch (const std::exception& error)
    {
        std::cerr << error.what() << '\n';
        return 1;
    }
    return 0;
}
