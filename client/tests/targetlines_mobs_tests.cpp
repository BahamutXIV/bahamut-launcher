#include "targetlines_mobs.h"

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <iostream>
#include <string_view>
#include <vector>

namespace
{

constexpr std::size_t kPayloadSize      = 0x108u;
constexpr std::size_t kClassNameOffset  = 0x24u;
constexpr std::size_t kInitParamsOffset = 0x44u;

enum class Profile
{
    OpeningEnemy,
    OpeningAlly,
    StandardNpc,
    PartialOpening,
};

void AppendBoolean(std::vector<std::uint8_t>& bytes, bool value)
{
    bytes.push_back(value ? 0x03u : 0x04u);
}

void AppendInt32(std::vector<std::uint8_t>& bytes, std::uint32_t value)
{
    bytes.push_back(0x00u);
    bytes.push_back(static_cast<std::uint8_t>((value >> 24u) & 0xFFu));
    bytes.push_back(static_cast<std::uint8_t>((value >> 16u) & 0xFFu));
    bytes.push_back(static_cast<std::uint8_t>((value >> 8u) & 0xFFu));
    bytes.push_back(static_cast<std::uint8_t>(value & 0xFFu));
}

void WriteFixedText(
    std::vector<std::uint8_t>& bytes,
    std::size_t                offset,
    std::string_view           value)
{
    for (std::size_t index = 0; index < value.size() && index < 0x20u; ++index)
    {
        bytes[offset + index] = static_cast<std::uint8_t>(value[index]);
    }
}

packet_observer::GameMessage MakeMessage(
    std::uint32_t    actorId,
    std::uint32_t    actorClassId,
    std::string_view classPath,
    std::string_view className,
    Profile          profile = Profile::OpeningEnemy)
{
    packet_observer::GameMessage message;
    message.opcode   = 0x00CCu;
    message.sourceId = actorId;
    message.payload.resize(kPayloadSize);
    WriteFixedText(message.payload, kClassNameOffset, className);

    std::vector<std::uint8_t> params;
    params.push_back(0x02u);
    for (const char value : classPath)
    {
        params.push_back(static_cast<std::uint8_t>(value));
    }
    params.push_back(0u);
    for (int index = 0; index < 5; ++index)
    {
        AppendBoolean(params, false);
    }
    AppendInt32(params, actorClassId);

    if (profile == Profile::StandardNpc)
    {
        AppendBoolean(params, false);
        AppendBoolean(params, false);
        AppendInt32(params, 0u);
        AppendInt32(params, 0u);
        params.push_back(0x0Fu);
        std::copy(params.begin(), params.end(), message.payload.begin() + kInitParamsOffset);
        return message;
    }

    AppendBoolean(params, true);
    AppendBoolean(params, true);
    AppendInt32(params, 10u);
    AppendInt32(params, 0u);
    AppendInt32(params, 1u);
    AppendBoolean(params, profile == Profile::OpeningAlly);
    for (int index = 0; index < 7; ++index)
    {
        if (profile == Profile::PartialOpening && index == 6)
        {
            std::copy(params.begin(), params.end(), message.payload.begin() + kInitParamsOffset);
            return message;
        }
        AppendBoolean(params, false);
    }
    if (profile != Profile::PartialOpening)
    {
        AppendInt32(params, 0u);
        params.push_back(0x0Fu);
    }
    std::copy(params.begin(), params.end(), message.payload.begin() + kInitParamsOffset);
    return message;
}

bool Expect(
    std::string_view name,
    bool             actual,
    bool             expected)
{
    if (actual == expected)
    {
        return true;
    }

    std::cerr << name << ": expected " << (expected ? "true" : "false")
              << ", got " << (actual ? "true" : "false") << '\n';
    return false;
}

bool Check(
    int&                                failures,
    std::string_view                    name,
    const packet_observer::GameMessage& message,
    bool                                expected)
{
    if (!Expect(name, bahamut_client::IsTargetlinesMobInstantiation(message), expected))
    {
        ++failures;
        return false;
    }
    return true;
}

} // namespace

