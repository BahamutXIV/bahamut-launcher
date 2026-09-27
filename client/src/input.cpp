#include "input.h"
#include "addon_host.h"
#include "input_policy.h"
#include "native_plugin_host.h"
#include "render_boundary.h"

#include <MinHook.h>
#include <imgui.h>
#include <imgui_impl_win32.h>
#include <xinput.h>

#include <array>
#include <cwchar>
#include <fstream>
#include <optional>
#include <string_view>

extern IMGUI_IMPL_API LRESULT ImGui_ImplWin32_WndProcHandler(
    HWND window, UINT message, WPARAM wParam, LPARAM lParam);

namespace
{

using XInputGetStateFunction         = DWORD(WINAPI*)(DWORD, XINPUT_STATE*);
using PadUpdateFunction              = void(__thiscall*)(void*);
using RetailKeyboardCallbackFunction = LRESULT(CALLBACK*)(
    HWND, UINT, WPARAM, LPARAM, UINT_PTR, DWORD_PTR);

constexpr unsigned int kDisconnectedControllerPollFrames = 120;
// Retail 1.23b PadDevice layout and update RVA: xivl-client-structs@25c9d48,
// manifests/input_stack_routing.json and manifests/structs.json.
constexpr std::uintptr_t kPadUpdateRva              = 0x009337a0;
constexpr std::uintptr_t kRetailKeyboardCallbackRva = 0x009317e0;
constexpr std::size_t    kPadRecordVectorOffset     = 0x1c;
constexpr std::size_t    kPadStateOffset            = 0x28;
constexpr std::size_t    kPadRecordSize             = 0x72c;
constexpr std::size_t    kDirectInputDeviceOffset   = 0x724;
constexpr std::size_t    kSelectedPadRecordOffset   = 0x1c8;
constexpr std::size_t    kDirectInputControllerSlot = kXInputControllerCount;

WNDPROC                                           gOriginalWindowProcedure        = nullptr;
XInputGetStateFunction                            gXInputGetState                 = nullptr;
PadUpdateFunction                                 gOriginalPadUpdate              = nullptr;
RetailKeyboardCallbackFunction                    gOriginalRetailKeyboardCallback = nullptr;
BahamutRuntimeTelemetry*                          gTelemetry                      = nullptr;
ControllerSelection                               gControllerSelection;
ClientPadMailbox                                  gDirectInputMailbox;
InputReleasePolicy                                gInputReleasePolicy;
std::array<std::uint32_t, kXInputControllerCount> gControllerPacketNumbers{};
std::array<bool, kXInputControllerCount>          gControllerConnected{};
unsigned int                                      gControllerPollFrame   = kDisconnectedControllerPollFrames - 1;
volatile LONG                                     gOverlayVisible        = 0;
NativePluginHost*                                 gNativePluginHost      = nullptr;
AddonHost*                                        gAddonHost             = nullptr;
HWND                                              gInputWindow           = nullptr;
volatile LONG                                     gInputWindowActive     = 0;
bool                                              gFillModeHotkeyEnabled = false;
bool                                              gFpsHotkeyEnabled      = false;

void RecordTestResult(std::string_view text);

bool IsExecutableProtection(DWORD protection)
{
    const DWORD base = protection & 0xff;
    return base == PAGE_EXECUTE || base == PAGE_EXECUTE_READ || base == PAGE_EXECUTE_READWRITE || base == PAGE_EXECUTE_WRITECOPY;
}

bool IsModuleCodeAddress(void* address, HMODULE expectedModule)
{
    MEMORY_BASIC_INFORMATION memory{};
    if (VirtualQuery(address, &memory, sizeof(memory)) == 0 || memory.State != MEM_COMMIT || !IsExecutableProtection(memory.Protect))
    {
        return false;
    }
    HMODULE owner = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                            reinterpret_cast<LPCWSTR>(address),
                            &owner))
    {
        return false;
    }
    return owner == expectedModule;
}

void* ResolvePadUpdateTarget()
{
    HMODULE host = GetModuleHandleW(nullptr);
    if (host == nullptr)
    {
        return nullptr;
    }
    wchar_t     path[MAX_PATH]{};
    const DWORD length = GetModuleFileNameW(host, path, ARRAYSIZE(path));
    if (length == 0 || length >= ARRAYSIZE(path))
    {
        return nullptr;
    }
    const wchar_t* name = wcsrchr(path, L'\\');
    name                = name == nullptr ? path : name + 1;
    if (_wcsicmp(name, L"bahamut-test-client.exe") == 0)
    {
        return reinterpret_cast<void*>(GetProcAddress(host, "BahamutTestPadUpdate"));
    }
    return reinterpret_cast<unsigned char*>(host) + kPadUpdateRva;
}

