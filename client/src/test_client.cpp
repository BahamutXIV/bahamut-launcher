#include <windows.h>

#include <d3d9.h>
#include <xinput.h>

#include "input.h"
#include "monitor_selection.h"
#include "runtime_contract.h"

#include <array>
#include <cstdint>
#include <cstring>
#include <fstream>
#include <string>

extern "C" __declspec(dllexport) unsigned char    gStubServerUtcPatch[16]{};
extern "C" __declspec(dllexport) unsigned char    gStubLobbyHostPatch[16]{};
extern "C" __declspec(dllexport) void             BahamutTestPadUpdate();
extern "C" __declspec(dllexport) LRESULT CALLBACK BahamutTestKeyboardCallback(
    HWND, UINT, WPARAM, LPARAM, UINT_PTR, DWORD_PTR);

namespace
{

LONG                    gForwardedAKeyDownCount      = 0;
LONG                    gForwardedAKeyUpCount        = 0;
LONG                    gForwardedMouseDownCount     = 0;
LONG                    gForwardedMouseUpCount       = 0;
LONG                    gForwardedF11KeyDownCount    = 0;
LONG                    gForwardedF11KeyUpCount      = 0;
LONG                    gForwardedF12KeyDownCount    = 0;
LONG                    gForwardedF12KeyUpCount      = 0;
LONG                    gRetailF11MessageCount       = 0;
LONG                    gRetailF12MessageCount       = 0;
constexpr std::size_t   kStubPadStateOffset          = 0x28;
constexpr std::size_t   kStubPadRecordVectorOffset   = 0x1c;
constexpr std::size_t   kStubSelectedPadRecordOffset = 0x1c8;
constexpr std::size_t   kStubDirectInputDeviceOffset = 0x724;
constexpr std::uint32_t kStubPadStart                = 0x10;
constexpr std::uint32_t kStubPadBack                 = 0x20;
constexpr std::uint32_t kStubPadA                    = 0x1000;
constexpr std::uint32_t kStubPadB                    = 0x2000;
alignas(4) std::array<unsigned char, 0x1d4> gStubPadDevice{};
alignas(4) std::array<unsigned char, 0x72c> gStubPadRecord{};
std::uint32_t gStubPadButtons                   = 0;
LONG          gStubPadOriginalCalls             = 0;
bool          gDirectInputBForwarded            = true;
bool          gDirectInputChordForwarded        = true;
bool          gDirectInputAForwarded            = true;
bool          gDirectInputDisconnected          = false;
bool          gDirectInputReconnected           = false;
bool          gXInputAForwarded                 = true;
bool          gXInputBForwarded                 = true;
bool          gXInputChordForwarded             = true;
bool          gPrintScreenOwnedWhileActive      = false;
bool          gPrintScreenReleasedWhileInactive = false;
bool          gFillModeRepeatDidNotRetoggle     = false;
bool          gFillModeSolidRequestIgnored      = false;
bool          gFillModeDisabledRestoredSolid    = false;
bool          gFillModeResetPreservedWireframe  = false;
bool          gFillModeInactiveIgnored          = false;
bool          gFillModeSecondaryRemainsSolid    = false;
bool          gFillModeOverlayRestoredWireframe = false;
bool          gFillModeDepthEnabledWireframe    = false;
bool          gFillModeDepthDisabledSolid       = false;
bool          gFillModeDepthReenabledWireframe  = false;
bool          gBorderlessApplied                = false;
bool          gBorderlessPreservedAfterReset    = false;
bool          gWindowedStylePreserved           = false;

void SendKey(HWND window, WPARAM key);
void SendRetailKey(HWND window, WPARAM key);

bool HasEnvironmentValue(const wchar_t* name)
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(name, value, ARRAYSIZE(value));
    return count != 0 && count < ARRAYSIZE(value) && value[0] != L'0';
}

UINT ScreenshotTestVirtualKey()
{
    wchar_t     value[32]{};
    const DWORD count = GetEnvironmentVariableW(
        L"BAHAMUT_RUNTIME_SCREENSHOT_HOTKEY", value, ARRAYSIZE(value));
    if (count == 0 || count >= ARRAYSIZE(value))
        return 0;
    if (wcscmp(value, L"print_screen") == 0)
        return VK_SNAPSHOT;
    if (wcscmp(value, L"insert") == 0)
        return VK_INSERT;
    if (count == 2 && value[0] == L'f' && value[1] >= L'1' && value[1] <= L'9')
    {
        return VK_F1 + static_cast<UINT>(value[1] - L'1');
    }
    return 0;
}

void SignalFromEnvironment(const wchar_t* name)
{
    wchar_t     prefix[256]{};
    const DWORD count = GetEnvironmentVariableW(L"BAHAMUT_STUB_EVENT_PREFIX", prefix, ARRAYSIZE(prefix));
    if (count == 0 || count >= ARRAYSIZE(prefix))
    {
        return;
    }

    std::wstring eventName(prefix);
    eventName += name;
    HANDLE eventHandle = OpenEventW(EVENT_MODIFY_STATE, FALSE, eventName.c_str());
    if (eventHandle != nullptr)
    {
        SetEvent(eventHandle);
        CloseHandle(eventHandle);
    }
}

void RecordStateFailure(int code)
{
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(L"BAHAMUT_TEST_STUB_RESULT_FILE",
                                                path,
                                                ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << "state_failure=" << code << "\n";
    }
}

void RecordDirectInputResult()
{
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(
        L"BAHAMUT_TEST_DIRECTINPUT_RESULT_FILE", path, ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << "directinput_b_forward=" << (gDirectInputBForwarded ? 1 : 0)
             << " directinput_chord_forward="
             << (gDirectInputChordForwarded ? 1 : 0)
             << " directinput_a_forward=" << (gDirectInputAForwarded ? 1 : 0)
             << " directinput_disconnect=" << (gDirectInputDisconnected ? 1 : 0)
             << " directinput_reconnect=" << (gDirectInputReconnected ? 1 : 0)
             << " original_calls=" << gStubPadOriginalCalls << "\n";
    }
}

