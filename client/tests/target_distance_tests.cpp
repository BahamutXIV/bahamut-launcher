#include "packet_observer.h"
#include "player_state.h"
#include "target_distance.h"

#include <cmath>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <limits>
#include <vector>

namespace
{

constexpr std::uint32_t kLocalActorId   = 11u;
constexpr std::uint32_t kNpcActorId     = 22u;
constexpr std::uint32_t kFriendActorId  = 33u;
constexpr std::uint32_t kUnknownActorId = 44u;
constexpr std::uint32_t kInvalidActorId = 55u;

void WriteU32(std::vector<std::uint8_t>& data, std::size_t offset, std::uint32_t value)
{
    std::memcpy(data.data() + offset, &value, sizeof(value));
}

void WriteFloat(std::vector<std::uint8_t>& data, std::size_t offset, float value)
{
    std::memcpy(data.data() + offset, &value, sizeof(value));
}

packet_observer::GameMessage MakeActorPosition(std::uint32_t actorId,
                                               float         x,
                                               float         y,
                                               float         z,
                                               std::uint16_t opcode = 0x00CEu)
{
    packet_observer::GameMessage message;
    message.direction = packet_observer::Direction::Incoming;
    message.opcode    = opcode;
    message.sourceId  = actorId;
    message.payload.resize(40u);
    WriteU32(message.payload, 4u, actorId);
    WriteFloat(message.payload, 8u, x);
    WriteFloat(message.payload, 12u, y);
    WriteFloat(message.payload, 16u, z);
    return message;
}

packet_observer::GameMessage MakeMoveActor(std::uint32_t actorId, float x, float y, float z)
{
    auto message = MakeActorPosition(actorId, x, y, z, 0x00CFu);
    // 0x00CF selects its actor from sourceId, not the CE payload actor field.
    WriteU32(message.payload, 4u, kFriendActorId);
    return message;
}

packet_observer::GameMessage MakeTarget(std::uint32_t targetId)
{
    packet_observer::GameMessage message;
    message.direction = packet_observer::Direction::Incoming;
    message.opcode    = 0x00DBu;
    message.sourceId  = kLocalActorId;
    message.payload.resize(4u);
    WriteU32(message.payload, 0u, targetId);
    return message;
}

bool ExpectDistance(bahamut_client::TargetDistanceService& service,
                    std::uint32_t                          actorId,
                    float                                  expected)
{
    const auto snapshot = service.Snapshot();
    return snapshot && snapshot->actorId == actorId && snapshot->yalms &&
           std::fabs(*snapshot->yalms - expected) < 0.001F;
}

bool ExpectNoDistance(bahamut_client::TargetDistanceService& service,
                      std::uint32_t                          actorId)
{
    const auto snapshot = service.Snapshot();
    return snapshot && snapshot->actorId == actorId && !snapshot->yalms;
}

bool ExpectPosition(bahamut_client::TargetDistanceService& service,
                    std::uint32_t                          sourceActorId,
                    std::uint32_t                          targetActorId,
                    std::uint32_t                          zoneId,
                    const bahamut_client::WorldPosition&   source,
                    const bahamut_client::WorldPosition&   target)
{
    const auto snapshot = service.PositionSnapshot();
    return snapshot && snapshot->sourceActorId == sourceActorId &&
           snapshot->targetActorId == targetActorId && snapshot->zoneId == zoneId &&
           snapshot->source.x == source.x && snapshot->source.y == source.y &&
           snapshot->source.z == source.z && snapshot->target.x == target.x &&
           snapshot->target.y == target.y && snapshot->target.z == target.z;
}

} // namespace

