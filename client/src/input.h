#pragma once

#include "native_plugin_api.h"
#include "runtime_api.h"

inline constexpr int kScreenshotHotkeyId = kBahamutScreenshotHotkeyId;

class NativePluginHost;
class AddonHost;

void ConfigureBuiltInHotkeys(bool fillMode, bool fps);
bool InstallInputBoundary(HWND window, BahamutRuntimeTelemetry* telemetry, AddonHost* addonHost, NativePluginHost* nativePluginHost);
void EnableOverlay();
void DisableOverlay();
bool IsOverlayVisible();
// Published by the overlay after each frame: whether an open menu window,
// such as Gear Sets, is under the pointer or has an active text field.
void SetOverlayMenuInput(bool mouse, bool keyboard);
void UpdateControllerObservation();
// Replays the window's activation to native plugins on the window thread,
// which RegisterHotKey requires. Safe from any thread; a no-op before install.
void RefreshNativePluginActivation();