void RecordXInputResult()
{
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(
        L"BAHAMUT_TEST_XINPUT_RESULT_FILE", path, ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << "xinput_a_forward=" << (gXInputAForwarded ? 1 : 0)
             << " xinput_b_forward=" << (gXInputBForwarded ? 1 : 0)
             << " xinput_chord_forward=" << (gXInputChordForwarded ? 1 : 0)
             << "\n";
    }
}

void RecordPrintScreenResult()
{
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(
        L"BAHAMUT_TEST_PRINT_SCREEN_RESULT_FILE", path, ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << "owned_while_active=" << (gPrintScreenOwnedWhileActive ? 1 : 0)
             << " released_while_inactive="
             << (gPrintScreenReleasedWhileInactive ? 1 : 0) << "\n";
    }
}

void RecordFillModeResult()
{
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(
        L"BAHAMUT_TEST_FILL_MODE_RESULT_FILE", path, ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << "f11_repeat_stable=" << (gFillModeRepeatDidNotRetoggle ? 1 : 0)
             << " solid_request_ignored=" << (gFillModeSolidRequestIgnored ? 1 : 0)
             << " disabled_restores_solid="
             << (gFillModeDisabledRestoredSolid ? 1 : 0)
             << " reset_preserves_wireframe="
             << (gFillModeResetPreservedWireframe ? 1 : 0)
             << " inactive_ignored=" << (gFillModeInactiveIgnored ? 1 : 0)
             << " secondary_remains_solid="
             << (gFillModeSecondaryRemainsSolid ? 1 : 0)
             << " overlay_restores_wireframe="
             << (gFillModeOverlayRestoredWireframe ? 1 : 0)
             << " depth_enabled_wireframe="
             << (gFillModeDepthEnabledWireframe ? 1 : 0)
             << " depth_disabled_solid="
             << (gFillModeDepthDisabledSolid ? 1 : 0)
             << " depth_reenabled_wireframe="
             << (gFillModeDepthReenabledWireframe ? 1 : 0)
             << " f11_inactive_forward_pair="
             << (gForwardedF11KeyDownCount == 1 && gForwardedF11KeyUpCount == 1 ? 1 : 0)
             << " retail_f11_suppressed="
             << (gRetailF11MessageCount == 2 ? 1 : 0)
             << "\n";
    }
}

void RecordFpsHotkeyResult()
{
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(
        L"BAHAMUT_TEST_FPS_HOTKEY_RESULT_FILE", path, ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << "f12_inactive_forward_pair="
             << (gForwardedF12KeyDownCount == 1 && gForwardedF12KeyUpCount == 1 ? 1 : 0)
             << " retail_f12_suppressed="
             << (gRetailF12MessageCount == 2 ? 1 : 0)
             << "\n";
    }
}

bool IsMonitorBorderless(HWND window)
{
    PhysicalCoordinateContext physicalCoordinates;
    RECT                      windowRect{};
    const LONG_PTR            style = GetWindowLongPtrW(window, GWL_STYLE);
    if (!GetWindowRect(window, &windowRect) || (style & WS_POPUP) == 0 ||
        (style & (WS_CAPTION | WS_THICKFRAME)) != 0)
    {
        return false;
    }

    const auto requestedId = ReadBorderlessMonitorId();
    if (requestedId.has_value() && !requestedId->empty())
    {
        const auto  monitors = EnumerateBorderlessMonitors();
        const auto* selected = SelectBorderlessMonitor(monitors, *requestedId);
        return selected != nullptr && EqualRect(&windowRect, &selected->bounds);
    }

    MONITORINFO monitorInfo{};
    monitorInfo.cbSize     = sizeof(monitorInfo);
    const HMONITOR monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
    return monitor != nullptr && GetMonitorInfoW(monitor, &monitorInfo) &&
           EqualRect(&windowRect, &monitorInfo.rcMonitor);
}

void RecordBorderlessResult()
{
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(
        L"BAHAMUT_TEST_BORDERLESS_RESULT_FILE", path, ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << "applied=" << (gBorderlessApplied ? 1 : 0)
             << " preserved_after_reset="
             << (gBorderlessPreservedAfterReset ? 1 : 0)
             << " windowed_style_preserved="
             << (gWindowedStylePreserved ? 1 : 0) << "\n";
    }
}

