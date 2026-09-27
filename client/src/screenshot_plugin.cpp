#include "native_plugin_api.h"
#include "screenshot.h"

#include <array>
#include <new>

namespace
{

struct ScreenshotPlugin
{
    HWND window                = nullptr;
    UINT hotkey                = 0;
    bool enabled               = false;
    bool active                = false;
    bool keyDown               = false;
    bool printScreenRegistered = false;
};

bool HasTestFault(const wchar_t* name)
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(name, value, ARRAYSIZE(value));
    return count != 0 && count < ARRAYSIZE(value) && value[0] != L'0';
}

void RecordInputTestResult(const char* text)
{
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(L"BAHAMUT_TEST_INPUT_RESULT_FILE",
                                                path,
                                                ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
        return;
    HANDLE file = CreateFileW(path, GENERIC_WRITE, FILE_SHARE_READ, nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
        return;
    DWORD written = 0;
    WriteFile(file, text, static_cast<DWORD>(lstrlenA(text)), &written, nullptr);
    CloseHandle(file);
}

void ReleasePrintScreen(ScreenshotPlugin& plugin)
{
    if (plugin.window != nullptr && plugin.printScreenRegistered)
    {
        UnregisterHotKey(plugin.window, kBahamutScreenshotHotkeyId);
    }
    plugin.printScreenRegistered = false;
}

void RefreshPrintScreen(ScreenshotPlugin& plugin)
{
    ReleasePrintScreen(plugin);
    if (!plugin.enabled || !plugin.active || plugin.window == nullptr || plugin.hotkey != VK_SNAPSHOT)
    {
        return;
    }
    plugin.printScreenRegistered = RegisterHotKey(plugin.window,
                                                  kBahamutScreenshotHotkeyId,
                                                  MOD_NOREPEAT,
                                                  VK_SNAPSHOT) != FALSE;
    if (plugin.printScreenRegistered)
    {
        RecordInputTestResult("print_screen_registration=ok\n");
    }
}

bool ApplyConfiguration(ScreenshotPlugin&                  plugin,
                        const BahamutNativePluginConfigV1& config)
{
    if (config.structSize < sizeof(BahamutNativePluginConfigV1) || config.storagePath == nullptr || (config.format != BahamutNativePluginFormatPng && config.format != BahamutNativePluginFormatBmp))
    {
        return false;
    }
    plugin.hotkey  = config.hotkey;
    plugin.enabled = config.enabled != FALSE;
    plugin.keyDown = false;
    ConfigureScreenshotCapture(config.storagePath,
                               config.format == BahamutNativePluginFormatBmp
                                   ? ScreenshotFormat::Bmp
                                   : ScreenshotFormat::Png,
                               config.hideOverlays != FALSE);
    RefreshPrintScreen(plugin);
    return true;
}

bool IsKeyDownMessage(UINT message)
{
    return message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
}

bool IsKeyUpMessage(UINT message)
{
    return message == WM_KEYUP || message == WM_SYSKEYUP;
}

void* WINAPI CreateScreenshot(const BahamutNativePluginConfigV1* config)
{
    if (config == nullptr)
        return nullptr;
    auto* plugin = new (std::nothrow) ScreenshotPlugin();
    if (plugin == nullptr || !ApplyConfiguration(*plugin, *config))
    {
        delete plugin;
        return nullptr;
    }
    return plugin;
}

BOOL WINAPI ConfigureScreenshot(void*                              instance,
                                const BahamutNativePluginConfigV1* config)
{
    if (instance == nullptr || config == nullptr)
        return FALSE;
    return ApplyConfiguration(*static_cast<ScreenshotPlugin*>(instance), *config)
               ? TRUE
               : FALSE;
}

BOOL WINAPI EnableScreenshot(void* instance, BOOL enabled)
{
    if (instance == nullptr)
        return FALSE;
    auto& plugin   = *static_cast<ScreenshotPlugin*>(instance);
    plugin.enabled = enabled != FALSE;
    plugin.keyDown = false;
    if (!plugin.enabled)
    {
        CancelScreenshotCapture();
    }
    RefreshPrintScreen(plugin);
    return TRUE;
}

BOOL WINAPI ScreenshotInput(void*                             instance,
                            const BahamutNativePluginInputV1* input,
                            std::uint32_t*                    flags)
{
    if (instance == nullptr || input == nullptr || flags == nullptr || input->structSize < sizeof(BahamutNativePluginInputV1))
    {
        return FALSE;
    }
    auto& plugin = *static_cast<ScreenshotPlugin*>(instance);
    *flags       = BahamutNativePluginEventNone;
    if (input->message == WM_ACTIVATE)
    {
        plugin.window = input->window;
        plugin.active = LOWORD(input->wParam) != WA_INACTIVE;
        RefreshPrintScreen(plugin);
        return TRUE;
    }
    if (input->message == WM_NCDESTROY)
    {
        ReleasePrintScreen(plugin);
        plugin.window = nullptr;
        plugin.active = false;
        return TRUE;
    }
    if (input->message == WM_KILLFOCUS || input->message == WM_CANCELMODE)
    {
        plugin.keyDown = false;
    }
    if (!plugin.enabled)
        return TRUE;
    if (input->message == WM_HOTKEY && input->wParam == kBahamutScreenshotHotkeyId && plugin.printScreenRegistered)
    {
        if (RequestScreenshotCapture())
        {
            *flags = BahamutNativePluginEventConsumed;
        }
        return TRUE;
    }
    const bool keyMessage = IsKeyDownMessage(input->message) || IsKeyUpMessage(input->message);
    if (!keyMessage || input->wParam != plugin.hotkey || plugin.hotkey == 0)
    {
        return TRUE;
    }
    if (plugin.hotkey == VK_SNAPSHOT && plugin.printScreenRegistered)
    {
        *flags = BahamutNativePluginEventConsumed;
        return TRUE;
    }
    bool handled = false;
    if (IsKeyDownMessage(input->message) && (input->lParam & (1LL << 30)) == 0)
    {
        if (!plugin.keyDown)
        {
            plugin.keyDown = true;
            handled        = RequestScreenshotCapture();
        }
    }
    else if (IsKeyUpMessage(input->message))
    {
        const bool wasDown = plugin.keyDown;
        plugin.keyDown     = false;
        handled            = wasDown ? IsScreenshotCaptureConfigured()
                                     : RequestScreenshotCapture();
    }
    if (handled)
        *flags = BahamutNativePluginEventConsumed;
    return TRUE;
}

BOOL WINAPI ScreenshotPresentBegin(void* instance, void* device, std::uint32_t* flags)
{
    UNREFERENCED_PARAMETER(device);
    if (instance == nullptr || flags == nullptr)
        return FALSE;
    if (HasTestFault(L"BAHAMUT_TEST_SCREENSHOT_PLUGIN_CALLBACK_FAULT"))
    {
        RaiseException(EXCEPTION_ACCESS_VIOLATION, 0, 0, nullptr);
    }
    const auto& plugin = *static_cast<ScreenshotPlugin*>(instance);
    *flags             = plugin.enabled && PendingScreenshotHidesOverlays()
                             ? BahamutNativePluginEventSuppressOverlay
                             : BahamutNativePluginEventNone;
    return TRUE;
}

BOOL WINAPI ScreenshotPresentEnd(void* instance, void* device, std::uint32_t* flags)
{
    if (instance == nullptr || flags == nullptr)
        return FALSE;
    *flags             = BahamutNativePluginEventNone;
    const auto& plugin = *static_cast<ScreenshotPlugin*>(instance);
    if (plugin.enabled)
    {
        CapturePendingScreenshot(static_cast<IDirect3DDevice9*>(device));
    }
    return TRUE;
}

void WINAPI ShutdownScreenshot(void* instance)
{
    if (instance == nullptr)
        return;
    auto& plugin = *static_cast<ScreenshotPlugin*>(instance);
    ReleasePrintScreen(plugin);
    CancelScreenshotCapture();
}

void WINAPI DestroyScreenshot(void* instance)
{
    delete static_cast<ScreenshotPlugin*>(instance);
}

BOOL WINAPI IgnorePlayerState(void*, const BahamutNativePluginPlayerStateV1*)
{
    return TRUE;
}

} // namespace

extern "C" BOOL WINAPI BahamutNativePluginGetApi(
    std::uint32_t requestedVersion, const BahamutNativePluginHostV3* host, BahamutNativePluginApiV3* api)
{
    if (requestedVersion != kBahamutNativePluginAbiVersion || host == nullptr || api == nullptr || api->structSize < sizeof(BahamutNativePluginApiV3) || host->abiVersion != kBahamutNativePluginAbiVersion || host->structSize < sizeof(BahamutNativePluginHostV3))
    {
        return FALSE;
    }
    static const std::array<char, 64> invalidId = []
    {
        std::array<char, 64> value{};
        value.fill('x');
        return value;
    }();
    *api = {
        HasTestFault(L"BAHAMUT_TEST_SCREENSHOT_PLUGIN_ABI_MISMATCH")
            ? kBahamutNativePluginAbiVersion + 1
            : kBahamutNativePluginAbiVersion,
        sizeof(BahamutNativePluginApiV3),
        HasTestFault(L"BAHAMUT_TEST_SCREENSHOT_PLUGIN_ID_UNTERMINATED")
            ? invalidId.data()
            : "screenshot",
        &CreateScreenshot,
        &ConfigureScreenshot,
        &EnableScreenshot,
        &ScreenshotInput,
        &ScreenshotPresentBegin,
        &ScreenshotPresentEnd,
        &IgnorePlayerState,
        &ShutdownScreenshot,
        &DestroyScreenshot,
    };
    return TRUE;
}

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID reserved)
{
    UNREFERENCED_PARAMETER(reserved);
    if (reason == DLL_PROCESS_ATTACH)
    {
        DisableThreadLibraryCalls(instance);
    }
    return TRUE;
}
