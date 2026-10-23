#include "seasonal_catalog.h"

#include <algorithm>
#include <array>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <string_view>
#include <vector>

namespace
{

using bahamut_client::seasonal::Catalog;
using bahamut_client::seasonal::ClassifyDescriptor;
using bahamut_client::seasonal::ClassifyTimeline;
using bahamut_client::seasonal::LayoutInputCount;
using bahamut_client::seasonal::LayoutInputs;
using bahamut_client::seasonal::TimelineDescriptor;
using bahamut_client::seasonal::TimelineRole;

constexpr std::uint64_t kFnvOffset       = 0xCBF29CE484222325ull;
constexpr std::uint64_t kFnvPrime        = 0x100000001B3ull;
constexpr std::uint32_t kWeatherMask     = 0xFFFF0000u;
constexpr std::uint32_t kWeatherSelected = 0xFFFFFFFFu;

std::uint64_t Fnv1a64(const std::vector<std::uint8_t>& bytes)
{
    std::uint64_t result = kFnvOffset;
    for (const auto byte : bytes)
    {
        result ^= byte;
        result *= kFnvPrime;
    }
    return result;
}

std::uint32_t WeatherId(std::size_t index)
{
    return index < 17 ? 8001u + static_cast<std::uint32_t>(index)
                      : 8027u + static_cast<std::uint32_t>(index - 17);
}

bool Selected(TimelineRole role, std::uint32_t weather_id)
{
    switch (role)
    {
        case TimelineRole::HalloweenShow:
            return weather_id == 8027;
        case TimelineRole::HalloweenHide:
            return weather_id != 8027;
        case TimelineRole::WinterShow:
            return false;
        case TimelineRole::WinterHide:
            return true;
        case TimelineRole::None:
            return false;
    }
    return false;
}

struct FakeMemory
{
    explicit FakeMemory(std::uintptr_t address)
    : base(address)
    , bytes(0x300000, 0)
    {
    }

    static bool Read(void*          context,
                     std::uintptr_t address,
                     void*          destination,
                     std::size_t    size)
    {
        const auto* memory = static_cast<const FakeMemory*>(context);
        if (address < memory->base || address - memory->base > memory->bytes.size() ||
            size > memory->bytes.size() - static_cast<std::size_t>(address - memory->base))
        {
            return false;
        }
        std::memcpy(destination,
                    memory->bytes.data() + static_cast<std::size_t>(address - memory->base),
                    size);
        return true;
    }

    template <typename T>
    void Store(std::uintptr_t address, const T& value)
    {
        StoreBytes(address, &value, sizeof(value));
    }

    void StoreBytes(std::uintptr_t address, const void* source, std::size_t size)
    {
        if (address < base || address - base > bytes.size() || size > bytes.size() - static_cast<std::size_t>(address - base))
        {
            valid = false;
            return;
        }
        std::memcpy(bytes.data() + static_cast<std::size_t>(address - base), source, size);
    }

    void StoreString(std::uintptr_t address, std::string_view value)
    {
        StoreBytes(address, value.data(), value.size());
        const char terminator = '\0';
        StoreBytes(address + value.size(), &terminator, sizeof(terminator));
    }

