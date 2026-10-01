#include "world_projection.h"

#include <algorithm>
#include <cmath>

namespace
{

using ClipPosition = std::array<double, 4>;

bool IsFinite(const bahamut_client::WorldPosition& position)
{
    return std::isfinite(position.x) && std::isfinite(position.y) && std::isfinite(position.z);
}

ClipPosition Transform(const std::array<float, 16>&         matrix,
                       const bahamut_client::WorldPosition& position)
{
    ClipPosition clip{};
    for (std::size_t column = 0; column < clip.size(); ++column)
    {
        clip[column] = static_cast<double>(position.x) * matrix[column] +
                       static_cast<double>(position.y) * matrix[4u + column] +
                       static_cast<double>(position.z) * matrix[8u + column] + matrix[12u + column];
    }
    return clip;
}

std::array<double, 6> ClipPlanes(const ClipPosition& point)
{
    return { point[3] + point[0], point[3] - point[0], point[3] + point[1], point[3] - point[1], point[2], point[3] - point[2] };
}

std::optional<bahamut_client::ScreenPosition> ToScreen(
    const ClipPosition& point, const bahamut_client::ProjectionViewport& viewport)
{
    if (point[3] <= 0.0)
    {
        return std::nullopt;
    }
    const double                   x = std::clamp(point[0] / point[3], -1.0, 1.0);
    const double                   y = std::clamp(point[1] / point[3], -1.0, 1.0);
    bahamut_client::ScreenPosition screen{
        static_cast<float>(viewport.x + (x + 1.0) * viewport.width / 2.0),
        static_cast<float>(viewport.y + (1.0 - y) * viewport.height / 2.0)
    };
    if (!std::isfinite(screen.x) || !std::isfinite(screen.y))
    {
        return std::nullopt;
    }
    return screen;
}

} // namespace

namespace bahamut_client
{

std::optional<ScreenSegment> ProjectWorldSegment(const SceneProjection& projection,
                                                 const WorldPosition&   source,
                                                 const WorldPosition&   target)
{
    const auto& viewport = projection.viewport;
    if (!IsFinite(source) || !IsFinite(target) || !std::isfinite(viewport.x) ||
        !std::isfinite(viewport.y) || !std::isfinite(viewport.width) ||
        !std::isfinite(viewport.height) || viewport.width <= 0.0F || viewport.height <= 0.0F ||
        !std::all_of(projection.worldToClip.begin(), projection.worldToClip.end(), [](float value)
                     {
                         return std::isfinite(value);
                     }))
    {
        return std::nullopt;
    }

    const auto start       = Transform(projection.worldToClip, source);
    const auto end         = Transform(projection.worldToClip, target);
    const auto startPlanes = ClipPlanes(start);
    const auto endPlanes   = ClipPlanes(end);
    double     first       = 0.0;
    double     last        = 1.0;
    for (std::size_t plane = 0; plane < startPlanes.size(); ++plane)
    {
        const double a = startPlanes[plane];
        const double b = endPlanes[plane];
        if (a < 0.0 && b < 0.0)
        {
            return std::nullopt;
        }
        if (a < 0.0)
        {
            first = std::max(first, a / (a - b));
        }
        else if (b < 0.0)
        {
            last = std::min(last, a / (a - b));
        }
        if (first > last)
        {
            return std::nullopt;
        }
    }

    ClipPosition clippedStart{};
    ClipPosition clippedEnd{};
    for (std::size_t component = 0; component < start.size(); ++component)
    {
        clippedStart[component] = start[component] + first * (end[component] - start[component]);
        clippedEnd[component]   = start[component] + last * (end[component] - start[component]);
    }
    const auto screenStart = ToScreen(clippedStart, viewport);
    const auto screenEnd   = ToScreen(clippedEnd, viewport);
    if (!screenStart || !screenEnd)
    {
        return std::nullopt;
    }
    return ScreenSegment{ *screenStart, *screenEnd };
}

} // namespace bahamut_client