void ExerciseFillModeFrame(HWND window, IDirect3DDevice9* device, IDirect3DDevice9* secondaryDevice, int frame)
{
    if (!HasEnvironmentValue(L"BAHAMUT_TEST_FILL_MODE_BOUNDARY"))
    {
        return;
    }
    DWORD fillMode = 0;
    if (frame == 10)
    {
        device->SetRenderState(D3DRS_ZENABLE, D3DZB_TRUE);
        BahamutTestKeyboardCallback(window, WM_KEYDOWN, VK_F11, 0, 0, 0);
        device->SetRenderState(D3DRS_FILLMODE, D3DFILL_SOLID);
        device->GetRenderState(D3DRS_FILLMODE, &fillMode);
        const bool wireframe           = fillMode == D3DFILL_WIREFRAME;
        gFillModeDepthEnabledWireframe = wireframe;
        BahamutTestKeyboardCallback(
            window, WM_KEYDOWN, VK_F11, 1LL << 30, 0, 0);
        device->GetRenderState(D3DRS_FILLMODE, &fillMode);
        gFillModeRepeatDidNotRetoggle = wireframe && fillMode == D3DFILL_WIREFRAME;
        device->SetRenderState(D3DRS_ZENABLE, D3DZB_FALSE);
        device->GetRenderState(D3DRS_FILLMODE, &fillMode);
        gFillModeDepthDisabledSolid = fillMode == D3DFILL_SOLID;
        device->SetRenderState(D3DRS_ZENABLE, D3DZB_TRUE);
        device->GetRenderState(D3DRS_FILLMODE, &fillMode);
        gFillModeDepthReenabledWireframe = fillMode == D3DFILL_WIREFRAME;
        BahamutTestKeyboardCallback(window, WM_KEYUP, VK_F11, static_cast<LPARAM>(-2147483647 - 1), 0, 0);
        return;
    }
    if (frame == 12)
    {
        device->SetRenderState(D3DRS_ZENABLE, D3DZB_TRUE);
        SendRetailKey(window, VK_F11);
        device->GetRenderState(D3DRS_FILLMODE, &fillMode);
        gFillModeDisabledRestoredSolid = fillMode == D3DFILL_SOLID;
        return;
    }
    if (frame == 30)
    {
        device->SetRenderState(D3DRS_ZENABLE, D3DZB_TRUE);
        SendRetailKey(window, VK_F11);
    }
    if (frame == 40)
    {
        device->SetRenderState(D3DRS_ZENABLE, D3DZB_TRUE);
        device->GetRenderState(D3DRS_FILLMODE, &fillMode);
        gFillModeResetPreservedWireframe = fillMode == D3DFILL_WIREFRAME;
    }
    if (frame == 50)
    {
        device->SetRenderState(D3DRS_ZENABLE, D3DZB_TRUE);
        SendMessageW(window, WM_ACTIVATE, WA_INACTIVE, 0);
        SendRetailKey(window, VK_F11);
        device->GetRenderState(D3DRS_FILLMODE, &fillMode);
        gFillModeInactiveIgnored = fillMode == D3DFILL_WIREFRAME;
        SendMessageW(window, WM_ACTIVATE, WA_ACTIVE, 0);
    }
    if (frame == 11)
    {
        device->SetRenderState(D3DRS_ZENABLE, D3DZB_TRUE);
        device->SetRenderState(D3DRS_FILLMODE, D3DFILL_SOLID);
        device->GetRenderState(D3DRS_FILLMODE, &fillMode);
        gFillModeSolidRequestIgnored = fillMode == D3DFILL_WIREFRAME;
        secondaryDevice->SetRenderState(D3DRS_FILLMODE, D3DFILL_SOLID);
        secondaryDevice->GetRenderState(D3DRS_FILLMODE, &fillMode);
        gFillModeSecondaryRemainsSolid = fillMode == D3DFILL_SOLID;
    }
}

void ExerciseFpsHotkeyFrame(HWND window, int frame)
{
    if (!HasEnvironmentValue(L"BAHAMUT_TEST_FPS_HOTKEY"))
    {
        return;
    }
    if (frame == 10)
    {
        BahamutTestKeyboardCallback(window, WM_KEYDOWN, VK_F12, 0, 0, 0);
        BahamutTestKeyboardCallback(
            window, WM_KEYDOWN, VK_F12, 1LL << 30, 0, 0);
        BahamutTestKeyboardCallback(window, WM_KEYUP, VK_F12, static_cast<LPARAM>(-2147483647 - 1), 0, 0);
        return;
    }
    if (frame == 50)
    {
        SendMessageW(window, WM_ACTIVATE, WA_INACTIVE, 0);
        SendRetailKey(window, VK_F12);
        SendMessageW(window, WM_ACTIVATE, WA_ACTIVE, 0);
    }
}

void InitializeDirectInputStub()
{
    auto*               begin    = gStubPadRecord.data();
    auto*               end      = begin + gStubPadRecord.size();
    const std::uint32_t selected = 1;
    memcpy(gStubPadDevice.data() + kStubPadRecordVectorOffset,
           &begin,
           sizeof(begin));
    memcpy(gStubPadDevice.data() + kStubPadRecordVectorOffset + sizeof(begin),
           &end,
           sizeof(end));
    memcpy(gStubPadDevice.data() + kStubSelectedPadRecordOffset,
           &selected,
           sizeof(selected));
    gStubPadRecord[1]       = 0;
    void* directInputDevice = gStubPadRecord.data();
    memcpy(gStubPadRecord.data() + kStubDirectInputDeviceOffset,
           &directInputDevice,
           sizeof(directInputDevice));
}

using StubPadUpdateFunction = void(__thiscall*)(void*);

void ExerciseDirectInputFrame(int frame)
{
    if (!HasEnvironmentValue(L"BAHAMUT_TEST_DIRECTINPUT_BOUNDARY"))
    {
        return;
    }
    if (frame == 12 || frame == 14 || frame == 15)
    {
        gStubPadButtons = kStubPadBack | kStubPadStart;
    }
    else if (frame == 9 || frame == 18 || frame == 19)
    {
        gStubPadButtons = kStubPadB;
    }
    else if (frame == 11 || frame == 13 || frame == 17 || frame == 21)
    {
        gStubPadButtons = kStubPadA;
    }
    else
    {
        gStubPadButtons = 0;
    }

    void* directInputDevice = frame == 12
                                  ? nullptr
                                  : gStubPadRecord.data();
    memcpy(gStubPadRecord.data() + kStubDirectInputDeviceOffset,
           &directInputDevice,
           sizeof(directInputDevice));
    // Called through a volatile pointer so clang cannot drop the convention-mismatched call.
    static volatile StubPadUpdateFunction padUpdate =
        reinterpret_cast<StubPadUpdateFunction>(&BahamutTestPadUpdate);
    padUpdate(gStubPadDevice.data());
    std::uint32_t observed = 0;
    memcpy(&observed, gStubPadDevice.data() + kStubPadStateOffset + sizeof(std::uint32_t) * 4, sizeof(observed));
    if (frame == 9 || frame == 18 || frame == 19)
    {
        gDirectInputBForwarded = gDirectInputBForwarded && observed == kStubPadB;
    }
    else if (frame == 14 || frame == 15)
    {
        gDirectInputChordForwarded = gDirectInputChordForwarded && observed == (kStubPadBack | kStubPadStart);
    }
    else if (frame == 11 || frame == 17 || frame == 21)
    {
        gDirectInputAForwarded = gDirectInputAForwarded && observed == kStubPadA;
    }
    else if (frame == 12)
    {
        gDirectInputDisconnected = observed == (kStubPadBack | kStubPadStart);
    }
    else if (frame == 13)
    {
        gDirectInputReconnected = gDirectInputDisconnected && observed == kStubPadA;
        gDirectInputAForwarded  = gDirectInputAForwarded && observed == kStubPadA;
    }
}

