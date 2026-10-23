#include "seasonal_catalog.h"

#include <array>
#include <cstring>
#include <limits>
#include <string_view>

namespace bahamut_client::seasonal
{
namespace
{

#include "seasonal_catalog.inc"

constexpr std::uint32_t kTimelineNameField        = 0x08;
constexpr std::uint32_t kTimelineScbField         = 0x14;
constexpr std::uint32_t kTimelineScbSizeField     = 0x18;
constexpr std::uint32_t kTimelinePropertyField    = 0x1C;
constexpr std::uint32_t kOwnerNameField           = 0x08;
constexpr std::uint32_t kPropertyNameField        = 0x08;
constexpr std::uint32_t kPropertyVectorField      = 0x10;
constexpr std::uint32_t kPropertyVectorCountField = 0x14;
constexpr std::uint32_t kTimelineRecordSize       = 0x28;
constexpr std::uint32_t kOwnerRecordSize          = 0x50;
constexpr std::uint32_t kOwnerChildRecordSize     = 0x30;
constexpr std::uint32_t kPropertyRecordSize       = 0x18;
constexpr std::uint32_t kScbHeaderBytes           = 0x14;
constexpr std::uint32_t kScbSizeField             = 0x10;
constexpr std::uint16_t kScbHeaderVersionSize     = 0x30;
constexpr std::uint32_t kWeatherMask              = 0xFFFF0000u;
constexpr std::uint32_t kWeatherSelected          = 0xFFFFFFFFu;
constexpr std::uint64_t kFnvOffset                = 0xCBF29CE484222325ull;
constexpr std::uint64_t kFnvPrime                 = 0x100000001B3ull;

static_assert(sizeof(kSeasonalCatalog) / sizeof(kSeasonalCatalog[0]) == 130);
static_assert(sizeof(kSeasonalInputs) / sizeof(kSeasonalInputs[0]) == 6);

bool AddOffset(std::uintptr_t base, std::uint32_t offset, std::uintptr_t& result) noexcept
{
    if (offset > std::numeric_limits<std::uintptr_t>::max() - base)
    {
        return false;
    }
    result = base + offset;
    return true;
}

bool ReadBytes(ReadMemory     read,
               void*          context,
               std::uintptr_t address,
               void*          destination,
               std::size_t    size) noexcept
{
    return read != nullptr && size != 0 && read(context, address, destination, size);
}

template <typename T>
bool ReadValue(ReadMemory read, void* context, std::uintptr_t address, T& value) noexcept
{
    return ReadBytes(read, context, address, &value, sizeof(value));
}

bool ReadFieldAddress(ReadMemory      read,
                      void*           context,
                      std::uintptr_t  record,
                      std::uint32_t   field,
                      std::uintptr_t& value) noexcept
{
    std::uintptr_t address = 0;
    std::uint32_t  raw     = 0;
    return AddOffset(record, field, address) && ReadValue(read, context, address, raw) &&
           (value = static_cast<std::uintptr_t>(raw), true);
}

bool ReadFieldU32(ReadMemory     read,
                  void*          context,
                  std::uintptr_t record,
                  std::uint32_t  field,
                  std::uint32_t& value) noexcept
{
    std::uintptr_t address = 0;
    return AddOffset(record, field, address) && ReadValue(read, context, address, value);
}

bool ReadExpectedString(ReadMemory     read,
                        void*          context,
                        std::uintptr_t address,
                        const char*    expected) noexcept
{
    if (expected == nullptr)
    {
        return false;
    }
    const std::string_view expected_view(expected);
    if (expected_view.size() >= 256)
    {
        return false;
    }
    std::array<char, 256> actual{};
    const auto            bytes = expected_view.size() + 1;
    if (!ReadBytes(read, context, address, actual.data(), bytes))
    {
        return false;
    }
    return actual[expected_view.size()] == '\0' &&
           std::memcmp(actual.data(), expected_view.data(), expected_view.size()) == 0;
}

bool ReadExpectedName(ReadMemory     read,
                      void*          context,
                      std::uintptr_t record,
                      std::uint32_t  field,
                      std::uintptr_t expected_address,
                      const char*    expected_name) noexcept
{
    std::uintptr_t actual_address = 0;
    return ReadFieldAddress(read, context, record, field, actual_address) &&
           actual_address == expected_address &&
           ReadExpectedString(read, context, actual_address, expected_name);
}

std::uint32_t WeatherId(std::size_t index) noexcept
{
    return index < 17 ? 8001u + static_cast<std::uint32_t>(index)
                      : 8027u + static_cast<std::uint32_t>(index - 17);
}

bool WeatherSelected(TimelineRole role, std::uint32_t weather_id) noexcept
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

bool ValidateVector(ReadMemory                read,
                    void*                     context,
                    const TimelineDescriptor& descriptor,
                    std::uintptr_t            vector) noexcept
{
    if (descriptor.vector_count != kWeatherVectorCount ||
        descriptor.vector_size != descriptor.vector_count * 8u)
    {
        return false;
    }
    for (std::size_t index = 0; index < kWeatherVectorCount; ++index)
    {
        const auto     byte_offset    = static_cast<std::uint32_t>(index * 8u);
        std::uintptr_t first_address  = 0;
        std::uintptr_t second_address = 0;
        std::uint32_t  first          = 0;
        std::uint32_t  second         = 0;
        if (!AddOffset(vector, byte_offset, first_address) ||
            !AddOffset(first_address, 4u, second_address) ||
            !ReadValue(read, context, first_address, first) ||
            !ReadValue(read, context, second_address, second))
        {
            return false;
        }
        const auto weather_id      = WeatherId(index);
        const bool selected        = WeatherSelected(descriptor.role, weather_id);
        const auto expected_first  = selected ? weather_id | kWeatherMask : weather_id;
        const auto expected_second = selected ? kWeatherSelected : 0u;
        if (first != expected_first || second != expected_second)
        {
            return false;
        }
    }
    return true;
}

bool ValidateScb(ReadMemory                read,
                 void*                     context,
                 const TimelineDescriptor& descriptor,
                 std::uintptr_t            scb) noexcept
{
    if (descriptor.scb_size < kScbHeaderBytes || descriptor.scb_size == 0)
    {
        return false;
    }
    std::array<std::uint8_t, 8> magic{};
    std::uint16_t               header_size           = 0;
    std::uint32_t               declared_size         = 0;
    std::uintptr_t              header_size_address   = 0;
    std::uintptr_t              declared_size_address = 0;
    if (!ReadBytes(read, context, scb, magic.data(), magic.size()) ||
        std::memcmp(magic.data(), "SEDBSCB\0", magic.size()) != 0 ||
        !AddOffset(scb, 0x0E, header_size_address) ||
        !AddOffset(scb, kScbSizeField, declared_size_address) ||
        !ReadValue(read, context, header_size_address, header_size) ||
        !ReadValue(read, context, declared_size_address, declared_size) ||
        header_size != kScbHeaderVersionSize || declared_size != descriptor.scb_size)
    {
        return false;
    }

    std::uint64_t                 fingerprint = kFnvOffset;
    std::array<std::uint8_t, 256> bytes{};
    std::size_t                   offset = 0;
    while (offset < descriptor.scb_size)
    {
        const auto     remaining     = static_cast<std::size_t>(descriptor.scb_size) - offset;
        const auto     chunk_size    = remaining < bytes.size() ? remaining : bytes.size();
        std::uintptr_t chunk_address = 0;
        if (!AddOffset(scb, static_cast<std::uint32_t>(offset), chunk_address) ||
            !ReadBytes(read, context, chunk_address, bytes.data(), chunk_size))
        {
            return false;
        }
        for (std::size_t index = 0; index < chunk_size; ++index)
        {
            fingerprint ^= bytes[index];
            fingerprint *= kFnvPrime;
        }
        offset += chunk_size;
    }
    return fingerprint == descriptor.scb_fingerprint;
}

bool RootMatches(const TimelineDescriptor& descriptor, std::uint32_t city_root) noexcept
{
    if (descriptor.city_root == city_root)
    {
        return true;
    }
    return descriptor.city_root == kGridaniaRoot &&
           (city_root == 106 || city_root == 115 || city_root == 122 || city_root == 123);
}

bool ValidateDescriptor(ReadMemory                read,
                        void*                     context,
                        const TimelineDescriptor& descriptor,
                        std::uintptr_t            timeline_record,
                        std::uintptr_t            owner_record) noexcept
{
    if (descriptor.role == TimelineRole::None || descriptor.timeline_size != kTimelineRecordSize ||
        descriptor.owner_size != kOwnerRecordSize || descriptor.owner_child_size != kOwnerChildRecordSize ||
        descriptor.property_size != kPropertyRecordSize || descriptor.vector_count != kWeatherVectorCount ||
        descriptor.vector_size != descriptor.vector_count * 8u || descriptor.scb_size == 0 ||
        timeline_record == 0 || owner_record == 0 || timeline_record < descriptor.timeline_offset)
    {
        return false;
    }

    const auto base = timeline_record - descriptor.timeline_offset;
    if (base == 0)
    {
        return false;
    }
    std::uintptr_t expected_owner         = 0;
    std::uintptr_t expected_name          = 0;
    std::uintptr_t expected_owner_name    = 0;
    std::uintptr_t expected_property      = 0;
    std::uintptr_t expected_property_name = 0;
    std::uintptr_t expected_vector        = 0;
    std::uintptr_t expected_scb           = 0;
    if (!AddOffset(base, descriptor.owner_offset, expected_owner) ||
        !AddOffset(base, descriptor.name_offset, expected_name) ||
        !AddOffset(base, descriptor.owner_name_offset, expected_owner_name) ||
        !AddOffset(base, descriptor.property_offset, expected_property) ||
        !AddOffset(base, descriptor.property_name_offset, expected_property_name) ||
        !AddOffset(base, descriptor.vector_offset, expected_vector) ||
        !AddOffset(base, descriptor.scb_offset, expected_scb) || expected_owner != owner_record)
    {
        return false;
    }

    std::uintptr_t property_record = 0;
    std::uintptr_t scb             = 0;
    std::uint32_t  scb_size        = 0;
    if (!ReadExpectedName(read,
                          context,
                          timeline_record,
                          kTimelineNameField,
                          expected_name,
                          descriptor.timeline_name) ||
        !ReadFieldAddress(read, context, timeline_record, kTimelineScbField, scb) ||
        scb != expected_scb ||
        !ReadFieldU32(read, context, timeline_record, kTimelineScbSizeField, scb_size) ||
        scb_size != descriptor.scb_size ||
        !ReadFieldAddress(read, context, timeline_record, kTimelinePropertyField, property_record) ||
        property_record != expected_property ||
        !ReadExpectedName(read,
                          context,
                          property_record,
                          kPropertyNameField,
                          expected_property_name,
                          descriptor.property_name) ||
        !ReadExpectedName(read,
                          context,
                          owner_record,
                          kOwnerNameField,
                          expected_owner_name,
                          descriptor.owner_name))
    {
        return false;
    }

    std::uintptr_t vector       = 0;
    std::uint32_t  vector_count = 0;
    if (!ReadFieldAddress(read, context, property_record, kPropertyVectorField, vector) ||
        vector != expected_vector ||
        !ReadFieldU32(read, context, property_record, kPropertyVectorCountField, vector_count) ||
        vector_count != descriptor.vector_count || !ValidateVector(read, context, descriptor, vector))
    {
        return false;
    }
    return ValidateScb(read, context, descriptor, scb);
}

} // namespace

const LayoutInput* LayoutInputData() noexcept
{
    return kSeasonalInputs;
}

std::size_t LayoutInputCount() noexcept
{
    return sizeof(kSeasonalInputs) / sizeof(kSeasonalInputs[0]);
}

std::span<const LayoutInput> LayoutInputs() noexcept
{
    return { kSeasonalInputs, LayoutInputCount() };
}

const TimelineDescriptor* CatalogData() noexcept
{
    return kSeasonalCatalog;
}

std::size_t CatalogSize() noexcept
{
    return sizeof(kSeasonalCatalog) / sizeof(kSeasonalCatalog[0]);
}

std::span<const TimelineDescriptor> Catalog() noexcept
{
    return { kSeasonalCatalog, CatalogSize() };
}

TimelineRole ClassifyDescriptor(ReadMemory                read,
                                void*                     context,
                                const TimelineDescriptor& descriptor,
                                std::uintptr_t            timeline_record,
                                std::uintptr_t            owner_record,
                                std::uint32_t             city_root) noexcept
{
    if (read == nullptr || !RootMatches(descriptor, city_root))
    {
        return TimelineRole::None;
    }
    return ValidateDescriptor(read, context, descriptor, timeline_record, owner_record) ? descriptor.role
                                                                                        : TimelineRole::None;
}

TimelineRole ClassifyTimeline(ReadMemory     read,
                              void*          context,
                              std::uintptr_t timeline_record,
                              std::uintptr_t owner_record,
                              std::uint32_t  city_root) noexcept
{
    if (read == nullptr || timeline_record == 0 || owner_record == 0)
    {
        return TimelineRole::None;
    }
    for (const auto& descriptor : Catalog())
    {
        const auto role = ClassifyDescriptor(read, context, descriptor, timeline_record, owner_record, city_root);
        if (role != TimelineRole::None)
        {
            return role;
        }
    }
    return TimelineRole::None;
}

} // namespace bahamut_client::seasonal
