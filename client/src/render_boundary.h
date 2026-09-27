#pragma once

#include "runtime_api.h"

class AddonHost;
class NativePluginHost;

namespace packet_observer
{

class PacketObserver;

}

bool IsWireframeEnabled();
void SetWireframeEnabled(bool enabled);

namespace bahamut_client
{

class PlayerStateService;
class ObjectDistanceService;
class CameraZoomService;

} // namespace bahamut_client

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
                           bool                                   borderless);