int main()
{
    constexpr std::string_view kSheepPath =
        "/chara/npc/monster/sheep/SheepLesserStandard";
    constexpr std::string_view kJellyPath =
        "/chara/npc/monster/jellyfish/JellyfishScenarioLimsaLv00";
    constexpr std::string_view kWolfPath =
        "/chara/npc/monster/winglizard/WinglizardLesserStandard";

    int failures = 0;
    Check(
        failures,
        "Lost Lamb ambient opening",
        MakeMessage(0x44000C3Cu, 2106001u, kSheepPath, "SheepLesserStandard"),
        true);
    Check(
        failures,
        "Jellyfish ambient opening",
        MakeMessage(0x44000C3Bu, 2205403u, kJellyPath, "JellyfishScenarioLimsaLv00"),
        true);
    Check(
        failures,
        "Winglizard ambient opening",
        MakeMessage(0x44000C3Du, 2100101u, kWolfPath, "WinglizardLesserStandard"),
        true);

    Check(
        failures,
        "active ally profile",
        MakeMessage(
            0x44000C3Eu,
            2106001u,
            kSheepPath,
            "SheepLesserStandard",
            Profile::OpeningAlly),
        false);
    Check(
        failures,
        "ordinary NPC tail",
        MakeMessage(
            0x44000C3Fu,
            2106001u,
            kSheepPath,
            "SheepLesserStandard",
            Profile::StandardNpc),
        false);
    Check(
        failures,
        "partial opening tail",
        MakeMessage(
            0x44000C40u,
            2106001u,
            kSheepPath,
            "SheepLesserStandard",
            Profile::PartialOpening),
        false);
    Check(
        failures,
        "unknown monster path",
        MakeMessage(
            0x44000C41u,
            2106001u,
            "/chara/npc/monster/sheep/SheepUnknown",
            "SheepUnknown"),
        false);
    Check(
        failures,
        "class ID and path mismatch",
        MakeMessage(
            0x44000C42u,
            2100101u,
            kSheepPath,
            "SheepLesserStandard"),
        false);
    Check(
        failures,
        "class name mismatch",
        MakeMessage(
            0x44000C43u,
            2106001u,
            kSheepPath,
            "OtherClass"),
        false);
    auto badClassPadding = MakeMessage(
        0x44000C44u,
        2106001u,
        kSheepPath,
        "SheepLesserStandard");
    badClassPadding.payload[kClassNameOffset + 20u] = 1u;
    Check(failures, "nonzero class name padding", badClassPadding, false);
    auto classNameAlone = MakeMessage(
        0x44000C45u,
        2106001u,
        kSheepPath,
        "SheepLesserStandard",
        Profile::StandardNpc);
    classNameAlone.payload[kInitParamsOffset] = 0x0Fu;
    Check(failures, "class name alone", classNameAlone, false);

    auto ally = MakeMessage(
        0x44000C46u,
        2290004u,
        "/chara/npc/monster/fighter/FighterAllyOpeningAttacker",
        "FighterAllyOpeningAttacker");
    Check(failures, "Fighter ally path", ally, false);

    auto player = MakeMessage(
        11u,
        2106001u,
        kSheepPath,
        "SheepLesserStandard");
    Check(failures, "player source ID", player, false);
    auto ordinaryNpc = MakeMessage(
        0x44000C47u,
        1000001u,
        "/chara/npc/populace/PopulaceStandard",
        "PopulaceStandard");
    Check(failures, "ordinary NPC class", ordinaryNpc, false);

    auto invalidDirection = MakeMessage(
        0x44000C48u,
        2106001u,
        kSheepPath,
        "SheepLesserStandard");
    invalidDirection.direction = packet_observer::Direction::Outgoing;
    Check(failures, "outgoing instantiation", invalidDirection, false);

    auto invalidOpcode = MakeMessage(
        0x44000C49u,
        2106001u,
        kSheepPath,
        "SheepLesserStandard");
    invalidOpcode.opcode = 0x00CBu;
    Check(failures, "wrong opcode", invalidOpcode, false);

    auto invalidSize = MakeMessage(
        0x44000C4Au,
        2106001u,
        kSheepPath,
        "SheepLesserStandard");
    invalidSize.payload.push_back(0u);
    Check(failures, "wrong payload size", invalidSize, false);

    for (const std::uint32_t actorId : { 0u, 0xFFFFFFFFu, 0xC0000000u })
    {
        auto invalidSource = MakeMessage(
            actorId,
            2106001u,
            kSheepPath,
            "SheepLesserStandard");
        Check(failures, "invalid source ID", invalidSource, false);
    }

    auto badPrelude = MakeMessage(
        0x44000C4Bu,
        2106001u,
        kSheepPath,
        "SheepLesserStandard");
    badPrelude.payload[kInitParamsOffset + 1u + kSheepPath.size() + 1u] = 0x03u;
    Check(failures, "non-false prelude boolean", badPrelude, false);

    auto badTerminator = MakeMessage(
        0x44000C4Cu,
        2106001u,
        kSheepPath,
        "SheepLesserStandard");
    for (std::size_t index = kInitParamsOffset; index < badTerminator.payload.size(); ++index)
    {
        if (badTerminator.payload[index] == 0x0Fu)
        {
            badTerminator.payload[index] = 0u;
            break;
        }
    }
    Check(failures, "missing terminator", badTerminator, false);

    auto nonzeroPadding = MakeMessage(
        0x44000C4Du,
        2106001u,
        kSheepPath,
        "SheepLesserStandard");
    nonzeroPadding.payload.back() = 1u;
    Check(failures, "nonzero tail padding", nonzeroPadding, false);

    return failures == 0 ? 0 : 1;
}