bool HasTestFault(const wchar_t* name)
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(name, value, ARRAYSIZE(value));
    return count != 0 && count < ARRAYSIZE(value) && value[0] != L'0';
}

void RecordTestResult(std::string_view text)
{
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(L"BAHAMUT_TEST_INPUT_RESULT_FILE",
                                                path,
                                                ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << text;
    }
}

void SetOverlayVisible(bool visible)
{
    const LONG previous = InterlockedExchange(&gOverlayVisible, visible ? 1 : 0);
    if (previous != (visible ? 1 : 0) && gTelemetry != nullptr)
    {
        InterlockedIncrement(&gTelemetry->overlayToggleCount);
    }
}

unsigned char* SelectedDirectInputRecord(void* device)
{
    const auto bytes = static_cast<unsigned char*>(device);
    const auto begin = *reinterpret_cast<unsigned char**>(
        bytes + kPadRecordVectorOffset);
    const auto end = *reinterpret_cast<unsigned char**>(
        bytes + kPadRecordVectorOffset + sizeof(void*));
    const std::uint32_t selected = *reinterpret_cast<std::uint32_t*>(
        bytes + kSelectedPadRecordOffset);
    const std::uintptr_t beginAddress = reinterpret_cast<std::uintptr_t>(begin);
    const std::uintptr_t endAddress   = reinterpret_cast<std::uintptr_t>(end);
    if (begin == nullptr || endAddress < beginAddress || selected == 0)
    {
        return nullptr;
    }
    const std::size_t count = (endAddress - beginAddress) / kPadRecordSize;
    if (selected > count)
    {
        return nullptr;
    }
    unsigned char* record = begin + (selected - 1) * kPadRecordSize;
    return record[1] == 0 ? record : nullptr;
}

void __fastcall HookedPadUpdate(void* device, void*)
{
    gOriginalPadUpdate(device);
    unsigned char* record    = device == nullptr
                                   ? nullptr
                                   : SelectedDirectInputRecord(device);
    const bool     connected = record != nullptr && *reinterpret_cast<void**>(record + kDirectInputDeviceOffset) != nullptr;
    if (!connected)
    {
        gDirectInputMailbox.Publish({}, false);
        return;
    }

    auto* state = reinterpret_cast<ClientPadState*>(
        static_cast<unsigned char*>(device) + kPadStateOffset);
    gDirectInputMailbox.Publish(*state, true);
}

bool IsKeyboardMessage(UINT message)
{
    return message == WM_CHAR || message == WM_KEYDOWN || message == WM_KEYUP || message == WM_SYSKEYDOWN || message == WM_SYSKEYUP;
}

bool IsVirtualKeyMessage(UINT message)
{
    return message == WM_KEYDOWN || message == WM_KEYUP || message == WM_SYSKEYDOWN || message == WM_SYSKEYUP;
}

InputMessageKind KeyMessageKind(UINT message)
{
    if (message == WM_KEYUP || message == WM_SYSKEYUP)
    {
        return InputMessageKind::VirtualKeyUp;
    }
    if (message == WM_CHAR)
    {
        return InputMessageKind::Character;
    }
    return InputMessageKind::VirtualKeyDown;
}

bool IsMouseMessage(UINT message)
{
    return message >= WM_MOUSEFIRST && message <= WM_MOUSELAST;
}

bool IsKeyDown(UINT message)
{
    return message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
}

bool IsKeyUp(UINT message)
{
    return message == WM_KEYUP || message == WM_SYSKEYUP;
}

