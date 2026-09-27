#include "dat_overlay.h"

#include <windows.h>

#include <array>
#include <cstdint>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <string_view>
#include <vector>

namespace
{

using namespace bahamut_dat_overlay;

struct TestTree
{
    TestTree()
    : root(std::filesystem::temp_directory_path() / (L"bahamut-dat-overlay-" + std::to_wstring(GetCurrentProcessId()) + L"-" + std::to_wstring(GetTickCount64())))
    {
        std::filesystem::create_directories(root);
    }

    ~TestTree()
    {
        std::error_code error;
        std::filesystem::remove_all(root, error);
    }

    std::filesystem::path root;
};

void WriteFile(const std::filesystem::path& path, std::string_view contents)
{
    std::filesystem::create_directories(path.parent_path());
    std::ofstream file(path, std::ios::binary | std::ios::trunc);
    file << contents;
}

bool TestPathSafetyAndPrecedence()
{
    TestTree   tree;
    const auto first  = tree.root / L"first";
    const auto second = tree.root / L"second";
    const auto game   = tree.root / L"game";
    WriteFile(first / L"data" / L"2A" / L"08.DAT", "first");
    WriteFile(second / L"data" / L"2A" / L"08.DAT", "second");
    WriteFile(game / L"data" / L"2A" / L"08.DAT", "retail");
    DatResolver resolver({ first, second }, game);
    const auto  selected = resolver.Resolve("data\\2A\\08.DAT");
    if (!selected || !std::filesystem::equivalent(
                         *selected, first / L"data" / L"2A" / L"08.DAT"))
    {
        return false;
    }
    const auto selectedFromRetailPath = resolver.Resolve(
        (game / L"data" / L"2A" / L"08.DAT").string());
    if (!selectedFromRetailPath || !std::filesystem::equivalent(
                                       *selectedFromRetailPath, first / L"data" / L"2A" / L"08.DAT"))
    {
        return false;
    }

    for (const std::string_view unsafe : {
             "../outside.DAT",
             "data/../outside.DAT",
             "/data/a.DAT",
             "\\data\\a.DAT",
             "C:/data/a.DAT",
             "data/./a.DAT",
             "data//a.DAT",
             "data:a.DAT",
         })
    {
        if (resolver.Resolve(unsafe))
        {
            return false;
        }
    }

    const auto outside = tree.root / L"outside";
    WriteFile(outside / L"escape.DAT", "outside");
    WriteFile(outside / L"data" / L"2A" / L"08.DAT", "outside");
    if (resolver.Resolve((outside / L"data" / L"2A" / L"08.DAT").string()))
    {
        return false;
    }
    const auto link = first / L"escape";
    if (CreateSymbolicLinkW(link.c_str(), outside.c_str(), SYMBOLIC_LINK_FLAG_DIRECTORY))
    {
        if (resolver.Resolve("escape/escape.DAT"))
        {
            return false;
        }
    }
    return true;
}

struct OriginalCapture
{
    const DatPathWrapper* path       = nullptr;
    const char*           mode       = nullptr;
    void*                 localFile  = nullptr;
    int                   retryCount = -1;
    std::string           bytes;
    std::string           expectedReplacement;
    std::uint32_t         capacity         = 0;
    std::uint32_t         usedBytes        = 0;
    std::uint8_t          heapFlag         = 0;
    bool                  replacementAlive = false;
};

OriginalCapture* gCapture = nullptr;

int __fastcall FakeOriginal(void* localFile, void* unusedEdx, const DatPathWrapper* path, const char* mode, int retryCount)
{
    UNREFERENCED_PARAMETER(unusedEdx);
    gCapture->localFile  = localFile;
    gCapture->path       = path;
    gCapture->mode       = mode;
    gCapture->retryCount = retryCount;
    gCapture->bytes.clear();
    if (path != nullptr && path->data != nullptr)
    {
        gCapture->bytes     = path->data;
        gCapture->capacity  = path->capacity;
        gCapture->usedBytes = path->usedBytes;
        gCapture->heapFlag  = path->heapFlag;
    }
    gCapture->replacementAlive = path != nullptr && gCapture->bytes == gCapture->expectedReplacement;
    return 73;
}

bool InstallTestHook(DatOverlay& overlay);

bool TestMissForwardAndReplacementLifetime()
{
    TestTree   tree;
    const auto package = tree.root / L"package";
    WriteFile(package / L"data" / L"replacement.DAT", "replacement");
    OriginalCapture capture;
    gCapture = &capture;
    DatOverlay overlay;
    if (!overlay.ConfigureForTesting({ package },
                                     reinterpret_cast<LocalFileOpen>(&FakeOriginal)))
    {
        gCapture = nullptr;
        return false;
    }
    capture.expectedReplacement =
        std::filesystem::canonical(package / L"data" / L"replacement.DAT").string();

    const std::string originalText = "data\\missing.DAT";
    DatPathWrapper    original{};
    original.data        = originalText.c_str();
    const char mode[]    = "rb";
    void*      localFile = reinterpret_cast<void*>(0x1234);
    if (!InstallTestHook(overlay))
    {
        gCapture = nullptr;
        return false;
    }
    std::array<std::uint8_t, sizeof(DatPathWrapper)> missSnapshot{};
    std::memcpy(missSnapshot.data(), &original, missSnapshot.size());
    if (DatOpenDetour(localFile, nullptr, &original, mode, 0) != 73 || capture.path != &original || capture.bytes != originalText || capture.mode != mode || capture.localFile != localFile || capture.retryCount != 0 || std::memcmp(&original, missSnapshot.data(), missSnapshot.size()) != 0)
    {
        gCapture = nullptr;
        return false;
    }

    const std::string replacementRequest = "data/replacement.DAT";
    original.data                        = replacementRequest.c_str();
    capture.replacementAlive             = false;
    std::array<std::uint8_t, sizeof(DatPathWrapper)> hitSnapshot{};
    std::memcpy(hitSnapshot.data(), &original, hitSnapshot.size());
    if (DatOpenDetour(localFile, nullptr, &original, mode, 0) != 73 || capture.path == &original || !capture.replacementAlive || capture.capacity != capture.expectedReplacement.size() + 1 || capture.usedBytes != capture.capacity || capture.heapFlag != 1 || std::memcmp(&original, hitSnapshot.data(), hitSnapshot.size()) != 0)
    {
        gCapture = nullptr;
        return false;
    }
    gCapture = nullptr;
    return true;
}

struct HookState
{
    static inline MH_STATUS initializeStatus = MH_OK;
    static inline MH_STATUS createStatus     = MH_OK;
    static inline MH_STATUS enableStatus     = MH_OK;
    static inline MH_STATUS disableStatus    = MH_OK;
    static inline int       initializeCount  = 0;
    static inline int       createCount      = 0;
    static inline int       enableCount      = 0;
    static inline int       disableCount     = 0;

