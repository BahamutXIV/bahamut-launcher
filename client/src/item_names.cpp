#include "item_names.h"

#include <algorithm>
#include <iterator>

namespace bahamut_client
{
namespace
{

struct ItemName
{
    std::uint32_t    id;
    std::string_view name;
};

// Sorted by ID.
constexpr ItemName kItemNames[] = {
#include "item_names.inc"
};

} // namespace

std::optional<std::uint32_t> ItemIdByName(std::string_view name)
{
    const auto found = std::find_if(std::begin(kItemNames), std::end(kItemNames), [&](const ItemName& item)
                                    {
                                        return item.name == name;
                                    });
    if (found == std::end(kItemNames))
        return std::nullopt;
    return found->id;
}

std::string_view ItemNameById(std::uint32_t id)
{
    const auto found = std::lower_bound(std::begin(kItemNames), std::end(kItemNames), id, [](const ItemName& item, std::uint32_t value)
                                        {
                                            return item.id < value;
                                        });
    if (found == std::end(kItemNames) || found->id != id)
        return {};
    return found->name;
}

} // namespace bahamut_client
