#include "targetlines_projection.h"

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <iostream>

namespace
{

constexpr bahamut_client::TargetlinesShaderDigest kSceneShader{
    0x32, 0x73, 0x7e, 0x6b, 0xcc, 0x46, 0xec, 0xd9, 0xb4, 0x0b, 0xec, 0x35, 0x4e, 0x14, 0x1e, 0xf4, 0x82, 0x56, 0xe5, 0x02, 0xc2, 0x4a, 0xf8, 0x35, 0x29, 0xa6, 0x54, 0x31, 0xfb, 0xce, 0x18, 0xd6
};

constexpr bahamut_client::TargetlinesShaderDigest kPassthroughShader{
    0x06, 0x68, 0x65, 0x3a, 0x2c, 0xb3, 0x9c, 0xab, 0x83, 0x0f, 0x29, 0x4d, 0x55, 0xec, 0x02, 0x6c, 0x6c, 0x50, 0x03, 0x01, 0x53, 0x79, 0x93, 0x35, 0xa5, 0xaf, 0x85, 0x92, 0x2c, 0x9b, 0x27, 0x05
};

std::array<float, 16> SceneMatrix()
{
    return {
        1.3399982452392578F, -0.03278748691082001F, -0.05507686361670494F, -0.05507548525929451F, 0.0F, 2.3150973320007324F, -0.2420833855867386F, -0.2420773208141327F, -0.07618624716997147F, -0.5766811966896057F, -0.9687168002128601F, -0.9686925411224365F, -1.5632683038711548F, -42.652557373046875F, 129.85470581054688F, 130.10145568847656F
    };
}

std::array<float, 16> ViewMatrix()
{
    return {
        0.9983876347541809F, -0.01374123152345419F, 0.05507548525929451F, 0.0F, 0.0F, 0.9702569842338562F, 0.2420773208141327F, 0.0F, -0.05676381289958954F, -0.24168699979782104F, 0.9686925411224365F, 0.0F, -1.164738655090332F, -17.875680923461914F, -130.10145568847656F, 1.0F
    };
}

std::array<float, 1024> Constants()
{
    std::array<float, 1024> constants{};
    const auto              identity = std::array<float, 16>{
        1.0F, 0.0F, 0.0F, 0.0F, 0.0F, 1.0F, 0.0F, 0.0F, 0.0F, 0.0F, 1.0F, 0.0F, 0.0F, 0.0F, 0.0F, 1.0F
    };
    const auto matrix = SceneMatrix();
    const auto view   = ViewMatrix();
    std::copy(identity.begin(), identity.end(), constants.begin());
    std::copy(matrix.begin(), matrix.end(), constants.begin() + 8u * 4u);
    std::copy(view.begin(), view.end(), constants.begin() + 12u * 4u);
    return constants;
}

std::array<float, 64> CameraRecords()
{
    std::array<float, 64> records{};
    const auto            matrix = SceneMatrix();
    const auto            view   = ViewMatrix();
    std::copy(matrix.begin(), matrix.end(), records.begin());
    std::copy(view.begin(), view.end(), records.begin() + 16u);
    return records;
}

bool Near(float actual, float expected)
{
    return std::fabs(actual - expected) < 0.001F;
}

} // namespace