void* ResolveRetailKeyboardCallbackTarget()
{
    HMODULE host = GetModuleHandleW(nullptr);
    if (host == nullptr)
    {
        return nullptr;
    }
    wchar_t     path[MAX_PATH]{};
    const DWORD length = GetModuleFileNameW(host, path, ARRAYSIZE(path));
    if (length == 0 || length >= ARRAYSIZE(path))
    {
        return nullptr;
    }
    const wchar_t* name = wcsrchr(path, L'\\');
    name                = name == nullptr ? path : name + 1;
    if (_wcsicmp(name, L"bahamut-test-client.exe") == 0)
    {
        auto* entry = reinterpret_cast<unsigned char*>(
            GetProcAddress(host, "_BahamutTestKeyboardCallback@24"));
        if (entry == nullptr)
        {
            // The llvm-mingw stub exports the stdcall callback undecorated.
            entry = reinterpret_cast<unsigned char*>(
                GetProcAddress(host, "BahamutTestKeyboardCallback"));
        }
        if (entry != nullptr && entry[0] == 0xe9)
        {
            const auto displacement = *reinterpret_cast<std::int32_t*>(entry + 1);
            return entry + 5 + displacement;
        }
        return entry;
    }
    return reinterpret_cast<unsigned char*>(host) + kRetailKeyboardCallbackRva;
}

bool IsShiftKey(WPARAM key)
{
    return key == VK_SHIFT || key == VK_LSHIFT || key == VK_RSHIFT;
}

bool IsConfiguredBuiltInKey(WPARAM key)
{
    return (key == VK_F11 && gFillModeHotkeyEnabled) || (key == VK_F12 && gFpsHotkeyEnabled);
}

bool HandleBuiltInKey(HWND window, UINT message, WPARAM key, LPARAM lParam)
{
    if (window != gInputWindow || !IsVirtualKeyMessage(message) || !IsConfiguredBuiltInKey(key) || InterlockedCompareExchange(&gInputWindowActive, 0, 0) == 0)
    {
        return false;
    }
    if (IsKeyDown(message) && (lParam & (1LL << 30)) == 0)
    {
        if (key == VK_F11)
        {
            SetWireframeEnabled(!IsWireframeEnabled());
        }
        else
        {
            if (gAddonHost != nullptr)
            {
                gAddonHost->DispatchCommand("/fps");
            }
        }
    }
    if (IsKeyDown(message))
    {
        gInputReleasePolicy.CaptureKey(static_cast<std::size_t>(key));
    }
    else
    {
        gInputReleasePolicy.RouteKey(
            static_cast<std::size_t>(key), InputMessageKind::VirtualKeyUp);
    }
    if (gTelemetry != nullptr)
    {
        InterlockedIncrement(&gTelemetry->inputCaptureCount);
    }
    return true;
}

LRESULT CALLBACK HookedRetailKeyboardCallback(HWND window, UINT message, WPARAM wParam, LPARAM lParam, UINT_PTR subclassId, DWORD_PTR referenceData)
{
    if (HandleBuiltInKey(window, message, wParam, lParam))
    {
        return 0;
    }
    return gOriginalRetailKeyboardCallback(window, message, wParam, lParam, subclassId, referenceData);
}

bool IsMouseButtonDown(UINT message)
{
    return message == WM_LBUTTONDOWN || message == WM_LBUTTONDBLCLK || message == WM_RBUTTONDOWN || message == WM_RBUTTONDBLCLK || message == WM_MBUTTONDOWN || message == WM_MBUTTONDBLCLK || message == WM_XBUTTONDOWN || message == WM_XBUTTONDBLCLK;
}

bool IsMouseButtonUp(UINT message)
{
    return message == WM_LBUTTONUP || message == WM_RBUTTONUP || message == WM_MBUTTONUP || message == WM_XBUTTONUP;
}

std::optional<std::size_t> MouseButton(UINT message, WPARAM wParam)
{
    switch (message)
    {
        case WM_LBUTTONDOWN:
        case WM_LBUTTONDBLCLK:
        case WM_LBUTTONUP:
            return 0;
        case WM_RBUTTONDOWN:
        case WM_RBUTTONDBLCLK:
        case WM_RBUTTONUP:
            return 1;
        case WM_MBUTTONDOWN:
        case WM_MBUTTONDBLCLK:
        case WM_MBUTTONUP:
            return 2;
        case WM_XBUTTONDOWN:
        case WM_XBUTTONDBLCLK:
        case WM_XBUTTONUP:
            return GET_XBUTTON_WPARAM(wParam) == XBUTTON1 ? 3 : 4;
        default:
            return std::nullopt;
    }
}

