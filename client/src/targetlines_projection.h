#pragma once

#include "world_projection.h"

#include <array>
#include <cstdint>
#include <optional>
#include <span>

namespace bahamut_client
{

using TargetlinesShaderDigest = std::array<std::uint8_t, 32>;

// Build-specific scene pass gate for Targetlines.
// The matrix is accepted only when all captured register relationships hold:
// c0-c3 is identity, c12-c15 equals camera +0x250, and c8-c11 equals camera
// +0x210 byte-for-byte. A same-size, full-viewport offscreen scene uses the
// same-pixel path; scaled passes and other composite mappings remain hidden.
// Provenance: preserved camera-20261001-021039.bin, SHA-256
// 7a25d0b8fc8ab65baaa379ddb409e7f098ee1ae24c32feeddf666369fb4bc362,
// from the supported executable identity in runtime_contract.h. D3DDisassemble
// used d3dcompiler_47.dll version 10.0.26100.9457. The fixture locator is
// draw sequence 17111, frame 8556, camera sequence 17107; its main-scene
// shader has the guarded c0-c3/c12-c15/c8-c11 relationships above.
[[nodiscard]] std::optional<SceneProjection> ReadTargetlinesSceneProjection(
    const TargetlinesShaderDigest& shaderDigest,
    std::span<const float>         constants,
    const std::array<float, 64>&   cameraRecords,
    const ProjectionViewport&      viewport,
    float                          minZ,
    float                          maxZ,
    std::uint32_t                  renderTargetWidth,
    std::uint32_t                  renderTargetHeight,
    std::uint32_t                  backBufferWidth,
    std::uint32_t                  backBufferHeight,
    bool                           renderTargetIsBackBuffer);

} // namespace bahamut_client
