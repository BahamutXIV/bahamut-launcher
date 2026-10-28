#include "gearsets_native_executor.h"

#include "fault_guard.h"

#include <windows.h>

#include <array>
#include <cstring>

namespace
{

using namespace bahamut_client;

// Engine and sender layout: client/README.md#client-evidence.
constexpr std::uint32_t kImageBase        = 0x00400000u;
constexpr std::uint32_t kContainerWaiting = 0x518u;
constexpr std::uint32_t kWaitingEngine    = 0x04u;
constexpr std::uint32_t kEngineReporter   = 0x1CCu;
constexpr std::uint32_t kReporterHalt     = 0x08u;
constexpr std::uint32_t kEngineSender     = 0x220u;
constexpr std::uint32_t kSenderState      = 0x04u;
constexpr std::uint32_t kStateActor       = 0x10u;
constexpr std::uint32_t kStateWord        = 0x14u;

// Network send and record lifetime: client/README.md#client-evidence.
constexpr std::uint32_t kNetSession        = 0x174ECu;
constexpr std::uint32_t kSessionLink       = 0x234u;
constexpr std::uint32_t kLinkConnection    = 0x70u;
constexpr std::uint32_t kConnectionState   = 0x8Cu;
constexpr std::uint32_t kConnectionReady   = 3u;
constexpr std::uint32_t kRecordOpcode      = 0x12Du;
constexpr std::uint32_t kRecordSize        = 0xC8u;
constexpr std::uint32_t kRecordPayload     = 0x18u;
constexpr std::uint32_t kSendPacketAddress = 0x004D6D30u;

using SendPacket = bool(__thiscall*)(void* network, const void* record, std::uint32_t, std::uint32_t);

// Request event type: client/README.md#client-evidence.
constexpr std::uint32_t kEventTypeGlobal = 0x0134C3FFu;

// Player event gates and teardown: client/README.md#client-evidence.
constexpr std::uint32_t kInvalidActorGlobal = 0x0130C778u;
constexpr std::uint32_t kPlayerEventA       = 0x128u;
constexpr std::uint32_t kPlayerEventB       = 0x12Cu;
constexpr std::uint32_t kPlayerManager      = 0xF8u;
constexpr std::uint32_t kManagerClientEvent = 0x08u;
constexpr std::uint32_t kManagerServerEvent = 0x0Cu;
constexpr std::uint32_t kManagerBusy        = 0x1Eu;

// EquipCommand's reference in the captured requests; the lookup must
// reproduce it from the actor-id map.
constexpr std::uint32_t kEquipCommandActor     = 12009u;
constexpr std::uint32_t kEquipCommandReference = 0xA0F02EE9u;

bool ReadRaw(void*, std::uint32_t address, void* out, std::size_t size)
{
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(out, reinterpret_cast<const void*>(static_cast<std::uintptr_t>(address)), size);
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

const GearsetsNativeMemory kMemory{ &ReadRaw, nullptr };

std::uint32_t Address(const void* pointer)
{
    return static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(pointer));
}

} // namespace

