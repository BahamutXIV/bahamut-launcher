#pragma once

#include "native_plugin_api.h"

#include <d3d9.h>

#include <filesystem>
#include <string>

struct NativePluginConfiguration
{
    std::filesystem::path storagePath;
    std::uint32_t         format       = BahamutNativePluginFormatPng;
    UINT                  hotkey       = 0;
    bool                  enabled      = false;
    bool                  hideOverlays = true;
};

class NativePluginHost
{
public:
    bool Load(const std::filesystem::path& path, const char* expectedId, const NativePluginConfiguration& configuration);
    bool Configure(const NativePluginConfiguration& configuration);
    bool SetEnabled(bool enabled);
    bool OnWindowMessage(HWND window, UINT message, WPARAM wParam, LPARAM lParam, bool& consumed);
    bool OnPresentBegin(IDirect3DDevice9* device, bool& suppressOverlay);
    bool OnPresentEnd(IDirect3DDevice9* device);
    bool PublishPlayerState(const BahamutNativePluginPlayerStateV1& state);
    void Shutdown();

    bool               IsLoaded() const;
    const std::string& LastError() const;

private:
    bool                        Fault(const char* message);
    BahamutNativePluginConfigV1 BorrowedConfiguration() const;

    HMODULE                   module_ = nullptr;
    BahamutNativePluginApiV3  api_{};
    void*                     instance_ = nullptr;
    NativePluginConfiguration configuration_;
    std::string               error_;
    HWND                      inputWindow_ = nullptr;
    bool                      faulted_     = false;
};