using TestSetXInputStateFunction = void(WINAPI*)(WORD);
using StubXInputGetStateFunction = DWORD(WINAPI*)(DWORD, XINPUT_STATE*);

void ExerciseXInputFrame(int frame)
{
    if (!HasEnvironmentValue(L"BAHAMUT_TEST_XINPUT_BOUNDARY"))
    {
        return;
    }
    HMODULE    module   = GetModuleHandleW(L"xinput1_3.dll");
    const auto setState = module == nullptr ? nullptr
                                            : reinterpret_cast<TestSetXInputStateFunction>(
                                                  GetProcAddress(module, "BahamutTestSetXInputState"));
    const auto getState = module == nullptr ? nullptr
                                            : reinterpret_cast<StubXInputGetStateFunction>(
                                                  GetProcAddress(module, "XInputGetState"));
    if (setState == nullptr || getState == nullptr)
    {
        gXInputAForwarded = false;
        return;
    }

    WORD buttons = 0;
    if (frame == 24 || frame == 28 || frame == 32)
    {
        buttons = XINPUT_GAMEPAD_A;
    }
    else if (frame == 25 || frame == 26)
    {
        buttons = XINPUT_GAMEPAD_B;
    }
    else if (frame == 29 || frame == 30)
    {
        buttons = XINPUT_GAMEPAD_BACK | XINPUT_GAMEPAD_START;
    }
    setState(buttons);
    XINPUT_STATE observed{};
    if (getState(0, &observed) != ERROR_SUCCESS)
    {
        gXInputAForwarded = false;
        return;
    }
    const WORD gameButtons = observed.Gamepad.wButtons;
    if (frame == 24 || frame == 28 || frame == 32)
    {
        gXInputAForwarded = gXInputAForwarded && gameButtons == XINPUT_GAMEPAD_A;
    }
    else if (frame == 25 || frame == 26)
    {
        gXInputBForwarded = gXInputBForwarded && gameButtons == XINPUT_GAMEPAD_B;
    }
    else if (frame == 29 || frame == 30)
    {
        gXInputChordForwarded = gXInputChordForwarded && gameButtons == (XINPUT_GAMEPAD_BACK | XINPUT_GAMEPAD_START);
    }
}

bool ParseHex(const wchar_t* value, std::array<unsigned char, 16>& output)
{
    wchar_t     text[33]{};
    const DWORD count = GetEnvironmentVariableW(value, text, ARRAYSIZE(text));
    if (count == 0 || count > 32 || (count % 2) != 0)
    {
        return false;
    }

    auto nibble = [](wchar_t c) -> int
    {
        if (c >= L'0' && c <= L'9')
            return c - L'0';
        if (c >= L'a' && c <= L'f')
            return c - L'a' + 10;
        if (c >= L'A' && c <= L'F')
            return c - L'A' + 10;
        return -1;
    };

    output.fill(0);
    for (DWORD i = 0; i < count / 2; ++i)
    {
        const int high = nibble(text[i * 2]);
        const int low  = nibble(text[i * 2 + 1]);
        if (high < 0 || low < 0)
        {
            return false;
        }
        output[i] = static_cast<unsigned char>((high << 4) | low);
    }
    return true;
}

bool MatchesExpected(const wchar_t* environmentName, const unsigned char* actual)
{
    std::array<unsigned char, 16> expected{};
    if (!ParseHex(environmentName, expected))
    {
        return false;
    }
    wchar_t     text[33]{};
    const DWORD count = GetEnvironmentVariableW(environmentName, text, ARRAYSIZE(text));
    return memcmp(actual, expected.data(), count / 2) == 0;
}

LRESULT CALLBACK StubWindowProcedure(HWND window, UINT message, WPARAM wParam, LPARAM lParam)
{
    if (message == WM_KEYDOWN && wParam == 'A')
        ++gForwardedAKeyDownCount;
    if (message == WM_KEYUP && wParam == 'A')
        ++gForwardedAKeyUpCount;
    if (message == WM_LBUTTONDOWN)
        ++gForwardedMouseDownCount;
    if (message == WM_LBUTTONUP)
        ++gForwardedMouseUpCount;
    if (message == WM_KEYDOWN && wParam == VK_F11)
    {
        ++gForwardedF11KeyDownCount;
    }
    if (message == WM_KEYUP && wParam == VK_F11)
    {
        ++gForwardedF11KeyUpCount;
    }
    if (message == WM_KEYDOWN && wParam == VK_F12)
    {
        ++gForwardedF12KeyDownCount;
    }
    if (message == WM_KEYUP && wParam == VK_F12)
    {
        ++gForwardedF12KeyUpCount;
    }
    return DefWindowProcW(window, message, wParam, lParam);
}

void SendKey(HWND window, WPARAM key)
{
    SendMessageW(window, WM_KEYDOWN, key, 0);
    SendMessageW(window, WM_KEYUP, key, static_cast<LPARAM>(-2147483647 - 1));
}

void SendRetailKey(HWND window, WPARAM key)
{
    BahamutTestKeyboardCallback(window, WM_KEYDOWN, key, 0, 0, 0);
    BahamutTestKeyboardCallback(window, WM_KEYUP, key, static_cast<LPARAM>(-2147483647 - 1), 0, 0);
}