int main()
{
    bahamut_client::PlayerStateService    player;
    bahamut_client::TargetDistanceService targetDistance(&player, nullptr);

    targetDistance.Observe(MakeActorPosition(kNpcActorId, 3.0F, 20.0F, 4.0F));
    targetDistance.Observe(MakeActorPosition(kFriendActorId, -5.0F, 20.0F, 12.0F));
    if (targetDistance.Snapshot())
    {
        std::cerr << "target appeared before player state was available\n";
        return 1;
    }

    bahamut_client::PlayerStateSnapshot playerSnapshot;
    playerSnapshot.actorId = kLocalActorId;
    playerSnapshot.zoneId  = 130u;
    playerSnapshot.y       = 10.0F;
    if (!player.Publish(playerSnapshot))
    {
        std::cerr << "valid player state was rejected\n";
        return 1;
    }

    targetDistance.Observe(MakeTarget(kNpcActorId));
    if (!ExpectDistance(targetDistance, kNpcActorId, 5.0F))
    {
        std::cerr << "pre-snapshot NPC position was not retained\n";
        return 1;
    }
    if (!ExpectPosition(targetDistance,
                        kLocalActorId,
                        kNpcActorId,
                        130u,
                        { 0.0F, 10.0F, 0.0F },
                        { 3.0F, 20.0F, 4.0F }))
    {
        std::cerr << "full target position did not preserve source and target heights\n";
        return 1;
    }

    targetDistance.Observe(MakeTarget(kFriendActorId));
    if (!ExpectDistance(targetDistance, kFriendActorId, 13.0F))
    {
        std::cerr << "pre-snapshot friendly actor position was not retained\n";
        return 1;
    }

    const auto otherTarget = targetDistance.PositionSnapshot(kNpcActorId);
    if (!otherTarget || otherTarget->targetActorId != kNpcActorId ||
        otherTarget->target.x != 3.0F || otherTarget->target.y != 20.0F ||
        !ExpectDistance(targetDistance, kFriendActorId, 13.0F))
    {
        std::cerr << "explicit action target lookup changed selection or lost its position\n";
        return 1;
    }

    targetDistance.Observe(MakeMoveActor(kNpcActorId, 6.0F, 30.0F, 8.0F));
    targetDistance.Observe(MakeTarget(kNpcActorId));
    if (!ExpectDistance(targetDistance, kNpcActorId, 10.0F) ||
        !ExpectPosition(targetDistance,
                        kLocalActorId,
                        kNpcActorId,
                        130u,
                        { 0.0F, 10.0F, 0.0F },
                        { 6.0F, 30.0F, 8.0F }))
    {
        std::cerr << "0x00CF did not update the full target position\n";
        return 1;
    }

    targetDistance.Observe(MakeActorPosition(kLocalActorId, 0.0F, 0.0F, 0.0F));
    playerSnapshot.x = 130.0F;
    if (!player.Publish(playerSnapshot))
    {
        std::cerr << "moved player state was rejected\n";
        return 1;
    }
    targetDistance.Observe(MakeTarget(kLocalActorId));
    if (!ExpectNoDistance(targetDistance, kLocalActorId))
    {
        std::cerr << "self-target reported distance from a stale server position\n";
        return 1;
    }
    auto animatedSelfTarget   = MakeTarget(kLocalActorId);
    animatedSelfTarget.opcode = 0x00D3u;
    targetDistance.Observe(animatedSelfTarget);
    if (!ExpectNoDistance(targetDistance, kLocalActorId))
    {
        std::cerr << "animated self-target reported a distance\n";
        return 1;
    }
    playerSnapshot.x = 0.0F;
    if (!player.Publish(playerSnapshot))
    {
        std::cerr << "restored player state was rejected\n";
        return 1;
    }
    targetDistance.Observe(MakeTarget(kNpcActorId));
    if (!ExpectDistance(targetDistance, kNpcActorId, 10.0F))
    {
        std::cerr << "switching from self-target did not restore target distance\n";
        return 1;
    }

    targetDistance.Observe(MakeTarget(kUnknownActorId));
    if (!ExpectNoDistance(targetDistance, kUnknownActorId))
    {
        std::cerr << "unknown actor position was reported as a distance\n";
        return 1;
    }

    targetDistance.Observe(MakeMoveActor(kInvalidActorId,
                                         9.0F,
                                         std::numeric_limits<float>::quiet_NaN(),
                                         9.0F));
    targetDistance.Observe(MakeTarget(kInvalidActorId));
    if (!ExpectNoDistance(targetDistance, kInvalidActorId) || targetDistance.PositionSnapshot())
    {
        std::cerr << "nonfinite target Y was accepted\n";
        return 1;
    }

    targetDistance.Observe(MakeTarget(0xC0000000u));
    if (targetDistance.Snapshot())
    {
        std::cerr << "invalid target did not clear the target snapshot\n";
        return 1;
    }

    auto added     = packet_observer::GameMessage{};
    added.opcode   = 0x00CAu;
    added.sourceId = kNpcActorId;
    added.payload.resize(8u);
    targetDistance.Observe(added);
    if (targetDistance.PositionSnapshot(kNpcActorId))
    {
        std::cerr << "actor ID reuse retained the preceding lifetime's position\n";
        return 1;
    }
    targetDistance.Observe(MakeActorPosition(kNpcActorId, 3.0F, 40.0F, 4.0F));
    auto initialized     = packet_observer::GameMessage{};
    initialized.opcode   = 0x00CCu;
    initialized.sourceId = kNpcActorId;
    initialized.payload.resize(0x108u);
    targetDistance.Observe(initialized);
    const auto freshPosition = targetDistance.PositionSnapshot(kNpcActorId);
    if (!freshPosition || freshPosition->target.y != 40.0F)
    {
        std::cerr << "CC init dropped the position recorded during the spawn\n";
        return 1;
    }

    auto setMap      = packet_observer::GameMessage{};
    setMap.direction = packet_observer::Direction::Incoming;
    setMap.opcode    = 0x0005u;
    setMap.sourceId  = kLocalActorId;
    setMap.payload.resize(16u);
    WriteU32(setMap.payload, 4u, 131u);
    player.Observe(setMap);
    targetDistance.Observe(setMap);
    if (targetDistance.Snapshot())
    {
        std::cerr << "zone change retained the previous target snapshot\n";
        return 1;
    }
    if (targetDistance.PositionSnapshot() || targetDistance.PositionSnapshot(kFriendActorId))
    {
        std::cerr << "zone change retained the previous full target position\n";
        return 1;
    }

    targetDistance.Observe(MakeActorPosition(kNpcActorId, 6.0F, 30.0F, 8.0F));
    player.Observe(setMap);
    targetDistance.Observe(setMap);
    playerSnapshot.zoneId = 131u;
    if (!player.Publish(playerSnapshot))
    {
        std::cerr << "valid player state after repeated SetMap was rejected\n";
        return 1;
    }
    targetDistance.Observe(MakeTarget(kNpcActorId));
    if (!ExpectDistance(targetDistance, kNpcActorId, 10.0F))
    {
        std::cerr << "repeated SetMap cleared a position from its pending zone\n";
        return 1;
    }

    WriteU32(setMap.payload, 4u, 132u);
    player.Observe(setMap);
    targetDistance.Observe(setMap);
    targetDistance.Observe(MakeActorPosition(kNpcActorId, 3.0F, 40.0F, 4.0F));
    WriteU32(setMap.payload, 4u, 133u);
    player.Observe(setMap);
    targetDistance.Observe(setMap);
    playerSnapshot.zoneId = 133u;
    if (!player.Publish(playerSnapshot))
    {
        std::cerr << "valid player state after a second zone change was rejected\n";
        return 1;
    }
    targetDistance.Observe(MakeTarget(kNpcActorId));
    if (!ExpectNoDistance(targetDistance, kNpcActorId))
    {
        std::cerr << "different pending zone retained an old position\n";
        return 1;
    }
    if (targetDistance.PositionSnapshot())
    {
        std::cerr << "different pending zone retained an old full target position\n";
        return 1;
    }

    player.Clear();
    if (targetDistance.Snapshot())
    {
        std::cerr << "logout retained the previous target snapshot\n";
        return 1;
    }
    if (targetDistance.PositionSnapshot())
    {
        std::cerr << "logout retained the previous full target position\n";
        return 1;
    }
    targetDistance.Observe(MakeActorPosition(kNpcActorId, 3.0F, 50.0F, 4.0F));
    if (!player.Publish(playerSnapshot))
    {
        std::cerr << "player state after logout was rejected\n";
        return 1;
    }
    targetDistance.Observe(MakeTarget(kNpcActorId));
    if (!ExpectNoDistance(targetDistance, kNpcActorId))
    {
        std::cerr << "logout allowed an unscoped position to cross sessions\n";
        return 1;
    }
    if (targetDistance.PositionSnapshot())
    {
        std::cerr << "logout allowed an unscoped full position to cross sessions\n";
        return 1;
    }

    targetDistance.Observe(MakeTarget(kNpcActorId));
    auto despawn      = packet_observer::GameMessage{};
    despawn.direction = packet_observer::Direction::Incoming;
    despawn.opcode    = 0x0114u;
    despawn.sourceId  = kNpcActorId;
    targetDistance.Observe(despawn);
    if (targetDistance.Snapshot())
    {
        std::cerr << "despawn did not clear the target snapshot\n";
        return 1;
    }
    if (targetDistance.PositionSnapshot() || targetDistance.PositionSnapshot(kNpcActorId))
    {
        std::cerr << "despawn did not clear the full target position\n";
        return 1;
    }

    return 0;
}
