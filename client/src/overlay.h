#pragma once

#include "runtime_api.h"

#include <d3d9.h>
#include <filesystem>

class AddonHost;
class NativePluginHost;

namespace bahamut_client
{

class PlayerStateService;

}

void ConfigureOverlayWindow(HWND window);
void ConfigureOverlayLayout(const std::filesystem::path& path);
bool BindOverlayDevice(IDirect3DDevice9* device);
void UnbindOverlayDevice(IDirect3DDevice9* device);
bool IsOverlayDevice(IDirect3DDevice9* device);
bool IsOverlayDrawing(IDirect3DDevice9* device);
void DrawOverlay(IDirect3DDevice9*                   device,
                 BahamutRuntimeTelemetry*            telemetry,
                 AddonHost*                          addonHost,
                 NativePluginHost*                   discordPluginHost,
                 bahamut_client::PlayerStateService* playerState);
void PrepareOverlayReset();
bool CompleteOverlayReset();
