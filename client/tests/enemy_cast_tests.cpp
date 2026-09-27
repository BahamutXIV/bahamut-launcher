#include "enemy_cast.h"
#include "packet_observer.h"
#include "player_state.h"

#include <array>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <stdexcept>
#include <vector>

namespace
{

constexpr std::uint32_t kPlayer = 0x10000001u;
constexpr std::uint32_t kEnemy  = 0x40000001u;

using bahamut_client::EnemyCastService;
using packet_observer::GameMessage;

void Check(bool condition, const char* message)
{
    if (!condition)
    {
        throw std::runtime_error(message);
    }
}

void WriteU32(std::vector<std::uint8_t>& bytes, std::size_t offset, std::uint32_t value)
{
    std::memcpy(bytes.data() + offset, &value, sizeof(value));
}

void WriteU16(std::vector<std::uint8_t>& bytes, std::size_t offset, std::uint16_t value)
{
    std::memcpy(bytes.data() + offset, &value, sizeof(value));
}

GameMessage Action(std::uint16_t opcode,
                   std::uint16_t command,
                   bool          start,
                   std::uint16_t text  = 30128u,
                   std::uint32_t actor = kEnemy)
{
    GameMessage message;
    message.opcode             = opcode;
    message.sourceId           = actor;
    const std::size_t capacity = opcode == 0x0139u ? 1u : opcode == 0x013Au ? 10u
                                                      : opcode == 0x013Bu   ? 18u
                                                                            : 0u;
    message.payload.resize(capacity == 0u ? 0x28u : capacity == 1u ? 0x38u
                                                : capacity == 10u  ? 0xB8u
                                                                   : 0x128u);
    WriteU32(message.payload, 0u, actor);
    WriteU32(message.payload, 4u, start ? 0x6F000001u : 0u);
    WriteU32(message.payload, 0x20u, static_cast<std::uint32_t>(capacity));
    WriteU16(message.payload, 0x24u, command);
    for (std::size_t index = 0; index < capacity; ++index)
    {
        WriteU32(message.payload, 0x28u + index * 4u, kPlayer);
    }
    if (capacity != 0u)
    {
        const std::size_t textOffset = capacity == 1u ? 0x2Eu : capacity == 10u ? 0x64u
                                                                                : 0x94u;
        WriteU16(message.payload, textOffset, start ? text : 0u);
    }
    return message;
}

GameMessage State(std::uint8_t state, std::uint32_t actor = kEnemy)
{
    GameMessage message;
    message.opcode   = 0x0134u;
    message.sourceId = actor;
    message.payload.resize(8u);
    message.payload[0] = state;
    return message;
}

void PublishPlayer(bahamut_client::PlayerStateService& player, std::uint32_t zone = 1u)
{
    bahamut_client::PlayerStateSnapshot snapshot;
    snapshot.actorId = kPlayer;
    snapshot.zoneId  = zone;
    Check(player.Publish(snapshot), "synthetic player state rejected");
}

bahamut_client::EnemyCastSnapshot OneCast(EnemyCastService& casts, std::uint16_t command, bool completed)
{
    const auto rows = casts.Snapshot();
    Check(rows.size() == 1u, "expected one cast row");
    const auto& row = rows.front();
    Check(row.actorId == kEnemy && row.commandId == command && row.casting && row.completed == completed,
          "cast identity or lifecycle state was incorrect");
    if (completed)
    {
        Check(row.progress == 1.0 && row.remainingSeconds == 0.0, "completed cast retained progress");
    }
    else
    {
        Check(row.progress >= 0.0 && row.progress < 1.0 && row.remainingSeconds > 0.0 &&
                  row.remainingSeconds <= row.durationSeconds,
              "active cast had invalid progress");
    }
    return row;
}

void StartAndCompletion()
{
    bahamut_client::PlayerStateService player;
    EnemyCastService                   casts(&player, nullptr);
    casts.Observe(Action(0x0139u, 23009u, true));
    Check(casts.Snapshot().empty(), "cast appeared without player state");
    PublishPlayer(player);

    // These packets test the consumer, including the catalog's capture-empty X18 shape.
    for (const auto opcode : std::array<std::uint16_t, 3>{ 0x0139u, 0x013Au, 0x013Bu })
    {
        casts.Observe(Action(opcode, 23009u, true, 30126u));
        const auto row = OneCast(casts, 23009u, false);
        Check(row.name == "nutkick" && row.durationSeconds == 3.0, "catalog cast estimate changed");
        casts.Observe(Action(opcode, 23009u, false));
        OneCast(casts, 23009u, true);
    }

    casts.Observe(Action(0x0139u, 65000u, true));
    const auto fallback = OneCast(casts, 65000u, false);
    Check(fallback.name == "Ability 65000" && fallback.durationSeconds == 3.5,
          "unknown command fallback changed");
    casts.Observe(Action(0x013Cu, 23009u, false));
    OneCast(casts, 65000u, false);
    casts.Observe(Action(0x013Cu, 65000u, false));
    OneCast(casts, 65000u, true);
    casts.Observe(Action(0x0139u, 23009u, true));
    casts.Observe(Action(0x013Cu, 0u, false));
    OneCast(casts, 23009u, true);
}

void InterruptionDespawnAndLateAction()
{
    bahamut_client::PlayerStateService player;
    PublishPlayer(player);
    EnemyCastService casts(&player, nullptr);
    for (const auto state : std::array<std::uint8_t, 3>{ 0u, 1u, 3u })
    {
        casts.Observe(State(2u));
        const auto engaged = casts.Snapshot();
        Check(engaged.size() == 1u && !engaged.front().casting, "engaged idle row missing");
        casts.Observe(Action(0x0139u, 23009u, true));
        OneCast(casts, 23009u, false);
        casts.Observe(State(state));
        Check(casts.Snapshot().empty(), "interruption retained cast or engaged row");
        casts.Observe(Action(0x013Cu, 23009u, false));
        Check(casts.Snapshot().empty(), "late completion revived interrupted cast");
    }

    for (const auto actor : { kEnemy, kPlayer })
    {
        casts.Observe(State(2u));
        casts.Observe(Action(0x0139u, 23009u, true));
        GameMessage despawn;
        despawn.opcode   = 0x00CBu;
        despawn.sourceId = actor;
        casts.Observe(despawn);
        Check(casts.Snapshot().empty(), "despawn retained cast or engaged row");
        casts.Observe(Action(0x013Cu, 23009u, false));
        Check(casts.Snapshot().empty(), "late completion revived despawned cast");
    }
    casts.Observe(Action(0x0139u, 23009u, true));
    PublishPlayer(player, 2u);
    Check(casts.Snapshot().empty(), "zone change retained a cast");
}

void RejectUnrelatedAndTruncatedActions()
{
    bahamut_client::PlayerStateService player;
    PublishPlayer(player);
    EnemyCastService casts(&player, nullptr);
    auto             outgoing = Action(0x0139u, 23009u, true);
    outgoing.direction        = packet_observer::Direction::Outgoing;
    casts.Observe(outgoing);
    auto friendly = Action(0x0139u, 23009u, true);
    WriteU32(friendly.payload, 0u, kPlayer);
    casts.Observe(friendly);
    auto truncated = Action(0x0139u, 23009u, true);
    truncated.payload.resize(0x2Fu);
    casts.Observe(truncated);
    casts.Observe(Action(0x0139u, 23009u, true, 1u));
    Check(casts.Snapshot().empty(), "unrelated or truncated action created a cast");
}

void ActorLifetime(std::uint16_t opcode)
{
    for (const auto actor : { kEnemy, kPlayer })
    {
        bahamut_client::PlayerStateService player;
        PublishPlayer(player);
        EnemyCastService casts(&player, nullptr);
        casts.Observe(State(2u));
        casts.Observe(Action(0x0139u, 23009u, true));
        casts.Observe(State(2u, kEnemy + 1u));

        GameMessage lifecycle;
        lifecycle.opcode   = opcode;
        lifecycle.sourceId = actor;
        lifecycle.payload.resize(8u);
        lifecycle.direction = packet_observer::Direction::Outgoing;
        casts.Observe(lifecycle);
        Check(casts.Snapshot().size() == 2u, "outgoing lifecycle cleared rows");
        lifecycle.direction = packet_observer::Direction::Incoming;
        if (opcode == 0x00CAu)
        {
            for (const auto size : { 7u, 9u })
            {
                lifecycle.payload.resize(size);
                casts.Observe(lifecycle);
                Check(casts.Snapshot().size() == 2u, "malformed AddActor cleared rows");
            }
            lifecycle.payload.resize(8u);
        }
        casts.Observe(lifecycle);
        casts.Observe(Action(0x013Cu, 23009u, false));
        const auto rows = casts.Snapshot();
        if (actor == kPlayer)
        {
            Check(rows.empty(), "local actor lifecycle retained cast or engaged rows");
        }
        else
        {
            Check(rows.size() == 1u && rows.front().actorId == kEnemy + 1u && !rows.front().casting,
                  "actor lifecycle retained stale rows or removed an unrelated actor");
            auto removeIdle     = lifecycle;
            removeIdle.sourceId = kEnemy + 1u;
            casts.Observe(removeIdle);
            Check(casts.Snapshot().empty(), "actor lifecycle retained an engaged-only row");
        }
        casts.Observe(Action(0x0139u, 65000u, true));
        OneCast(casts, 65000u, false);
    }
}

void RemoveActor()
{
    ActorLifetime(0x00CBu);
}

void ReusedActorId()
{
    ActorLifetime(0x00CAu);
}

void DeleteAllActors()
{
    bahamut_client::PlayerStateService player;
    PublishPlayer(player);
    EnemyCastService casts(&player, nullptr);
    casts.Observe(Action(0x0139u, 23009u, true));
    casts.Observe(Action(0x0139u, 23009u, true, 30128u, kEnemy + 1u));
    casts.Observe(Action(0x013Cu, 23009u, false, 0u, kEnemy + 1u));
    casts.Observe(State(2u, kEnemy + 2u));
    Check(casts.Snapshot().size() == 3u, "active, completed, and engaged rows missing");

    GameMessage clear;
    clear.opcode = 0x0007u;
    casts.Observe(clear);
    Check(casts.Snapshot().empty(), "DeleteAllActors retained cast or engaged rows");
    casts.Observe(Action(0x013Cu, 23009u, false));
    Check(casts.Snapshot().empty(), "late completion revived a deleted actor");
    casts.Observe(Action(0x0139u, 65000u, true));
    OneCast(casts, 65000u, false);
}

void InterruptionResult()
{
    bahamut_client::PlayerStateService player;
    PublishPlayer(player);
    EnemyCastService casts(&player, nullptr);
    for (const auto command : std::array<std::uint16_t, 2>{ 23009u, 0u })
    {
        casts.Observe(State(2u));
        casts.Observe(Action(0x0139u, 23009u, true));
        auto interrupted = Action(0x0139u, 65000u, false);
        WriteU32(interrupted.payload, 4u, 0x7F000002u);
        WriteU16(interrupted.payload, 0x2Eu, 30201u);
        casts.Observe(interrupted);
        OneCast(casts, 23009u, false);
        WriteU16(interrupted.payload, 0x24u, command);
        casts.Observe(interrupted);
        const auto rows = casts.Snapshot();
        Check(rows.size() == 1u && !rows.front().casting && !rows.front().completed,
              "interruption result displayed Done instead of the engaged idle row");
        casts.Observe(Action(0x013Cu, 23009u, false));
        Check(!casts.Snapshot().front().casting, "late completion revived an interrupted cast");
    }
}

void UnfinishedCastsBeforeCompletedRows()
{
    bahamut_client::PlayerStateService player;
    PublishPlayer(player);
    EnemyCastService casts(&player, nullptr);
    for (std::uint32_t index = 0u; index < 4u; ++index)
    {
        casts.Observe(Action(0x0139u, 23009u, true, 30128u, kEnemy + index));
        casts.Observe(Action(0x013Cu, 23009u, false, 0u, kEnemy + index));
    }
    casts.Observe(State(2u, kEnemy + 4u));
    casts.Observe(Action(0x0139u, 65000u, true, 30128u, kEnemy + 5u));
    casts.Observe(Action(0x0139u, 22302u, true, 30128u, kEnemy + 6u));
    const auto rows = casts.Snapshot();
    Check(rows.size() == 7u, "mixed cast rows missing");
    Check(rows[0].actorId == kEnemy + 6u && rows[0].casting && !rows[0].completed &&
              rows[1].actorId == kEnemy + 5u && rows[1].casting && !rows[1].completed,
          "completed rows hid unfinished casts from the first four Lua rows");
    Check(rows[0].remainingSeconds < rows[1].remainingSeconds,
          "unfinished casts lost remaining-time ordering");
    for (std::size_t index = 2u; index < 6u; ++index)
    {
        Check(rows[index].actorId == kEnemy + index - 2u && rows[index].completed,
              "completed rows lost deterministic actor ordering");
    }
    Check(rows.back().actorId == kEnemy + 4u && !rows.back().casting,
          "engaged idle row sorted ahead of cast rows");
}

} // namespace

int main()
{
    struct TestCase
    {
        const char* name;
        void (*run)();
    };

    const TestCase tests[] = {
        { "StartAndCompletion", StartAndCompletion },
        { "InterruptionDespawnAndLateAction", InterruptionDespawnAndLateAction },
        { "RejectUnrelatedAndTruncatedActions", RejectUnrelatedAndTruncatedActions },
        { "RemoveActor", RemoveActor },
        { "ReusedActorId", ReusedActorId },
        { "DeleteAllActors", DeleteAllActors },
        { "InterruptionResult", InterruptionResult },
        { "UnfinishedCastsBeforeCompletedRows", UnfinishedCastsBeforeCompletedRows },
    };
    int failures = 0;
    for (const auto& test : tests)
    {
        try
        {
            test.run();
        }
        catch (const std::exception& error)
        {
            std::cerr << test.name << ": " << error.what() << '\n';
            ++failures;
        }
    }
    return failures == 0 ? 0 : 1;
}
