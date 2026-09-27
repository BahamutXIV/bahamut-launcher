#include "dat_overlay.h"
#include "fault_guard.h"

#include "runtime_contract.h"

#include <algorithm>
#include <array>
#include <atomic>
#include <cstring>
#include <cwctype>
#include <filesystem>
#include <limits>
#include <new>
#include <string>
#include <system_error>

namespace
{

using namespace bahamut_dat_overlay;

constexpr std::array<unsigned char, kDatOpenPatternLength> kDatOpenPattern = {
    0x6A,
    0x00,
    0x68,
    0x00,
    0x00,
    0x00,
    0x00,
    0x83,
    0xC6,
    0x04,
    0x56,
    0x8B,
    0xCF,
    0xE8,
    0x00,
    0x00,
    0x00,
    0x00,
};

constexpr std::array<bool, kDatOpenPatternLength> kDatOpenMask = {
    true,
    true,
    true,
    false,
    false,
    false,
    false,
    true,
    true,
    true,
    true,
    true,
    true,
    true,
    false,
    false,
    false,
    false,
};

bool MatchesDatOpenPattern(const unsigned char* bytes)
{
    for (std::size_t index = 0; index != kDatOpenPattern.size(); ++index)
    {
        if (kDatOpenMask[index] && bytes[index] != kDatOpenPattern[index])
        {
            return false;
        }
    }
    return true;
}

std::wstring LowerPath(std::filesystem::path path)
{
    std::wstring value = path.lexically_normal().wstring();
    std::replace(value.begin(), value.end(), L'/', L'\\');
    std::transform(value.begin(), value.end(), value.begin(), [](wchar_t character)
                   {
                       return static_cast<wchar_t>(std::towlower(character));
                   });
    return value;
}

bool IsContained(const std::filesystem::path& root,
                 const std::filesystem::path& candidate)
{
    const std::wstring rootText      = LowerPath(root);
    const std::wstring candidateText = LowerPath(candidate);
    if (candidateText == rootText)
    {
        return true;
    }
    return candidateText.size() > rootText.size() && candidateText.compare(0, rootText.size(), rootText) == 0 && candidateText[rootText.size()] == L'\\';
}

bool IsReparsePoint(const std::filesystem::path& path)
{
    const DWORD attributes = GetFileAttributesW(path.c_str());
    return attributes != INVALID_FILE_ATTRIBUTES && (attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
}

bool HasReparseComponent(const std::filesystem::path& root,
                         const std::filesystem::path& candidate)
{
    std::error_code error;
    const auto      relative = candidate.lexically_relative(root);
    if (relative.empty() || relative == std::filesystem::path(L"."))
    {
        return IsReparsePoint(root);
    }

    std::filesystem::path current = root;
    for (const auto& component : relative)
    {
        if (component == std::filesystem::path(L"."))
        {
            continue;
        }
        if (component == std::filesystem::path(L".."))
        {
            return true;
        }
        current /= component;
        const auto status = std::filesystem::symlink_status(current, error);
        if (error)
        {
            return true;
        }
        if (std::filesystem::is_symlink(status) || IsReparsePoint(current))
        {
            return true;
        }
    }
    return false;
}

bool ReadPathData(const DatPathWrapper* wrapper, std::string& value)
{
    if (wrapper == nullptr)
    {
        return false;
    }

    const char* data = nullptr;
    BAHAMUT_FAULT_TRY
    {
        data = wrapper->data;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
    if (data == nullptr)
    {
        return false;
    }

    value.clear();
    value.reserve(128);
    for (std::size_t index = 0; index != 32768; ++index)
    {
        char character = '\0';
        BAHAMUT_FAULT_TRY
        {
            character = data[index];
        }
        BAHAMUT_FAULT_EXCEPT
        {
            value.clear();
            return false;
        }
        if (character == '\0')
        {
            return !value.empty();
        }
        value.push_back(character);
    }
    value.clear();
    return false;
}

MH_STATUS WINAPI MinHookInitialize()
{
    return MH_Initialize();
}

MH_STATUS WINAPI MinHookCreate(LPVOID target, LPVOID detour, LPVOID* original)
{
    return MH_CreateHook(target, detour, original);
}

MH_STATUS WINAPI MinHookEnable(LPVOID target)
{
    return MH_EnableHook(target);
}

MH_STATUS WINAPI MinHookDisable(LPVOID target)
{
    return MH_DisableHook(target);
}

struct DatForwardState
{
    DatResolver   resolver;
    LocalFileOpen original = nullptr;
};

// Published states live for the process lifetime. This keeps an already-entered
// detour safe while MinHook restores the target during shutdown. The trampoline
// itself is retained until process exit because an entered detour may still use it.
std::atomic<DatForwardState*> gDatForwardState{ nullptr };

int ForwardOpen(const DatResolver& resolver, LocalFileOpen original, void* localFile, const DatPathWrapper* path, const char* mode, int retryCount);

} // namespace

namespace bahamut_dat_overlay
{

const HookApi& DefaultHookApi()
{
    static const HookApi api{
        MinHookInitialize,
        MinHookCreate,
        MinHookEnable,
        MinHookDisable,
    };
    return api;
}

std::optional<std::uintptr_t> LocateDatOpenTarget(const std::uint8_t* image,
                                                  std::size_t         imageSize,
                                                  std::uintptr_t      imageBase)
{
    if (image == nullptr || imageSize < kDatOpenPatternLength || imageBase > kDatOpenPatternAddress)
    {
        return std::nullopt;
    }

    const std::uintptr_t expectedOffset = kDatOpenPatternAddress - imageBase;
    std::size_t          matchCount     = 0;
    std::size_t          matchOffset    = 0;
    for (std::size_t offset = 0;
         offset + kDatOpenPatternLength <= imageSize;
         ++offset)
    {
        if (!MatchesDatOpenPattern(image + offset))
        {
            continue;
        }
        ++matchCount;
        matchOffset = offset;
        if (matchCount > 1)
        {
            return std::nullopt;
        }
    }
    if (matchCount != 1 || matchOffset != expectedOffset || matchOffset + kDatOpenPatternLength > imageSize || imageBase + matchOffset + 13 != kDatOpenCallAddress)
    {
        return std::nullopt;
    }

    std::int32_t displacement = 0;
    std::memcpy(&displacement, image + matchOffset + 14, sizeof(displacement));
    const std::uintptr_t callTarget = imageBase + matchOffset + kDatOpenPatternLength + static_cast<std::intptr_t>(displacement);
    if (callTarget != kDatOpenAddress)
    {
        return std::nullopt;
    }
    return callTarget;
}

bool NormalizeDatPath(std::string_view requested, std::string& normalized)
{
    normalized.clear();
    if (requested.empty() || requested.size() > 32768 || requested.front() == '/' || requested.front() == '\\')
    {
        return false;
    }

    std::size_t componentStart = 0;
    for (std::size_t index = 0; index <= requested.size(); ++index)
    {
        const bool atEnd     = index == requested.size();
        const char character = atEnd ? '/' : requested[index];
        if (character != '/' && character != '\\' && !atEnd)
        {
            if (static_cast<unsigned char>(character) < 0x20 || static_cast<unsigned char>(character) >= 0x7f || character == ':' || character == '*' || character == '?' || character == '"' || character == '<' || character == '>' || character == '|')
            {
                return false;
            }
            continue;
        }

        if (index == componentStart)
        {
            return false;
        }
        const std::string_view component = requested.substr(componentStart,
                                                            index - componentStart);
        if (component == "." || component == "..")
        {
            return false;
        }
        if (!normalized.empty())
        {
            normalized.push_back('/');
        }
        normalized.append(component);
        componentStart = index + 1;
    }
    return !normalized.empty();
}

DatResolver::DatResolver(const std::vector<std::filesystem::path>& packageRoots)
: DatResolver(packageRoots, std::filesystem::current_path())
{
}

DatResolver::DatResolver(const std::vector<std::filesystem::path>& packageRoots,
                         const std::filesystem::path&              sourceRoot)
{
    std::error_code sourceError;
    const auto      sourceStatus = std::filesystem::symlink_status(sourceRoot, sourceError);
    if (!sourceError && std::filesystem::is_directory(sourceStatus) && !std::filesystem::is_symlink(sourceStatus) && !IsReparsePoint(sourceRoot))
    {
        const auto canonical = std::filesystem::canonical(sourceRoot, sourceError);
        if (!sourceError && std::filesystem::is_directory(std::filesystem::status(canonical, sourceError)) && !sourceError && !IsReparsePoint(canonical))
        {
            sourceRoot_ = canonical;
        }
    }
    for (const auto& root : packageRoots)
    {
        std::error_code error;
        const auto      status = std::filesystem::symlink_status(root, error);
        if (error || std::filesystem::is_symlink(status) || !std::filesystem::is_directory(status) || IsReparsePoint(root))
        {
            continue;
        }
        const auto canonical = std::filesystem::canonical(root, error);
        if (error || !std::filesystem::is_directory(std::filesystem::status(canonical, error)) || error || IsReparsePoint(canonical))
        {
            continue;
        }
        packageRoots_.push_back(canonical);
    }
}

bool DatResolver::IsConfigured() const
{
    return !packageRoots_.empty();
}

std::optional<std::filesystem::path> DatResolver::Resolve(
    std::string_view requested) const
{
    std::string                 lookup(requested);
    const std::filesystem::path requestedPath(lookup);
    if (requestedPath.is_absolute())
    {
        if (sourceRoot_.empty())
        {
            return std::nullopt;
        }
        std::error_code sourceError;
        const auto      canonical = std::filesystem::weakly_canonical(
            requestedPath, sourceError);
        if (sourceError || !IsContained(sourceRoot_, canonical))
        {
            return std::nullopt;
        }
        lookup = canonical.lexically_relative(sourceRoot_).generic_string();
    }
    std::string normalized;
    if (!NormalizeDatPath(lookup, normalized))
    {
        return std::nullopt;
    }

    const std::filesystem::path relative =
        std::filesystem::path(std::wstring(normalized.begin(), normalized.end()));
    for (const auto& root : packageRoots_)
    {
        const auto      candidate = root / relative;
        std::error_code error;
        const auto      metadata = std::filesystem::symlink_status(candidate, error);
        if (error || !std::filesystem::is_regular_file(metadata) || IsReparsePoint(candidate) || HasReparseComponent(root, candidate))
        {
            continue;
        }
        const auto canonical = std::filesystem::canonical(candidate, error);
        if (error || !IsContained(root, canonical) || HasReparseComponent(root, candidate))
        {
            continue;
        }
        return canonical;
    }
    return std::nullopt;
}

DatOverlay::~DatOverlay()
{
    static_cast<void>(Shutdown());
}

bool DatOverlay::Install(const std::vector<std::filesystem::path>& packageRoots,
                         bool                                      exactBuild,
                         const HookApi&                            api)
{
    if (!Shutdown())
    {
        return false;
    }
    // A created MinHook trampoline is retained until process exit so an
    // already-entered detour can always forward safely. Installation is
    // therefore intentionally single-shot for one runtime instance.
    if (hookTarget_ != nullptr)
    {
        return false;
    }
    resolver_ = DatResolver(packageRoots);
    if (!exactBuild || !resolver_.IsConfigured())
    {
        return true;
    }

    HMODULE module = GetModuleHandleW(nullptr);
    if (module == nullptr || reinterpret_cast<std::uintptr_t>(module) != bahamut_runtime_contract::kRetailImageBase)
    {
        return false;
    }
    const auto* dos = reinterpret_cast<const IMAGE_DOS_HEADER*>(module);
    if (dos->e_magic != IMAGE_DOS_SIGNATURE || dos->e_lfanew < 0 || static_cast<std::uintptr_t>(dos->e_lfanew) > std::numeric_limits<std::size_t>::max() - sizeof(IMAGE_NT_HEADERS32))
    {
        return false;
    }
    const auto* nt = reinterpret_cast<const IMAGE_NT_HEADERS32*>(
        reinterpret_cast<const unsigned char*>(module) + dos->e_lfanew);
    if (nt->Signature != IMAGE_NT_SIGNATURE || nt->OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR32_MAGIC || nt->OptionalHeader.SizeOfImage < kDatOpenPatternLength)
    {
        return false;
    }
    const auto target = LocateDatOpenTarget(
        reinterpret_cast<const std::uint8_t*>(module),
        nt->OptionalHeader.SizeOfImage,
        reinterpret_cast<std::uintptr_t>(module));
    return target && InstallAt(*target, api);
}

bool DatOverlay::InstallAt(std::uintptr_t target, const HookApi& api)
{
    if (api.initialize == nullptr || api.create == nullptr || api.enable == nullptr || api.disable == nullptr)
    {
        return false;
    }
    const MH_STATUS initializeStatus = api.initialize();
    if (initializeStatus != MH_OK && initializeStatus != MH_ERROR_ALREADY_INITIALIZED)
    {
        return false;
    }

    hookApi_                      = api;
    LPVOID          targetPointer = reinterpret_cast<LPVOID>(target);
    LPVOID          original      = nullptr;
    const MH_STATUS createStatus  = api.create(targetPointer,
                                               reinterpret_cast<LPVOID>(&DatOpenDetour),
                                               &original);
    if (createStatus != MH_OK)
    {
        return false;
    }
    hookTarget_ = targetPointer;
    if (original == nullptr)
    {
        return false;
    }
    original_          = reinterpret_cast<LocalFileOpen>(original);
    auto* forwardState = new (std::nothrow) DatForwardState{ resolver_, original_ };
    if (forwardState == nullptr)
    {
        return false;
    }
    gDatForwardState.store(forwardState, std::memory_order_release);
    if (api.enable(hookTarget_) != MH_OK)
    {
        // Treat an indeterminate enable result as live until disable proves
        // otherwise. The immutable forwarding state remains valid for any
        // call that entered while MinHook was changing the target.
        installed_ = true;
        static_cast<void>(Shutdown());
        return false;
    }
    installed_ = true;
    return true;
}

bool DatOverlay::Shutdown()
{
    if (!installed_)
    {
        return true;
    }
    const MH_STATUS disableStatus = hookApi_.disable(hookTarget_);
    if (disableStatus != MH_OK && disableStatus != MH_ERROR_DISABLED)
    {
        return false;
    }
    installed_ = false;
    return true;
}

bool DatOverlay::IsInstalled() const
{
    return installed_;
}

#ifdef BAHAMUT_DAT_OVERLAY_TEST
bool DatOverlay::ConfigureForTesting(
    const std::vector<std::filesystem::path>& packageRoots, LocalFileOpen original)
{
    if (!Shutdown())
    {
        return false;
    }
    resolver_ = DatResolver(packageRoots);
    original_ = original;
    return resolver_.IsConfigured() && original_ != nullptr;
}

bool DatOverlay::InstallTargetForTesting(std::uintptr_t target, const HookApi& api, LocalFileOpen original)
{
    if (!Shutdown())
    {
        return false;
    }
    original_ = original;
    return InstallAt(target, api);
}
#endif

int DatOverlay::Forward(void* localFile, const DatPathWrapper* path, const char* mode, int retryCount)
{
    return ForwardOpen(resolver_, original_, localFile, path, mode, retryCount);
}

} // namespace bahamut_dat_overlay

namespace
{

int ForwardOpen(const DatResolver& resolver, LocalFileOpen original, void* localFile, const DatPathWrapper* path, const char* mode, int retryCount)
{
    if (original == nullptr)
    {
        return 0;
    }
    try
    {
        std::string requested;
        if (!ReadPathData(path, requested))
        {
            return original(localFile, path, mode, retryCount);
        }
        const auto replacement = resolver.Resolve(requested);
        if (!replacement)
        {
            return original(localFile, path, mode, retryCount);
        }

        DatPathWrapper    replacementWrapper{};
        const std::string replacementText = replacement->string();
        if (replacementText.empty() || replacementText.size() + 1 > std::numeric_limits<std::uint32_t>::max())
        {
            return original(localFile, path, mode, retryCount);
        }
        replacementWrapper.data      = replacementText.c_str();
        replacementWrapper.capacity  = static_cast<std::uint32_t>(replacementText.size() + 1);
        replacementWrapper.usedBytes = replacementWrapper.capacity;
        replacementWrapper.heapFlag  = 1;
        return original(localFile, &replacementWrapper, mode, retryCount);
    }
    catch (...)
    {
        return original(localFile, path, mode, retryCount);
    }
}

} // namespace

namespace bahamut_dat_overlay
{

int __fastcall DatOpenDetour(void* localFile, void* unusedEdx, const DatPathWrapper* path, const char* mode, int retryCount)
{
    UNREFERENCED_PARAMETER(unusedEdx);
    DatForwardState* state = gDatForwardState.load(std::memory_order_acquire);
    if (state == nullptr)
    {
        return 0;
    }
    return ForwardOpen(state->resolver, state->original, localFile, path, mode, retryCount);
}

} // namespace bahamut_dat_overlay
