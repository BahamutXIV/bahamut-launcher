#pragma once

#include <windows.h>

namespace bahamut_loader
{

// The pinned client asserts at 16 CPU bits; see docs/troubleshooting.md.
constexpr DWORD_PTR LimitGameProcessorMask(DWORD_PTR mask)
{
    DWORD_PTR limited = 0;
    unsigned  count   = 0;
    for (DWORD_PTR bit = 1; bit != 0 && count < 15; bit <<= 1)
    {
        if ((mask & bit) != 0)
        {
            limited |= bit;
            ++count;
        }
    }
    return limited;
}

} // namespace bahamut_loader
