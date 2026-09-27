#include <windows.h>
#include <xinput.h>

namespace
{

XINPUT_STATE gState{};

}

extern "C" DWORD WINAPI StubXInputGetState(DWORD index, XINPUT_STATE* state)
{
    if (index != 0 || state == nullptr)
    {
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    *state = gState;
    return ERROR_SUCCESS;
}

extern "C" void WINAPI BahamutTestSetXInputState(WORD buttons)
{
    ++gState.dwPacketNumber;
    gState.Gamepad          = {};
    gState.Gamepad.wButtons = buttons;
}
