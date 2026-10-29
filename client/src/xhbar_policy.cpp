#include "xhbar_policy.h"

namespace bahamut_client::xhbar
{

namespace
{

Side SelectSide(Side current, std::uint16_t triggers)
{
    if (triggers == (kLeftTrigger | kRightTrigger))
        return current == Side::Left || current == Side::LeftThenRight ? Side::LeftThenRight : Side::RightThenLeft;
    if (triggers == kLeftTrigger)
        return Side::Left;
    if (triggers == kRightTrigger)
        return Side::Right;
    return Side::None;
}

std::size_t EdgeBase(Side side)
{
    switch (side)
    {
        case Side::Right:
            return 8;
        case Side::RightThenLeft:
            return 16;
        case Side::LeftThenRight:
            return 24;
        default:
            return 0;
    }
}

} // namespace

Routing InputPolicy::Update(const Sample& sample)
{
    Routing result;
    if (!sample.connected)
    {
        if (synchronized_)
            ++ownershipEpoch_;
        inhibited_ |= previous_;
        synchronized_ = false;
        playable_     = false;
        side_         = Side::None;
        return result;
    }

    // Consume through release; never capture a hold already forwarded to native input.
    result.consume = owned_;
    owned_ &= sample.held;
    inhibited_ &= sample.held;
    const auto fresh          = static_cast<std::uint16_t>(sample.held & ~previous_);
    const bool changed        = !synchronized_ || !(sample.generation == generation_);
    const bool sessionChanged = !synchronized_ || sample.generation.session != generation_.session;
    previous_                 = sample.held;
    generation_               = sample.generation;
    synchronized_             = true;
    if (changed || (playable_ && !sample.playable))
        ++ownershipEpoch_;
    playable_ = sample.playable;
    if (changed || !sample.playable)
    {
        if (sessionChanged || !sample.playable)
        {
            inhibited_ |= sample.held;
            side_ = Side::None;
        }
        else
        {
            const auto triggers = static_cast<std::uint16_t>(owned_ & sample.held & ~inhibited_ & (kLeftTrigger | kRightTrigger));
            side_               = SelectSide(side_, triggers);
            if (triggers != 0)
                owned_ |= fresh & kSlots;
            if (sample.selectionChordAvailable && (owned_ & sample.held & ~inhibited_ & kRightShoulder))
                owned_ |= fresh & (kUp | kDown);
            inhibited_ |= sample.held & kSlots;
            result.consume |= owned_;
            result.side = side_;
        }
        return result;
    }

    owned_ |= fresh & (kLeftTrigger | kRightTrigger);
    const auto triggers = static_cast<std::uint16_t>(owned_ & sample.held & ~inhibited_ & (kLeftTrigger | kRightTrigger));
    side_               = SelectSide(side_, triggers);

    const bool cycling = sample.selectionChordAvailable && (sample.held & ~inhibited_ & (fresh | owned_) & kRightShoulder) && (sample.held & (kUp | kDown));
    if (cycling)
    {
        owned_ |= fresh & (kRightShoulder | kUp | kDown);
        if (side_ != Side::None)
            owned_ |= fresh & kSlots;
        const auto direction = static_cast<std::uint16_t>(sample.held & (kUp | kDown));
        if (direction == kUp && (fresh & kUp))
            result.cycle = -1;
        else if (direction == kDown && (fresh & kDown))
            result.cycle = 1;
    }
    else if (side_ != Side::None)
    {
        owned_ |= fresh & kSlots;
        for (std::size_t index = 0; index < 8; ++index)
        {
            if (fresh & (1u << index))
                result.edges[result.edgeCount++] = { EdgeBase(side_) + index, sample.generation, ownershipEpoch_ };
        }
    }

    result.consume |= owned_;
    result.side = side_;
    return result;
}

namespace
{

// Positions 1-32; edges per side: Up, Down, Left, Right, A, B, X, Y.
constexpr std::array<int, kSlotEdges> kPositions      = { 13, 15, 16, 14, 11, 10, 12, 9, 5, 7, 8, 6, 3, 2, 4, 1, 21, 23, 24, 22, 19, 18, 20, 17, 29, 31, 32, 30, 27, 26, 28, 25 };
constexpr int                         kBoundPositions = 30;

} // namespace

ShortcutTarget Shortcut(std::size_t slot)
{
    if (slot >= kPositions.size() || kPositions[slot] > kBoundPositions)
        return {};
    return { (kPositions[slot] - 1) / 10 + 1, (kPositions[slot] - 1) % 10 + 1 };
}

bool InputPolicy::IsCurrent(const SlotEdge& edge, const Sample& context) const
{
    return synchronized_ && playable_ && context.connected && context.playable && edge.slot < kSlotEdges && edge.generation == context.generation && edge.generation == generation_ && edge.ownershipEpoch == ownershipEpoch_;
}

} // namespace bahamut_client::xhbar