int main()
{
    // Derived from camera-20261001-021039.bin (SHA-256
    // 7a25d0b8fc8ab65baaa379ddb409e7f098ee1ae24c32feeddf666369fb4bc362),
    // supported executable identity in runtime_contract.h. D3DDisassemble
    // used d3dcompiler_47.dll version 10.0.26100.9457; this is draw
    // 17111/frame 8556/camera 17107 from the main-scene fixture.
    auto                                     constants = Constants();
    const auto                               camera    = CameraRecords();
    const bahamut_client::ProjectionViewport viewport{ 0.0F, 0.0F, 2560.0F, 1440.0F };
    const auto                               projection = bahamut_client::ReadTargetlinesSceneProjection(
        kSceneShader, constants, camera, viewport, 0.0F, 1.0F, 2560u, 1440u, 2560u, 1440u, true);
    if (!projection)
    {
        std::cerr << "captured scene projection was rejected\n";
        return 1;
    }
    const auto segment = bahamut_client::ProjectWorldSegment(
        *projection, { 7.563312F, 45.491718F, 112.506584F }, { 11.8629999F, 43.632999F, 91.931999F });
    if (!segment || !Near(segment->source.x, 1280.0135F) || !Near(segment->source.y, 903.0612F) ||
        !Near(segment->target.x, 1594.4758F) || !Near(segment->target.y, 600.3446F))
    {
        std::cerr << "captured fixture projected endpoints changed\n";
        return 1;
    }

    if (bahamut_client::ReadTargetlinesSceneProjection(
            kPassthroughShader, constants, camera, viewport, 0.0F, 1.0F, 2560u, 1440u, 2560u, 1440u, true))
    {
        std::cerr << "unapproved shader passed the projection gate\n";
        return 1;
    }

    auto cameraMismatch = camera;
    cameraMismatch[16u] += 0.01F;
    if (bahamut_client::ReadTargetlinesSceneProjection(
            kSceneShader, constants, cameraMismatch, viewport, 0.0F, 1.0F, 2560u, 1440u, 2560u, 1440u, true))
    {
        std::cerr << "camera c12-c15 mismatch passed the projection gate\n";
        return 1;
    }

    auto depthMismatch = constants;
    depthMismatch[8u * 4u + 2u] += 0.01F;
    if (bahamut_client::ReadTargetlinesSceneProjection(
            kSceneShader, depthMismatch, camera, viewport, 0.0F, 1.0F, 2560u, 1440u, 2560u, 1440u, true))
    {
        std::cerr << "depth-modified c8-c11 passed the full matrix gate\n";
        return 1;
    }

    if (bahamut_client::ReadTargetlinesSceneProjection(
            kSceneShader, constants, camera, { 2048.0F, 0.0F, 600.0F, 100.0F }, 0.0F, 1.0F, 2560u, 1440u, 2560u, 1440u, true) ||
        bahamut_client::ReadTargetlinesSceneProjection(
            kSceneShader, constants, camera, viewport, 0.0F, 0.5F, 2560u, 1440u, 2560u, 1440u, true) ||
        bahamut_client::ReadTargetlinesSceneProjection(
            kSceneShader, constants, camera, { 0.0F, 0.0F, 2561.0F, 1440.0F }, 0.0F, 1.0F, 2560u, 1440u, 2560u, 1440u, true))
    {
        std::cerr << "unsupported viewport or depth range passed the projection gate\n";
        return 1;
    }

    const auto offscreen = bahamut_client::ReadTargetlinesSceneProjection(
        kSceneShader, constants, camera, viewport, 0.0F, 1.0F, 2560u, 1440u, 2560u, 1440u, false);
    const auto offscreenSegment = offscreen ? bahamut_client::ProjectWorldSegment(
                                                  *offscreen, { 7.563312F, 45.491718F, 112.506584F }, { 11.8629999F, 43.632999F, 91.931999F })
                                            : std::nullopt;
    if (!offscreenSegment || !Near(offscreenSegment->source.x, segment->source.x) ||
        !Near(offscreenSegment->source.y, segment->source.y) ||
        !Near(offscreenSegment->target.x, segment->target.x) || !Near(offscreenSegment->target.y, segment->target.y))
    {
        std::cerr << "same-pixel offscreen candidate was rejected or scaled\n";
        return 1;
    }
    if (bahamut_client::ReadTargetlinesSceneProjection(
            kSceneShader, constants, camera, { 0.0F, 0.0F, 2048.0F, 1152.0F }, 0.0F, 1.0F, 2560u, 1440u, 2560u, 1440u, false) ||
        bahamut_client::ReadTargetlinesSceneProjection(
            kSceneShader, constants, camera, viewport, 0.0F, 1.0F, 2048u, 1152u, 2560u, 1440u, false))
    {
        std::cerr << "offscreen viewport or surface mismatch passed the same-pixel gate\n";
        return 1;
    }

    // camera-20261001-032513.bin, supported identity in runtime_contract.h:
    // SHA-256 9f5ec65fcf90134162bb8f741f14fc0434bce60cabd26639e6a2c7d2b77370c9,
    // draw 9779/frame 4578/camera 9777. Actual offscreen RT, full 2560x1440.
    const std::array<float, 64> capturedConstants{
        1.0F, 0.0F, 0.0F, 0.0F, 0.0F, 1.0F, 0.0F, 0.0F, 0.0F, 0.0F, 1.0F, 0.0F, 0.0F, 0.0F, 0.0F, 1.0F, 0.9999999403953552F, 0.0F, 0.0F, 0.0F, 0.0F, 0.9999999403953552F, 0.0F, 0.0F, 0.0F, 0.0F, 0.9999999403953552F, 0.0F, 0.0F, 0.0F, 0.0F, 1.0F, 0.9331228137016296F, -0.37160295248031616F, -0.7017218470573425F, -0.7017042636871338F, 0.0F, 2.3293840885162354F, -0.21667662262916565F, -0.2166711986064911F, -0.964718222618103F, -0.3594326078891754F, -0.6787397861480713F, -0.6787227988243103F, 78.11122131347656F, -47.49421310424805F, 130.5403289794922F, 130.78704833984375F, 0.695238471031189F, -0.1557387411594391F, 0.7017042636871338F, 0.0F, 0.0F, 0.9762445688247681F, 0.2166711986064911F, 0.0F, -0.7187791466712952F, -0.150638148188591F, 0.6787227988243103F, 0.0F, 58.19804763793945F, -19.904817581176758F, -130.78704833984375F, 1.0F
    };
    const std::array<float, 16> capturedProjection{
        0.9331228137016296F, -0.37160295248031616F, -0.7017218470573425F, -0.7017042636871338F, 0.0F, 2.3293840885162354F, -0.21667662262916565F, -0.2166711986064911F, -0.964718222618103F, -0.3594326078891754F, -0.6787397861480713F, -0.6787227988243103F, 78.11122131347656F, -47.49421310424805F, 130.5403289794922F, 130.78704833984375F
    };
    const std::array<float, 16> capturedView{
        0.695238471031189F, -0.1557387411594391F, 0.7017042636871338F, 0.0F, 0.0F, 0.9762445688247681F, 0.2166711986064911F, 0.0F, -0.7187791466712952F, -0.150638148188591F, 0.6787227988243103F, 0.0F, 58.19804763793945F, -19.904817581176758F, -130.78704833984375F, 1.0F
    };
    std::array<float, 64> capturedCamera{};
    std::copy(capturedProjection.begin(), capturedProjection.end(), capturedCamera.begin());
    std::copy(capturedView.begin(), capturedView.end(), capturedCamera.begin() + 16u);
    const auto capturedCandidate = bahamut_client::ReadTargetlinesSceneProjection(
        kSceneShader, capturedConstants, capturedCamera, viewport, 0.0F, 1.0F, 2560u, 1440u, 2560u, 1440u, false);
    const auto capturedSegment = capturedCandidate ? bahamut_client::ProjectWorldSegment(
                                                         *capturedCandidate, { 40.31669235229492F, 44.03174591064453F, 119.96419525146484F }, { 11.86299991607666F, 43.632999420166016F, 91.93199920654297F })
                                                   : std::nullopt;
    if (!capturedSegment || !Near(capturedSegment->source.x, 1280.0002F) ||
        !Near(capturedSegment->source.y, 909.0366F) || !Near(capturedSegment->target.x, 1292.4525F) ||
        !Near(capturedSegment->target.y, 482.5418F))
    {
        std::cerr << "captured V2 offscreen fixture was rejected or projected incorrectly\n";
        return 1;
    }
    return 0;
}
