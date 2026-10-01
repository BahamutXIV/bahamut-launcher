#include "targetlines.h"

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <string_view>
#include <vector>

namespace
{

constexpr std::uint32_t kPlayer = 11u;
constexpr std::uint32_t kAlly   = 22u;
constexpr std::uint32_t kMob    = 0x44000003u;
using Clock                     = bahamut_client::TargetlinesService::Clock;

void WriteU32(std::vector<std::uint8_t>& data, std::size_t offset, std::uint32_t value)
{
    std::memcpy(data.data() + offset, &value, sizeof(value));
}

void WriteU16(std::vector<std::uint8_t>& data, std::size_t offset, std::uint16_t value)
{
    std::memcpy(data.data() + offset, &value, sizeof(value));
}

packet_observer::GameMessage Position(std::uint32_t actor, float x)
{
    packet_observer::GameMessage message;
    message.opcode   = 0x00CEu;
    message.sourceId = actor;
    message.payload.resize(40u);
    WriteU32(message.payload, 4u, actor);
    std::memcpy(message.payload.data() + 8u, &x, sizeof(x));
    return message;
}

packet_observer::GameMessage Target(std::uint32_t actor)
{
    packet_observer::GameMessage message;
    message.opcode   = 0x00DBu;
    message.sourceId = kPlayer;
    message.payload.resize(4u);
    WriteU32(message.payload, 0u, actor);
    return message;
}

packet_observer::GameMessage Mob(std::uint32_t actor, bool allyProfile = false)
{
    // Lost Lamb CC fixture: current Bahamut ambient mob spawn profile.
    constexpr std::string_view   path = "/chara/npc/monster/sheep/SheepLesserStandard";
    constexpr std::string_view   name = "SheepLesserStandard";
    packet_observer::GameMessage message;
    message.opcode   = 0x00CCu;
    message.sourceId = actor;
    message.payload.resize(0x108u);
    std::copy(name.begin(), name.end(), message.payload.begin() + 0x24u);
    std::vector<std::uint8_t> params{ 0x02u };
    params.insert(params.end(), path.begin(), path.end());
    params.push_back(0u);
    params.insert(params.end(), 5u, 0x04u);
    constexpr std::uint32_t classId = 2106001u;
    params.insert(params.end(), { 0x00u, static_cast<std::uint8_t>(classId >> 24u), static_cast<std::uint8_t>(classId >> 16u), static_cast<std::uint8_t>(classId >> 8u), static_cast<std::uint8_t>(classId), 0x03u, 0x03u, 0x00u, 0u, 0u, 0u, 10u, 0x00u, 0u, 0u, 0u, 0u, 0x00u, 0u, 0u, 0u, 1u, static_cast<std::uint8_t>(allyProfile ? 0x03u : 0x04u) });
    params.insert(params.end(), 7u, 0x04u);
    params.insert(params.end(), { 0x00u, 0u, 0u, 0u, 0u, 0x0Fu });
    std::copy(params.begin(), params.end(), message.payload.begin() + 0x44u);
    return message;
}

packet_observer::GameMessage Result(std::uint16_t command, std::uint32_t target, bool start)
{
    packet_observer::GameMessage message;
    message.opcode   = 0x0139u;
    message.sourceId = kPlayer;
    message.payload.resize(0x38u);
    WriteU32(message.payload, 0u, kPlayer);
    WriteU32(message.payload, 4u, start ? 0x6F000003u : 0u);
    WriteU32(message.payload, 0x20u, 1u);
    WriteU16(message.payload, 0x24u, command);
    WriteU32(message.payload, 0x28u, target);
    WriteU16(message.payload, 0x2Eu, start ? 30128u : 0u);
    message.payload[0x34u] = start ? 1u : 0u;
    message.payload[0x35u] = 1u;
    return message;
}

packet_observer::GameMessage Deadline(std::uint16_t command, std::uint32_t duration)
{
    packet_observer::GameMessage message;
    message.opcode    = 0x0137u;
    message.sourceId  = kPlayer;
    message.timestamp = 1000u;
    message.payload.resize(0xA8u);
    auto& data = message.payload;
    data[1u]   = 4u;
    WriteU32(data, 2u, 0xF683A451u);
    WriteU32(data, 6u, command);
    data[10u] = 4u;
    WriteU32(data, 11u, 0x59C40D5Du);
    WriteU32(data, 15u, message.timestamp + duration);
    constexpr std::string_view target = "playerWork/castState";
    data[19u]                         = 0x96u;
    std::memcpy(data.data() + 20u, target.data(), target.size());
    data[0] = static_cast<std::uint8_t>(19u + target.size());
    return message;
}

bool Green(const bahamut_client::TargetlinesSnapshot& frame, std::uint32_t actor)
{
    for (std::size_t index = 0; index < frame.count; ++index)
    {
        if (frame.arcs[index].friendly && frame.arcs[index].positions.targetActorId == actor)
        {
            return true;
        }
    }
    return false;
}

bool Red(const bahamut_client::TargetlinesSnapshot& frame, std::uint32_t actor)
{
    for (std::size_t index = 0; index < frame.count; ++index)
    {
        if (!frame.arcs[index].friendly && frame.arcs[index].positions.targetActorId == actor)
        {
            return true;
        }
    }
    return false;
}

} // namespace

