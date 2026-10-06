#pragma once

#include "world_projection.h"

#include <array>
#include <cstddef>

namespace bahamut_client
{

constexpr std::size_t kTargetlineArcSampleCount  = 48u;
constexpr std::size_t kTargetlineArcSegmentCount = kTargetlineArcSampleCount - 1u;

struct TargetlineArcStyle
{
    float endpointLift = 0.0F;
    // The peak rise above the line between the lifted endpoints.
    float archHeight = 0.0F;
};

struct TargetlineArcSegments
{
    std::array<ScreenSegment, kTargetlineArcSegmentCount> segments{};
    std::size_t                                           count = 0u;
};

// Samples a world-space quadratic with world Y as its up axis. Each adjacent
// sample pair is clipped independently, so offscreen gaps remain disconnected.
[[nodiscard]] TargetlineArcSegments ProjectTargetlineArc(
    const SceneProjection&    projection,
    const WorldPosition&      source,
    const WorldPosition&      target,
    const TargetlineArcStyle& style);

} // namespace bahamut_client
