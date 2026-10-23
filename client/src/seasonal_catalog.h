#pragma once

#include <cstddef>
#include <cstdint>
#include <span>

namespace bahamut_client::seasonal
{

enum class TimelineRole : std::uint8_t
{
    None,
    HalloweenShow,
    HalloweenHide,
    WinterShow,
    WinterHide,
};

using ReadMemory = bool (*)(void*          context,
                            std::uintptr_t address,
                            void*          destination,
                            std::size_t    size);

struct LayoutInput
{
    const char*   path;
    std::uint64_t bytes;
    const char*   sha256;
};

struct TimelineDescriptor
{
    std::uint32_t city_root;
    std::uint32_t resource_id;
    TimelineRole  role;

    std::uint32_t timeline_offset;
    std::uint32_t timeline_size;
    std::uint32_t owner_offset;
    std::uint32_t owner_size;
    std::uint32_t owner_child_offset;
    std::uint32_t owner_child_size;
    std::uint32_t owner_child_name_offset;
    std::uint32_t property_offset;
    std::uint32_t property_size;
    std::uint32_t vector_offset;
    std::uint32_t vector_size;
    std::uint32_t vector_count;

    std::uint32_t name_offset;
    std::uint32_t owner_name_offset;
    std::uint32_t property_name_offset;
    std::uint32_t scb_offset;
    std::uint32_t scb_size;
    std::uint64_t scb_fingerprint;

    const char* timeline_name;
    const char* owner_name;
    const char* owner_child_name;
    const char* property_name;
    const char* scb_sha256;
    const char* source_sha256;
    const char* source_path;
};

constexpr std::size_t   kWeatherVectorCount = 23;
constexpr std::uint32_t kGridaniaRoot       = 103;

const LayoutInput*           LayoutInputData() noexcept;
std::size_t                  LayoutInputCount() noexcept;
std::span<const LayoutInput> LayoutInputs() noexcept;

const TimelineDescriptor*           CatalogData() noexcept;
std::size_t                         CatalogSize() noexcept;
std::span<const TimelineDescriptor> Catalog() noexcept;

TimelineRole ClassifyDescriptor(ReadMemory                read,
                                void*                     context,
                                const TimelineDescriptor& descriptor,
                                std::uintptr_t            timeline_record,
                                std::uintptr_t            owner_record,
                                std::uint32_t             city_root) noexcept;

TimelineRole ClassifyTimeline(ReadMemory     read,
                              void*          context,
                              std::uintptr_t timeline_record,
                              std::uintptr_t owner_record,
                              std::uint32_t  city_root) noexcept;

} // namespace bahamut_client::seasonal
