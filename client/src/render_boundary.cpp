#include "render_boundary.h"
#include "addon_host.h"
#include "camera_zoom.h"
#include "chat_boundary.h"
#include "command_boundary.h"
#include "input.h"
#include "monitor_selection.h"
#include "native_plugin_host.h"
#include "object_distance.h"
#include "overlay.h"
#include "packet_observer.h"
#include "player_state.h"

#include <MinHook.h>
#include <d3d9.h>

#include <cstdio>
#include <cwchar>
#include <fstream>
#include <string>
#include <string_view>

namespace
{

constexpr std::size_t kDirect3dCreateDeviceMethodIndex   = 16;
constexpr std::size_t kDeviceResetMethodIndex            = 16;
constexpr std::size_t kDevicePresentMethodIndex          = 17;
constexpr std::size_t kDeviceSetRenderStateMethodIndex   = 57;
constexpr std::size_t kSwapChainPresentMethodIndex       = 3;
constexpr LONG        kRateLogInterval                   = 300;
constexpr LONG        kD3d9LoadFailureStage              = 1;
constexpr LONG        kDirect3dCreate9ExportFailureStage = 2;
constexpr LONG        kAddressQueryFailureStage          = 3;
constexpr LONG        kAddressStateFailureStage          = 4;
constexpr LONG        kAddressProtectionFailureStage     = 5;
constexpr LONG        kAddressOwnerFailureStage          = 6;
constexpr LONG        kAddressPathFailureStage           = 7;
constexpr LONG        kAddressNameFailureStage           = 8;
constexpr LONG        kMinHookInitializeFailureStage     = 9;
constexpr LONG        kMinHookCreateFailureStage         = 10;
constexpr LONG        kMinHookEnableFailureStage         = 11;

using Direct3DCreate9Function  = IDirect3D9*(WINAPI*)(UINT);
using CreateDeviceFunction     = HRESULT(STDMETHODCALLTYPE*)(IDirect3D9*, UINT, D3DDEVTYPE, HWND, DWORD, D3DPRESENT_PARAMETERS*, IDirect3DDevice9**);
using ResetFunction            = HRESULT(STDMETHODCALLTYPE*)(IDirect3DDevice9*, D3DPRESENT_PARAMETERS*);
using DevicePresentFunction    = HRESULT(STDMETHODCALLTYPE*)(IDirect3DDevice9*,
                                                             const RECT*,
                                                             const RECT*,
                                                             HWND,
                                                             const RGNDATA*);
using SetRenderStateFunction   = HRESULT(STDMETHODCALLTYPE*)(IDirect3DDevice9*,
                                                             D3DRENDERSTATETYPE,
                                                             DWORD);
using SwapChainPresentFunction = HRESULT(STDMETHODCALLTYPE*)(IDirect3DSwapChain9*,
                                                             const RECT*,
                                                             const RECT*,
                                                             HWND,
                                                             const RGNDATA*,
                                                             DWORD);

Direct3DCreate9Function             gOriginalDirect3DCreate9  = nullptr;
CreateDeviceFunction                gOriginalCreateDevice     = nullptr;
ResetFunction                       gOriginalReset            = nullptr;
DevicePresentFunction               gOriginalDevicePresent    = nullptr;
SetRenderStateFunction              gOriginalSetRenderState   = nullptr;
SwapChainPresentFunction            gOriginalSwapChainPresent = nullptr;
BahamutRuntimeTelemetry*            gTelemetry                = nullptr;
HANDLE                              gFrameEvent               = nullptr;
AddonHost*                          gAddonHost                = nullptr;
NativePluginHost*                   gNativePluginHost         = nullptr;
NativePluginHost*                   gDiscordPluginHost        = nullptr;
bahamut_client::PlayerStateService* gPlayerState              = nullptr;
// WineD3D's device Present bypasses the swap-chain Present hook, so both are
// hooked; a swap-chain Present nested in a hooked device Present only forwards.
thread_local bool gInDevicePresent = false;
// The first hooked device owns fill-mode state for the process, matching the
// existing gOverlayDevice ownership contract.
IDirect3DDevice9* gOwnedDevice                     = nullptr;
volatile LONG     gWireframeEnabled                = 0;
volatile LONG     gDepthTestEnabled                = 0;
bool              gBorderlessEnabled               = false;
HWND              gBorderlessWindow                = nullptr;
LONG_PTR          gBorderlessOriginalStyle         = 0;
LONG_PTR          gBorderlessOriginalExtendedStyle = 0;
RECT              gBorderlessOriginalRect{};
std::wstring      gBorderlessMonitorId;

void QueueAddonChat(void* context, std::string_view source, std::string_view message)
{
    auto* host = static_cast<AddonHost*>(context);
    if (host != nullptr)
    {
        host->QueueChat(source, message);
    }
}

bool SetWindowStyle(HWND window, int index, LONG_PTR value)
{
    SetLastError(ERROR_SUCCESS);
    return SetWindowLongPtrW(window, index, value) != 0 || GetLastError() == ERROR_SUCCESS;
}

void RestoreBorderlessWindow()
{
    if (gBorderlessWindow == nullptr || !IsWindow(gBorderlessWindow))
    {
        gBorderlessWindow = nullptr;
        return;
    }
    PhysicalCoordinateContext physicalCoordinates;
    SetWindowStyle(gBorderlessWindow, GWL_STYLE, gBorderlessOriginalStyle);
    SetWindowStyle(gBorderlessWindow, GWL_EXSTYLE, gBorderlessOriginalExtendedStyle);
    SetWindowPos(gBorderlessWindow, nullptr, gBorderlessOriginalRect.left, gBorderlessOriginalRect.top, gBorderlessOriginalRect.right - gBorderlessOriginalRect.left, gBorderlessOriginalRect.bottom - gBorderlessOriginalRect.top, SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED);
    gBorderlessWindow = nullptr;
}

bool ApplyBorderlessWindow(HWND window)
{
    if (!gBorderlessEnabled || window == nullptr)
    {
        return true;
    }
    PhysicalCoordinateContext physicalCoordinates;
    if (gBorderlessWindow != nullptr && gBorderlessWindow != window)
    {
        RestoreBorderlessWindow();
    }
    RECT originalRect{};
    if (!GetWindowRect(window, &originalRect))
    {
        return false;
    }
    RECT bounds{};
    if (gBorderlessMonitorId.empty())
    {
        MONITORINFO monitorInfo{};
        monitorInfo.cbSize     = sizeof(monitorInfo);
        const HMONITOR monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
        if (monitor == nullptr || !GetMonitorInfoW(monitor, &monitorInfo))
        {
            return false;
        }
        bounds = monitorInfo.rcMonitor;
    }
    else
    {
        const auto  monitors = EnumerateBorderlessMonitors();
        const auto* selected = SelectBorderlessMonitor(monitors, gBorderlessMonitorId);
        if (selected == nullptr)
        {
            return false;
        }
        bounds = selected->bounds;
    }
    const bool     firstApplication        = gBorderlessWindow == nullptr;
    const LONG_PTR originalStyle           = firstApplication
                                                 ? GetWindowLongPtrW(window, GWL_STYLE)
                                                 : gBorderlessOriginalStyle;
    const LONG_PTR originalExtendedStyle   = firstApplication
                                                 ? GetWindowLongPtrW(window, GWL_EXSTYLE)
                                                 : gBorderlessOriginalExtendedStyle;
    const LONG_PTR borderlessStyle         = (originalStyle & (WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS)) | WS_POPUP;
    const LONG_PTR borderlessExtendedStyle = originalExtendedStyle & ~(WS_EX_DLGMODALFRAME | WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE | WS_EX_STATICEDGE);
    if (!SetWindowStyle(window, GWL_STYLE, borderlessStyle) || !SetWindowStyle(window, GWL_EXSTYLE, borderlessExtendedStyle))
    {
        SetWindowStyle(window, GWL_STYLE, originalStyle);
        SetWindowStyle(window, GWL_EXSTYLE, originalExtendedStyle);
        return false;
    }
    if (!SetWindowPos(window, nullptr, bounds.left, bounds.top, bounds.right - bounds.left, bounds.bottom - bounds.top, SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED))
    {
        SetWindowStyle(window, GWL_STYLE, originalStyle);
        SetWindowStyle(window, GWL_EXSTYLE, originalExtendedStyle);
        SetWindowPos(window, nullptr, originalRect.left, originalRect.top, originalRect.right - originalRect.left, originalRect.bottom - originalRect.top, SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED);
        return false;
    }
    if (firstApplication)
    {
        gBorderlessWindow                = window;
        gBorderlessOriginalStyle         = originalStyle;
        gBorderlessOriginalExtendedStyle = originalExtendedStyle;
        gBorderlessOriginalRect          = originalRect;
        OutputDebugStringA("Bahamut runtime: display mode=Borderless\n");
    }
    return true;
}

bool IsDepthTestEnabledValue(DWORD value)
{
    return value == D3DZB_TRUE || value == D3DZB_USEW;
}

bool RefreshOwnedDepthTestState(IDirect3DDevice9* device)
{
    const LONG cached = InterlockedCompareExchange(&gDepthTestEnabled, 0, 0);
    DWORD      value  = D3DZB_FALSE;
    if (FAILED(device->GetRenderState(D3DRS_ZENABLE, &value)))
    {
        return cached != 0;
    }
    const bool enabled = IsDepthTestEnabledValue(value);
    if (device == gOwnedDevice)
    {
        InterlockedExchange(&gDepthTestEnabled, enabled ? 1 : 0);
    }
    return enabled;
}

void ApplyOwnedFillMode(IDirect3DDevice9* device)
{
    if (device != gOwnedDevice || gOriginalSetRenderState == nullptr)
    {
        return;
    }
    const bool wireframe = IsWireframeEnabled() && InterlockedCompareExchange(&gDepthTestEnabled, 0, 0) != 0 && !IsOverlayDrawing(device);
    gOriginalSetRenderState(device, D3DRS_FILLMODE, wireframe ? D3DFILL_WIREFRAME : D3DFILL_SOLID);
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
    const DWORD count = GetEnvironmentVariableW(L"BAHAMUT_TEST_RENDER_RESULT_FILE",
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

void RollbackDeviceHooks(void* reset, void* setRenderState, void* devicePresent, void* swapChainPresent, bool resetCreated, bool setRenderStateCreated, bool devicePresentCreated, bool swapChainPresentCreated)
{
    RestoreBorderlessWindow();
    SetWireframeEnabled(false);
    if (resetCreated)
    {
        MH_DisableHook(reset);
        MH_RemoveHook(reset);
        gOriginalReset = nullptr;
    }
    if (setRenderStateCreated)
    {
        MH_DisableHook(setRenderState);
        MH_RemoveHook(setRenderState);
        gOriginalSetRenderState = nullptr;
    }
    if (devicePresentCreated)
    {
        MH_DisableHook(devicePresent);
        MH_RemoveHook(devicePresent);
        gOriginalDevicePresent = nullptr;
    }
    if (swapChainPresentCreated)
    {
        MH_DisableHook(swapChainPresent);
        MH_RemoveHook(swapChainPresent);
        gOriginalSwapChainPresent = nullptr;
    }
    gOwnedDevice = nullptr;
    InterlockedExchange(&gDepthTestEnabled, 0);
}

bool RollbackCreate9Hook(void* create9)
{
    SetWireframeEnabled(false);
    MH_DisableHook(create9);
    const bool removed       = MH_RemoveHook(create9) == MH_OK;
    gOriginalDirect3DCreate9 = nullptr;
    return removed;
}

void RollbackPlayerStateHook(bahamut_client::PlayerStateService* playerState)
{
    if (playerState != nullptr)
    {
        static_cast<void>(playerState->Shutdown());
    }
}

bool IsExecutableProtection(DWORD protection)
{
    const DWORD base = protection & 0xff;
    return base == PAGE_EXECUTE || base == PAGE_EXECUTE_READ || base == PAGE_EXECUTE_READWRITE || base == PAGE_EXECUTE_WRITECOPY;
}

bool IsSystemAppHelpPath(const wchar_t* path)
{
    wchar_t    windowsDirectory[MAX_PATH]{};
    const UINT length = GetWindowsDirectoryW(windowsDirectory, ARRAYSIZE(windowsDirectory));
    if (length == 0 || length >= ARRAYSIZE(windowsDirectory))
    {
        return false;
    }

    const std::wstring windows(windowsDirectory, length);
    const std::wstring system32 = windows + L"\\System32\\apphelp.dll";
    const std::wstring syswow64 = windows + L"\\SysWOW64\\apphelp.dll";
    return _wcsicmp(path, system32.c_str()) == 0 || _wcsicmp(path, syswow64.c_str()) == 0;
}

bool IsD3d9CodeAddress(void* address, LONG* failureStage = nullptr, BahamutRuntimeBootstrapDiagnostics* diagnostics = nullptr)
{
    MEMORY_BASIC_INFORMATION memory{};
    if (VirtualQuery(address, &memory, sizeof(memory)) == 0)
    {
        if (failureStage != nullptr)
        {
            *failureStage = kAddressQueryFailureStage;
        }
        return false;
    }
    if (memory.State != MEM_COMMIT)
    {
        if (failureStage != nullptr)
        {
            *failureStage = kAddressStateFailureStage;
        }
        return false;
    }
    if (!IsExecutableProtection(memory.Protect))
    {
        if (failureStage != nullptr)
        {
            *failureStage = kAddressProtectionFailureStage;
        }
        return false;
    }

    HMODULE owner = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                            reinterpret_cast<LPCWSTR>(address),
                            &owner))
    {
        if (failureStage != nullptr)
        {
            *failureStage = kAddressOwnerFailureStage;
        }
        return false;
    }
    wchar_t     path[MAX_PATH]{};
    const DWORD length = GetModuleFileNameW(owner, path, ARRAYSIZE(path));
    if (length == 0 || length >= ARRAYSIZE(path))
    {
        if (failureStage != nullptr)
        {
            *failureStage = kAddressPathFailureStage;
        }
        return false;
    }
    const wchar_t* name     = wcsrchr(path, L'\\');
    name                    = name == nullptr ? path : name + 1;
    const bool d3d9Owner    = _wcsicmp(name, L"d3d9.dll") == 0;
    const bool appHelpOwner = _wcsicmp(name, L"apphelp.dll") == 0 && IsSystemAppHelpPath(path);
    if (!d3d9Owner && !appHelpOwner)
    {
        if (diagnostics != nullptr)
        {
            std::wmemcpy(diagnostics->renderBoundaryOwnerPath, path, length + 1);
        }
        if (failureStage != nullptr)
        {
            *failureStage = kAddressNameFailureStage;
        }
        return false;
    }
    return true;
}

// Per-frame work for one present on the overlay device; callers run it before
// the original Present.
void ObservePresent(IDirect3DDevice9* device)
{
    bool hideOverlayForCapture = false;
    gNativePluginHost->OnPresentBegin(device, hideOverlayForCapture);
    if (gTelemetry != nullptr)
    {
        LARGE_INTEGER now{};
        QueryPerformanceCounter(&now);
        BahamutBeginTimingWrite(gTelemetry);
        const LONG frame = InterlockedIncrement(&gTelemetry->frameCount);
        if (frame == 1)
        {
            gTelemetry->firstFrameCounter = now;
            if (gFrameEvent != nullptr)
            {
                SetEvent(gFrameEvent);
            }
        }
        gTelemetry->lastFrameCounter = now;
        BahamutEndTimingWrite(gTelemetry);
        if (frame % kRateLogInterval == 0)
        {
            LARGE_INTEGER frequency{};
            QueryPerformanceFrequency(&frequency);
            const double seconds = static_cast<double>(now.QuadPart - gTelemetry->firstFrameCounter.QuadPart) / static_cast<double>(frequency.QuadPart);
            char         message[160]{};
            std::snprintf(message, sizeof(message), "Bahamut runtime: Present frames=%ld rate=%.2f fps\n", frame, seconds > 0.0 ? static_cast<double>(frame - 1) / seconds : 0.0);
            OutputDebugStringA(message);
        }
    }
    if (gTelemetry != nullptr && !hideOverlayForCapture)
    {
        if (SUCCEEDED(device->BeginScene()))
        {
            DrawOverlay(device, gTelemetry, gAddonHost, gDiscordPluginHost, gPlayerState);
            if (FAILED(device->EndScene()))
            {
                ++gTelemetry->overlayFailureCount;
            }
        }
        else
        {
            ++gTelemetry->overlayFailureCount;
        }
    }
    gNativePluginHost->OnPresentEnd(device);
}

HRESULT STDMETHODCALLTYPE HookedSwapChainPresent(IDirect3DSwapChain9* swapChain,
                                                 const RECT*          sourceRect,
                                                 const RECT*          destinationRect,
                                                 HWND                 destinationWindowOverride,
                                                 const RGNDATA*       dirtyRegion,
                                                 DWORD                flags)
{
    IDirect3DDevice9* device = nullptr;
    if (gInDevicePresent || FAILED(swapChain->GetDevice(&device)) || device == nullptr)
    {
        return gOriginalSwapChainPresent(swapChain, sourceRect, destinationRect, destinationWindowOverride, dirtyRegion, flags);
    }
    if (IsOverlayDevice(device))
    {
        ObservePresent(device);
    }
    device->Release();
    return gOriginalSwapChainPresent(swapChain, sourceRect, destinationRect, destinationWindowOverride, dirtyRegion, flags);
}

HRESULT STDMETHODCALLTYPE HookedDevicePresent(IDirect3DDevice9* device,
                                              const RECT*       sourceRect,
                                              const RECT*       destinationRect,
                                              HWND              destinationWindowOverride,
                                              const RGNDATA*    dirtyRegion)
{
    if (gInDevicePresent || !IsOverlayDevice(device))
    {
        return gOriginalDevicePresent(device, sourceRect, destinationRect, destinationWindowOverride, dirtyRegion);
    }
    ObservePresent(device);
    gInDevicePresent     = true;
    const HRESULT result = gOriginalDevicePresent(device, sourceRect, destinationRect, destinationWindowOverride, dirtyRegion);
    gInDevicePresent     = false;
    return result;
}

HRESULT STDMETHODCALLTYPE HookedSetRenderState(IDirect3DDevice9*  device,
                                               D3DRENDERSTATETYPE state,
                                               DWORD              value)
{
    if (!IsOverlayDevice(device))
    {
        return gOriginalSetRenderState(device, state, value);
    }
    if (state == D3DRS_ZENABLE)
    {
        const HRESULT result = gOriginalSetRenderState(device, state, value);
        if (SUCCEEDED(result))
        {
            InterlockedExchange(&gDepthTestEnabled,
                                IsDepthTestEnabledValue(value) ? 1 : 0);
            if (IsWireframeEnabled())
            {
                ApplyOwnedFillMode(device);
            }
        }
        return result;
    }
    if (state == D3DRS_FILLMODE && IsWireframeEnabled())
    {
        RefreshOwnedDepthTestState(device);
        value = InterlockedCompareExchange(&gDepthTestEnabled, 0, 0) != 0 && !IsOverlayDrawing(device)
                    ? D3DFILL_WIREFRAME
                    : D3DFILL_SOLID;
    }
    return gOriginalSetRenderState(device, state, value);
}

HRESULT STDMETHODCALLTYPE HookedReset(IDirect3DDevice9*      device,
                                      D3DPRESENT_PARAMETERS* parameters)
{
    if (!IsOverlayDevice(device))
    {
        return gOriginalReset(device, parameters);
    }
    PrepareOverlayReset();
    const bool    wireframeEnabled = IsWireframeEnabled();
    const HRESULT result           = gOriginalReset(device, parameters);
    if (SUCCEEDED(result))
    {
        // Keep reset-time ImGui/device setup solid, then restore the user's
        // selected game fill mode after the backend has recreated its state.
        SetWireframeEnabled(false);
        if (gTelemetry != nullptr)
        {
            ++gTelemetry->resetCount;
            if (!CompleteOverlayReset())
            {
                ++gTelemetry->overlayFailureCount;
            }
        }
        SetWireframeEnabled(wireframeEnabled);
        if (gBorderlessWindow != nullptr && !ApplyBorderlessWindow(gBorderlessWindow))
        {
            OutputDebugStringA(
                "Bahamut runtime: could not restore borderless window after reset\n");
        }
    }
    return result;
}

HRESULT STDMETHODCALLTYPE HookedCreateDevice(IDirect3D9* direct3d, UINT adapter, D3DDEVTYPE deviceType, HWND focusWindow, DWORD behaviorFlags, D3DPRESENT_PARAMETERS* parameters, IDirect3DDevice9** device)
{
    const HRESULT result = gOriginalCreateDevice(direct3d, adapter, deviceType, focusWindow, behaviorFlags, parameters, device);
    if (SUCCEEDED(result) && device != nullptr && *device != nullptr && gOriginalReset == nullptr && gOriginalSetRenderState == nullptr && gOriginalDevicePresent == nullptr && gOriginalSwapChainPresent == nullptr)
    {
        void**               methods          = *reinterpret_cast<void***>(*device);
        void*                reset            = methods[kDeviceResetMethodIndex];
        void*                setRenderState   = methods[kDeviceSetRenderStateMethodIndex];
        void*                devicePresent    = methods[kDevicePresentMethodIndex];
        IDirect3DSwapChain9* swapChain        = nullptr;
        const bool           hasSwapChain     = SUCCEEDED((*device)->GetSwapChain(0, &swapChain)) && swapChain != nullptr;
        void*                swapChainPresent = nullptr;
        if (hasSwapChain)
        {
            void** swapChainMethods = *reinterpret_cast<void***>(swapChain);
            swapChainPresent        = swapChainMethods[kSwapChainPresentMethodIndex];
        }
        bool resetCreated            = false;
        bool setRenderStateCreated   = false;
        bool devicePresentCreated    = false;
        bool swapChainPresentCreated = false;
        bool hooksReady              = hasSwapChain && IsD3d9CodeAddress(reset) && IsD3d9CodeAddress(setRenderState) && IsD3d9CodeAddress(swapChainPresent) && reset != setRenderState && reset != swapChainPresent && setRenderState != swapChainPresent;
        // The device Present hook is optional: Windows d3d9 reaches the hooked
        // swap-chain Present anyway, so a foreign slot must not cost the boundary.
        const bool devicePresentUsable = hooksReady && IsD3d9CodeAddress(devicePresent) && devicePresent != reset && devicePresent != setRenderState && devicePresent != swapChainPresent;
        if (hooksReady)
        {
            hooksReady   = MH_CreateHook(reset, reinterpret_cast<void*>(&HookedReset), reinterpret_cast<void**>(&gOriginalReset)) == MH_OK;
            resetCreated = hooksReady;
        }
        if (hooksReady)
        {
            hooksReady            = MH_CreateHook(setRenderState, reinterpret_cast<void*>(&HookedSetRenderState), reinterpret_cast<void**>(&gOriginalSetRenderState)) == MH_OK;
            setRenderStateCreated = hooksReady;
        }
        if (hooksReady && devicePresentUsable)
        {
            devicePresentCreated = MH_CreateHook(devicePresent, reinterpret_cast<void*>(&HookedDevicePresent), reinterpret_cast<void**>(&gOriginalDevicePresent)) == MH_OK;
            if (!devicePresentCreated)
            {
                gOriginalDevicePresent = nullptr;
            }
        }
        if (hooksReady && HasTestFault(L"BAHAMUT_TEST_RENDER_DEVICE_HOOK_FAILURE"))
        {
            hooksReady = false;
        }
        if (hooksReady)
        {
            hooksReady              = MH_CreateHook(swapChainPresent, reinterpret_cast<void*>(&HookedSwapChainPresent), reinterpret_cast<void**>(&gOriginalSwapChainPresent)) == MH_OK;
            swapChainPresentCreated = hooksReady;
        }
        if (hooksReady)
        {
            hooksReady = MH_QueueEnableHook(reset) == MH_OK && MH_QueueEnableHook(setRenderState) == MH_OK && (!devicePresentCreated || MH_QueueEnableHook(devicePresent) == MH_OK) && MH_QueueEnableHook(swapChainPresent) == MH_OK && MH_ApplyQueued() == MH_OK;
        }
        if (hooksReady)
        {
            hooksReady   = BindOverlayDevice(*device);
            gOwnedDevice = *device;
            HWND window  = focusWindow;
            if (parameters != nullptr && parameters->hDeviceWindow != nullptr)
            {
                window = parameters->hDeviceWindow;
            }
            if (hooksReady)
            {
                ConfigureOverlayWindow(window);
                hooksReady = InstallInputBoundary(
                    window, gTelemetry, gAddonHost, gNativePluginHost);
            }
            if (hooksReady)
            {
                hooksReady = ApplyBorderlessWindow(window);
            }
            if (hooksReady)
            {
                EnableOverlay();
                OutputDebugStringA(
                    "Bahamut runtime: D3D9 Present/reset and Win32 input boundaries acquired\n");
            }
        }
        if (swapChain != nullptr)
        {
            swapChain->Release();
        }
        if (!hooksReady)
        {
            RollbackDeviceHooks(reset, setRenderState, devicePresent, swapChainPresent, resetCreated, setRenderStateCreated, devicePresentCreated, swapChainPresentCreated);
            UnbindOverlayDevice(*device);
            ConfigureOverlayWindow(nullptr);
            DisableOverlay();
            if (HasTestFault(L"BAHAMUT_TEST_RENDER_DEVICE_HOOK_FAILURE"))
            {
                const bool resetRemoved         = !resetCreated || MH_RemoveHook(reset) != MH_OK;
                const bool devicePresentRemoved = !devicePresentCreated || MH_RemoveHook(devicePresent) != MH_OK;
                const bool pointersCleared      = gOriginalReset == nullptr && gOriginalSetRenderState == nullptr && gOriginalDevicePresent == nullptr && gOriginalSwapChainPresent == nullptr;
                RecordTestResult(resetRemoved && devicePresentRemoved && pointersCleared
                                     ? "render_device_rollback=ok\n"
                                     : "render_device_rollback=failed\n");
            }
            if (gTelemetry != nullptr)
            {
                ++gTelemetry->overlayFailureCount;
            }
        }
    }
    return result;
}

IDirect3D9* WINAPI HookedDirect3DCreate9(UINT sdkVersion)
{
    IDirect3D9* direct3d = gOriginalDirect3DCreate9(sdkVersion);
    if (direct3d != nullptr && gOriginalCreateDevice == nullptr)
    {
        void** methods      = *reinterpret_cast<void***>(direct3d);
        void*  createDevice = methods[kDirect3dCreateDeviceMethodIndex];
        bool   hookReady    = IsD3d9CodeAddress(createDevice);
        bool   hookCreated  = false;
        if (hookReady)
        {
            hookReady   = MH_CreateHook(createDevice, reinterpret_cast<void*>(&HookedCreateDevice), reinterpret_cast<void**>(&gOriginalCreateDevice)) == MH_OK;
            hookCreated = hookReady;
        }
        if (hookReady)
        {
            hookReady = MH_EnableHook(createDevice) == MH_OK;
        }
        if (!hookReady && hookCreated)
        {
            MH_DisableHook(createDevice);
            MH_RemoveHook(createDevice);
            gOriginalCreateDevice = nullptr;
        }
        if (hookReady)
        {
            OutputDebugStringA("Bahamut runtime: D3D9 CreateDevice boundary acquired\n");
        }
    }
    return direct3d;
}

} // namespace

