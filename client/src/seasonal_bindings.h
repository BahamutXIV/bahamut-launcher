#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <string_view>

namespace bahamut_client::seasonal
{

struct NativeBinding
{
    std::string_view             name;
    std::uint32_t                rva;
    std::array<std::uint8_t, 16> signature;
    std::size_t                  length;
};

struct NativeCallSite
{
    std::uint32_t rva;
    std::uint32_t targetRva;
};

struct NativeCell
{
    std::uint32_t rva;
    std::uint32_t value;
};

#include "seasonal_bindings.inc"

} // namespace bahamut_client::seasonal