int ExerciseOffscreenRenderTarget(IDirect3DDevice9* device)
{
    constexpr UINT     width           = 128;
    constexpr UINT     height          = 96;
    constexpr D3DCOLOR sentinel        = D3DCOLOR_ARGB(255, 0x12, 0x34, 0x56);
    IDirect3DSurface9* originalTarget  = nullptr;
    IDirect3DSurface9* originalDepth   = nullptr;
    IDirect3DSurface9* offscreenTarget = nullptr;
    IDirect3DSurface9* readback        = nullptr;
    auto               release         = [&]()
    {
        if (readback != nullptr)
            readback->Release();
        if (offscreenTarget != nullptr)
            offscreenTarget->Release();
        if (originalDepth != nullptr)
            originalDepth->Release();
        if (originalTarget != nullptr)
            originalTarget->Release();
    };

    const HRESULT depthResult = device->GetDepthStencilSurface(&originalDepth);
    if (FAILED(device->GetRenderTarget(0, &originalTarget)) || (FAILED(depthResult) && depthResult != D3DERR_NOTFOUND) || FAILED(device->CreateRenderTarget(width, height, D3DFMT_A8R8G8B8, D3DMULTISAMPLE_NONE, 0, FALSE, &offscreenTarget, nullptr)) || FAILED(device->CreateOffscreenPlainSurface(width, height, D3DFMT_A8R8G8B8, D3DPOOL_SYSTEMMEM, &readback, nullptr)))
    {
        release();
        return 67;
    }
    if (FAILED(device->SetDepthStencilSurface(nullptr)) || FAILED(device->SetRenderTarget(0, offscreenTarget)) || FAILED(device->Clear(0, nullptr, D3DCLEAR_TARGET, sentinel, 1.0f, 0)))
    {
        device->SetRenderTarget(0, originalTarget);
        device->SetDepthStencilSurface(originalDepth);
        release();
        return 68;
    }
    for (int scene = 0; scene < 2; ++scene)
    {
        if (FAILED(device->BeginScene()) || FAILED(device->EndScene()))
        {
            device->SetRenderTarget(0, originalTarget);
            device->SetDepthStencilSurface(originalDepth);
            release();
            return 68;
        }
    }
    const bool     restored  = SUCCEEDED(device->SetRenderTarget(0, originalTarget)) && SUCCEEDED(device->SetDepthStencilSurface(originalDepth));
    bool           unchanged = restored && SUCCEEDED(device->GetRenderTargetData(offscreenTarget, readback));
    D3DLOCKED_RECT locked{};
    if (unchanged && SUCCEEDED(readback->LockRect(&locked, nullptr, D3DLOCK_READONLY)))
    {
        for (UINT y = 0; y < height && unchanged; ++y)
        {
            const auto* row = reinterpret_cast<const D3DCOLOR*>(
                static_cast<const unsigned char*>(locked.pBits) + y * locked.Pitch);
            for (UINT x = 0; x < width; ++x)
            {
                if (row[x] != sentinel)
                {
                    unchanged = false;
                    break;
                }
            }
        }
        readback->UnlockRect();
    }
    else
    {
        unchanged = false;
    }
    if (!unchanged)
    {
        release();
        return 69;
    }
    const bool extraScene = SUCCEEDED(device->BeginScene()) && SUCCEEDED(device->EndScene());
    release();
    return extraScene ? 0 : 70;
}

