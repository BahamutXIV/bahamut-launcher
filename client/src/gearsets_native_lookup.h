#pragma once

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string_view>

namespace bahamut_client
{

// Read-only access to retail memory at 32-bit addresses. read returns false
// when any requested byte is unreadable.
struct GearsetsNativeMemory
{
    bool (*read)(void* context, std::uint32_t address, void* out, std::size_t size) = nullptr;
    void* context                                                                   = nullptr;

    template <typename T>
    [[nodiscard]] std::optional<T> Get(std::uint32_t address) const
    {
        T value{};
        if (read == nullptr || address == 0u || !read(context, address, &value, sizeof(value)))
        {
            return std::nullopt;
        }
        return value;
    }
};

enum class GearsetsStaticActorState : std::uint8_t
{
    Loaded,
    NotLoaded,
    Unreadable,
    Unmodeled, // the game would take a path these reads do not model
};

struct GearsetsStaticActorLookup
{
    GearsetsStaticActorState state     = GearsetsStaticActorState::Unreadable;
    std::uint32_t            reference = 0;
    std::uint32_t            control   = 0;
};

std::string_view GearsetsStaticActorStateName(GearsetsStaticActorState state);

// Start bucket of the engine's dense hash probe (0x00CEB9C0): Park-Miller
// over key ^ 0xDEADBEEF with truncating division (0x009D6E98).
std::uint32_t GearsetsNativeHashBucket(std::uint32_t key, std::uint32_t mask);

// The answer _getStaticActor(actorId) gets from its loaded test (0x00CC7A50)
// before it would create a wait object and yield, by reads only. engine is
// the GameEngine reached through container+0x518 and +4.
GearsetsStaticActorLookup FindLoadedStaticActor(const GearsetsNativeMemory& memory, std::uint32_t engine, std::uint32_t actorId);

} // namespace bahamut_client
