#include "world_projection.h"

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

bool At(const bahamut_client::ScreenPosition& point, float x, float y)
{
    return std::fabs(point.x - x) < 0.001F && std::fabs(point.y - y) < 0.001F;
}

} // namespace

int main()
{
    using bahamut_client::ProjectWorldSegment;
    using bahamut_client::WorldPosition;
    auto projection = Perspective();
    auto segment    = ProjectWorldSegment(projection, { 0.0F, 0.0F, 5.0F }, { 1.0F, 1.0F, 5.0F });
    if (!segment || !At(segment->source, 400.0F, 300.0F) || !At(segment->target, 480.0F, 240.0F))
    {
        std::cerr << "perspective projection lost target height or inverted screen Y\n";
        return 1;
    }

    projection.worldToClip[0] = 2.0F;
    projection.worldToClip[5] = 2.0F;
    segment                   = ProjectWorldSegment(projection, { 0.0F, 0.0F, 5.0F }, { 1.0F, 1.0F, 5.0F });
    if (!segment || !At(segment->target, 560.0F, 180.0F))
    {
        std::cerr << "changed projection scale was not applied\n";
        return 1;
    }

    projection             = Perspective();
    projection.worldToClip = { 0.0F, 0.0F, 10.0F / 9.0F, 1.0F, 0.0F, 1.0F, 0.0F, 0.0F, -1.0F, 0.0F, 0.0F, 0.0F, 0.0F, 0.0F, -10.0F / 9.0F, 0.0F };
    segment                = ProjectWorldSegment(projection, { 5.0F, 0.0F, 0.0F }, { 5.0F, 1.0F, -1.0F });
    if (!segment || !At(segment->source, 400.0F, 300.0F) || !At(segment->target, 480.0F, 240.0F))
    {
        std::cerr << "rotated world-to-clip matrix was not applied\n";
        return 1;
    }

    projection          = Perspective();
    projection.viewport = { 20.0F, 30.0F, 400.0F, 200.0F };
    segment             = ProjectWorldSegment(projection, { 0.0F, 0.0F, 5.0F }, { 1.0F, 1.0F, 5.0F });
    if (!segment || !At(segment->source, 220.0F, 130.0F) || !At(segment->target, 260.0F, 110.0F))
    {
        std::cerr << "viewport offset or resized dimensions were ignored\n";
        return 1;
    }

    projection = Perspective();
    segment    = ProjectWorldSegment(projection, { 0.0F, 0.0F, 5.0F }, { 10.0F, 0.0F, 5.0F });
    if (!segment || !At(segment->source, 400.0F, 300.0F) || !At(segment->target, 800.0F, 300.0F))
    {
        std::cerr << "offscreen target was not clipped to the viewport\n";
        return 1;
    }

    segment = ProjectWorldSegment(projection, { 0.0F, 0.0F, -5.0F }, { 0.0F, 0.0F, 5.0F });
    if (!segment || !At(segment->source, 400.0F, 300.0F) || !At(segment->target, 400.0F, 300.0F))
    {
        std::cerr << "near-plane crossing did not produce finite clipped endpoints\n";
        return 1;
    }
    if (ProjectWorldSegment(projection, { 0.0F, 0.0F, -5.0F }, { 1.0F, 1.0F, -5.0F }) ||
        ProjectWorldSegment(projection, { 0.0F, 0.0F, 11.0F }, { 1.0F, 1.0F, 12.0F }) ||
        ProjectWorldSegment(projection, { 10.0F, 0.0F, 5.0F }, { 11.0F, 0.0F, 5.0F }))
    {
        std::cerr << "segment outside the scene frustum was accepted\n";
        return 1;
    }

    if (ProjectWorldSegment(projection, { 0.0F, std::numeric_limits<float>::quiet_NaN(), 5.0F }, WorldPosition{ 1.0F, 1.0F, 5.0F }))
    {
        std::cerr << "nonfinite actor position was accepted\n";
        return 1;
    }
    projection.worldToClip[5] = std::numeric_limits<float>::infinity();
    if (ProjectWorldSegment(projection, { 0.0F, 0.0F, 5.0F }, { 1.0F, 1.0F, 5.0F }))
    {
        std::cerr << "nonfinite scene matrix was accepted\n";
        return 1;
    }
    projection                = Perspective();
    projection.viewport.width = 0.0F;
    if (ProjectWorldSegment(projection, { 0.0F, 0.0F, 5.0F }, { 1.0F, 1.0F, 5.0F }) ||
        ProjectWorldSegment({}, { 0.0F, 0.0F, 5.0F }, { 1.0F, 1.0F, 5.0F }))
    {
        std::cerr << "unavailable scene projection or viewport was accepted\n";
        return 1;
    }
    return 0;
}