int ExerciseFrameBoundary(HINSTANCE instance)
{
    const wchar_t className[] = L"BahamutBootstrapStub";
    WNDCLASSW     windowClass{};
    windowClass.lpfnWndProc   = StubWindowProcedure;
    windowClass.hInstance     = instance;
    windowClass.lpszClassName = className;
    if (RegisterClassW(&windowClass) == 0)
    {
        return 43;
    }
    HWND        window   = CreateWindowExW(0, className, L"Bahamut bootstrap stub", WS_OVERLAPPEDWINDOW, 0, 0, 320, 240, nullptr, nullptr, instance, nullptr);
    IDirect3D9* direct3d = Direct3DCreate9(D3D_SDK_VERSION);
    if (window == nullptr || direct3d == nullptr)
    {
        if (direct3d != nullptr)
            direct3d->Release();
        if (window != nullptr)
            DestroyWindow(window);
        UnregisterClassW(className, instance);
        return 43;
    }
    D3DPRESENT_PARAMETERS parameters{};
    parameters.Windowed       = TRUE;
    parameters.SwapEffect     = D3DSWAPEFFECT_DISCARD;
    parameters.hDeviceWindow  = window;
    IDirect3DDevice9* device  = nullptr;
    const HRESULT     created = direct3d->CreateDevice(D3DADAPTER_DEFAULT, D3DDEVTYPE_HAL, window, D3DCREATE_SOFTWARE_VERTEXPROCESSING, &parameters, &device);
    if (FAILED(created))
    {
        direct3d->Release();
        DestroyWindow(window);
        UnregisterClassW(className, instance);
        return 43;
    }
    if (HasEnvironmentValue(L"BAHAMUT_RUNTIME_BORDERLESS"))
    {
        gBorderlessApplied = IsMonitorBorderless(window);
    }
    const LONG_PTR createdStyle               = GetWindowLongPtrW(window, GWL_STYLE);
    gWindowedStylePreserved                   = (createdStyle & WS_OVERLAPPEDWINDOW) == WS_OVERLAPPEDWINDOW;
    IDirect3DDevice9*     secondaryDevice     = nullptr;
    D3DPRESENT_PARAMETERS secondaryParameters = parameters;
    const HRESULT         secondaryCreated    = direct3d->CreateDevice(D3DADAPTER_DEFAULT,
                                                                       D3DDEVTYPE_HAL,
                                                                       window,
                                                                       D3DCREATE_SOFTWARE_VERTEXPROCESSING,
                                                                       &secondaryParameters,
                                                                       &secondaryDevice);
    if (FAILED(secondaryCreated))
    {
        device->Release();
        direct3d->Release();
        DestroyWindow(window);
        UnregisterClassW(className, instance);
        return 44;
    }
    const int offscreenFailure = ExerciseOffscreenRenderTarget(device);
    if (offscreenFailure != 0)
    {
        secondaryDevice->Release();
        device->Release();
        direct3d->Release();
        DestroyWindow(window);
        UnregisterClassW(className, instance);
        RecordStateFailure(offscreenFailure);
        return offscreenFailure;
    }
    D3DMATRIX world{};
    world._11 = 2.0f;
    world._22 = 3.0f;
    world._33 = 4.0f;
    world._44 = 1.0f;
    D3DMATRIX view{};
    view._11 = 5.0f;
    view._22 = 6.0f;
    view._33 = 7.0f;
    view._44 = 1.0f;
    D3DMATRIX projection{};
    projection._11 = 8.0f;
    projection._22 = 9.0f;
    projection._33 = 10.0f;
    projection._44 = 1.0f;
    D3DVIEWPORT9 viewport{ 3, 4, 200, 150, 0.1f, 0.9f };
    RECT         scissor{ 5, 6, 180, 140 };
    if (HasEnvironmentValue(L"BAHAMUT_TEST_FILL_MODE_BOUNDARY") || HasEnvironmentValue(L"BAHAMUT_TEST_FPS_HOTKEY"))
    {
        ShowWindow(window, SW_SHOW);
        SetForegroundWindow(window);
        SetFocus(window);
        SendMessageW(window, WM_ACTIVATE, WA_ACTIVE, 0);
    }
    int stateFailure      = 0;
    int beginSceneRetries = 0;
    for (int frame = 0; frame < 120; ++frame)
    {
        ExerciseDirectInputFrame(frame);
        ExerciseXInputFrame(frame);
        ExerciseFillModeFrame(window, device, secondaryDevice, frame);
        ExerciseFpsHotkeyFrame(window, frame);
        const bool overlayRestoreProbe = frame == 10 && HasEnvironmentValue(L"BAHAMUT_TEST_FILL_MODE_BOUNDARY");
        device->SetRenderState(D3DRS_ZENABLE,
                               overlayRestoreProbe ? D3DZB_TRUE : D3DZB_FALSE);
        device->SetRenderState(D3DRS_LIGHTING, TRUE);
        device->SetRenderState(D3DRS_CULLMODE, D3DCULL_CW);
        device->SetRenderState(D3DRS_FILLMODE, D3DFILL_SOLID);
        device->SetTextureStageState(0, D3DTSS_COLOROP, D3DTOP_DISABLE);
        device->SetSamplerState(0, D3DSAMP_MINFILTER, D3DTEXF_POINT);
        device->SetTransform(D3DTS_WORLD, &world);
        device->SetTransform(D3DTS_VIEW, &view);
        device->SetTransform(D3DTS_PROJECTION, &projection);
        device->SetViewport(&viewport);
        device->SetScissorRect(&scissor);
        std::array<IDirect3DSurface9*, 4> expectedRenderTargets{};
        for (DWORD index = 0; index < expectedRenderTargets.size(); ++index)
        {
            const HRESULT result = device->GetRenderTarget(index, &expectedRenderTargets[index]);
            if (FAILED(result) && result != D3DERR_NOTFOUND)
            {
                stateFailure = 59;
            }
        }
        IDirect3DSurface9* expectedDepthStencil = nullptr;
        const HRESULT      depthResult          = device->GetDepthStencilSurface(&expectedDepthStencil);
        if (FAILED(depthResult) && depthResult != D3DERR_NOTFOUND)
        {
            stateFailure = 60;
        }
        if (stateFailure != 0)
        {
            for (IDirect3DSurface9* target : expectedRenderTargets)
            {
                if (target != nullptr)
                    target->Release();
            }
            if (expectedDepthStencil != nullptr)
                expectedDepthStencil->Release();
            break;
        }
        if (FAILED(device->BeginScene()))
        {
            for (IDirect3DSurface9* target : expectedRenderTargets)
            {
                if (target != nullptr)
                    target->Release();
            }
            if (expectedDepthStencil != nullptr)
                expectedDepthStencil->Release();
            if (++beginSceneRetries > 8)
            {
                stateFailure = 50;
                break;
            }
            --frame;
            Sleep(1);
            continue;
        }
        device->EndScene();
        // Even frames present through the device and odd frames through the
        // swap chain, so the exact frame count covers both runtime boundaries.
        if (frame % 2 == 0)
        {
            if (FAILED(device->Present(nullptr, nullptr, nullptr, nullptr)))
            {
                stateFailure = 75;
            }
        }
        else
        {
            IDirect3DSwapChain9* swapChain = nullptr;
            if (FAILED(device->GetSwapChain(0, &swapChain)) || swapChain == nullptr)
            {
                stateFailure = 74;
            }
            else
            {
                if (FAILED(swapChain->Present(nullptr, nullptr, nullptr, nullptr, 0)))
                {
                    stateFailure = 75;
                }
                swapChain->Release();
            }
        }
        if (frame == 10 && HasEnvironmentValue(L"BAHAMUT_TEST_FILL_MODE_BOUNDARY"))
        {
            DWORD fillMode                    = 0;
            gFillModeOverlayRestoredWireframe = SUCCEEDED(
                                                    device->GetRenderState(D3DRS_FILLMODE, &fillMode)) &&
                                                fillMode == D3DFILL_WIREFRAME;
            device->SetRenderState(D3DRS_ZENABLE, D3DZB_FALSE);
        }
        if (stateFailure != 0)
        {
            for (IDirect3DSurface9* target : expectedRenderTargets)
            {
                if (target != nullptr)
                    target->Release();
            }
            if (expectedDepthStencil != nullptr)
                expectedDepthStencil->Release();
            break;
        }
        if (HasEnvironmentValue(L"BAHAMUT_TEST_STUB_STATE_CORRUPTION"))
        {
            D3DMATRIX corruptedView = view;
            corruptedView._11 += 1.0f;
            device->SetTransform(D3DTS_VIEW, &corruptedView);
        }
        DWORD                             zEnable        = 0;
        DWORD                             lighting       = 0;
        DWORD                             cullMode       = 0;
        DWORD                             colorOperation = 0;
        DWORD                             minFilter      = 0;
        D3DMATRIX                         actualWorld{};
        D3DMATRIX                         actualView{};
        D3DMATRIX                         actualProjection{};
        D3DVIEWPORT9                      actualViewport{};
        RECT                              actualScissor{};
        std::array<IDirect3DSurface9*, 4> actualRenderTargets{};
        IDirect3DSurface9*                actualDepthStencil = nullptr;
        if (FAILED(device->GetRenderState(D3DRS_ZENABLE, &zEnable)) || zEnable != D3DZB_FALSE)
            stateFailure = 51;
        else if (FAILED(device->GetRenderState(D3DRS_LIGHTING, &lighting)) || lighting != TRUE)
            stateFailure = 52;
        else if (FAILED(device->GetRenderState(D3DRS_CULLMODE, &cullMode)) || cullMode != D3DCULL_CW)
            stateFailure = 53;
        else if (FAILED(device->GetTextureStageState(0, D3DTSS_COLOROP, &colorOperation)) || colorOperation != D3DTOP_DISABLE)
            stateFailure = 54;
        else if (FAILED(device->GetSamplerState(0, D3DSAMP_MINFILTER, &minFilter)) || minFilter != D3DTEXF_POINT)
            stateFailure = 55;
        else if (FAILED(device->GetTransform(D3DTS_WORLD, &actualWorld)) || memcmp(&actualWorld, &world, sizeof(world)) != 0)
            stateFailure = 56;
        else if (FAILED(device->GetTransform(D3DTS_VIEW, &actualView)) || memcmp(&actualView, &view, sizeof(view)) != 0)
            stateFailure = 59;
        else if (FAILED(device->GetTransform(D3DTS_PROJECTION, &actualProjection)) || memcmp(&actualProjection, &projection, sizeof(projection)) != 0)
            stateFailure = 60;
        else if (FAILED(device->GetViewport(&actualViewport)) || memcmp(&actualViewport, &viewport, sizeof(viewport)) != 0)
            stateFailure = 57;
        else if (FAILED(device->GetScissorRect(&actualScissor)) || !EqualRect(&actualScissor, &scissor))
            stateFailure = 58;
        if (stateFailure == 0)
        {
            for (DWORD index = 0; index < actualRenderTargets.size(); ++index)
            {
                const HRESULT result  = device->GetRenderTarget(index, &actualRenderTargets[index]);
                const bool    matches = expectedRenderTargets[index] == nullptr
                                            ? result == D3DERR_NOTFOUND
                                            : SUCCEEDED(result) && actualRenderTargets[index] == expectedRenderTargets[index];
                if (!matches)
                {
                    stateFailure = 65;
                    break;
                }
            }
        }
        if (stateFailure == 0)
        {
            const HRESULT result  = device->GetDepthStencilSurface(&actualDepthStencil);
            const bool    matches = expectedDepthStencil == nullptr
                                        ? result == D3DERR_NOTFOUND
                                        : SUCCEEDED(result) && actualDepthStencil == expectedDepthStencil;
            if (!matches)
                stateFailure = 66;
        }
        for (IDirect3DSurface9* target : expectedRenderTargets)
        {
            if (target != nullptr)
                target->Release();
        }
        for (IDirect3DSurface9* target : actualRenderTargets)
        {
            if (target != nullptr)
                target->Release();
        }
        if (expectedDepthStencil != nullptr)
            expectedDepthStencil->Release();
        if (actualDepthStencil != nullptr)
            actualDepthStencil->Release();
        if (stateFailure != 0)
        {
            break;
        }
        if (frame == 5)
        {
            SendMessageW(window, WM_KEYDOWN, 'A', 0);
            SendMessageW(window, WM_KEYUP, 'A', static_cast<LPARAM>(-2147483647 - 1));
            SendMessageW(window, WM_LBUTTONDOWN, MK_LBUTTON, MAKELPARAM(20, 20));
            SendMessageW(window, WM_LBUTTONUP, 0, MAKELPARAM(20, 20));
            SendMessageW(window, WM_MOUSEMOVE, 0, MAKELPARAM(20, 20));
        }
        if (frame == 8 && HasEnvironmentValue(L"BAHAMUT_TEST_DIRECTINPUT_BOUNDARY"))
        {
            SendKey(window, VK_F12);
        }
        if (frame == 9 && HasEnvironmentValue(L"BAHAMUT_TEST_PRINT_SCREEN_OWNERSHIP"))
        {
            constexpr int probeHotkeyId = 0xB141;
            SendMessageW(window, WM_ACTIVATE, WA_ACTIVE, 0);
            const bool duplicateRegistered = RegisterHotKey(
                                                 window, probeHotkeyId, MOD_NOREPEAT, VK_SNAPSHOT) != FALSE;
            gPrintScreenOwnedWhileActive   = !duplicateRegistered;
            if (duplicateRegistered)
            {
                UnregisterHotKey(window, probeHotkeyId);
            }

            SendMessageW(window, WM_ACTIVATE, WA_INACTIVE, 0);
            gPrintScreenReleasedWhileInactive = RegisterHotKey(
                                                    window, probeHotkeyId, MOD_NOREPEAT, VK_SNAPSHOT) != FALSE;
            if (gPrintScreenReleasedWhileInactive)
            {
                UnregisterHotKey(window, probeHotkeyId);
            }
        }
        if ((frame == 39 || frame == 79) && !(frame == 79 && HasEnvironmentValue(L"BAHAMUT_TEST_OVERLAY_BACKEND_RETRY")))
        {
            if (frame == 39 && HasEnvironmentValue(L"BAHAMUT_TEST_FILL_MODE_BOUNDARY"))
            {
                device->SetRenderState(D3DRS_ZENABLE, D3DZB_TRUE);
            }
            const UINT width  = frame == 39 ? 400 : 320;
            const UINT height = frame == 39 ? 300 : 240;
            SetWindowPos(window, nullptr, 0, 0, static_cast<int>(width), static_cast<int>(height), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            parameters.BackBufferWidth  = width;
            parameters.BackBufferHeight = height;
            if (FAILED(device->Reset(&parameters)))
            {
                stateFailure = frame == 39 ? 61 : 62;
                break;
            }
        }
        if (frame == 80 && HasEnvironmentValue(L"BAHAMUT_TEST_SCREENSHOT_REQUEST"))
        {
            const UINT screenshotKey = ScreenshotTestVirtualKey();
            if (screenshotKey == VK_SNAPSHOT)
            {
                SendMessageW(window, WM_ACTIVATE, WA_ACTIVE, 0);
                SendMessageW(window, WM_HOTKEY, kScreenshotHotkeyId, MAKELPARAM(MOD_NOREPEAT, VK_SNAPSHOT));
                SendMessageW(window, WM_ACTIVATE, WA_INACTIVE, 0);
            }
            else
            {
                SendMessageW(window, WM_KEYUP, screenshotKey, static_cast<LPARAM>(-2147483647 - 1));
            }
        }
        Sleep(1);
    }
    if (stateFailure == 0)
    {
        if (FAILED(secondaryDevice->BeginScene()))
        {
            stateFailure = 63;
        }
        else
        {
            secondaryDevice->EndScene();
        }
        if (stateFailure == 0 && FAILED(secondaryDevice->Reset(&secondaryParameters)))
        {
            stateFailure = 64;
        }
    }
    if (HasEnvironmentValue(L"BAHAMUT_RUNTIME_BORDERLESS"))
    {
        gBorderlessPreservedAfterReset = IsMonitorBorderless(window);
    }
    secondaryDevice->Release();
    device->Release();
    direct3d->Release();
    DestroyWindow(window);
    UnregisterClassW(className, instance);
    if (stateFailure != 0)
    {
        RecordStateFailure(stateFailure);
        return stateFailure;
    }
    if (gForwardedAKeyDownCount != 1 || gForwardedAKeyUpCount != 1)
        return 71;
    if (gForwardedMouseDownCount != 1 || gForwardedMouseUpCount != 1)
        return 72;
    RecordDirectInputResult();
    RecordXInputResult();
    RecordPrintScreenResult();
    RecordFillModeResult();
    RecordFpsHotkeyResult();
    RecordBorderlessResult();
    return 0;
}

} // namespace

