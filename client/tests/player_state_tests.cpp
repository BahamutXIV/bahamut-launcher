#include "area_names.h"
#include "packet_observer.h"
#include "player_state.h"

#include <cmath>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <optional>
#include <string_view>
#include <thread>
#include <vector>

namespace
{

static_assert(bahamut_client::AreaNameForZone(157u) == "The Mun-Tuy Cellars");
static_assert(bahamut_client::AreaRegionNameForZone(230u) == "La Noscea");
static_assert(bahamut_client::AreaRegionNameForZone(231u) == "Coerthas");
static_assert(bahamut_client::AreaRegionNameForZone(155u) == "The Black Shroud");
static_assert(bahamut_client::AreaRegionNameForZone(175u) == "Thanalan");
static_assert(bahamut_client::AreaRegionNameForZone(190u) == "Mor Dhona");
static_assert(bahamut_client::AreaRegionNameForZone(244u) == "Eorzea");
static_assert(bahamut_client::AreaRegionNameForZone(999u).empty());

constexpr std::size_t kActorPointerOffset     = 0x17838u;
constexpr std::size_t kActorIdOffset          = 0x1783Cu;
constexpr std::size_t kMapLayoutOffset        = 0x4E0u;
constexpr std::size_t kMapStateOffset         = 0x94u;
constexpr std::size_t kZoneOffset             = 0x98u;
constexpr std::size_t kPendingMapStateOffset  = 0xACu;
constexpr std::size_t kActorXOffset           = 0x9Cu;
constexpr std::size_t kActorYOffset           = 0xA0u;
constexpr std::size_t kActorZOffset           = 0xA4u;
constexpr std::size_t kActorParentOffset      = 0x98u;
constexpr std::size_t kActorFacingOffset      = 0xACu;
constexpr std::size_t kWaitingContainerOffset = 0x518u;
constexpr std::size_t kNameDataOffset         = 0x04u;
constexpr std::size_t kNameCapacityOffset     = 0x08u;
constexpr std::size_t kNameCountOffset        = 0x0Cu;
constexpr std::size_t kNameOwnershipOffset    = 0x15u;
constexpr std::size_t kNameInlineBufferOffset = 0x16u;
constexpr std::size_t kNameHasNoIdOffset      = 0x59u;
constexpr float       kTwoPi                  = 6.283185307179586476925286766559F;

template <typename T>
void Write(std::vector<std::uint8_t>& bytes, std::size_t offset, const T& value)
{
    std::memcpy(bytes.data() + offset, &value, sizeof(value));
}

bool Expect(bool condition, const char* message)
{
    if (!condition)
    {
        std::cerr << message << '\n';
        return false;
    }
    return true;
}

bool TestCapturePublishesCopy()
{
    std::vector<std::uint8_t> container(0x18000u);
    std::vector<std::uint8_t> actor(0xB0u);
    std::vector<std::uint8_t> mapLayout(0xB0u);
    std::vector<std::uint8_t> waiting(0x08u);
    std::vector<std::uint8_t> engine(0x224u);
    std::vector<std::uint8_t> holder(0x08u);
    std::vector<std::uint8_t> holderState(0x10u);
    std::vector<std::uint8_t> player(0x64u);
    std::vector<std::uint8_t> nameState(0x5Cu);
    std::vector<std::uint8_t> allocatedName(0x40u);
    const std::uint32_t       actorAddress = static_cast<std::uint32_t>(
        reinterpret_cast<std::uintptr_t>(actor.data()));
    const std::uint32_t mapLayoutAddress = static_cast<std::uint32_t>(
        reinterpret_cast<std::uintptr_t>(mapLayout.data()));
    const std::uint32_t actorId  = 0x10203040u;
    const std::uint32_t zoneId   = 230u;
    const float         x        = 1.25F;
    const float         y        = 2.5F;
    const float         z        = -3.75F;
    const float         rotation = 0.5F;
    const auto          address  = [](const std::vector<std::uint8_t>& value)
    {
        return static_cast<std::uint32_t>(
            reinterpret_cast<std::uintptr_t>(value.data()));
    };
    Write(container, kActorPointerOffset, actorAddress);
    Write(container, kActorIdOffset, actorId);
    Write(container, kMapLayoutOffset, mapLayoutAddress);
    Write(actor, kActorXOffset, x);
    Write(actor, kActorYOffset, y);
    Write(actor, kActorZOffset, z);
    Write(actor, kActorFacingOffset, rotation);
    Write(mapLayout, kMapStateOffset, 105u);
    Write(mapLayout, kZoneOffset, zoneId);
    Write(container, kWaitingContainerOffset, address(waiting));
    Write(waiting, 0x04u, address(engine));
    Write(engine, 0x220u, address(holder));
    Write(holder, 0x04u, address(holderState));
    Write(holderState, 0x0Cu, address(player));
    Write(player, 0x60u, address(nameState));
    Write(nameState, 0u, std::int32_t{ -1 });
    Write(nameState, kNameDataOffset, address(nameState) + static_cast<std::uint32_t>(kNameInlineBufferOffset));
    Write(nameState, kNameCapacityOffset, 0x40u);
    Write(nameState, kNameCountOffset, 20u);
    Write(nameState, kNameOwnershipOffset, std::uint8_t{ 1 });
    Write(nameState, kNameHasNoIdOffset, std::uint8_t{ 1 });
    std::memcpy(nameState.data() + kNameInlineBufferOffset,
                "Gridaniaclonea Test",
                20u);

    bahamut_client::PlayerStateService service;
    if (!Expect(service.CaptureFromContainer(container.data()),
                "valid game layout should publish a snapshot"))
    {
        return false;
    }
    const auto first = service.Snapshot();
    if (!Expect(first.has_value(), "published snapshot should be readable") || !Expect(first->actorId == actorId && first->zoneId == zoneId, "snapshot should publish the SetMap zone rather than the map state id") || !Expect(first->areaName == "Limsa Lominsa", "snapshot should resolve the zone's English area name") || !Expect(first->x == x && first->y == y && first->z == z && first->rotation == rotation, "snapshot should preserve all copied position fields") || !Expect(first->displayName == "Gridaniaclonea Test", "snapshot should copy the inline display name"))
    {
        return false;
    }

    const float changedX = 99.0F;
    Write(actor, kActorXOffset, changedX);
    std::memcpy(nameState.data() + kNameInlineBufferOffset, "Changed Name", 13u);
    Write(nameState, kNameCountOffset, 13u);
    if (!Expect(first->x == x,
                "a previously returned snapshot must not alias game memory"))
    {
        return false;
    }
    return Expect(service.CaptureFromContainer(container.data()),
                  "a later game-thread capture should publish") &&
           Expect(first->displayName == "Gridaniaclonea Test",
                  "a returned display name must not alias game memory") &&
           Expect(service.Snapshot()->x == changedX,
                  "a later capture should publish the changed copy") &&
           ([&]()
            {
                const char allocated[] = "Allocated Name";
                std::memcpy(allocatedName.data(), allocated, sizeof(allocated));
                Write(nameState, kNameDataOffset, address(allocatedName));
                Write(nameState, kNameCapacityOffset, 0x40u);
                Write(nameState, kNameCountOffset, static_cast<std::uint32_t>(sizeof(allocated)));
                Write(nameState, kNameOwnershipOffset, std::uint8_t{ 0 });
                if (!service.CaptureFromContainer(container.data()) || service.Snapshot()->displayName != "Allocated Name")
                {
                    return Expect(false,
                                  "snapshot should copy allocated display-name storage");
                }
                allocatedName[0] = 0xc3;
                allocatedName[1] = 0x28;
                allocatedName[2] = 0;
                Write(nameState, kNameCountOffset, 3u);
                if (!service.CaptureFromContainer(container.data()) || !service.Snapshot()->displayName.empty())
                {
                    return Expect(false,
                                  "invalid UTF-8 should fall back to an unnamed snapshot");
                }
                Write(nameState, kNameOwnershipOffset, std::uint8_t{ 2 });
                return Expect(service.CaptureFromContainer(container.data()) && service.Snapshot()->displayName.empty(),
                              "invalid ownership should fall back to an unnamed snapshot");
            })();
}

bool TestInvalidStateIsNil()
{
    bahamut_client::PlayerStateService service;
    if (!Expect(!service.Snapshot().has_value(), "initial state should be nil") || !Expect(!service.CaptureFromContainer(nullptr), "null container should be rejected") || !Expect(!service.Snapshot().has_value(), "null container should leave nil state"))
    {
        return false;
    }

    bahamut_client::PlayerStateSnapshot snapshot;
    snapshot.actorId  = 1;
    snapshot.zoneId   = 2;
    snapshot.x        = 1.0F;
    snapshot.y        = 2.0F;
    snapshot.z        = 3.0F;
    snapshot.rotation = 4.0F;
    if (!Expect(service.Publish(snapshot), "valid test snapshot should publish"))
    {
        return false;
    }
    snapshot.zoneId = 0;
    if (!Expect(!service.Publish(snapshot), "zero zone should be rejected"))
    {
        return false;
    }

    std::vector<std::uint8_t> container(0x18000u);
    std::vector<std::uint8_t> mapLayout(0xB0u);
    const std::uint32_t       invalidActorAddress = 0xC0000000u;
    const std::uint32_t       mapLayoutAddress    = static_cast<std::uint32_t>(
        reinterpret_cast<std::uintptr_t>(mapLayout.data()));
    Write(container, kActorPointerOffset, invalidActorAddress);
    Write(container, kActorIdOffset, 1u);
    Write(container, kMapLayoutOffset, mapLayoutAddress);
    return Expect(!service.CaptureFromContainer(container.data()),
                  "the documented active-actor sentinel should be rejected") &&
           Expect(!service.Snapshot().has_value(),
                  "a sentinel actor should leave nil state");
}

bool TestEnrichedStateIsCopied()
{
    bahamut_client::PlayerStateService  service;
    bahamut_client::PlayerStateSnapshot snapshot;
    snapshot.actorId     = 9;
    snapshot.zoneId      = 230;
    snapshot.x           = 1.0F;
    snapshot.y           = 2.0F;
    snapshot.z           = 3.0F;
    snapshot.rotation    = 4.0F;
    snapshot.displayName = "Name";
    snapshot.areaName    = "Ul'dah";
    snapshot.baseClassId = 8;
    snapshot.jobId       = 19;
    snapshot.level       = 50;
    if (!Expect(service.Publish(snapshot), "enriched snapshot should publish"))
    {
        return false;
    }
    const auto copy = service.Snapshot();
    return Expect(copy.has_value(), "enriched snapshot should be readable") &&
           Expect(copy->areaName == snapshot.areaName &&
                      copy->baseClassId == snapshot.baseClassId &&
                      copy->jobId == snapshot.jobId && copy->level == snapshot.level,
                  "enriched fields should be copied into the published snapshot");
}

packet_observer::GameMessage ClassLevelMessage(
    std::uint32_t actorId, std::optional<std::uint8_t> classId, std::optional<std::uint16_t> level, std::string_view target = "/_init")
{
    packet_observer::GameMessage message;
    message.opcode   = 0x0137u;
    message.sourceId = actorId;
    auto& data       = message.payload;
    data.push_back(0u);
    if (classId)
    {
        data.insert(data.end(), { 1u, 0x24u, 0xCEu, 0x32u, 0x75u, *classId });
    }
    if (level)
    {
        data.insert(data.end(), { 2u, 0x88u, 0x35u, 0x06u, 0x96u, static_cast<std::uint8_t>(*level & 0xFFu), static_cast<std::uint8_t>(*level >> 8u) });
    }
    data.push_back(static_cast<std::uint8_t>(0x82u + target.size()));
    data.insert(data.end(), target.begin(), target.end());
    data[0] = static_cast<std::uint8_t>(data.size() - 1u);
    data.resize(136u);
    return message;
}

packet_observer::GameMessage JobMessage(std::uint32_t actorId, std::uint8_t jobId)
{
    packet_observer::GameMessage message;
    message.opcode   = 0x01A4u;
    message.sourceId = actorId;
    message.payload.resize(8u);
    message.payload[0] = jobId;
    return message;
}

packet_observer::GameMessage MapMessage(std::uint32_t actorId, std::uint32_t zoneId)
{
    packet_observer::GameMessage message;
    message.opcode   = 0x0005u;
    message.sourceId = actorId;
    message.payload.resize(16u);
    Write(message.payload, 4u, zoneId);
    return message;
}

bool TestObservedClassJobJoinsLocalCapture()
{
    std::vector<std::uint8_t> container(0x18000u);
    std::vector<std::uint8_t> actor(0xB0u);
    std::vector<std::uint8_t> mapLayout(0xB0u);
    constexpr std::uint32_t   actorId = 0x029B2941u;
    Write(container, kActorPointerOffset, static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(actor.data())));
    Write(container, kActorIdOffset, actorId);
    Write(container, kMapLayoutOffset, static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(mapLayout.data())));
    Write(mapLayout, kMapStateOffset, 105u);
    Write(mapLayout, kZoneOffset, 230u);

    bahamut_client::PlayerStateService service;
    service.Observe(ClassLevelMessage(actorId, std::uint8_t{ 3 }, std::uint16_t{ 42 }));
    service.Observe(JobMessage(actorId, 16u));
    if (!Expect(service.CaptureFromContainer(container.data()),
                "first capture should join earlier local property packets") ||
        !Expect(service.Snapshot()->baseClassId == 3u &&
                    service.Snapshot()->level == 42u &&
                    service.Snapshot()->jobId == 16u,
                "initial class, level, and job should share the copied snapshot"))
    {
        return false;
    }

    service.Observe(ClassLevelMessage(actorId + 1u, std::uint8_t{ 4 }, std::uint16_t{ 50 }));
    service.Observe(ClassLevelMessage(actorId, std::uint8_t{ 4 }, std::uint16_t{ 50 }, "wrong/target"));
    auto malformed       = ClassLevelMessage(actorId, std::uint8_t{ 4 }, std::uint16_t{ 50 });
    malformed.payload[0] = 0xFFu;
    service.Observe(malformed);
    auto badJob = JobMessage(actorId, 19u);
    badJob.payload.pop_back();
    service.Observe(badJob);
    if (!Expect(service.CaptureFromContainer(container.data()) &&
                    service.Snapshot()->baseClassId == 3u &&
                    service.Snapshot()->level == 42u &&
                    service.Snapshot()->jobId == 16u,
                "peer and malformed packets must not replace local values"))
    {
        return false;
    }

    service.Observe(ClassLevelMessage(actorId, std::nullopt, std::uint16_t{ 43 }, "charaWork/stateForAll"));
    service.Observe(JobMessage(actorId, 0u));
    if (!Expect(service.CaptureFromContainer(container.data()) &&
                    service.Snapshot()->baseClassId == 3u &&
                    service.Snapshot()->level == 43u &&
                    service.Snapshot()->jobId == 0u,
                "partial level update and job removal should preserve base class"))
    {
        return false;
    }

    service.Observe(MapMessage(actorId, 231u));
    service.Observe(ClassLevelMessage(actorId, std::uint8_t{ 4 }, std::uint16_t{ 1 }, "charaWork/stateAtQuicklyForAll"));
    if (!Expect(!service.CaptureFromContainer(container.data()) &&
                    !service.Snapshot(),
                "new-zone packets should wait for the committed client zone"))
    {
        return false;
    }
    Write(mapLayout, kPendingMapStateOffset, 106u);
    if (!Expect(!service.CaptureFromContainer(container.data()) &&
                    !service.Snapshot(),
                "pending zone should clear enriched player state"))
    {
        return false;
    }
    if (!Expect(!service.CaptureFromContainer(container.data()) &&
                    !service.Snapshot(),
                "pending zone should keep later initial packets unpublished"))
    {
        return false;
    }
    Write(mapLayout, kMapStateOffset, 106u);
    Write(mapLayout, kZoneOffset, 231u);
    Write(mapLayout, kPendingMapStateOffset, 0u);
    if (!Expect(service.CaptureFromContainer(container.data()) &&
                    service.Snapshot()->baseClassId == 4u &&
                    service.Snapshot()->jobId == 0u &&
                    service.Snapshot()->level == 1u,
                "new zone should use later packets without inheriting the old job"))
    {
        return false;
    }

    constexpr std::uint32_t replacementId = actorId + 2u;
    service.Observe(ClassLevelMessage(replacementId, std::uint8_t{ 8 }, std::uint16_t{ 30 }));
    service.Observe(JobMessage(replacementId, 19u));
    Write(container, kActorIdOffset, replacementId);
    if (!Expect(service.CaptureFromContainer(container.data()) &&
                    service.Snapshot()->actorId == replacementId &&
                    service.Snapshot()->baseClassId == 8u &&
                    service.Snapshot()->jobId == 19u &&
                    service.Snapshot()->level == 30u,
                "actor replacement should join only replacement packets"))
    {
        return false;
    }
    Write(container, kActorIdOffset, 0u);
    if (!Expect(!service.CaptureFromContainer(container.data()) &&
                    !service.Snapshot(),
                "logout should clear the enriched snapshot"))
    {
        return false;
    }
    Write(container, kActorIdOffset, replacementId);
    return Expect(service.CaptureFromContainer(container.data()) &&
                      service.Snapshot()->baseClassId == 0u &&
                      service.Snapshot()->jobId == 0u &&
                      service.Snapshot()->level == 0u,
                  "login without new packets must not reuse the old values");
}