void ClearInputReleaseLocks(HWND window)
{
    const InputLockSnapshot snapshot = gInputReleasePolicy.Clear();
    if (ImGui::GetCurrentContext() == nullptr)
    {
        return;
    }
    for (std::size_t key = 0; key < snapshot.capturedKeys.size(); ++key)
    {
        if (snapshot.capturedKeys[key])
        {
            ImGui_ImplWin32_WndProcHandler(
                window, WM_KEYUP, static_cast<WPARAM>(key), 0);
        }
    }
    constexpr UINT mouseMessages[] = {
        WM_LBUTTONUP, WM_RBUTTONUP, WM_MBUTTONUP, WM_XBUTTONUP, WM_XBUTTONUP
    };
    for (std::size_t button = 0; button < snapshot.capturedMouse.size(); ++button)
    {
        if (!snapshot.capturedMouse[button])
        {
            continue;
        }
        const WPARAM wParam = button < 3
                                  ? 0
                                  : MAKEWPARAM(0, button == 3 ? XBUTTON1 : XBUTTON2);
        ImGui_ImplWin32_WndProcHandler(window, mouseMessages[button], wParam, 0);
    }
}

LRESULT CALLBACK OverlayWindowProcedure(HWND window, UINT message, WPARAM wParam, LPARAM lParam)
{
    if (window == gInputWindow && message == WM_ACTIVATE)
    {
        const bool active = LOWORD(wParam) != WA_INACTIVE;
        InterlockedExchange(&gInputWindowActive, active ? 1 : 0);
    }
    const bool                       keyboard    = IsKeyboardMessage(message);
    const bool                       virtualKey  = IsVirtualKeyMessage(message);
    const bool                       mouse       = IsMouseMessage(message);
    const std::size_t                key         = virtualKey && wParam < 256
                                                       ? static_cast<std::size_t>(wParam)
                                                       : 0;
    const std::optional<std::size_t> mouseButton = MouseButton(message, wParam);

    if (message == WM_KILLFOCUS || message == WM_CANCELMODE)
    {
        ClearInputReleaseLocks(window);
    }
    bool consumed = false;
    gNativePluginHost->OnWindowMessage(
        window, message, wParam, lParam, consumed);
    if (consumed)
    {
        if (gTelemetry != nullptr)
        {
            InterlockedIncrement(&gTelemetry->inputCaptureCount);
        }
        return 0;
    }
    const bool              activeGameWindow = window == gInputWindow && InterlockedCompareExchange(&gInputWindowActive, 0, 0) != 0;
    const InputReleaseRoute keyRoute         = virtualKey && wParam < 256
                                                   ? gInputReleasePolicy.RouteKey(key, KeyMessageKind(message))
                                                   : InputReleaseRoute::None;
    if (keyRoute == InputReleaseRoute::ImGui)
    {
        if (IsKeyUp(message) && ImGui::GetCurrentContext() != nullptr)
        {
            ImGui_ImplWin32_WndProcHandler(window, message, wParam, lParam);
        }
        if (gTelemetry != nullptr)
        {
            InterlockedIncrement(&gTelemetry->inputCaptureCount);
        }
        return 0;
    }
    if (keyRoute == InputReleaseRoute::Forward)
    {
        if (IsShiftKey(wParam) && ImGui::GetCurrentContext() != nullptr)
        {
            ImGui_ImplWin32_WndProcHandler(window, message, wParam, lParam);
        }
        if (gTelemetry != nullptr)
        {
            InterlockedIncrement(&gTelemetry->inputForwardCount);
        }
        return CallWindowProcW(gOriginalWindowProcedure, window, message, wParam, lParam);
    }

    const InputReleaseRoute mouseRoute = mouseButton
                                             ? gInputReleasePolicy.RouteMouse(*mouseButton,
                                                                              IsMouseButtonUp(message)
                                                                                  ? MouseMessageKind::ButtonUp
                                                                                  : MouseMessageKind::ButtonDown)
                                             : InputReleaseRoute::None;
    if (mouseRoute == InputReleaseRoute::ImGui)
    {
        if (ImGui::GetCurrentContext() != nullptr)
        {
            ImGui_ImplWin32_WndProcHandler(window, message, wParam, lParam);
        }
        if (gTelemetry != nullptr)
        {
            InterlockedIncrement(&gTelemetry->inputCaptureCount);
        }
        return 0;
    }
    if (mouseRoute == InputReleaseRoute::Forward)
    {
        if (IsMouseButtonUp(message) && ImGui::GetCurrentContext() != nullptr)
        {
            ImGui_ImplWin32_WndProcHandler(window, message, wParam, lParam);
        }
        if (gTelemetry != nullptr)
        {
            InterlockedIncrement(&gTelemetry->inputForwardCount);
        }
        return CallWindowProcW(gOriginalWindowProcedure, window, message, wParam, lParam);
    }

    if (HandleBuiltInKey(window, message, wParam, lParam))
    {
        return 0;
    }
    if (virtualKey && IsConfiguredBuiltInKey(wParam) && IsKeyDown(message) && !activeGameWindow)
    {
        gInputReleasePolicy.ForwardKey(key);
        if (gTelemetry != nullptr)
        {
            InterlockedIncrement(&gTelemetry->inputForwardCount);
        }
        return CallWindowProcW(gOriginalWindowProcedure,
                               window,
                               message,
                               wParam,
                               lParam);
    }

    if (ImGui::GetCurrentContext() != nullptr)
    {
        const bool shiftMessage   = virtualKey && IsShiftKey(wParam);
        const bool shiftDragInput = mouse && (ImGui::GetIO().KeyShift || (GetKeyState(VK_SHIFT) & 0x8000) != 0);
        if (shiftMessage || shiftDragInput)
        {
            ImGui_ImplWin32_WndProcHandler(window, message, wParam, lParam);
            const ImGuiIO& io          = ImGui::GetIO();
            const bool     interactive = io.KeyShift;
            const bool     capture     = interactive && ((keyboard && (io.WantCaptureKeyboard || io.WantTextInput)) || (mouse && io.WantCaptureMouse));
            if (capture)
            {
                if (virtualKey && IsKeyDown(message) && wParam < 256)
                {
                    gInputReleasePolicy.CaptureKey(key);
                }
                if (mouseButton && IsMouseButtonDown(message))
                {
                    gInputReleasePolicy.CaptureMouse(*mouseButton);
                }
                if (gTelemetry != nullptr)
                {
                    InterlockedIncrement(&gTelemetry->inputCaptureCount);
                }
                return 0;
            }
        }
    }

    if (virtualKey && IsKeyDown(message) && wParam < 256)
    {
        gInputReleasePolicy.ForwardKey(key);
    }
    if (mouseButton && IsMouseButtonDown(message))
    {
        gInputReleasePolicy.ForwardMouse(*mouseButton);
    }
    if ((keyboard || mouse) && gTelemetry != nullptr)
    {
        InterlockedIncrement(&gTelemetry->inputForwardCount);
    }
    return CallWindowProcW(gOriginalWindowProcedure, window, message, wParam, lParam);
}

} // namespace

