#include "object_distance.h"

#include <limits>

int main()
{
    if (bahamut_client::ScaleObjectDistance(40.0F, 1.25F) != 50.0F ||
        bahamut_client::ScaleObjectDistance(40.0F, 1.5F) != 60.0F ||
        bahamut_client::ScaleObjectDistance(40.0F, 2.0F) != 80.0F)
    {
        return 1;
    }
    if (bahamut_client::ScaleObjectDistance(-1.0F, 2.0F) != -1.0F ||
        bahamut_client::ScaleObjectDistance(std::numeric_limits<float>::max(), 2.0F) !=
            std::numeric_limits<float>::max())
    {
        return 2;
    }
    return 0;
}