bool TestZoneTransitionClearsSnapshot()
{
    std::vector<std::uint8_t> container(0x18000u);
    std::vector<std::uint8_t> actor(0xB0u);
    std::vector<std::uint8_t> mapLayout(0xB0u);
    Write(container, kActorPointerOffset, static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(actor.data())));
    Write(container, kActorIdOffset, 7u);
    Write(container, kMapLayoutOffset, static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(mapLayout.data())));
    Write(mapLayout, kMapStateOffset, 105u);
    Write(mapLayout, kZoneOffset, 230u);

    bahamut_client::PlayerStateService service;
    if (!Expect(service.CaptureFromContainer(container.data()) &&
                    service.Snapshot()->zoneId == 230u &&
                    service.Snapshot()->areaName == "Limsa Lominsa",
                "SetMap zone should publish independently of map state id"))
    {
        return false;
    }

    Write(mapLayout, kPendingMapStateOffset, 106u);
    if (!Expect(!service.CaptureFromContainer(container.data()) &&
                    !service.Snapshot().has_value(),
                "pending zone transition should clear the old snapshot"))
    {
        return false;
    }

    Write(mapLayout, kMapStateOffset, 106u);
    Write(mapLayout, kZoneOffset, 231u);
    Write(mapLayout, kPendingMapStateOffset, 0u);
    if (!Expect(service.CaptureFromContainer(container.data()) &&
                    service.Snapshot()->zoneId == 231u &&
                    service.Snapshot()->areaName == "Dzemael Darkhold",
                "committed new zone should publish"))
    {
        return false;
    }

    Write(mapLayout, kZoneOffset, 177u);
    if (!Expect(service.CaptureFromContainer(container.data()) &&
                    service.Snapshot()->areaName.empty(),
                "placeholder area should have no display label"))
    {
        return false;
    }

    Write(mapLayout, kZoneOffset, 500u);
    if (!Expect(service.CaptureFromContainer(container.data()) &&
                    service.Snapshot()->areaName.empty(),
                "unknown area should have no display label"))
    {
        return false;
    }

    Write(mapLayout, kZoneOffset, 0u);
    return Expect(!service.CaptureFromContainer(container.data()) &&
                      !service.Snapshot().has_value(),
                  "cleared zone should clear the snapshot");
}

