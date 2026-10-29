#pragma once

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>

namespace bahamut_client::xhbar
{

enum class IconFormat
{
    Argb8,
    Dxt1,
    Dxt5
};

// The base GTEX mip borrows the caller's bytes.
struct IconImage
{
    std::uint32_t        width  = 0;
    std::uint32_t        height = 0;
    IconFormat           format = IconFormat::Dxt5;
    const unsigned char* data   = nullptr;
    std::size_t          bytes  = 0;
};

// Install-relative DAT path, e.g. "data/1C/5C/00/9D.DAT" for 30157; empty outside the ten icon bands.
std::optional<std::string> IconResourcePath(std::uint32_t icon);

// Unsupported GTEX headers, formats or extents return nullopt.
std::optional<IconImage> ParseIcon(const unsigned char* file, std::size_t size);

} // namespace bahamut_client::xhbar
