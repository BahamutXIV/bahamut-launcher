#include "targetlines_arc.h"

#include <cmath>

namespace
{

struct ArcPoint
{
    double x = 0.0;
    double y = 0.0;
    double z = 0.0;
};

bool Finite(const bahamut_client::WorldPosition& position)
{
    return std::isfinite(position.x) && std::isfinite(position.y) && std::isfinite(position.z);
}

bool ValidStyle(const bahamut_client::TargetlineArcStyle& style)
{
    return std::isfinite(style.endpointLift) && std::isfinite(style.archHeight) &&
           style.endpointLift >= 0.0F && style.archHeight >= 0.0F;
}

bool SamePosition(const bahamut_client::WorldPosition& source,
                  const bahamut_client::WorldPosition& target)
{
    return source.x == target.x && source.y == target.y && source.z == target.z;
}

ArcPoint Lifted(const bahamut_client::WorldPosition& position, double endpointLift)
{
    return { static_cast<double>(position.x), static_cast<double>(position.y) + endpointLift, static_cast<double>(position.z) };
}

ArcPoint Sample(const ArcPoint& source, const ArcPoint& control, const ArcPoint& target, double t)
{
    const double oneMinusT     = 1.0 - t;
    const double sourceWeight  = oneMinusT * oneMinusT;
    const double controlWeight = 2.0 * oneMinusT * t;
    const double targetWeight  = t * t;
    return { sourceWeight * source.x + controlWeight * control.x + targetWeight * target.x,
             sourceWeight * source.y + controlWeight * control.y + targetWeight * target.y,
             sourceWeight * source.z + controlWeight * control.z + targetWeight * target.z };
}

bool ToWorldPosition(const ArcPoint& point, bahamut_client::WorldPosition& result)
{
    result = { static_cast<float>(point.x), static_cast<float>(point.y), static_cast<float>(point.z) };
    return std::isfinite(result.x) && std::isfinite(result.y) && std::isfinite(result.z);
}

} // namespace

namespace bahamut_client
{

TargetlineArcSegments ProjectTargetlineArc(
    const SceneProjection&    projection,
    const WorldPosition&      source,
    const WorldPosition&      target,
    const TargetlineArcStyle& style)
{
    TargetlineArcSegments result;
    if (!Finite(source) || !Finite(target) || SamePosition(source, target) || !ValidStyle(style))
    {
        return result;
    }

    const double   endpointLift = static_cast<double>(style.endpointLift);
    const double   archHeight   = static_cast<double>(style.archHeight);
    const ArcPoint arcSource    = Lifted(source, endpointLift);
    const ArcPoint arcTarget    = Lifted(target, endpointLift);
    const ArcPoint control{
        (arcSource.x + arcTarget.x) / 2.0,
        (arcSource.y + arcTarget.y) / 2.0 + 2.0 * archHeight,
        (arcSource.z + arcTarget.z) / 2.0
    };

    WorldPosition previous{};
    if (!ToWorldPosition(arcSource, previous))
    {
        return result;
    }
    for (std::size_t sample = 1u; sample < kTargetlineArcSampleCount; ++sample)
    {
        const double  t = static_cast<double>(sample) /
                          static_cast<double>(kTargetlineArcSampleCount - 1u);
        WorldPosition current{};
        if (!ToWorldPosition(Sample(arcSource, control, arcTarget, t), current))
        {
            return result;
        }
        if (const auto projected = ProjectWorldSegment(projection, previous, current))
        {
            result.segments[result.count++] = *projected;
        }
        previous = current;
    }
    return result;
}

} // namespace bahamut_client