bool TestRelativePositionIsResolved()
{
    std::vector<std::uint8_t> container(0x18000u);
    std::vector<std::uint8_t> actor(0xB0u);
    std::vector<std::uint8_t> parent(0xB0u);
    std::vector<std::uint8_t> mapLayout(0xB0u);
    const std::uint32_t       actorAddress = static_cast<std::uint32_t>(
        reinterpret_cast<std::uintptr_t>(actor.data()));
    const std::uint32_t parentAddress = static_cast<std::uint32_t>(
        reinterpret_cast<std::uintptr_t>(parent.data()));
    const std::uint32_t mapLayoutAddress = static_cast<std::uint32_t>(
        reinterpret_cast<std::uintptr_t>(mapLayout.data()));
    Write(container, kActorPointerOffset, actorAddress);
    Write(container, kActorIdOffset, 7u);
    Write(container, kMapLayoutOffset, mapLayoutAddress);
    Write(mapLayout, kMapStateOffset, 105u);
    Write(mapLayout, kZoneOffset, 230u);
    Write(actor, kActorParentOffset, parentAddress);
    Write(actor, kActorXOffset, 1.0F);
    Write(actor, kActorYOffset, 2.0F);
    Write(actor, kActorZOffset, 3.0F);
    Write(actor, kActorFacingOffset, 0.5F);
    Write(parent, kActorXOffset, 10.0F);
    Write(parent, kActorYOffset, 20.0F);
    Write(parent, kActorZOffset, 30.0F);
    Write(parent, kActorFacingOffset, 3.0F);

    bahamut_client::PlayerStateService service;
    if (!Expect(service.CaptureFromContainer(container.data()),
                "a parent-relative actor should publish a snapshot"))
    {
        return false;
    }
    const auto snapshot = service.Snapshot();
    if (!Expect(snapshot.has_value(), "relative snapshot should be readable") || !Expect(snapshot->x == 11.0F && snapshot->y == 22.0F && snapshot->z == 33.0F && std::fabs(snapshot->rotation - (3.5F - kTwoPi)) < 0.0001F,
                                                                                         "positive parent-relative facing should use the retail signed wrap"))
    {
        return false;
    }
    Write(actor, kActorFacingOffset, -0.5F);
    Write(parent, kActorFacingOffset, -3.0F);
    return Expect(service.CaptureFromContainer(container.data()),
                  "negative parent-relative facing should publish") &&
           Expect(std::fabs(service.Snapshot()->rotation - (kTwoPi - 3.5F)) < 0.0001F,
                  "negative parent-relative facing should use the retail signed wrap");
}