    static void Reset()
    {
        initializeStatus = MH_OK;
        createStatus     = MH_OK;
        enableStatus     = MH_OK;
        disableStatus    = MH_OK;
        initializeCount  = 0;
        createCount      = 0;
        enableCount      = 0;
        disableCount     = 0;
    }

    static MH_STATUS WINAPI Initialize()
    {
        ++initializeCount;
        return initializeStatus;
    }

    static MH_STATUS WINAPI Create(LPVOID, LPVOID, LPVOID* original)
    {
        ++createCount;
        if (createStatus == MH_OK)
        {
            *original = reinterpret_cast<LPVOID>(&FakeOriginal);
        }
        return createStatus;
    }

    static MH_STATUS WINAPI Enable(LPVOID)
    {
        ++enableCount;
        return enableStatus;
    }

    static MH_STATUS WINAPI Disable(LPVOID)
    {
        ++disableCount;
        return disableStatus;
    }

    static HookApi Api()
    {
        return { Initialize, Create, Enable, Disable };
    }
};

bool InstallTestHook(DatOverlay& overlay)
{
    HookState::Reset();
    return overlay.InstallTargetForTesting(kDatOpenAddress, HookState::Api(), reinterpret_cast<LocalFileOpen>(&FakeOriginal));
}

bool TestProcessLifetimeHookRollback()
{
    TestTree   tree;
    const auto package = tree.root / L"package";
    WriteFile(package / L"data" / L"unused.DAT", "unused");
    const HookApi api = HookState::Api();
    HookState::Reset();
    DatOverlay exactBuildRejected;
    if (exactBuildRejected.Install({ package }, true, api) || exactBuildRejected.IsInstalled() || HookState::initializeCount != 0 || HookState::createCount != 0)
    {
        return false;
    }

    DatOverlay overlay;
    if (!overlay.InstallTargetForTesting(kDatOpenAddress, api, reinterpret_cast<LocalFileOpen>(&FakeOriginal)) || !overlay.IsInstalled() || !overlay.Shutdown() || overlay.IsInstalled() || HookState::disableCount != 1)
    {
        return false;
    }
    if (overlay.Install({}, true, api))
    {
        return false;
    }

    HookState::Reset();
    HookState::enableStatus = MH_ERROR_ENABLED;
    DatOverlay failedEnable;
    if (failedEnable.InstallTargetForTesting(kDatOpenAddress, api, reinterpret_cast<LocalFileOpen>(&FakeOriginal)) || failedEnable.IsInstalled() || HookState::disableCount != 1)
    {
        return false;
    }

    HookState::Reset();
    HookState::createStatus = MH_ERROR_NOT_EXECUTABLE;
    DatOverlay failedCreate;
    if (failedCreate.InstallTargetForTesting(kDatOpenAddress, api, reinterpret_cast<LocalFileOpen>(&FakeOriginal)) || failedCreate.IsInstalled() || HookState::disableCount != 0)
    {
        return false;
    }

    HookState::Reset();
    DatOverlay disabled;
    return disabled.Install({}, true, api) && !disabled.IsInstalled() && disabled.Install({ package }, false, api);
}

bool TestLocatorMutation()
{
    constexpr std::uintptr_t base   = 0x00400000u;
    const std::size_t        offset = static_cast<std::size_t>(
        kDatOpenPatternAddress - base);
    std::vector<std::uint8_t>                             image(offset + kDatOpenPatternLength, 0);
    const std::array<std::uint8_t, kDatOpenPatternLength> pattern = {
        0x6A,
        0x00,
        0x68,
        0x78,
        0x56,
        0x34,
        0x12,
        0x83,
        0xC6,
        0x04,
        0x56,
        0x8B,
        0xCF,
        0xE8,
        0,
        0,
        0,
        0,
    };
    std::copy(pattern.begin(), pattern.end(), image.begin() + offset);
    const std::int32_t displacement = static_cast<std::int32_t>(
        kDatOpenAddress - (kDatOpenCallAddress + 5));
    std::memcpy(image.data() + offset + 14, &displacement, sizeof(displacement));
    if (!LocateDatOpenTarget(image.data(), image.size(), base) || *LocateDatOpenTarget(image.data(), image.size(), base) != kDatOpenAddress)
    {
        return false;
    }
    image[offset + 11] = 0x8A;
    return !LocateDatOpenTarget(image.data(), image.size(), base);
}

} // namespace

int main()
{
    if (!TestPathSafetyAndPrecedence())
    {
        std::cerr << "path safety or precedence failed\n";
        return 1;
    }
    if (!TestMissForwardAndReplacementLifetime())
    {
        std::cerr << "miss forwarding or replacement lifetime failed\n";
        return 2;
    }
    if (!TestLocatorMutation())
    {
        std::cerr << "hook locator mutation failed\n";
        return 3;
    }
    if (!TestProcessLifetimeHookRollback())
    {
        std::cerr << "process-lifetime hook rollback failed\n";
        return 4;
    }
    std::cout << "dat overlay tests passed\n";
    return 0;
}
