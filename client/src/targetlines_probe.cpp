#include "targetlines_probe.h"

#include "fault_guard.h"
#include "overlay.h"
#include "runtime_contract.h"
#include "target_distance.h"
#include "targetlines.h"
#ifdef BAHAMUT_TARGETLINES_PROBE
#include "targetlines_capture.h"
#endif
#include "targetlines_projection.h"

#include <MinHook.h>
#include <bcrypt.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <filesystem>
#include <mutex>
#include <new>
#include <optional>
#include <span>
#include <string>

namespace
{

// The exact-build observation and copied record layout are documented in targetlines_probe.h.
constexpr std::uintptr_t               kCameraUpdateRva = 0x002195D0u;
constexpr std::array<std::uint8_t, 16> kCameraSignature{
    0x55, 0x8B, 0xEC, 0x83, 0xE4, 0xF0, 0x81, 0xEC, 0xF4, 0x00, 0x00, 0x00, 0xA1, 0xB0, 0xA8, 0x2E
};
constexpr std::size_t   kMaximumConstants       = 256u;
constexpr std::size_t   kMaximumShaderBytes     = 16u * 1024u;
constexpr std::size_t   kMaximumSamplesPerFrame = 4u;
constexpr std::uint64_t kMaximumCaptureBytes    = 128u * 1024u * 1024u;

using CameraUpdate                                                  = void(__thiscall*)(void*, std::int32_t*);
using DrawIndexedPrimitive                                          = HRESULT(STDMETHODCALLTYPE*)(IDirect3DDevice9*, D3DPRIMITIVETYPE, INT, UINT, UINT, UINT, UINT);
CameraUpdate                                        gOriginalCamera = nullptr;
DrawIndexedPrimitive                                gOriginalDraw   = nullptr;
IDirect3DDevice9*                                   gDevice         = nullptr;
bahamut_client::TargetDistanceService*              gTargets        = nullptr;
bahamut_client::TargetlinesService*                 gArcs           = nullptr;
std::atomic<bool>                                   gEnabled        = false;
bool                                                gRenderActive   = false;
std::mutex                                          gMutex;
HANDLE                                              gFile          = INVALID_HANDLE_VALUE;
std::uint64_t                                       gBytes         = 0;
std::uint64_t                                       gSequence      = 0;
std::uint32_t                                       gFrame         = 0;
std::uint32_t                                       gGeneration    = 0;
UINT                                                gConstantCount = 0;
std::array<std::uintptr_t, kMaximumSamplesPerFrame> gSampledSurfaces{};
std::size_t                                         gSampleCount = 0;

#pragma pack(push, 1)

struct FileHeader
{
    char          magic[8]{ 'B', 'T', 'L', 'P', '0', '0', '0', '2' };
    char          clientSha256[65]{};
    std::uint64_t clientByteLength = bahamut_runtime_contract::kClientByteLength;
    char          gameVersion[16]{};
    std::int64_t  frequency = 0;
};

struct RecordHeader
{
    std::uint32_t kind         = 0;
    std::uint32_t payloadBytes = 0;
    std::uint64_t sequence     = 0;
    std::int64_t  counter      = 0;
    std::uint32_t thread       = 0;
    std::uint32_t frame        = 0;
    std::uint32_t generation   = 0;
};

struct CameraRecord
{
    std::uint64_t         sequence = 0;
    std::array<float, 64> records{};
};

struct DrawRecord
{
    std::uint64_t cameraSequence = 0;
    std::uint32_t surface        = 0;
    D3DVIEWPORT9  viewport{};
    std::uint32_t renderWidth      = 0;
    std::uint32_t renderHeight     = 0;
    std::uint32_t backBufferWidth  = 0;
    std::uint32_t backBufferHeight = 0;
    std::uint32_t constantCount    = 0;
    std::uint32_t shaderBytes      = 0;
    std::uint32_t sourceActorId    = 0;
    std::uint32_t targetActorId    = 0;
    std::uint32_t zoneId           = 0;
    std::uint32_t hasPositions     = 0;
    float         positions[6]{};
    std::uint32_t renderTargetIsBackBuffer = 0;
};

#pragma pack(pop)

static_assert(sizeof(FileHeader) == 105u && sizeof(RecordHeader) == 36u);
static_assert(sizeof(CameraRecord) == 264u && sizeof(DrawRecord) == 104u);
std::array<CameraRecord, 2> gCameras{};

struct PendingScene
{
    bahamut_client::SceneProjection     projection;
    bahamut_client::TargetlinesSnapshot relationships;
    std::uint32_t                       backBufferWidth  = 0;
    std::uint32_t                       backBufferHeight = 0;
};

std::optional<PendingScene>                           gPendingScene;
std::optional<bahamut_client::TargetlinesRenderFrame> gPresentedFrame;

void LogTargetlinesStatus(const wchar_t* stage, long nativeCode)
{
    std::wstring message = L"Bahamut Targetlines: ";
    message += stage;
    message += L" (code=";
    message += std::to_wstring(nativeCode);
    message += L")\n";
    OutputDebugStringW(message.c_str());
}

#ifdef BAHAMUT_TARGETLINES_PROBE
void LogTargetlinesCapturePath(const std::filesystem::path& requestedPath,
                               const std::filesystem::path& selectedPath)
{
    std::wstring message = L"Bahamut Targetlines: capture-file-opened requested=";
    message += requestedPath.wstring();
    message += L" selected=";
    message += selectedPath.wstring();
    message += L"\n";
    OutputDebugStringW(message.c_str());
}
#endif

bool CopyCamera(void* camera, CameraRecord& record)
{
    BAHAMUT_FAULT_TRY
    {
        constexpr std::uintptr_t offsets[]{ 0x210u, 0x250u, 0x2D0u, 0x370u };
        for (std::size_t index = 0; index < std::size(offsets); ++index)
        {
            std::memcpy(record.records.data() + index * 16u,
                        reinterpret_cast<const void*>(reinterpret_cast<std::uintptr_t>(camera) + offsets[index]),
                        64u);
        }
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool MatchesSignature(const void* target)
{
    BAHAMUT_FAULT_TRY
    {
        return std::memcmp(target, kCameraSignature.data(), kCameraSignature.size()) == 0;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool ComputeShaderDigest(std::span<const std::uint8_t> code, bahamut_client::TargetlinesShaderDigest& digest)
{
    if (code.empty())
    {
        return false;
    }
    BCRYPT_ALG_HANDLE  algorithm    = nullptr;
    BCRYPT_HASH_HANDLE hash         = nullptr;
    PUCHAR             object       = nullptr;
    DWORD              objectLength = 0;
    DWORD              resultLength = 0;
    bool               ok           = BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0) == 0 &&
                                      BCryptGetProperty(algorithm, BCRYPT_OBJECT_LENGTH, reinterpret_cast<PUCHAR>(&objectLength), sizeof(objectLength), &resultLength, 0) == 0;
    if (ok)
    {
        object = new (std::nothrow) UCHAR[objectLength];
        ok     = object != nullptr && BCryptCreateHash(algorithm, &hash, object, objectLength, nullptr, 0, 0) == 0;
    }
    if (ok)
    {
        ok = BCryptHashData(hash, const_cast<PUCHAR>(code.data()), static_cast<ULONG>(code.size()), 0) == 0;
    }
    if (ok)
    {
        ok = BCryptFinishHash(hash, digest.data(), static_cast<ULONG>(digest.size()), 0) == 0;
    }
    if (hash != nullptr)
    {
        BCryptDestroyHash(hash);
    }
    if (algorithm != nullptr)
    {
        BCryptCloseAlgorithmProvider(algorithm, 0);
    }
    delete[] object;
    return ok;
}

void StopCapture()
{
    if (gFile != INVALID_HANDLE_VALUE)
    {
        CloseHandle(gFile);
        gFile = INVALID_HANDLE_VALUE;
    }
}

void Stop()
{
    gEnabled.store(false);
    gRenderActive = false;
    if (gArcs != nullptr)
    {
        gArcs->SetCastingEnabled(false);
    }
    gPendingScene.reset();
    gPresentedFrame.reset();
    StopCapture();
}

bool Write(const void* data, std::size_t size)
{
    DWORD written = 0;
    DWORD error   = ERROR_SUCCESS;
    if (gFile == INVALID_HANDLE_VALUE)
    {
        error = ERROR_INVALID_HANDLE;
    }
    else if (size > kMaximumCaptureBytes - gBytes)
    {
        error = ERROR_FILE_TOO_LARGE;
    }
    else if (!WriteFile(gFile, data, static_cast<DWORD>(size), &written, nullptr))
    {
        error = GetLastError();
    }
    else if (written != size)
    {
        error = ERROR_WRITE_FAULT;
    }
    if (error != ERROR_SUCCESS)
    {
        StopCapture();
        SetLastError(error);
        return false;
    }
    gBytes += size;
    return true;
}

bool BeginRecord(std::uint32_t kind, std::size_t payloadBytes)
{
    if (gFile == INVALID_HANDLE_VALUE)
    {
        return false;
    }
    RecordHeader header;
    header.kind         = kind;
    header.payloadBytes = static_cast<std::uint32_t>(payloadBytes);
    header.sequence     = ++gSequence;
    LARGE_INTEGER counter{};
    QueryPerformanceCounter(&counter);
    header.counter    = counter.QuadPart;
    header.thread     = GetCurrentThreadId();
    header.frame      = gFrame;
    header.generation = gGeneration;
    if (sizeof(header) + payloadBytes > kMaximumCaptureBytes - gBytes)
    {
        StopCapture();
        return false;
    }
    return Write(&header, sizeof(header));
}

void __fastcall HookedCamera(void* camera, void*, std::int32_t* delta)
{
    std::uint32_t generation = 0;
    bool          capture    = gEnabled.load();
    if (capture)
    {
        std::lock_guard<std::mutex> lock(gMutex);
        capture    = gRenderActive || gFile != INVALID_HANDLE_VALUE;
        generation = gGeneration;
    }
    gOriginalCamera(camera, delta);
    if (!capture || !gEnabled.load())
    {
        return;
    }
    std::lock_guard<std::mutex> lock(gMutex);
    // Camera preparation and Reset can run on different threads. Discard updates
    // that span Reset and serialize the camera copy under the same lock.
    if (!gEnabled.load() || generation != gGeneration || (!gRenderActive && gFile == INVALID_HANDLE_VALUE))
    {
        return;
    }
    CameraRecord record;
    if (!CopyCamera(camera, record))
    {
        return;
    }
    if (BeginRecord(1u, sizeof(record)))
    {
        record.sequence = gSequence;
        static_cast<void>(Write(&record, sizeof(record)));
    }
    else
    {
        record.sequence = ++gSequence;
    }
    // Capture limits affect the writer only. The current scene still needs
    // camera updates to keep the addon aligned after capture has finished.
    gCameras[1] = gCameras[0];
    gCameras[0] = record;
}

void CaptureDraw(IDirect3DDevice9* device)
{
    std::lock_guard<std::mutex> lock(gMutex);
    const bool                  candidateNeeded = gRenderActive && !gPendingScene.has_value();
    if (!gEnabled.load() || device != gDevice ||
        (!candidateNeeded && (gFile == INVALID_HANDLE_VALUE || gSampleCount >= gSampledSurfaces.size())))
    {
        return;
    }
    const auto relationships = gRenderActive && gArcs != nullptr ? gArcs->Snapshot() : bahamut_client::TargetlinesSnapshot{};
    if (relationships.count == 0u && gFile == INVALID_HANDLE_VALUE)
    {
        return;
    }
    const auto positions = relationships.count != 0u ? std::optional{ relationships.arcs[0].positions }
                           : gTargets != nullptr     ? gTargets->PositionSnapshot()
                                                     : std::nullopt;
    if (!positions)
    {
        return;
    }
    IDirect3DSurface9* surface = nullptr;
    if (FAILED(device->GetRenderTarget(0, &surface)) || surface == nullptr)
    {
        return;
    }
    const auto      surfaceId = reinterpret_cast<std::uintptr_t>(surface);
    D3DSURFACE_DESC renderDesc{};
    const HRESULT   description    = surface->GetDesc(&renderDesc);
    const bool      alreadySampled = std::find(gSampledSurfaces.begin(), gSampledSurfaces.begin() + gSampleCount, surfaceId) !=
                                     gSampledSurfaces.begin() + gSampleCount;
    if (FAILED(description) || (alreadySampled && !candidateNeeded))
    {
        surface->Release();
        return;
    }
    IDirect3DSurface9* backBuffer = nullptr;
    if (FAILED(device->GetBackBuffer(0u, 0u, D3DBACKBUFFER_TYPE_MONO, &backBuffer)) || backBuffer == nullptr)
    {
        surface->Release();
        return;
    }
    D3DSURFACE_DESC backDesc{};
    if (FAILED(backBuffer->GetDesc(&backDesc)) || backDesc.Width == 0u || backDesc.Height == 0u)
    {
        backBuffer->Release();
        surface->Release();
        return;
    }
    const bool renderTargetIsBackBuffer = surface == backBuffer;
    backBuffer->Release();
    surface->Release();
    const bool recordable         = gFile != INVALID_HANDLE_VALUE && !alreadySampled && gSampleCount < gSampledSurfaces.size();
    const bool sameSizeScene      = renderDesc.Width == backDesc.Width && renderDesc.Height == backDesc.Height;
    const bool considerProjection = candidateNeeded && (renderTargetIsBackBuffer || sameSizeScene);
    if (!recordable && !considerProjection)
    {
        return;
    }
    std::array<float, kMaximumConstants * 4u> constants{};
    if (FAILED(device->GetVertexShaderConstantF(0u, constants.data(), gConstantCount)))
    {
        return;
    }
    std::uint64_t       cameraSequence = 0;
    const CameraRecord* matchedCamera  = nullptr;
    for (const auto& camera : gCameras)
    {
        if (camera.sequence == 0u)
        {
            continue;
        }
        for (std::size_t index = 0; index + 4u <= gConstantCount; ++index)
        {
            // Select the camera by matching the observed view bytes, never by draw ordinal.
            if (std::memcmp(constants.data() + index * 4u, camera.records.data() + 16u, 64u) == 0)
            {
                cameraSequence = camera.sequence;
                matchedCamera  = &camera;
                break;
            }
        }
        if (cameraSequence != 0u)
        {
            break;
        }
    }
    if (cameraSequence == 0u)
    {
        return;
    }
    if (!recordable && considerProjection &&
        (gConstantCount < 16u ||
         std::memcmp(constants.data() + 12u * 4u, matchedCamera->records.data() + 16u, 16u * sizeof(float)) != 0 ||
         std::memcmp(constants.data() + 8u * 4u, matchedCamera->records.data(), 16u * sizeof(float)) != 0))
    {
        return;
    }
    IDirect3DVertexShader9* shader = nullptr;
    if (FAILED(device->GetVertexShader(&shader)) || shader == nullptr)
    {
        return;
    }
    std::array<std::uint8_t, kMaximumShaderBytes> shaderCode{};
    UINT                                          shaderBytes    = 0;
    bool                                          readableShader = SUCCEEDED(shader->GetFunction(nullptr, &shaderBytes)) &&
                                                                   shaderBytes != 0u && shaderBytes <= shaderCode.size();
    if (readableShader)
    {
        readableShader = SUCCEEDED(shader->GetFunction(shaderCode.data(), &shaderBytes));
    }
    shader->Release();
    if (!readableShader || shaderBytes > shaderCode.size())
    {
        return;
    }
    DrawRecord record;
    if (FAILED(device->GetViewport(&record.viewport)))
    {
        return;
    }
    record.cameraSequence           = cameraSequence;
    record.surface                  = static_cast<std::uint32_t>(surfaceId);
    record.renderWidth              = renderDesc.Width;
    record.renderHeight             = renderDesc.Height;
    record.backBufferWidth          = backDesc.Width;
    record.backBufferHeight         = backDesc.Height;
    record.constantCount            = gConstantCount;
    record.shaderBytes              = shaderBytes;
    record.hasPositions             = 1u;
    record.sourceActorId            = positions->sourceActorId;
    record.targetActorId            = positions->targetActorId;
    record.zoneId                   = positions->zoneId;
    record.renderTargetIsBackBuffer = renderTargetIsBackBuffer ? 1u : 0u;
    const auto& source              = positions->source;
    const auto& target              = positions->target;
    const float values[]{ source.x, source.y, source.z, target.x, target.y, target.z };
    std::memcpy(record.positions, values, sizeof(values));
    bahamut_client::TargetlinesShaderDigest shaderDigest{};
    const bool                              shaderDigestAvailable = considerProjection &&
                                                                    ComputeShaderDigest(std::span<const std::uint8_t>(shaderCode.data(), shaderBytes), shaderDigest);
    if (recordable)
    {
        const std::size_t constantBytes = gConstantCount * 16u;
        const bool        recordWritten = BeginRecord(2u, sizeof(record) + constantBytes + shaderBytes) &&
                                          Write(&record, sizeof(record)) && Write(constants.data(), constantBytes) && Write(shaderCode.data(), shaderBytes);
        if (recordWritten)
        {
            gSampledSurfaces[gSampleCount++] = surfaceId;
        }
    }
    if (!considerProjection || !shaderDigestAvailable || matchedCamera == nullptr)
    {
        return;
    }
    const bahamut_client::ProjectionViewport viewport{
        static_cast<float>(record.viewport.X),
        static_cast<float>(record.viewport.Y),
        static_cast<float>(record.viewport.Width),
        static_cast<float>(record.viewport.Height)
    };
    const auto projection = bahamut_client::ReadTargetlinesSceneProjection(
        shaderDigest,
        std::span<const float>(constants.data(), gConstantCount * 4u),
        matchedCamera->records,
        viewport,
        record.viewport.MinZ,
        record.viewport.MaxZ,
        record.renderWidth,
        record.renderHeight,
        record.backBufferWidth,
        record.backBufferHeight,
        renderTargetIsBackBuffer);
    if (projection)
    {
        PendingScene pending;
        pending.projection       = *projection;
        pending.relationships    = relationships;
        pending.backBufferWidth  = record.backBufferWidth;
        pending.backBufferHeight = record.backBufferHeight;
        gPendingScene            = pending;
    }
}

HRESULT STDMETHODCALLTYPE HookedDraw(IDirect3DDevice9* device, D3DPRIMITIVETYPE type, INT baseVertexIndex, UINT minimumVertexIndex, UINT vertexCount, UINT startIndex, UINT primitiveCount)
{
    if (gEnabled.load() && device == gDevice && !IsOverlayDrawing(device))
    {
        CaptureDraw(device);
    }
    return gOriginalDraw(device, type, baseVertexIndex, minimumVertexIndex, vertexCount, startIndex, primitiveCount);
}

} // namespace

namespace bahamut_client
{

bool TargetlinesProbeRequested()
{
#ifdef BAHAMUT_TARGETLINES_PROBE
    return GetEnvironmentVariableW(L"BAHAMUT_TARGETLINES_PROBE_FILE", nullptr, 0) > 1u;
#else
    return false;
#endif
}

void StartTargetlinesRenderer(TargetDistanceService* targets, TargetlinesService* arcs, bool addonEnabled)
{
    if (gEnabled.load())
    {
        return;
    }
    std::lock_guard<std::mutex> lock(gMutex);
    gTargets = targets;
    gArcs    = arcs;
    if (gArcs != nullptr)
    {
        gArcs->SetCastingEnabled(addonEnabled);
    }
    if (!addonEnabled && !TargetlinesProbeRequested())
    {
        return;
    }
    // Runtime bootstrap checks the complete retail identity; the entry signature
    // also prevents a test stub from receiving the retail hook.
    if (reinterpret_cast<std::uintptr_t>(GetModuleHandleW(nullptr)) != bahamut_runtime_contract::kRetailImageBase)
    {
        LogTargetlinesStatus(L"camera-hook-identity-rejected", ERROR_BAD_EXE_FORMAT);
        return;
    }
    void* target = reinterpret_cast<void*>(bahamut_runtime_contract::kRetailImageBase + kCameraUpdateRva);
    if (!MatchesSignature(target))
    {
        LogTargetlinesStatus(L"camera-hook-signature-rejected", ERROR_INVALID_DATA);
        return;
    }
#ifdef BAHAMUT_TARGETLINES_PROBE
    wchar_t     path[32768]{};
    const DWORD length = GetEnvironmentVariableW(L"BAHAMUT_TARGETLINES_PROBE_FILE", path, ARRAYSIZE(path));
    if (length != 0u && length < ARRAYSIZE(path) && std::filesystem::path(path).is_absolute())
    {
        const auto capture = OpenTargetlinesCapture(path);
        gFile              = capture.handle;
        if (gFile == INVALID_HANDLE_VALUE)
        {
            LogTargetlinesStatus(L"capture-file-open-failed", static_cast<long>(capture.error));
        }
        else
        {
            LogTargetlinesCapturePath(path, capture.path);
            FileHeader header;
            std::memcpy(header.clientSha256, bahamut_runtime_contract::kClientSha256, sizeof(header.clientSha256));
            std::memcpy(header.gameVersion, bahamut_runtime_contract::kGameVersion, sizeof(bahamut_runtime_contract::kGameVersion));
            LARGE_INTEGER frequency{};
            QueryPerformanceFrequency(&frequency);
            header.frequency = frequency.QuadPart;
            if (!Write(&header, sizeof(header)))
            {
                LogTargetlinesStatus(L"header-write-failed", static_cast<long>(GetLastError()));
            }
            else
            {
                LogTargetlinesStatus(L"header-written", ERROR_SUCCESS);
            }
        }
    }
    if (!addonEnabled && gFile == INVALID_HANDLE_VALUE)
    {
        return;
    }
#endif
    const MH_STATUS cameraHook = MH_CreateHook(target, reinterpret_cast<void*>(&HookedCamera), reinterpret_cast<void**>(&gOriginalCamera));
    if (cameraHook != MH_OK)
    {
        LogTargetlinesStatus(L"camera-hook-create-failed", static_cast<long>(cameraHook));
        Stop();
        return;
    }
    gRenderActive = addonEnabled;
    gEnabled.store(true);
    const MH_STATUS cameraEnable = MH_EnableHook(target);
    if (cameraEnable != MH_OK)
    {
        LogTargetlinesStatus(L"camera-hook-enable-failed", static_cast<long>(cameraEnable));
        Stop();
        static_cast<void>(MH_RemoveHook(target));
        gOriginalCamera = nullptr;
    }
}

void SetTargetlinesRendererActive(bool enabled)
{
    std::lock_guard<std::mutex> lock(gMutex);
    const bool                  active = enabled && gEnabled.load();
    if (active != gRenderActive)
    {
        gRenderActive = active;
        ++gGeneration;
        gPendingScene.reset();
        gPresentedFrame.reset();
        gCameras = {};
    }
    if (gArcs != nullptr)
    {
        gArcs->SetCastingEnabled(active);
    }
}

void BindTargetlinesRendererDevice(IDirect3DDevice9* device, void* drawIndexedPrimitive)
{
    if (!gEnabled.load() || gDevice != nullptr)
    {
        return;
    }
    std::lock_guard<std::mutex> lock(gMutex);
    D3DCAPS9                    caps{};
    const HRESULT               capsResult = device->GetDeviceCaps(&caps);
    if (FAILED(capsResult))
    {
        LogTargetlinesStatus(L"device-capabilities-failed", static_cast<long>(capsResult));
        Stop();
        return;
    }
    if (caps.MaxVertexShaderConst < 4u)
    {
        LogTargetlinesStatus(L"device-capabilities-rejected", ERROR_NOT_SUPPORTED);
        Stop();
        return;
    }
    const MH_STATUS drawHook = MH_CreateHook(drawIndexedPrimitive, reinterpret_cast<void*>(&HookedDraw), reinterpret_cast<void**>(&gOriginalDraw));
    if (drawHook != MH_OK)
    {
        LogTargetlinesStatus(L"device-hook-create-failed", static_cast<long>(drawHook));
        Stop();
        return;
    }
    gConstantCount             = std::min<UINT>(caps.MaxVertexShaderConst, static_cast<UINT>(kMaximumConstants));
    gDevice                    = device;
    const MH_STATUS drawEnable = MH_EnableHook(drawIndexedPrimitive);
    if (drawEnable != MH_OK)
    {
        LogTargetlinesStatus(L"device-hook-enable-failed", static_cast<long>(drawEnable));
        Stop();
        static_cast<void>(MH_RemoveHook(drawIndexedPrimitive));
        gOriginalDraw = nullptr;
        gDevice       = nullptr;
    }
}

void PresentTargetlinesRenderer()
{
    std::lock_guard<std::mutex> lock(gMutex);
    gPresentedFrame.reset();
    if (!gEnabled.load())
    {
        gPendingScene.reset();
        return;
    }
    if (gRenderActive && gPendingScene && gArcs != nullptr)
    {
        const auto             current = gArcs->Snapshot();
        TargetlinesRenderFrame frame;
        frame.backBufferWidth  = gPendingScene->backBufferWidth;
        frame.backBufferHeight = gPendingScene->backBufferHeight;
        for (std::size_t index = 0; index < gPendingScene->relationships.count; ++index)
        {
            const auto&               saved    = gPendingScene->relationships.arcs[index];
            const TargetlineSnapshot* matching = nullptr;
            for (std::size_t currentIndex = 0; currentIndex < current.count; ++currentIndex)
            {
                const auto& candidate = current.arcs[currentIndex];
                if (candidate.positions.sourceActorId == saved.positions.sourceActorId &&
                    candidate.positions.targetActorId == saved.positions.targetActorId &&
                    candidate.positions.zoneId == saved.positions.zoneId &&
                    candidate.friendly == saved.friendly && candidate.relationshipSequence == saved.relationshipSequence)
                {
                    matching = &candidate;
                    break;
                }
            }
            if (matching != nullptr)
            {
                const auto&              source   = saved.positions.source;
                const auto&              target   = saved.positions.target;
                const float              distance = std::hypot(target.x - source.x, target.z - source.z);
                const TargetlineArcStyle style{ 1.1F, std::clamp(distance * 0.32F, 1.0F, 6.0F) };
                const auto               pieces = ProjectTargetlineArc(gPendingScene->projection, source, target, style);
                if (pieces.count != 0u)
                {
                    frame.arcs[frame.count++] = { pieces, saved.friendly, matching->opacity };
                }
            }
        }
        if (frame.count != 0u)
        {
            gPresentedFrame = frame;
        }
    }
    gPendingScene.reset();
    static_cast<void>(BeginRecord(3u, 0u));
    ++gFrame;
    gSampleCount = 0;
}

void ResetTargetlinesRenderer()
{
    std::lock_guard<std::mutex> lock(gMutex);
    gPendingScene.reset();
    gPresentedFrame.reset();
    if (gEnabled.load())
    {
        ++gGeneration;
        gCameras     = {};
        gSampleCount = 0;
        static_cast<void>(BeginRecord(4u, 0u));
    }
}

std::optional<TargetlinesRenderFrame> TargetlinesFrame()
{
    std::lock_guard<std::mutex> lock(gMutex);
    if (!gEnabled.load() || !gRenderActive || !gPresentedFrame)
    {
        return std::nullopt;
    }
    return gPresentedFrame;
}

} // namespace bahamut_client
