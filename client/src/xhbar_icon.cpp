#include "xhbar_icon.h"

#include <cstdio>
#include <cstring>

namespace bahamut_client::xhbar
{
namespace
{

// GTEX: format +6, mip count +7, big-endian width/height +0x0A/+0x0C, data base +0x14, mip offset/size +0x18/+0x1C.
// Retail 1.23b base mip follows the 32-byte header.
constexpr std::size_t   kHeaderBytes = 0x20;
constexpr unsigned char kArgb8       = 4;
constexpr unsigned char kDxt1        = 24;
constexpr unsigned char kDxt5        = 26;

std::uint32_t ReadBig16(const unsigned char* bytes)
{
    return (static_cast<std::uint32_t>(bytes[0]) << 8) | bytes[1];
}

std::uint32_t ReadBig32(const unsigned char* bytes)
{
    return (static_cast<std::uint32_t>(bytes[0]) << 24) | (static_cast<std::uint32_t>(bytes[1]) << 16) |
           (static_cast<std::uint32_t>(bytes[2]) << 8) | bytes[3];
}

} // namespace

std::optional<std::string> IconResourcePath(std::uint32_t icon)
{
    // Icon N: resource 0x1C59 + N / 10000, low sixteen bits N % 10000; last band 0x1C62.
    if (icon >= 100000)
        return std::nullopt;
    const std::uint32_t resource = ((0x1C59u + icon / 10000u) << 16) | (icon % 10000u);
    char                path[32]{};
    std::snprintf(path,
                  sizeof(path),
                  "data/%02X/%02X/%02X/%02X.DAT",
                  static_cast<unsigned>((resource >> 24) & 0xFF),
                  static_cast<unsigned>((resource >> 16) & 0xFF),
                  static_cast<unsigned>((resource >> 8) & 0xFF),
                  static_cast<unsigned>(resource & 0xFF));
    return std::string(path);
}

std::optional<IconImage> ParseIcon(const unsigned char* file, std::size_t size)
{
    if (file == nullptr || size < kHeaderBytes || std::memcmp(file, "GTEX", 4) != 0 || file[7] == 0)
        return std::nullopt;
    IconImage image;
    image.width  = ReadBig16(file + 0x0A);
    image.height = ReadBig16(file + 0x0C);
    if (image.width == 0 || image.height == 0 || image.width > 512 || image.height > 512)
        return std::nullopt;
    std::size_t needed = 0;
    switch (file[6])
    {
        case kArgb8:
            image.format = IconFormat::Argb8;
            needed       = static_cast<std::size_t>(image.width) * image.height * 4u;
            break;
        case kDxt1:
        case kDxt5:
            // Block formats need whole 4x4 blocks.
            if (image.width % 4 != 0 || image.height % 4 != 0)
                return std::nullopt;
            image.format = file[6] == kDxt1 ? IconFormat::Dxt1 : IconFormat::Dxt5;
            needed       = static_cast<std::size_t>(image.width / 4) * (image.height / 4) * (file[6] == kDxt1 ? 8u : 16u);
            break;
        default:
            return std::nullopt;
    }
    const std::size_t base   = ReadBig32(file + 0x14);
    const std::size_t offset = ReadBig32(file + 0x18);
    const std::size_t bytes  = ReadBig32(file + 0x1C);
    if (base < kHeaderBytes || bytes < needed || base > size || offset > size - base || needed > size - base - offset)
        return std::nullopt;
    image.data  = file + base + offset;
    image.bytes = needed;
    return image;
}

} // namespace bahamut_client::xhbar
