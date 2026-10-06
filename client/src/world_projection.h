#pragma once

#include <array>
#include <optional>

namespace bahamut_client
{

struct WorldPosition
{
    float x = 0.0F;
    float y = 0.0F;
    float z = 0.0F;
};

struct ProjectionViewport
{
    float x      = 0.0F;
    float y      = 0.0F;
    float width  = 0.0F;
    float height = 0.0F;
};

struct SceneProjection
{
    // Row-vector, row-major world-to-clip matrix; D3D depth is 0 <= z <= w.
    std::array<float, 16> worldToClip{};
    ProjectionViewport    viewport;
};

struct ScreenPosition
{
    float x = 0.0F;
    float y = 0.0F;
};

struct ScreenSegment
{
    ScreenPosition source;
    ScreenPosition target;
};

[[nodiscard]] std::optional<ScreenSegment> ProjectWorldSegment(
    const SceneProjection& projection,
    const WorldPosition&   source,
    const WorldPosition&   target);

} // namespace bahamut_client