bool TestConcurrentCopyIsConsistent()
{
    bahamut_client::PlayerStateService  service;
    bahamut_client::PlayerStateSnapshot first;
    first.actorId                              = 1;
    first.zoneId                               = 2;
    first.x                                    = 1.0F;
    first.y                                    = 1.0F;
    first.z                                    = 1.0F;
    first.rotation                             = 1.0F;
    first.displayName                          = "Alpha Player";
    bahamut_client::PlayerStateSnapshot second = first;
    second.x                                   = 2.0F;
    second.y                                   = 2.0F;
    second.z                                   = 2.0F;
    second.rotation                            = 2.0F;
    second.displayName                         = "Bravo Player";

    bool consistent = true;
    service.Publish(first);
    std::thread writer([&]()
                       {
                           for (std::size_t i = 0; i != 100000u; ++i)
                           {
                               service.Publish((i & 1u) == 0u ? first : second);
                           }
                       });
    std::size_t successfulReads = 0;
    for (std::size_t index = 0; index != 100000u; ++index)
    {
        const auto snapshot = service.Snapshot();
        successfulReads += snapshot.has_value() ? 1u : 0u;
        if (snapshot.has_value() && !(((snapshot->x == 1.0F && snapshot->y == 1.0F && snapshot->z == 1.0F && snapshot->rotation == 1.0F) && snapshot->displayName == "Alpha Player") || (snapshot->x == 2.0F && snapshot->y == 2.0F && snapshot->z == 2.0F && snapshot->rotation == 2.0F && snapshot->displayName == "Bravo Player")))
        {
            consistent = false;
            break;
        }
    }
    writer.join();
    return Expect(successfulReads != 0,
                  "concurrent snapshot test must complete a read") &&
           Expect(consistent,
                  "seqlock reads should never combine fields from two snapshots");
}

} // namespace

int wmain()
{
    return TestCapturePublishesCopy() && TestInvalidStateIsNil() &&
                   TestEnrichedStateIsCopied() && TestObservedClassJobJoinsLocalCapture() &&
                   TestZoneTransitionClearsSnapshot() && TestRelativePositionIsResolved() &&
                   TestConcurrentCopyIsConsistent()
               ? 0
               : 1;
}
