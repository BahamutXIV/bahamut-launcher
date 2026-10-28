#pragma once

#include <cstdint>
#include <optional>
#include <string_view>

namespace bahamut_client
{

// Enum-style names for equipment-range item types, such as WEATHERED_CANE.
// Names are advisory labels for Gear Sets files; the item type ID is the identity.
std::optional<std::uint32_t> ItemIdByName(std::string_view name);
std::string_view             ItemNameById(std::uint32_t id);

} // namespace bahamut_client
