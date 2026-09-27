#pragma once

#include <windows.h>

#include <MinHook.h>

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace bahamut_dat_overlay
{

inline constexpr std::uintptr_t kDatOpenAddress            = 0x00453C00u;
inline constexpr std::uintptr_t kDatOpenPatternAddress     = 0x00C96972u;
inline constexpr std::uintptr_t kDatOpenCallAddress        = 0x00C9697Fu;
inline constexpr std::size_t    kDatOpenPatternLength      = 18u;
inline constexpr std::size_t    kDatPathWrapperSize        = 84u;
inline constexpr std::size_t    kDatPathDataOffset         = 0u;
inline constexpr std::size_t    kDatPathCapacityOffset     = 4u;
inline constexpr std::size_t    kDatPathUsedBytesOffset    = 8u;
inline constexpr std::size_t    kDatPathHeapFlagOffset     = 17u;
inline constexpr std::size_t    kDatPathInlineBufferOffset = 18u;
inline constexpr std::size_t    kDatPathInlineCapacity     = 64u;

// The client path wrapper is borrowed at the pre-open seam. Only its first
// pointer is consumed here; the remaining bytes preserve the verified layout
// when a launcher-owned replacement is passed to the client.
#pragma pack(push, 1)

struct DatPathWrapper
{
    const char*   data      = nullptr;
    std::uint32_t capacity  = 0;
    std::uint32_t usedBytes = 0;
    std::uint8_t  reserved[5]{};
    std::uint8_t  heapFlag = 0;
    char          inlineBuffer[kDatPathInlineCapacity]{};
    std::uint8_t  tail[2]{};
};

#pragma pack(pop)

static_assert(sizeof(DatPathWrapper) == kDatPathWrapperSize,
              "the retail DAT path wrapper layout is part of the hook contract");
static_assert(offsetof(DatPathWrapper, data) == kDatPathDataOffset,
              "the retail DAT path data pointer offset is part of the hook contract");
static_assert(offsetof(DatPathWrapper, capacity) == kDatPathCapacityOffset,
              "the retail DAT path capacity offset is part of the hook contract");
static_assert(offsetof(DatPathWrapper, usedBytes) == kDatPathUsedBytesOffset,
              "the retail DAT path used-byte offset is part of the hook contract");
static_assert(offsetof(DatPathWrapper, heapFlag) == kDatPathHeapFlagOffset,
              "the retail DAT path heap flag offset is part of the hook contract");
static_assert(offsetof(DatPathWrapper, inlineBuffer) == kDatPathInlineBufferOffset,
              "the retail DAT path inline buffer offset is part of the hook contract");

using LocalFileOpen = int(__thiscall*)(void*                 localFile,
                                       const DatPathWrapper* path,
                                       const char*           mode,
                                       int                   retryCount);

struct HookApi
{
    using Initialize = MH_STATUS(WINAPI*)();
    using Create     = MH_STATUS(WINAPI*)(LPVOID target, LPVOID detour, LPVOID* original);
    using Enable     = MH_STATUS(WINAPI*)(LPVOID target);
    using Disable    = MH_STATUS(WINAPI*)(LPVOID target);

    Initialize initialize = nullptr;
    Create     create     = nullptr;
    Enable     enable     = nullptr;
    Disable    disable    = nullptr;
};

const HookApi& DefaultHookApi();

// Return the one verified LocalFile-open target from an image, or no value if
// the signature is absent, duplicated, relocated, or points elsewhere.
std::optional<std::uintptr_t> LocateDatOpenTarget(const std::uint8_t* image,
                                                  std::size_t         imageSize,
                                                  std::uintptr_t      imageBase);

// Convert a client-relative DAT path to slash-separated package lookup form.
// Both slash forms are accepted because retail requests use backslashes; all
// rooted, drive-qualified, empty, dot, and parent components are rejected.
bool NormalizeDatPath(std::string_view requested, std::string& normalized);

class DatResolver
{
public:
    explicit DatResolver(const std::vector<std::filesystem::path>& packageRoots);
    DatResolver(const std::vector<std::filesystem::path>& packageRoots,
                const std::filesystem::path&              sourceRoot);

    bool                                 IsConfigured() const;
    std::optional<std::filesystem::path> Resolve(std::string_view requested) const;

private:
    std::vector<std::filesystem::path> packageRoots_;
    std::filesystem::path              sourceRoot_;
};

int __fastcall DatOpenDetour(void* localFile, void* unusedEdx, const DatPathWrapper* path, const char* mode, int retryCount);

class DatOverlay
{
public:
    DatOverlay() = default;
    ~DatOverlay();

    DatOverlay(const DatOverlay&)            = delete;
    DatOverlay& operator=(const DatOverlay&) = delete;

    // exactBuild must come from the already verified retail identity gate.
    // Empty roots are a successful disabled/no-op configuration.
    bool Install(const std::vector<std::filesystem::path>& packageRoots,
                 bool                                      exactBuild,
                 const HookApi&                            api = DefaultHookApi());
    bool Shutdown();

    bool IsInstalled() const;

#ifdef BAHAMUT_DAT_OVERLAY_TEST
    bool ConfigureForTesting(const std::vector<std::filesystem::path>& packageRoots,
                             LocalFileOpen                             original);
    bool InstallTargetForTesting(std::uintptr_t target, const HookApi& api, LocalFileOpen original);
#endif

private:
    friend int __fastcall DatOpenDetour(void* localFile, void* unusedEdx, const DatPathWrapper* path, const char* mode, int retryCount);

    int  Forward(void* localFile, const DatPathWrapper* path, const char* mode, int retryCount);
    bool InstallAt(std::uintptr_t target, const HookApi& api);

    DatResolver   resolver_{ std::vector<std::filesystem::path>{} };
    LocalFileOpen original_   = nullptr;
    LPVOID        hookTarget_ = nullptr;
    HookApi       hookApi_{};
    bool          installed_ = false;
};

} // namespace bahamut_dat_overlay
