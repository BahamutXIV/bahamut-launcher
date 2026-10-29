#pragma once

#include "gearsets_equip_request.h"
#include "gearsets_native_lookup.h"
#include "gearsets_transaction.h"

#include <cstdint>
#include <optional>

namespace bahamut_client
{

// Sending requires an established connection, no event in progress or installed,
// and a resolved EquipCommand reference; the command actor need not be loaded.
struct GearsetsNativeRoute
{
    std::uint32_t network   = 0;
    std::uint32_t actor     = 0;
    std::uint32_t owner     = 0;
    std::uint32_t stateWord = 0;
    std::uint8_t  eventType = 0;
};

// container is the player-state container, player the MyPlayer control.
std::optional<GearsetsNativeRoute> ReadGearsetsNativeRoute(const GearsetsNativeMemory& memory, std::uint32_t container, std::uint32_t player);

// The request for one operation: wire slots are 1-based equipment positions.
// nullopt for an item without a bag position.
std::optional<GearsetsEquipCommandRequest> GearsetsNativeRequest(const GearsetsOperation& operation, const GearsetsNativeRoute& route);

// Retail 1.23b equips/unequips on the game window thread, without Lua or local events.
// The server's inventory replies apply the change. See client/README.md#gearsets.
class GearsetsNativeExecutor final : public GearsetsExecutor
{
public:
    // The server's replies began 146-282 ms after each captured request.
    static constexpr std::uint64_t kConfirmationMs = 5000u;
    // A class change's dependent-slot clears arrive in the same inventory
    // bracket as the main hand; this bounds a class/job update that trails it.
    static constexpr std::uint64_t kSettleMs = 1000u;

    std::optional<GearsetsPlan>    Plan(const GearsetIntent& intent) const override;
    std::optional<GearsetsFailure> Preflight(const GearsetIntent& intent, const GearsetsView& view) const override;
    void                           BeginFrame(const void* container, const void* player) override;
    GearsetsSubmission             Submit(const GearsetsOperation& operation) override;

private:
    const void* container_ = nullptr;
    const void* player_    = nullptr;
};

} // namespace bahamut_client
