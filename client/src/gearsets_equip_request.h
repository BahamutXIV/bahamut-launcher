#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <span>

namespace bahamut_client
{

// Equip request payload layout: client/README.md#client-evidence.
inline constexpr std::size_t kGearsetsCommandRequestSize = 184u;

struct GearsetsEquipCommandRequest
{
    std::uint32_t actor     = 0;
    std::uint32_t owner     = 0; // EquipCommand reference, 0xA0F02EE9
    std::uint32_t stateWord = 0;
    std::uint8_t  eventType = 0;
    std::uint16_t bagSlot   = 0; // 0-based package 0 position
    std::uint8_t  equipSlot = 0; // 1-based wire slot
    std::uint64_t uid       = 0;
    bool          unequip   = false; // nil item and no unique IDs; bagSlot and uid unused
};

// Standard reflected CRC32 (poly 0xEDB88320), which 0x00D3AB60 computes.
std::uint32_t GearsetsCrc32(std::span<const std::uint8_t> bytes);

// Equip/unequip parameter tags: client/README.md#client-evidence.
std::array<std::uint8_t, kGearsetsCommandRequestSize> EncodeGearsetsEquipCommandRequest(const GearsetsEquipCommandRequest& request);

} // namespace bahamut_client