int main()
{
    bahamut_client::PlayerStateService    player;
    bahamut_client::TargetDistanceService targets(&player, nullptr);
    bahamut_client::TargetlinesService    lines(&player, &targets);
    bahamut_client::PlayerStateSnapshot   state;
    state.actorId = kPlayer;
    state.zoneId  = 128u;
    player.Publish(state);
    targets.Observe(Position(kPlayer, 5.0F));
    targets.Observe(Position(kAlly, 10.0F));
    targets.Observe(Position(kMob, 20.0F));
    auto now = Clock::time_point{} + std::chrono::seconds(10);
    targets.Observe(Target(kPlayer));
    if (lines.Snapshot(now).count != 0u)
    {
        std::cerr << "self selection drew despite a differing cached self position\n";
        return 1;
    }
    targets.Observe(Target(kAlly));
    if (lines.Snapshot(now).count != 0u)
    {
        std::cerr << "ordinary friendly selection created an arc\n";
        return 1;
    }
    targets.Observe(Target(kMob));
    if (lines.Snapshot(now).count != 0u)
    {
        std::cerr << "NPC ID band alone was treated as a verified mob\n";
        return 1;
    }
    lines.Observe(Mob(kMob), now);
    if (!Red(lines.Snapshot(now), kMob))
    {
        std::cerr << "verified selected Lost Lamb did not create a red arc\n";
        return 1;
    }
    lines.Observe(Result(27346u, kAlly, true), now);
    auto frame = lines.Snapshot(now);
    if (frame.count != 2u || !Red(frame, kMob) || !Green(frame, kAlly) || frame.arcs[0].positions.sourceActorId != kPlayer)
    {
        std::cerr << "accepted ally heal cast did not create a green arc\n";
        return 1;
    }
    lines.SetCastingEnabled(false);
    lines.Observe(Deadline(27346u, 5u), now);
    lines.Observe(Result(27346u, kAlly, true), now);
    if (!Red(lines.Snapshot(now), kMob) || Green(lines.Snapshot(now), kAlly))
    {
        std::cerr << "disabled casting lost a stationary mob or retained an ally cast\n";
        return 1;
    }
    lines.SetCastingEnabled(true);
    if (!Red(lines.Snapshot(now), kMob) || Green(lines.Snapshot(now), kAlly))
    {
        std::cerr << "reenabling inherited a cast from the disabled interval\n";
        return 1;
    }
    lines.Observe(Result(27346u, kAlly, true), now);
    if (Green(lines.Snapshot(now + std::chrono::seconds(4)), kAlly))
    {
        std::cerr << "disabled cast deadline was reused on reenabling\n";
        return 1;
    }
    lines.Observe(Result(27346u, kAlly, true), now);
    targets.Observe(Target(kPlayer));
    if (!Green(lines.Snapshot(now), kAlly))
    {
        std::cerr << "selection changed the accepted cast target\n";
        return 1;
    }
    auto unrelated = Result(27351u, kAlly, false);
    lines.Observe(unrelated, now);
    if (!Green(lines.Snapshot(now), kAlly))
    {
        std::cerr << "unmatched completion cleared the active cast\n";
        return 1;
    }
    auto completed   = Result(27346u, kAlly, false);
    completed.opcode = 0x013Cu;
    completed.payload.resize(0x28u);
    WriteU32(completed.payload, 0x20u, 0u);
    lines.Observe(completed, now);
    now += std::chrono::milliseconds(125);
    frame = lines.Snapshot(now);
    if (!Green(frame, kAlly) || frame.arcs[0].opacity >= 1.0F || frame.arcs[0].opacity <= 0.0F ||
        lines.Snapshot(now + std::chrono::milliseconds(126)).count != 0u)
    {
        std::cerr << "zero-row completion did not end with a bounded fade\n";
        return 1;
    }
    lines.Observe(Result(27351u, kAlly, true), now);
    if (!Green(lines.Snapshot(now), kAlly))
    {
        std::cerr << "accepted ally buff was not eligible\n";
        return 1;
    }
    auto interrupted = Result(27351u, kPlayer, false);
    WriteU32(interrupted.payload, 4u, 0x7F000002u);
    WriteU16(interrupted.payload, 0x2Eu, 30201u);
    lines.Observe(interrupted, now);
    if (lines.Snapshot(now).count != 0u)
    {
        std::cerr << "interruption retained its ally arc\n";
        return 1;
    }
    lines.Observe(Result(27346u, kPlayer, true), now);
    if (lines.Snapshot(now).count != 0u)
    {
        std::cerr << "self heal cast created a green arc\n";
        return 1;
    }
    lines.Observe(Result(999u, kAlly, true), now);
    auto outgoing      = Result(27346u, kAlly, true);
    outgoing.direction = packet_observer::Direction::Outgoing;
    lines.Observe(outgoing, now);
    auto malformed = Result(27346u, kAlly, true);
    malformed.payload.pop_back();
    lines.Observe(malformed, now);
    if (lines.Snapshot(now).count != 0u)
    {
        std::cerr << "self, unknown, outgoing or malformed cast created an arc\n";
        return 1;
    }
    lines.Observe(Result(27346u, kAlly, true), now);
    if (lines.Snapshot(now + std::chrono::seconds(4)).count != 0u)
    {
        std::cerr << "missing completion left an unbounded arc\n";
        return 1;
    }
    auto wrongGroup         = Deadline(27346u, 5u);
    wrongGroup.payload[20u] = 'X';
    lines.Observe(wrongGroup, now);
    lines.Observe(Result(27346u, kAlly, true), now);
    if (lines.Snapshot(now + std::chrono::seconds(4)).count != 0u)
    {
        std::cerr << "unrelated property group changed the cast deadline\n";
        return 1;
    }
    lines.Observe(Deadline(27346u, 5u), now);
    lines.Observe(Result(27346u, kAlly, true), now);
    if (!Green(lines.Snapshot(now + std::chrono::seconds(4)), kAlly) ||
        lines.Snapshot(now + std::chrono::seconds(6)).count != 0u)
    {
        std::cerr << "verified server deadline was ignored\n";
        return 1;
    }
    lines.Observe(Result(27346u, kAlly, true), now);
    packet_observer::GameMessage removed;
    removed.opcode   = 0x0114u;
    removed.sourceId = kAlly;
    targets.Observe(removed);
    lines.Observe(removed, now);
    if (lines.Snapshot(now).count != 0u)
    {
        std::cerr << "despawn retained an ally cast relationship\n";
        return 1;
    }
    targets.Observe(Position(kAlly, 10.0F));
    lines.Observe(Result(27346u, kAlly, true), now);
    auto added     = packet_observer::GameMessage{};
    added.opcode   = 0x00CAu;
    added.sourceId = kAlly;
    added.payload.resize(8u);
    targets.Observe(added);
    lines.Observe(added, now);
    targets.Observe(Position(kAlly, 12.0F));
    if (Green(lines.Snapshot(now), kAlly))
    {
        std::cerr << "actor ID reuse retained its previous ally cast\n";
        return 1;
    }
    targets.Observe(Target(kMob));
    if (!Red(lines.Snapshot(now), kMob))
    {
        std::cerr << "an unrelated actor birth cleared the selected mob\n";
        return 1;
    }
    const auto previousMobSequence = lines.Snapshot(now).arcs[0].relationshipSequence;
    added.sourceId                 = kMob;
    targets.Observe(added);
    lines.Observe(added, now);
    targets.Observe(Position(kMob, 30.0F));
    targets.Observe(Target(kMob));
    if (Red(lines.Snapshot(now), kMob))
    {
        std::cerr << "new actor lifetime inherited its old mob classification\n";
        return 1;
    }
    lines.Observe(Mob(kMob, true), now);
    if (Red(lines.Snapshot(now), kMob))
    {
        std::cerr << "ally init profile created a red selection arc\n";
        return 1;
    }
    lines.Observe(Mob(kMob), now);
    frame = lines.Snapshot(now);
    if (!Red(frame, kMob) || frame.arcs[0].positions.target.x != 30.0F ||
        frame.arcs[0].relationshipSequence == previousMobSequence)
    {
        std::cerr << "fresh mob init lost its new position or reused the old relationship token\n";
        return 1;
    }
    targets.Observe(Target(0xC0000000u));
    if (lines.Snapshot(now).count != 0u)
    {
        std::cerr << "clearing selection retained its red arc\n";
        return 1;
    }
    lines.Observe(Result(27346u, kAlly, true), now);
    auto map     = packet_observer::GameMessage{};
    map.opcode   = 0x0005u;
    map.sourceId = kPlayer;
    map.payload.resize(16u);
    WriteU32(map.payload, 4u, 129u);
    player.Observe(map);
    targets.Observe(map);
    lines.Observe(map, now);
    if (lines.Snapshot(now).count != 0u)
    {
        std::cerr << "zone transition retained an ally cast\n";
        return 1;
    }
    state.zoneId = 129u;
    player.Publish(state);
    targets.Observe(Position(kMob, 30.0F));
    targets.Observe(Target(kMob));
    if (Red(lines.Snapshot(now), kMob))
    {
        std::cerr << "zone transition retained a prior mob classification\n";
        return 1;
    }
    targets.Observe(Position(kAlly, 12.0F));
    lines.Observe(Result(27346u, kAlly, true), now);
    player.Clear();
    if (lines.Snapshot(now).count != 0u)
    {
        std::cerr << "logout retained a cast relationship\n";
        return 1;
    }
    return 0;
}
