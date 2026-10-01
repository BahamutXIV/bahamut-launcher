#include "targetlines_projection.h"

#include <algorithm>
#include <cmath>
#include <cstring>

namespace
{

using bahamut_client::TargetlinesShaderDigest;

// Allowlist provenance: preserved camera-20261001-021039.bin (SHA-256
// 7a25d0b8fc8ab65baaa379ddb409e7f098ee1ae24c32feeddf666369fb4bc362),
// supported by runtime_contract.h. D3DDisassemble used d3dcompiler_47.dll
// version 10.0.26100.9457. The three hashes below are the main-scene shader
// variants; draw 17111/frame 8556/camera 17107 is the captured fixture. The
// observed role is c0-c3 identity, c12-c15 camera +0x250, and c8-c11 camera
// +0x210, all checked again by ReadTargetlinesSceneProjection.
constexpr TargetlinesShaderDigest kSceneShaderDigests[]{
    { 0x32, 0x73, 0x7e, 0x6b, 0xcc, 0x46, 0xec, 0xd9, 0xb4, 0x0b, 0xec, 0x35, 0x4e, 0x14, 0x1e, 0xf4, 0x82, 0x56, 0xe5, 0x02, 0xc2, 0x4a, 0xf8, 0x35, 0x29, 0xa6, 0x54, 0x31, 0xfb, 0xce, 0x18, 0xd6 },
    { 0xb2, 0xba, 0xd2, 0x6f, 0x21, 0xc2, 0xd8, 0x8b, 0x0d, 0x7b, 0xfe, 0xa9, 0xc9, 0x5a, 0x76, 0xac, 0xe3, 0xa4, 0x27, 0xd3, 0x79, 0xf4, 0x3a, 0x54, 0x8d, 0xc1, 0x3f, 0x3f, 0xbe, 0x75, 0x3f, 0xfb },
    { 0x83, 0x94, 0x36, 0x4e, 0x85, 0x69, 0xa5, 0xa0, 0xd8, 0x83, 0xb1, 0xe8, 0x88, 0x8a, 0x1a, 0x70, 0x9e, 0x4b, 0x36, 0x3b, 0xd6, 0xb3, 0x06, 0xfe, 0xf2, 0xbd, 0xb7, 0x83, 0xe4, 0xf1, 0x4e, 0x71 }
};

constexpr std::array<float, 16> kIdentity{
    1.0F, 0.0F, 0.0F, 0.0F, 0.0F, 1.0F, 0.0F, 0.0F, 0.0F, 0.0F, 1.0F, 0.0F, 0.0F, 0.0F, 0.0F, 1.0F
};

bool IsSceneShader(const TargetlinesShaderDigest& digest)
{
    return std::any_of(std::begin(kSceneShaderDigests), std::end(kSceneShaderDigests), [&digest](const auto& allowed)
                       {
                           return digest == allowed;
                       });
}

bool Finite(std::span<const float> values)
{
    return std::all_of(values.begin(), values.end(), [](float value)
                       {
                           return std::isfinite(value);
                       });
}

bool SupportedViewport(const bahamut_client::ProjectionViewport& viewport,
                       float                                     minZ,
                       float                                     maxZ,
                       std::uint32_t                             renderTargetWidth,
                       std::uint32_t                             renderTargetHeight,
                       std::uint32_t                             backBufferWidth,
                       std::uint32_t                             backBufferHeight,
                       bool                                      renderTargetIsBackBuffer)
{
    if (!std::isfinite(viewport.x) || !std::isfinite(viewport.y) || !std::isfinite(viewport.width) ||
        !std::isfinite(viewport.height) || !std::isfinite(minZ) || !std::isfinite(maxZ) ||
        viewport.x < 0.0F || viewport.y < 0.0F || viewport.width <= 0.0F || viewport.height <= 0.0F ||
        minZ != 0.0F || maxZ != 1.0F || backBufferWidth == 0u || backBufferHeight == 0u ||
        renderTargetWidth != backBufferWidth || renderTargetHeight != backBufferHeight)
    {
        return false;
    }
    if (!renderTargetIsBackBuffer)
    {
        // Equal dimensions do not prove a composite path. Use same-pixel mapping
        // only for a full-viewport surface with matching dimensions.
        return viewport.x == 0.0F && viewport.y == 0.0F &&
               viewport.width == static_cast<float>(backBufferWidth) &&
               viewport.height == static_cast<float>(backBufferHeight);
    }
    return viewport.x + viewport.width <= static_cast<float>(backBufferWidth) &&
           viewport.y + viewport.height <= static_cast<float>(backBufferHeight);
}

} // namespace

namespace bahamut_client
{

std::optional<SceneProjection> ReadTargetlinesSceneProjection(
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
    bool                           renderTargetIsBackBuffer)
{
    constexpr std::size_t kC0  = 0u;
    constexpr std::size_t kC8  = 8u * 4u;
    constexpr std::size_t kC12 = 12u * 4u;
    if (!IsSceneShader(shaderDigest) || constants.size() < 16u * 4u ||
        !Finite(std::span<const float>(cameraRecords.data(), 32u)) ||
        !Finite(constants.subspan(kC0, 16u * 4u)) ||
        !SupportedViewport(viewport, minZ, maxZ, renderTargetWidth, renderTargetHeight, backBufferWidth, backBufferHeight, renderTargetIsBackBuffer))
    {
        return std::nullopt;
    }

    if (std::memcmp(constants.data() + kC0, kIdentity.data(), sizeof(kIdentity)) != 0 ||
        std::memcmp(constants.data() + kC12, cameraRecords.data() + 16u, 16u * sizeof(float)) != 0 ||
        std::memcmp(constants.data() + kC8, cameraRecords.data(), 16u * sizeof(float)) != 0)
    {
        return std::nullopt;
    }

    SceneProjection projection;
    std::memcpy(projection.worldToClip.data(), constants.data() + kC8, sizeof(projection.worldToClip));
    projection.viewport = viewport;
    if (!Finite(std::span<const float>(projection.worldToClip.data(), projection.worldToClip.size())))
    {
        return std::nullopt;
    }
    return projection;
}

} // namespace bahamut_client
