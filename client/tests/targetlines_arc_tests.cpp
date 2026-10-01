#include "targetlines_arc.h"

#include <algorithm>
#include <cmath>
#include <iostream>
#include <limits>

namespace
{

bahamut_client::SceneProjection Perspective()
{
    bahamut_client::SceneProjection projection;
    projection.worldToClip = { 1.0F, 0.0F, 0.0F, 0.0F, 0.0F, 1.0F, 0.0F, 0.0F, 0.0F, 0.0F, 10.0F / 9.0F, 1.0F, 0.0F, 0.0F, -10.0F / 9.0F, 0.0F };
    projection.viewport    = { 0.0F, 0.0F, 800.0F, 600.0F };
    return projection;
}

bool Near(float actual, float expected)
{
    return std::fabs(actual - expected) < 0.01F;
}

bool Finite(const bahamut_client::ScreenSegment& segment)
{
    return std::isfinite(segment.source.x) && std::isfinite(segment.source.y) &&
           std::isfinite(segment.target.x) && std::isfinite(segment.target.y);
}

} // namespace

int main()
{
    using bahamut_client::ProjectTargetlineArc;
    using bahamut_client::TargetlineArcStyle;
    using bahamut_client::WorldPosition;

    const auto               projection = Perspective();
    const TargetlineArcStyle style{ 1.0F, 2.0F };
    const auto               curved = ProjectTargetlineArc(projection, { -1.0F, 0.0F, 5.0F }, { 1.0F, 0.0F, 5.0F }, style);
    if (curved.count != bahamut_client::kTargetlineArcSegmentCount || !Near(curved.segments.front().source.x, 320.0F) ||
        !Near(curved.segments.front().source.y, 240.0F) || !Near(curved.segments.back().target.x, 480.0F) ||
        !Near(curved.segments.back().target.y, 240.0F))
    {
        std::cerr << "arc did not preserve supplied endpoint lift or bounded sample count\n";
        return 1;
    }

    bool  raised  = false;
    float lowestY = std::numeric_limits<float>::infinity();
    for (std::size_t index = 0u; index < curved.count; ++index)
    {
        if (!Finite(curved.segments[index]))
        {
            std::cerr << "arc emitted a nonfinite screen segment\n";
            return 1;
        }
        raised  = raised || curved.segments[index].source.y < 230.0F || curved.segments[index].target.y < 230.0F;
        lowestY = std::min({ lowestY, curved.segments[index].source.y, curved.segments[index].target.y });
    }
    if (!raised || std::fabs(lowestY - 120.0F) >= 0.25F)
    {
        std::cerr << "arch height did not produce the requested peak rise\n";
        return 1;
    }

    const auto invalid = ProjectTargetlineArc(
        projection,
        { -1.0F, 0.0F, 5.0F },
        { 1.0F, 0.0F, 5.0F },
        TargetlineArcStyle{ std::numeric_limits<float>::quiet_NaN(), 2.0F });
    if (invalid.count != 0u ||
        ProjectTargetlineArc(projection, { 0.0F, 0.0F, 5.0F }, { 0.0F, 0.0F, 5.0F }, TargetlineArcStyle{ 1.0F, 2.0F }).count != 0u ||
        ProjectTargetlineArc(projection, { -1.0F, 0.0F, 5.0F }, { 1.0F, 0.0F, 5.0F }, TargetlineArcStyle{ -1.0F, 2.0F }).count != 0u ||
        ProjectTargetlineArc(projection,
                             { -1.0F, 0.0F, 5.0F },
                             { 1.0F, 0.0F, 5.0F },
                             TargetlineArcStyle{ 1.0F, std::numeric_limits<float>::infinity() })
                .count != 0u ||
        ProjectTargetlineArc(projection,
                             { std::numeric_limits<float>::infinity(), 0.0F, 5.0F },
                             { 1.0F, 0.0F, 5.0F },
                             style)
                .count != 0u)
    {
        std::cerr << "invalid arc input yielded drawable segments\n";
        return 1;
    }

    const auto disconnected = ProjectTargetlineArc(
        projection, { -1.0F, 0.0F, 5.0F }, { 1.0F, 0.0F, 5.0F }, TargetlineArcStyle{ 0.0F, 20.0F });
    if (disconnected.count == 0u || disconnected.count >= bahamut_client::kTargetlineArcSegmentCount)
    {
        std::cerr << "offscreen arc did not produce bounded clipped pieces\n";
        return 1;
    }
    bool gap = false;
    for (std::size_t index = 1u; index < disconnected.count; ++index)
    {
        gap = gap || std::fabs(disconnected.segments[index].source.x - disconnected.segments[index - 1u].target.x) > 2.0F;
    }
    if (disconnected.count < 2u || !gap)
    {
        std::cerr << "offscreen arc pieces were joined across a gap\n";
        return 1;
    }

    const auto nearPlane = ProjectTargetlineArc(
        projection, { 0.0F, 0.0F, -5.0F }, { 1.0F, 0.0F, 5.0F }, TargetlineArcStyle{});
    if (nearPlane.count == 0u || nearPlane.count >= bahamut_client::kTargetlineArcSegmentCount ||
        !Finite(nearPlane.segments.front()) || nearPlane.segments.front().source.x < 399.0F)
    {
        std::cerr << "near-plane crossing was not independently clipped\n";
        return 1;
    }

    const auto offscreen = ProjectTargetlineArc(
        projection, { -10.0F, 0.0F, 5.0F }, { 0.0F, 0.0F, 5.0F }, TargetlineArcStyle{});
    if (offscreen.count == 0u || !Near(offscreen.segments.front().source.x, 0.0F) ||
        !Near(offscreen.segments[offscreen.count - 1u].target.x, 400.0F))
    {
        std::cerr << "offscreen endpoint was not clipped to the viewport\n";
        return 1;
    }
    return 0;
}
