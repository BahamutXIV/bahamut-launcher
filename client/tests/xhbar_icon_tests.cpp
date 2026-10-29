#include "xhbar_icon.h"

#include <iostream>
#include <stdexcept>
#include <vector>

using namespace bahamut_client::xhbar;

namespace
{

void Require(bool condition, const char* reason)
{
    if (!condition)
        throw std::runtime_error(reason);
}

// Header bytes of retail data/1C/5C/00/9D.DAT (icon 30157, 64x64 DXT5).
std::vector<unsigned char> RetailHeader()
{
    std::vector<unsigned char> file = {
        0x47, 0x54, 0x45, 0x58, 0x01, 0x01, 0x1a, 0x01, 0x02, 0x00, 0x00, 0x40, 0x00, 0x40, 0x00, 0x01, 0x00, 0x00, 0x00, 0x18, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00
    };
    file.resize(file.size() + 0x1000, 0xAB);
    return file;
}

void Paths()
{
    Require(IconResourcePath(30157) == "data/1C/5C/00/9D.DAT", "retail Cure icon path");
    Require(IconResourcePath(0) == "data/1C/59/00/00.DAT", "first band start");
    Require(IconResourcePath(99999) == "data/1C/62/27/0F.DAT", "last band end");
    Require(!IconResourcePath(100000).has_value(), "icon past the bands");
}

void RetailIcon()
{
    const auto file  = RetailHeader();
    const auto image = ParseIcon(file.data(), file.size());
    Require(image.has_value(), "retail header rejected");
    Require(image->width == 64 && image->height == 64 && image->format == IconFormat::Dxt5, "retail extent or format");
    Require(image->data == file.data() + 0x20 && image->bytes == 0x1000, "retail base mip");
}

void Rejections()
{
    auto file = RetailHeader();
    Require(!ParseIcon(file.data(), file.size() - 1).has_value(), "truncated mip accepted");
    Require(!ParseIcon(file.data(), 0x1F).has_value(), "short header accepted");
    auto badMagic = file;
    badMagic[0]   = 'X';
    Require(!ParseIcon(badMagic.data(), badMagic.size()).has_value(), "bad magic accepted");
    auto badFormat = file;
    badFormat[6]   = 25;
    Require(!ParseIcon(badFormat.data(), badFormat.size()).has_value(), "unknown format accepted");
    auto smallSize  = file;
    smallSize[0x1E] = 0x0F;
    Require(!ParseIcon(smallSize.data(), smallSize.size()).has_value(), "short mip size accepted");
    auto offset  = file;
    offset[0x1B] = 0x10;
    Require(!ParseIcon(offset.data(), offset.size()).has_value(), "mip past end accepted");
    auto odd  = file;
    odd[0x0B] = 0x3E;
    Require(!ParseIcon(odd.data(), odd.size()).has_value(), "partial DXT block accepted");
    auto noMip = file;
    noMip[7]   = 0;
    Require(!ParseIcon(noMip.data(), noMip.size()).has_value(), "zero mips accepted");
}

} // namespace

int main()
{
    try
    {
        Paths();
        RetailIcon();
        Rejections();
    }
    catch (const std::exception& error)
    {
        std::cerr << error.what() << '\n';
        return 1;
    }
    std::cout << "xhbar icon path and GTEX parsing passed\n";
    return 0;
}