    bool                      valid = true;
    std::uintptr_t            base;
    std::vector<std::uint8_t> bytes;
};

bool FailRead(void*, std::uintptr_t, void*, std::size_t)
{
    return false;
}

struct SyntheticRecord
{
    FakeMemory         memory;
    TimelineDescriptor descriptor;
    std::uintptr_t     timeline;
    std::uintptr_t     owner;
};

SyntheticRecord MakeSynthetic(const TimelineDescriptor& source, std::uintptr_t base)
{
    SyntheticRecord result{ FakeMemory(base), source, base + source.timeline_offset, base + source.owner_offset };
    auto&           memory     = result.memory;
    const auto&     descriptor = result.descriptor;

    memory.Store(result.timeline + 0x08, static_cast<std::uint32_t>(base + descriptor.name_offset));
    memory.Store(result.timeline + 0x14, static_cast<std::uint32_t>(base + descriptor.scb_offset));
    memory.Store(result.timeline + 0x18, descriptor.scb_size);
    memory.Store(result.timeline + 0x1C, static_cast<std::uint32_t>(base + descriptor.property_offset));
    memory.Store(result.owner + 0x08, static_cast<std::uint32_t>(base + descriptor.owner_name_offset));
    memory.Store(base + descriptor.property_offset + 0x08,
                 static_cast<std::uint32_t>(base + descriptor.property_name_offset));
    memory.Store(base + descriptor.property_offset + 0x10,
                 static_cast<std::uint32_t>(base + descriptor.vector_offset));
    memory.Store(base + descriptor.property_offset + 0x14, descriptor.vector_count);

    memory.StoreString(base + descriptor.name_offset, descriptor.timeline_name);
    memory.StoreString(base + descriptor.owner_name_offset, descriptor.owner_name);
    memory.StoreString(base + descriptor.owner_child_name_offset, descriptor.owner_child_name);
    memory.StoreString(base + descriptor.property_name_offset, descriptor.property_name);

    for (std::size_t index = 0; index < descriptor.vector_count; ++index)
    {
        const auto weather_id = WeatherId(index);
        const auto selected   = Selected(descriptor.role, weather_id);
        const auto first      = selected ? weather_id | kWeatherMask : weather_id;
        const auto second     = selected ? kWeatherSelected : 0u;
        memory.Store(base + descriptor.vector_offset + index * 8u, first);
        memory.Store(base + descriptor.vector_offset + index * 8u + 4u, second);
    }

    std::vector<std::uint8_t> scb(descriptor.scb_size, 0);
    std::memcpy(scb.data(), "SEDBSCB\0", 8);
    const std::uint16_t header_size = 0x30;
    const std::uint32_t total_size  = descriptor.scb_size;
    std::memcpy(scb.data() + 0x0E, &header_size, sizeof(header_size));
    std::memcpy(scb.data() + 0x10, &total_size, sizeof(total_size));
    memory.StoreBytes(base + descriptor.scb_offset, scb.data(), scb.size());
    result.descriptor.scb_fingerprint = Fnv1a64(scb);
    return result;
}

bool CatalogMetadata()
{
    if (Catalog().size() != 130 || LayoutInputCount() != 6 || LayoutInputs().size() != 6)
    {
        return false;
    }
    std::array<std::size_t, 5> roles{};
    for (const auto& descriptor : Catalog())
    {
        const auto index = static_cast<std::size_t>(descriptor.role);
        if (index >= roles.size() || descriptor.timeline_name == nullptr || descriptor.owner_name == nullptr ||
            descriptor.property_name == nullptr || descriptor.scb_sha256 == nullptr ||
            descriptor.source_sha256 == nullptr || descriptor.source_path == nullptr)
        {
            return false;
        }
        ++roles[index];
    }
    return roles[1] == 60 && roles[2] == 60 && roles[3] == 5 && roles[4] == 5;
}

bool RelocationAndGuards()
{
    const auto source = Catalog().front();
    for (const auto base : { std::uintptr_t{ 0x10000000 }, std::uintptr_t{ 0x20000000 } })
    {
        auto record = MakeSynthetic(source, base);
        if (!record.memory.valid ||
            ClassifyDescriptor(FakeMemory::Read,
                               &record.memory,
                               record.descriptor,
                               record.timeline,
                               record.owner,
                               source.city_root) != TimelineRole::HalloweenShow)
        {
            return false;
        }

        if (ClassifyDescriptor(FakeMemory::Read,
                               &record.memory,
                               record.descriptor,
                               record.timeline,
                               record.owner + 4,
                               source.city_root) != TimelineRole::None)
        {
            return false;
        }

        record.memory.StoreString(base + source.name_offset, "wrong_timeline");
        if (ClassifyDescriptor(FakeMemory::Read,
                               &record.memory,
                               record.descriptor,
                               record.timeline,
                               record.owner,
                               source.city_root) != TimelineRole::None)
        {
            return false;
        }
        record.memory.StoreString(base + source.name_offset, source.timeline_name);

        const auto          vector_first = base + source.vector_offset;
        const std::uint32_t wrong_vector = 0;
        record.memory.Store(vector_first, wrong_vector);
        if (ClassifyDescriptor(FakeMemory::Read,
                               &record.memory,
                               record.descriptor,
                               record.timeline,
                               record.owner,
                               source.city_root) != TimelineRole::None)
        {
            return false;
        }
        const auto weather_id = WeatherId(0);
        record.memory.Store(vector_first, weather_id);

        auto wrong_fingerprint = record.descriptor;
        ++wrong_fingerprint.scb_fingerprint;
        if (ClassifyDescriptor(FakeMemory::Read,
                               &record.memory,
                               wrong_fingerprint,
                               record.timeline,
                               record.owner,
                               source.city_root) != TimelineRole::None)
        {
            return false;
        }

        if (ClassifyDescriptor(FakeMemory::Read,
                               &record.memory,
                               record.descriptor,
                               record.timeline + 4,
                               record.owner,
                               source.city_root) != TimelineRole::None ||
            ClassifyTimeline(FakeMemory::Read,
                             &record.memory,
                             record.timeline + 4,
                             record.owner,
                             source.city_root) != TimelineRole::None)
        {
            return false;
        }
    }
    return true;
}

bool GridaniaAliases()
{
    const auto source = *std::find_if(Catalog().begin(), Catalog().end(), [](const auto& descriptor)
                                      {
                                          return descriptor.city_root == 103;
                                      });
    auto       record = MakeSynthetic(source, 0x30000000);
    for (const auto root : { 103u, 106u, 115u, 122u, 123u })
    {
        if (ClassifyDescriptor(FakeMemory::Read,
                               &record.memory,
                               record.descriptor,
                               record.timeline,
                               record.owner,
                               root) != source.role)
        {
            return false;
        }
    }
    return ClassifyDescriptor(FakeMemory::Read,
                              &record.memory,
                              record.descriptor,
                              record.timeline,
                              record.owner,
                              104) == TimelineRole::None &&
           ClassifyDescriptor(FakeMemory::Read,
                              &record.memory,
                              record.descriptor,
                              record.timeline,
                              record.owner,
                              999) == TimelineRole::None;
}

} // namespace

int main()
{
    if (!CatalogMetadata())
    {
        return 1;
    }
    if (!RelocationAndGuards())
    {
        return 2;
    }
    if (!GridaniaAliases())
    {
        return 3;
    }
    if (ClassifyTimeline(nullptr, nullptr, 1, 1, 101) != TimelineRole::None)
    {
        return 4;
    }
    const auto source = Catalog().front();
    if (ClassifyDescriptor(FailRead, nullptr, source, 1, 1, source.city_root) != TimelineRole::None)
    {
        return 5;
    }
    std::cout << "seasonal catalog tests passed: 130 records, two reload bases, owner/name/vector/fingerprint guards, aliases and unrelated timeline rejection\n";
    return 0;
}