void ConfigureBuiltInHotkeys(bool fillMode, bool fps)
{
    gFillModeHotkeyEnabled = fillMode;
    gFpsHotkeyEnabled      = fps;
}

bool InstallInputBoundary(HWND window, BahamutRuntimeTelemetry* telemetry, AddonHost* addonHost, NativePluginHost* nativePluginHost)
{
    if (window == nullptr || gOriginalWindowProcedure != nullptr)
    {
        return window != nullptr && gOriginalWindowProcedure != nullptr;
    }

    constexpr const wchar_t* moduleNames[] = {
        L"xinput1_3.dll", L"xinput1_4.dll", L"xinput9_1_0.dll"
    };
    HMODULE xinput = nullptr;
    for (const wchar_t* name : moduleNames)
    {
        xinput = GetModuleHandleW(name);
        if (xinput == nullptr)
            xinput = LoadLibraryW(name);
        if (xinput != nullptr)
        {
            break;
        }
    }
    void* getState               = xinput == nullptr ? nullptr
                                                     : reinterpret_cast<void*>(GetProcAddress(xinput, "XInputGetState"));
    void* padUpdate              = ResolvePadUpdateTarget();
    void* retailKeyboardCallback = ResolveRetailKeyboardCallbackTarget();
    if (getState == nullptr || !IsModuleCodeAddress(getState, xinput) || padUpdate == nullptr || retailKeyboardCallback == nullptr || !IsModuleCodeAddress(padUpdate, GetModuleHandleW(nullptr)) || !IsModuleCodeAddress(retailKeyboardCallback, GetModuleHandleW(nullptr)))
    {
        return false;
    }
    gXInputGetState = reinterpret_cast<XInputGetStateFunction>(getState);
    if (MH_CreateHook(padUpdate, reinterpret_cast<void*>(&HookedPadUpdate), reinterpret_cast<void**>(&gOriginalPadUpdate)) != MH_OK)
    {
        gXInputGetState    = nullptr;
        gOriginalPadUpdate = nullptr;
        return false;
    }
    if (MH_CreateHook(retailKeyboardCallback, reinterpret_cast<void*>(&HookedRetailKeyboardCallback), reinterpret_cast<void**>(&gOriginalRetailKeyboardCallback)) != MH_OK)
    {
        MH_RemoveHook(padUpdate);
        gXInputGetState                 = nullptr;
        gOriginalPadUpdate              = nullptr;
        gOriginalRetailKeyboardCallback = nullptr;
        return false;
    }
    if (MH_QueueEnableHook(padUpdate) != MH_OK || MH_QueueEnableHook(retailKeyboardCallback) != MH_OK || MH_ApplyQueued() != MH_OK)
    {
        MH_DisableHook(padUpdate);
        MH_DisableHook(retailKeyboardCallback);
        MH_RemoveHook(padUpdate);
        MH_RemoveHook(retailKeyboardCallback);
        gXInputGetState                 = nullptr;
        gOriginalPadUpdate              = nullptr;
        gOriginalRetailKeyboardCallback = nullptr;
        return false;
    }

    const LONG_PTR original = HasTestFault(L"BAHAMUT_TEST_INPUT_BOUNDARY_FAILURE")
                                  ? 0
                                  : SetWindowLongPtrW(window, GWLP_WNDPROC, reinterpret_cast<LONG_PTR>(&OverlayWindowProcedure));
    if (original == 0)
    {
        MH_DisableHook(padUpdate);
        MH_DisableHook(retailKeyboardCallback);
        MH_RemoveHook(padUpdate);
        MH_RemoveHook(retailKeyboardCallback);
        gXInputGetState                 = nullptr;
        gOriginalPadUpdate              = nullptr;
        gOriginalRetailKeyboardCallback = nullptr;
        RecordTestResult(gXInputGetState == nullptr && gOriginalPadUpdate == nullptr && gOriginalRetailKeyboardCallback == nullptr && gOriginalWindowProcedure == nullptr
                             ? "input_boundary_rollback=ok\n"
                             : "input_boundary_rollback=failed\n");
        return false;
    }
    gOriginalWindowProcedure = reinterpret_cast<WNDPROC>(original);
    gTelemetry               = telemetry;
    gAddonHost               = addonHost;
    gNativePluginHost        = nativePluginHost;
    gInputWindow             = window;
    InterlockedExchange(&gInputWindowActive,
                        GetForegroundWindow() == window ? 1 : 0);
    if (GetForegroundWindow() == window)
    {
        bool consumed = false;
        gNativePluginHost->OnWindowMessage(
            window, WM_ACTIVATE, WA_ACTIVE, 0, consumed);
    }
    return gOriginalWindowProcedure != nullptr;
}