bool IsWireframeEnabled()
{
    return InterlockedCompareExchange(&gWireframeEnabled, 0, 0) != 0;
}

void SetWireframeEnabled(bool enabled)
{
    const LONG value    = enabled ? 1 : 0;
    const LONG previous = InterlockedExchange(&gWireframeEnabled, value);
    if (previous != value)
    {
        OutputDebugStringA(enabled
                               ? "Bahamut runtime: fill mode=Wireframe\n"
                               : "Bahamut runtime: fill mode=Solid\n");
    }
    if (gOwnedDevice != nullptr && gOriginalSetRenderState != nullptr)
    {
        const bool depthEnabled = RefreshOwnedDepthTestState(gOwnedDevice);
        const bool wireframe    = enabled && depthEnabled && !IsOverlayDrawing(gOwnedDevice);
        gOriginalSetRenderState(gOwnedDevice, D3DRS_FILLMODE, wireframe ? D3DFILL_WIREFRAME : D3DFILL_SOLID);
    }
}

bool InstallRenderBoundary(BahamutRuntimeTelemetry*               telemetry,
                           BahamutRuntimeBootstrapDiagnostics*    diagnostics,
                           HANDLE                                 frameEvent,
                           AddonHost*                             addonHost,
                           NativePluginHost*                      nativePluginHost,
                           NativePluginHost*                      discordPluginHost,
                           bahamut_client::PlayerStateService*    playerState,
                           packet_observer::PacketObserver*       packetObserver,
                           bahamut_client::ObjectDistanceService* objectDistance,
                           bool                                   objectDistanceEnabled,
                           bahamut_client::CameraZoomService*     cameraZoom,
                           bool                                   cameraZoomEnabled,
                           bool                                   borderless)
{
    HMODULE direct3d = LoadLibraryW(L"d3d9.dll");
    if (direct3d == nullptr)
    {
        telemetry->resetCount = kD3d9LoadFailureStage;
        return false;
    }
    void* create9 = reinterpret_cast<void*>(GetProcAddress(direct3d, "Direct3DCreate9"));
    if (create9 == nullptr)
    {
        telemetry->resetCount = kDirect3dCreate9ExportFailureStage;
        return false;
    }
    if (HasTestFault(L"BAHAMUT_RUNTIME_TEST_STUB") && HasTestFault(L"BAHAMUT_TEST_RENDER_BOUNDARY_OWNER_FAILURE"))
    {
        create9 = reinterpret_cast<void*>(&HookedDirect3DCreate9);
    }
    LONG addressFailureStage = 0;
    if (!IsD3d9CodeAddress(create9, &addressFailureStage, diagnostics))
    {
        telemetry->resetCount = addressFailureStage;
        return false;
    }

    gTelemetry           = telemetry;
    gFrameEvent          = frameEvent;
    gAddonHost           = addonHost;
    gNativePluginHost    = nativePluginHost;
    gDiscordPluginHost   = discordPluginHost;
    gPlayerState         = playerState;
    gBorderlessEnabled   = borderless;
    gBorderlessMonitorId = borderless ? ReadBorderlessMonitorId().value_or(L"") : L"";
    if (HasTestFault(L"BAHAMUT_TEST_TIMING_SEQUENCE_ODD"))
    {
        telemetry->timingSequence = 1;
    }
    if (MH_Initialize() != MH_OK)
    {
        telemetry->resetCount = kMinHookInitializeFailureStage;
        return false;
    }
    if (playerState != nullptr && !playerState->InstallHook())
    {
        MH_Uninitialize();
        telemetry->resetCount = kMinHookCreateFailureStage;
        return false;
    }
    const bool playerStateHookInstalled = playerState != nullptr && playerState->IsInstalled();
    const bool packetObserverRequired   = !HasTestFault(L"BAHAMUT_RUNTIME_TEST_STUB") && addonHost != nullptr &&
                                          (addonHost->IsLoaded("distance") || addonHost->IsLoaded("targethp") ||
                                           addonHost->IsLoaded("zonename") || addonHost->IsLoaded("packetlogger") ||
                                           addonHost->IsLoaded("combatparser"));
    if (packetObserverRequired && (packetObserver == nullptr || !packetObserver->InstallHook()))
    {
        const bool packetRemoved = packetObserver == nullptr || packetObserver->Shutdown();
        if (playerStateHookInstalled)
        {
            RollbackPlayerStateHook(playerState);
        }
        if (packetRemoved)
        {
            MH_Uninitialize();
        }
        telemetry->resetCount = kMinHookCreateFailureStage;
        return false;
    }
    const bool packetObserverHookInstalled = packetObserverRequired;
    if (objectDistanceEnabled && (objectDistance == nullptr || !objectDistance->InstallHook()))
    {
        const bool objectRemoved = objectDistance == nullptr || objectDistance->Shutdown();
        const bool packetRemoved = !packetObserverHookInstalled || packetObserver->Shutdown();
        if (playerStateHookInstalled)
        {
            RollbackPlayerStateHook(playerState);
        }
        if (objectRemoved && packetRemoved)
        {
            MH_Uninitialize();
        }
        telemetry->resetCount = kMinHookCreateFailureStage;
        return false;
    }
    const bool objectDistanceHookInstalled = objectDistanceEnabled && objectDistance != nullptr && objectDistance->IsInstalled();
    if (cameraZoomEnabled && (cameraZoom == nullptr || !cameraZoom->InstallHook()))
    {
        const bool cameraRemoved = cameraZoom == nullptr || cameraZoom->Shutdown();
        const bool objectRemoved = !objectDistanceHookInstalled || objectDistance->Shutdown();
        const bool packetRemoved = !packetObserverHookInstalled || packetObserver->Shutdown();
        if (playerStateHookInstalled)
        {
            RollbackPlayerStateHook(playerState);
        }
        if (cameraRemoved && objectRemoved && packetRemoved)
        {
            MH_Uninitialize();
        }
        telemetry->resetCount = kMinHookCreateFailureStage;
        return false;
    }
    const bool cameraZoomHookInstalled = cameraZoomEnabled && cameraZoom != nullptr && cameraZoom->IsInstalled();
    const auto rollbackGameplayHooks   = [&]()
    {
        const bool cameraRemoved = !cameraZoomHookInstalled || cameraZoom->Shutdown();
        const bool objectRemoved = !objectDistanceHookInstalled || objectDistance->Shutdown();
        const bool packetRemoved = !packetObserverHookInstalled || packetObserver->Shutdown();
        return cameraRemoved && objectRemoved && packetRemoved;
    };
    const bool                         commandBoundaryRequired      = !HasTestFault(L"BAHAMUT_RUNTIME_TEST_STUB") && addonHost != nullptr && addonHost->HasCommandAddons();
    const bool                         chatBoundaryRequired         = !HasTestFault(L"BAHAMUT_RUNTIME_TEST_STUB") && addonHost != nullptr && (commandBoundaryRequired || addonHost->IsLoaded("chatlogs"));
    const CommandBoundaryInstallResult commandBoundaryResult        = commandBoundaryRequired
                                                                          ? InstallCommandBoundary(addonHost)
                                                                          : CommandBoundaryInstallResult::Disabled;
    const bool                         commandBoundaryHookInstalled = commandBoundaryResult == CommandBoundaryInstallResult::Installed;
    if (commandBoundaryRequired && !commandBoundaryHookInstalled)
    {
        const bool gameplayHooksRemoved = rollbackGameplayHooks();
        if (playerStateHookInstalled)
        {
            RollbackPlayerStateHook(playerState);
        }
        if (gameplayHooksRemoved && commandBoundaryResult != CommandBoundaryInstallResult::RollbackFailed)
        {
            MH_Uninitialize();
        }
        telemetry->resetCount = kMinHookCreateFailureStage;
        return false;
    }
    const ChatBoundaryInstallResult chatBoundaryResult        = chatBoundaryRequired
                                                                    ? InstallChatBoundary({ addonHost, &QueueAddonChat })
                                                                    : ChatBoundaryInstallResult::Disabled;
    const bool                      chatBoundaryHookInstalled = chatBoundaryResult == ChatBoundaryInstallResult::Installed;
    if (chatBoundaryRequired && !chatBoundaryHookInstalled)
    {
        const bool gameplayHooksRemoved   = rollbackGameplayHooks();
        const bool commandBoundaryRemoved = !commandBoundaryHookInstalled || ShutdownCommandBoundary();
        if (playerStateHookInstalled)
        {
            RollbackPlayerStateHook(playerState);
        }
        if (gameplayHooksRemoved && commandBoundaryRemoved && chatBoundaryResult != ChatBoundaryInstallResult::RollbackFailed)
        {
            MH_Uninitialize();
        }
        telemetry->resetCount = kMinHookCreateFailureStage;
        return false;
    }
    if (MH_CreateHook(create9, reinterpret_cast<void*>(&HookedDirect3DCreate9), reinterpret_cast<void**>(&gOriginalDirect3DCreate9)) != MH_OK)
    {
        const bool gameplayHooksRemoved   = rollbackGameplayHooks();
        gOriginalDirect3DCreate9          = nullptr;
        const bool commandBoundaryRemoved = !commandBoundaryHookInstalled || ShutdownCommandBoundary();
        const bool chatBoundaryRemoved    = !chatBoundaryHookInstalled || ShutdownChatBoundary();
        if (playerStateHookInstalled)
        {
            RollbackPlayerStateHook(playerState);
        }
        if (gameplayHooksRemoved && commandBoundaryRemoved && chatBoundaryRemoved)
        {
            MH_Uninitialize();
        }
        telemetry->resetCount = kMinHookCreateFailureStage;
        return false;
    }
    if (HasTestFault(L"BAHAMUT_TEST_RENDER_BOUNDARY_FAILURE"))
    {
        const bool gameplayHooksRemoved   = rollbackGameplayHooks();
        const bool removed                = RollbackCreate9Hook(create9);
        const bool commandBoundaryRemoved = !commandBoundaryHookInstalled || ShutdownCommandBoundary();
        const bool chatBoundaryRemoved    = !chatBoundaryHookInstalled || ShutdownChatBoundary();
        if (playerStateHookInstalled)
        {
            RollbackPlayerStateHook(playerState);
        }
        RecordTestResult(removed && !IsOverlayVisible() && gameplayHooksRemoved && commandBoundaryRemoved && chatBoundaryRemoved
                             ? "render_boundary_rollback=ok\nvisible=0\n"
                             : "render_boundary_rollback=failed\nvisible=1\n");
        if (gameplayHooksRemoved && commandBoundaryRemoved && chatBoundaryRemoved)
        {
            MH_Uninitialize();
        }
        telemetry->resetCount = kMinHookCreateFailureStage;
        return false;
    }
    const bool enabled = MH_EnableHook(create9) == MH_OK;
    if (!enabled)
    {
        const bool gameplayHooksRemoved = rollbackGameplayHooks();
        (void)RollbackCreate9Hook(create9);
        const bool commandBoundaryRemoved = !commandBoundaryHookInstalled || ShutdownCommandBoundary();
        const bool chatBoundaryRemoved    = !chatBoundaryHookInstalled || ShutdownChatBoundary();
        if (playerStateHookInstalled)
        {
            RollbackPlayerStateHook(playerState);
        }
        if (gameplayHooksRemoved && commandBoundaryRemoved && chatBoundaryRemoved)
        {
            MH_Uninitialize();
        }
        telemetry->resetCount = kMinHookEnableFailureStage;
    }
    return enabled;
}
