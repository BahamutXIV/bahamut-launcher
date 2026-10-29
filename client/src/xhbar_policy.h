#pragma once

#include <array>
#include <cstddef>
#include <cstdint>

namespace bahamut_client
{

// Logical controls, not retail PadDevice fields or XInput bit positions.
namespace xhbar
{

inline constexpr std::uint16_t kSlots         = 0x00ff;
inline constexpr std::uint16_t kLeftTrigger   = 0x0100;
inline constexpr std::uint16_t kRightTrigger  = 0x0200;
inline constexpr std::uint16_t kRightShoulder = 0x0400;
inline constexpr std::uint16_t kUp            = 0x0001;
inline constexpr std::uint16_t kDown          = 0x0002;

enum class Side
{
    None,
    Left,
    Right,
    RightThenLeft,
    LeftThenRight
};

struct Generation
{
    std::uint64_t session                             = 0;
    std::uint64_t layout                              = 0;
    std::uint64_t actions                             = 0;
    bool          operator==(const Generation&) const = default;
};

struct Sample
{
    std::uint16_t held      = 0;
    bool          connected = false;
    // False emits no edges and inhibits held controls.
    bool       playable                = false;
    bool       selectionChordAvailable = false;
    Generation generation{};
};

struct SlotEdge
{
    std::size_t   slot = 0;
    Generation    generation{};
    std::uint64_t ownershipEpoch = 0;
};

struct Routing
{
    std::uint16_t           consume = 0;
    Side                    side    = Side::None;
    int                     cycle   = 0;
    std::array<SlotEdge, 8> edges{};
    std::size_t             edgeCount = 0;
};

inline constexpr std::size_t kSlotEdges = 32;

// Absolute action bar target; page 0 means unbound.
struct ShortcutTarget
{
    int page   = 0;
    int button = 0;
};

ShortcutTarget Shortcut(std::size_t slot);

// Update once per pad update on the pad thread; keep alive while disabled so captured holds cannot leak.
class InputPolicy
{
public:
    Routing Update(const Sample& sample);
    bool    IsCurrent(const SlotEdge& edge, const Sample& context) const;

private:
    std::uint16_t previous_  = 0;
    std::uint16_t owned_     = 0;
    std::uint16_t inhibited_ = 0;
    Generation    generation_{};
    Side          side_           = Side::None;
    bool          synchronized_   = false;
    bool          playable_       = false;
    std::uint64_t ownershipEpoch_ = 0;
};

} // namespace xhbar
} // namespace bahamut_client
