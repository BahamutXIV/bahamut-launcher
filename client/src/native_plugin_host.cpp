#include "native_plugin_host.h"
#include "fault_guard.h"

namespace
{

bool AddressBelongsToModule(const void* address, HMODULE expected, bool executable)
{
    if (address == nullptr)
        return false;
    MEMORY_BASIC_INFORMATION memory{};
    if (VirtualQuery(address, &memory, sizeof(memory)) == 0 || memory.State != MEM_COMMIT)
    {
        return false;
    }
    if (executable)
    {
        const DWORD protection = memory.Protect & 0xff;
        if (protection != PAGE_EXECUTE && protection != PAGE_EXECUTE_READ && protection != PAGE_EXECUTE_READWRITE && protection != PAGE_EXECUTE_WRITECOPY)
        {
            return false;
        }
    }
    HMODULE owner = nullptr;
    return GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                              reinterpret_cast<LPCWSTR>(address),
                              &owner) &&
           owner == expected;
}

bool PluginIdMatches(const char* actual, const char* expected)
{
    if (actual == nullptr || expected == nullptr)
        return false;
    BAHAMUT_FAULT_TRY
    {
        constexpr std::size_t kMaximumPluginIdLength = 64;
        for (std::size_t index = 0; index < kMaximumPluginIdLength; ++index)
        {
            if (actual[index] != expected[index])
                return false;
            if (actual[index] == '\0')
                return true;
        }
        return false;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool QueryPlugin(BahamutNativePluginGetApiFunction query,
                 const BahamutNativePluginHostV3*  host,
                 BahamutNativePluginApiV3*         api)
{
    BAHAMUT_FAULT_TRY
    {
        return query(kBahamutNativePluginAbiVersion, host, api) != FALSE;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

void* CreatePlugin(BahamutNativePluginCreateV1        callback,
                   const BahamutNativePluginConfigV1* config)
{
    BAHAMUT_FAULT_TRY
    {
        return callback(config);
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return nullptr;
    }
}

bool ConfigurePlugin(BahamutNativePluginConfigureV1 callback, void* instance, const BahamutNativePluginConfigV1* config)
{
    BAHAMUT_FAULT_TRY
    {
        return callback(instance, config) != FALSE;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool EnablePlugin(BahamutNativePluginSetEnabledV1 callback, void* instance, bool enabled)
{
    BAHAMUT_FAULT_TRY
    {
        return callback(instance, enabled ? TRUE : FALSE) != FALSE;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool InputPlugin(BahamutNativePluginInputCallbackV1 callback, void* instance, const BahamutNativePluginInputV1* input, std::uint32_t* flags)
{
    BAHAMUT_FAULT_TRY
    {
        return callback(instance, input, flags) != FALSE;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool DevicePlugin(BahamutNativePluginDeviceCallbackV1 callback, void* instance, IDirect3DDevice9* device, std::uint32_t* flags)
{
    BAHAMUT_FAULT_TRY
    {
        return callback(instance, device, flags) != FALSE;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool PlayerStatePlugin(BahamutNativePluginPlayerStateCallbackV1 callback,
                       void*                                    instance,
                       const BahamutNativePluginPlayerStateV1*  state)
{
    BAHAMUT_FAULT_TRY
    {
        return callback(instance, state) != FALSE;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool InvokePluginShutdown(BahamutNativePluginShutdownV1 shutdown, void* instance)
{
    BAHAMUT_FAULT_TRY
    {
        shutdown(instance);
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool InvokePluginDestroy(BahamutNativePluginDestroyV1 destroy, void* instance)
{
    BAHAMUT_FAULT_TRY
    {
        destroy(instance);
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

} // namespace

bool NativePluginHost::Load(const std::filesystem::path&     path,
                            const char*                      expectedId,
                            const NativePluginConfiguration& configuration)
{
    if (IsLoaded())
        return Configure(configuration);
    if (path.empty() || !path.is_absolute() || expectedId == nullptr)
    {
        return Fault("plugin path or expected id is invalid");
    }
    const DWORD attributes = GetFileAttributesW(path.c_str());
    if (attributes == INVALID_FILE_ATTRIBUTES || (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0)
    {
        return Fault("plugin file is missing");
    }
    HMODULE module = LoadLibraryW(path.c_str());
    if (module == nullptr)
    {
        return Fault("plugin DLL could not be loaded");
    }
    auto query = reinterpret_cast<BahamutNativePluginGetApiFunction>(
        GetProcAddress(module, kBahamutNativePluginGetApiExport));
    if (!AddressBelongsToModule(reinterpret_cast<const void*>(query), module, true))
    {
        FreeLibrary(module);
        return Fault("plugin query export is missing or invalid");
    }
    const BahamutNativePluginHostV3 host{
        kBahamutNativePluginAbiVersion,
        sizeof(BahamutNativePluginHostV3),
    };
    BahamutNativePluginApiV3 api{};
    api.structSize = sizeof(api);
    if (!QueryPlugin(query, &host, &api) || api.abiVersion != kBahamutNativePluginAbiVersion || api.structSize < sizeof(BahamutNativePluginApiV3) || !AddressBelongsToModule(api.id, module, false) || !PluginIdMatches(api.id, expectedId))
    {
        FreeLibrary(module);
        return Fault("plugin ABI or identity is incompatible");
    }
    const void* callbacks[] = {
        reinterpret_cast<const void*>(api.create),
        reinterpret_cast<const void*>(api.configure),
        reinterpret_cast<const void*>(api.setEnabled),
        reinterpret_cast<const void*>(api.onInput),
        reinterpret_cast<const void*>(api.onPresentBegin),
        reinterpret_cast<const void*>(api.onPresentEnd),
        reinterpret_cast<const void*>(api.onPlayerState),
        reinterpret_cast<const void*>(api.shutdown),
        reinterpret_cast<const void*>(api.destroy),
    };
    for (const void* callback : callbacks)
    {
        if (!AddressBelongsToModule(callback, module, true))
        {
            FreeLibrary(module);
            return Fault("plugin callback table is invalid");
        }
    }
    module_                                    = module;
    api_                                       = api;
    configuration_                             = configuration;
    const BahamutNativePluginConfigV1 borrowed = BorrowedConfiguration();
    instance_                                  = CreatePlugin(api_.create, &borrowed);
    if (instance_ == nullptr)
    {
        return Fault("plugin instance could not be created");
    }
    error_.clear();
    faulted_ = false;
    return true;
}

bool NativePluginHost::Configure(
    const NativePluginConfiguration& configuration)
{
    configuration_ = configuration;
    if (instance_ == nullptr)
        return true;
    const BahamutNativePluginConfigV1 borrowed = BorrowedConfiguration();
    return ConfigurePlugin(api_.configure, instance_, &borrowed) || Fault("plugin configure callback failed");
}

bool NativePluginHost::SetEnabled(bool enabled)
{
    configuration_.enabled = enabled;
    if (instance_ == nullptr)
        return !enabled;
    return EnablePlugin(api_.setEnabled, instance_, enabled) || Fault("plugin enable callback failed");
}

bool NativePluginHost::OnWindowMessage(HWND window, UINT message, WPARAM wParam, LPARAM lParam, bool& consumed)
{
    consumed = false;
    if (!IsLoaded())
        return true;
    inputWindow_ = window;
    const BahamutNativePluginInputV1 input{
        sizeof(BahamutNativePluginInputV1), window, message, wParam, lParam
    };
    std::uint32_t flags = BahamutNativePluginEventNone;
    if (!InputPlugin(api_.onInput, instance_, &input, &flags))
    {
        return Fault("plugin input callback failed");
    }
    if (message == WM_NCDESTROY)
    {
        inputWindow_ = nullptr;
    }
    consumed = (flags & BahamutNativePluginEventConsumed) != 0;
    return true;
}

bool NativePluginHost::OnPresentBegin(IDirect3DDevice9* device,
                                      bool&             suppressOverlay)
{
    suppressOverlay = false;
    if (!IsLoaded())
        return true;
    std::uint32_t flags = BahamutNativePluginEventNone;
    if (!DevicePlugin(api_.onPresentBegin, instance_, device, &flags))
    {
        return Fault("plugin Present-begin callback failed");
    }
    suppressOverlay = (flags & BahamutNativePluginEventSuppressOverlay) != 0;
    return true;
}

bool NativePluginHost::OnPresentEnd(IDirect3DDevice9* device)
{
    if (!IsLoaded())
        return true;
    std::uint32_t flags = BahamutNativePluginEventNone;
    return DevicePlugin(api_.onPresentEnd, instance_, device, &flags) || Fault("plugin Present-end callback failed");
}

bool NativePluginHost::PublishPlayerState(
    const BahamutNativePluginPlayerStateV1& state)
{
    if (!IsLoaded() || api_.onPlayerState == nullptr)
    {
        return true;
    }
    return PlayerStatePlugin(api_.onPlayerState, instance_, &state) ||
           Fault("plugin player-state callback failed");
}

void NativePluginHost::Shutdown()
{
    if (inputWindow_ != nullptr)
    {
        UnregisterHotKey(inputWindow_, kBahamutScreenshotHotkeyId);
        inputWindow_ = nullptr;
    }
    void* instance = instance_;
    bool  quiesced = true;
    if (instance != nullptr)
    {
        quiesced = InvokePluginShutdown(api_.shutdown, instance);
        if (quiesced)
        {
            quiesced = InvokePluginDestroy(api_.destroy, instance);
        }
        if (!quiesced)
        {
            Fault("plugin shutdown callback failed");
        }
    }
    if (!quiesced)
    {
        return;
    }
    instance_              = nullptr;
    configuration_.enabled = false;
    api_                   = {};
    if (module_ != nullptr)
    {
        FreeLibrary(module_);
        module_ = nullptr;
    }
}

bool NativePluginHost::IsLoaded() const
{
    return module_ != nullptr && instance_ != nullptr && !faulted_;
}

const std::string& NativePluginHost::LastError() const
{
    return error_;
}

bool NativePluginHost::Fault(const char* message)
{
    error_                 = message == nullptr ? "native plugin failed" : message;
    configuration_.enabled = false;
    if (instance_ != nullptr && api_.setEnabled != nullptr)
    {
        (void)EnablePlugin(api_.setEnabled, instance_, false);
    }
    if (inputWindow_ != nullptr)
    {
        UnregisterHotKey(inputWindow_, kBahamutScreenshotHotkeyId);
        inputWindow_ = nullptr;
    }
    faulted_ = true;
    OutputDebugStringA("Bahamut runtime: ");
    OutputDebugStringA(error_.c_str());
    OutputDebugStringA("\n");
    return false;
}

BahamutNativePluginConfigV1 NativePluginHost::BorrowedConfiguration() const
{
    return {
        sizeof(BahamutNativePluginConfigV1),
        configuration_.storagePath.c_str(),
        configuration_.format,
        configuration_.hotkey,
        configuration_.enabled ? TRUE : FALSE,
        configuration_.hideOverlays ? TRUE : FALSE,
    };
}
