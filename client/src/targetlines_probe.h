#pragma once

#include "targetlines_arc.h"

#include <d3d9.h>

#include <array>
#include <cstdint>
#include <optional>

namespace bahamut_client
{

class TargetDistanceService;
class TargetlinesService;

// ffxivgame.exe identity: runtime_contract.h. Capstone 5.0.7 / pefile 2024.8.26
// observed CameraActor::update at VA 0x006195D0 with ECX this, one int32_t*
// stack argument and ret 4. Its builder wrote separate 64-byte records at
// camera +0x210/+0x250/+0x2D0, and update wrote +0x370 through +0x3A0.
// The guarded +0x210/+0x250 scene pass in targetlines_projection.h was aligned
// with world-XYZ endpoints in owner captures. Other passes remain hidden.
// Camera and drawing data stay native; the addon requests only its own arcs.
bool TargetlinesProbeRequested();
void StartTargetlinesRenderer(TargetDistanceService* targets, TargetlinesService* arcs, bool addonEnabled);
void SetTargetlinesRendererActive(bool enabled);
void BindTargetlinesRendererDevice(IDirect3DDevice9* device, void* drawIndexedPrimitive);
void PresentTargetlinesRenderer();
void ResetTargetlinesRenderer();

struct TargetlinesRenderArc
{
    TargetlineArcSegments pieces;
    bool                  friendly = false;
    float                 opacity  = 1.0F;
};

struct TargetlinesRenderFrame
{
    std::array<TargetlinesRenderArc, 2> arcs{};
    std::size_t                         count            = 0;
    std::uint32_t                       backBufferWidth  = 0;
    std::uint32_t                       backBufferHeight = 0;
};

[[nodiscard]] std::optional<TargetlinesRenderFrame> TargetlinesFrame();

} // namespace bahamut_client