void EnableOverlay()
{
    InterlockedExchange(&gOverlayVisible, 1);
}

void DisableOverlay()
{
    SetOverlayVisible(false);
}

bool IsOverlayVisible()
{
    return InterlockedCompareExchange(&gOverlayVisible, 0, 0) != 0;
}

void UpdateControllerObservation()
{
    if (gXInputGetState == nullptr)
    {
        return;
    }
    std::array<ControllerPresence, kControllerSlotCount> controllers{};
    ++gControllerPollFrame;
    for (DWORD index = 0; index < gControllerConnected.size(); ++index)
    {
        const bool shouldPoll = gControllerConnected[index] || gControllerPollFrame % kDisconnectedControllerPollFrames == 0;
        if (shouldPoll)
        {
            XINPUT_STATE state{};
            gControllerConnected[index] = gXInputGetState(
                                              index, &state) == ERROR_SUCCESS;
            if (gControllerConnected[index])
            {
                gControllerPacketNumbers[index] = state.dwPacketNumber;
            }
        }
        controllers[index].connected = gControllerConnected[index];
        if (controllers[index].connected)
        {
            controllers[index].packetNumber = gControllerPacketNumbers[index];
        }
    }
    const ClientPadSnapshot directInput     = gDirectInputMailbox.Snapshot();
    controllers[kDirectInputControllerSlot] = {
        directInput.connected,
        directInput.packetNumber
    };

    const int previous = gControllerSelection.Active();
    const int active   = gControllerSelection.Update(controllers);
    if (active != previous && gTelemetry != nullptr)
    {
        InterlockedIncrement(&gTelemetry->controllerSwitchCount);
    }
}
