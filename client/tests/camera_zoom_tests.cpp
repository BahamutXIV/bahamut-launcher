#include "camera_zoom.h"

#include <cmath>
#include <cstddef>
#include <cstdint>
#include <limits>

namespace
{

int gOriginalCalls = 0;

struct FakeCamera
{
    std::byte padding[0x3F4];
    float     distances[3];
};

static_assert(offsetof(FakeCamera, distances) == 0x3F4);

void OriginalSetter(void* camera, std::uint32_t index, float distance)
{
    ++gOriginalCalls;
    static_cast<FakeCamera*>(camera)->distances[index] = distance;
}

} // namespace

int main()
{
    FakeCamera camera{};
    bahamut_client::ApplyCameraZoom(&camera, 2, 9.0F, 15.0F, &OriginalSetter);
    if (gOriginalCalls != 1 || camera.distances[2] != 9.0F)
    {
        return 1;
    }
    bahamut_client::ApplyCameraZoom(&camera, 2, 12.0F, 15.0F, &OriginalSetter);
    if (gOriginalCalls != 1 || camera.distances[2] != 12.0F)
    {
        return 2;
    }
    bahamut_client::ApplyCameraZoom(&camera, 2, 18.0F, 13.0F, &OriginalSetter);
    if (gOriginalCalls != 1 || camera.distances[2] != 13.0F)
    {
        return 3;
    }
    bahamut_client::ApplyCameraZoom(&camera, 2, std::numeric_limits<float>::infinity(), 13.0F, &OriginalSetter);
    if (gOriginalCalls != 2 || !std::isinf(camera.distances[2]))
    {
        return 4;
    }
    return 0;
}