extern "C" __declspec(dllexport) __declspec(noinline)
LRESULT CALLBACK
BahamutTestKeyboardCallback(
    HWND window, UINT message, WPARAM wParam, LPARAM lParam, UINT_PTR subclassId, DWORD_PTR referenceData)
{
    UNREFERENCED_PARAMETER(subclassId);
    UNREFERENCED_PARAMETER(referenceData);
    if ((message == WM_KEYDOWN || message == WM_KEYUP) && wParam == VK_F11)
    {
        ++gRetailF11MessageCount;
    }
    if ((message == WM_KEYDOWN || message == WM_KEYUP) && wParam == VK_F12)
    {
        ++gRetailF12MessageCount;
    }
    return SendMessageW(window, message, wParam, lParam);
}

extern "C" __declspec(dllexport) void BahamutTestPadUpdate()
{
    ++gStubPadOriginalCalls;
    memset(gStubPadDevice.data() + kStubPadStateOffset, 0, 0x2c);
    memcpy(gStubPadDevice.data() + kStubPadStateOffset + sizeof(std::uint32_t) * 4, &gStubPadButtons, sizeof(gStubPadButtons));
}

int WINAPI wWinMain(HINSTANCE instance, HINSTANCE, PWSTR, int)
{
    if (HasEnvironmentValue(L"BAHAMUT_STUB_DIRECT_LAUNCH"))
    {
        wchar_t    runtimeEvent[2]{};
        const bool noRuntimeModule      = GetModuleHandleW(L"bahamut.dll") == nullptr;
        const bool noScreenshotPlugin   = GetModuleHandleW(L"screenshot.dll") == nullptr;
        const bool noRuntimeEnvironment = GetEnvironmentVariableW(
                                              L"BAHAMUT_RUNTIME_READY_EVENT", runtimeEvent, ARRAYSIZE(runtimeEvent)) == 0;
        return noRuntimeModule && noScreenshotPlugin && noRuntimeEnvironment ? 0 : 81;
    }
    const bool patchesMatch = MatchesExpected(L"BAHAMUT_STUB_EXPECT_SERVER_UTC", gStubServerUtcPatch) && MatchesExpected(L"BAHAMUT_STUB_EXPECT_LOBBY_HOST", gStubLobbyHostPatch);

    SignalFromEnvironment(L"_entry");
    if (!patchesMatch)
    {
        SignalFromEnvironment(L"_patch_failed");
        return 42;
    }

    SignalFromEnvironment(L"_resume");
    InitializeDirectInputStub();
    return ExerciseFrameBoundary(instance);
}

// Keep the marker in the image so the helper can recognize only this
// in-repo target without accepting an arbitrary executable.
extern "C" __declspec(dllexport) const char* BahamutStubIdentityMarker()
{
    return bahamut_runtime_contract::kStubMarker;
}