namespace bahamut_client
{

std::optional<GearsetsNativeRoute> ReadGearsetsNativeRoute(const GearsetsNativeMemory& memory, std::uint32_t container, std::uint32_t player)
{
    if (container == 0u || player == 0u)
        return std::nullopt;
    const auto          waiting = memory.Get<std::uint32_t>(container + kContainerWaiting);
    const std::uint32_t engine  = waiting ? memory.Get<std::uint32_t>(*waiting + kWaitingEngine).value_or(0u) : 0u;
    const auto          halt    = memory.Get<std::uint8_t>(memory.Get<std::uint32_t>(engine + kEngineReporter).value_or(0u) + kReporterHalt);
    if (engine == 0u || !halt || *halt != 0u)
        return std::nullopt;

    const std::uint32_t state      = memory.Get<std::uint32_t>(memory.Get<std::uint32_t>(engine + kEngineSender).value_or(0u) + kSenderState).value_or(0u);
    const auto          actor      = memory.Get<std::uint32_t>(state + kStateActor);
    const auto          stateWord  = memory.Get<std::uint32_t>(state + kStateWord);
    const std::uint32_t network    = memory.Get<std::uint32_t>(memory.Get<std::uint32_t>(state).value_or(0u)).value_or(0u);
    const std::uint32_t session    = memory.Get<std::uint32_t>(network + kNetSession).value_or(0u);
    const std::uint32_t link       = memory.Get<std::uint32_t>(session + kSessionLink).value_or(0u);
    const std::uint32_t connection = memory.Get<std::uint32_t>(link + kLinkConnection).value_or(0u);
    const auto          connState  = memory.Get<std::uint32_t>(connection + kConnectionState);
    // The item reference names the sending actor as the bag's owner, as in
    // the captured request.
    if (state == 0u || !actor || *actor == 0u || !stateWord || network == 0u || session == 0u || link == 0u || connection == 0u ||
        !connState || *connState != kConnectionReady)
        return std::nullopt;

    const auto          invalid     = memory.Get<std::uint32_t>(kInvalidActorGlobal);
    const auto          eventA      = memory.Get<std::uint32_t>(player + kPlayerEventA);
    const auto          eventB      = memory.Get<std::uint32_t>(player + kPlayerEventB);
    const std::uint32_t manager     = memory.Get<std::uint32_t>(player + kPlayerManager).value_or(0u);
    const auto          busy        = memory.Get<std::uint8_t>(manager + kManagerBusy);
    const auto          clientEvent = memory.Get<std::uint32_t>(manager + kManagerClientEvent);
    const auto          serverEvent = memory.Get<std::uint32_t>(manager + kManagerServerEvent);
    if (!invalid || !eventA || !eventB || *eventA != *invalid || *eventB != *invalid || !busy || *busy != 0u || !clientEvent ||
        *clientEvent != 0u || !serverEvent || *serverEvent != 0u)
        return std::nullopt;
    const auto eventType = memory.Get<std::uint8_t>(kEventTypeGlobal);
    if (!eventType || *eventType != 0u)
        return std::nullopt;
    const GearsetsStaticActorLookup command = FindLoadedStaticActor(memory, engine, kEquipCommandActor);
    // Only the reference reaches the wire; whether the client has loaded the
    // command actor does not.
    if (command.reference != kEquipCommandReference)
        return std::nullopt;
    return GearsetsNativeRoute{ network, *actor, command.reference, *stateWord, *eventType };
}

std::optional<GearsetsEquipCommandRequest> GearsetsNativeRequest(const GearsetsOperation& operation, const GearsetsNativeRoute& route)
{
    GearsetsEquipCommandRequest request;
    request.actor     = route.actor;
    request.owner     = route.owner;
    request.stateWord = route.stateWord;
    request.eventType = route.eventType;
    request.equipSlot = static_cast<std::uint8_t>(operation.target.slot + 1u);
    if (operation.target.state == EquipmentSlotState::Empty)
    {
        request.unequip = true;
        return request;
    }
    if (operation.target.state != EquipmentSlotState::Item || !operation.bagSlot)
        return std::nullopt;
    request.bagSlot = *operation.bagSlot;
    request.uid     = operation.target.uid;
    return request;
}

std::optional<GearsetsPlan> GearsetsNativeExecutor::Plan(const GearsetIntent&) const
{
    // kEquipmentSlots starts with the main hand, whose class change can clear
    // other slots, so it is applied first.
    return GearsetsPlan{ { kEquipmentSlots.begin(), kEquipmentSlots.end() }, kConfirmationMs, kSettleMs };
}

std::optional<GearsetsFailure> GearsetsNativeExecutor::Preflight(const GearsetIntent& intent, const GearsetsView&) const
{
    // The class follows the main-hand weapon, and the Gear window never
    // empties it.
    if (intent.slots[0].state != EquipmentSlotState::Item)
        return GearsetsFailure{ "main-hand-required", intent.slots[0].slot };
    return std::nullopt;
}

void GearsetsNativeExecutor::BeginFrame(const void* container, const void* player)
{
    container_ = container;
    player_    = player;
}

GearsetsSubmission GearsetsNativeExecutor::Submit(const GearsetsOperation& operation)
{
    if (Address(GetModuleHandleW(nullptr)) != kImageBase)
        return GearsetsSubmission::Unavailable;
    const auto route = ReadGearsetsNativeRoute(kMemory, Address(container_), Address(player_));
    if (!route)
        return GearsetsSubmission::Deferred;
    const auto request = GearsetsNativeRequest(operation, *route);
    if (!request)
        return GearsetsSubmission::Unavailable;
    const auto payload = EncodeGearsetsEquipCommandRequest(*request);

    // Zero the unread record bytes; builder layout: client/README.md#client-evidence.
    alignas(4) std::array<std::uint8_t, kRecordPayload + kGearsetsCommandRequestSize> record{};
    std::memcpy(record.data() + 0x00u, &kRecordOpcode, sizeof(kRecordOpcode));
    std::memcpy(record.data() + 0x04u, &kRecordSize, sizeof(kRecordSize));
    std::memcpy(record.data() + kRecordPayload, payload.data(), payload.size());
    const auto sendPacket = reinterpret_cast<SendPacket>(static_cast<std::uintptr_t>(kSendPacketAddress));
    return sendPacket(reinterpret_cast<void*>(static_cast<std::uintptr_t>(route->network)), record.data(), 0u, 0u) ? GearsetsSubmission::Accepted
                                                                                                                   : GearsetsSubmission::Rejected;
}

} // namespace bahamut_client
