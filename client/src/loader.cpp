#include <windows.h>

#include <bcrypt.h>
#include <psapi.h>
#include <tlhelp32.h>

#include "process_affinity.h"
#include "runtime_api.h"

#include <algorithm>
#include <array>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <limits>
#include <new>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <vector>

namespace
{

constexpr DWORD       kDefaultReadyTimeoutMs = 1500;
constexpr DWORD       kRemoteLoadTimeoutMs   = 5000;
constexpr std::size_t kMaxImagePatches       = 8;

struct ImagePatch
{
    DWORD                      rva = 0;
    std::vector<unsigned char> bytes;
};

struct Options
{
    std::wstring               clientPath;
    std::wstring               modulePath;
    std::vector<unsigned char> serverUtc;
    std::vector<unsigned char> lobbyHost;
    std::wstring               eventPrefix   = L"Local\\BahamutRuntime_";
    DWORD                      timeoutMs     = kDefaultReadyTimeoutMs;
    bool                       noSignal      = false;
    bool                       testStub      = false;
    bool                       waitForClient = false;
    std::wstring               launchArguments;
    std::wstring               fault;
    std::vector<ImagePatch>    patches;
};

struct ProcessState
{
    HANDLE              process       = nullptr;
    HANDLE              primaryThread = nullptr;
    std::vector<HANDLE> remoteThreads;
    std::vector<void*>  remoteBuffers;
};

void CloseHandleIfSet(HANDLE& handle)
{
    if (handle != nullptr)
    {
        CloseHandle(handle);
        handle = nullptr;
    }
}

bool TerminateAndClose(ProcessState& state, UINT exitCode)
{
    bool terminated = false;
    if (state.process != nullptr)
    {
        TerminateProcess(state.process, exitCode);
        terminated = WaitForSingleObject(state.process, 2000) == WAIT_OBJECT_0;
    }
    if (terminated)
    {
        for (HANDLE thread : state.remoteThreads)
        {
            WaitForSingleObject(thread, 2000);
            CloseHandle(thread);
        }
        state.remoteThreads.clear();
        for (void* buffer : state.remoteBuffers)
        {
            VirtualFreeEx(state.process, buffer, 0, MEM_RELEASE);
        }
        state.remoteBuffers.clear();
        CloseHandleIfSet(state.primaryThread);
        CloseHandleIfSet(state.process);
    }
    return terminated;
}

std::optional<DWORD> ParentProcessId()
{
    HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    if (snapshot == INVALID_HANDLE_VALUE)
    {
        return std::nullopt;
    }
    PROCESSENTRY32W entry{};
    entry.dwSize                          = sizeof(entry);
    const DWORD          currentProcessId = GetCurrentProcessId();
    std::optional<DWORD> parentProcessId;
    if (Process32FirstW(snapshot, &entry))
    {
        do
        {
            if (entry.th32ProcessID == currentProcessId)
            {
                parentProcessId = entry.th32ParentProcessID;
                break;
            }
        } while (Process32NextW(snapshot, &entry));
    }
    CloseHandle(snapshot);
    return parentProcessId;
}

bool TransferProcessHandleToParent(HANDLE process, std::uintptr_t& transferred)
{
    const auto parentProcessId = ParentProcessId();
    if (!parentProcessId || *parentProcessId == 0)
    {
        return false;
    }
    HANDLE parentProcess = OpenProcess(PROCESS_DUP_HANDLE, FALSE, *parentProcessId);
    if (parentProcess == nullptr)
    {
        return false;
    }
    HANDLE     duplicate  = nullptr;
    const BOOL duplicated = DuplicateHandle(GetCurrentProcess(), process, parentProcess, &duplicate, 0, FALSE, DUPLICATE_SAME_ACCESS);
    CloseHandle(parentProcess);
    if (!duplicated || duplicate == nullptr)
    {
        return false;
    }
    transferred = reinterpret_cast<std::uintptr_t>(duplicate);
    return true;
}

// Wait mode's EXIT line is defined in docs/handshake.md.
int WaitForClientExit(HANDLE& process, DWORD processId)
{
    const DWORD wait  = WaitForSingleObject(process, INFINITE);
    DWORD       code  = 0;
    const bool  known = wait == WAIT_OBJECT_0 && GetExitCodeProcess(process, &code) != FALSE;
    CloseHandleIfSet(process);
    if (wait != WAIT_OBJECT_0)
    {
        std::printf("EXIT pid=%lu unconfirmed\n", processId);
        std::fflush(stdout);
        return 1;
    }
    if (!known)
    {
        std::printf("EXIT pid=%lu code_unavailable\n", processId);
        std::fflush(stdout);
        return 1;
    }
    std::printf("EXIT pid=%lu code=%lu\n", processId, code);
    std::fflush(stdout);
    return static_cast<int>(code);
}

void ReleaseRemoteThread(ProcessState& state, HANDLE thread)
{
    const auto found = std::find(state.remoteThreads.begin(), state.remoteThreads.end(), thread);
    if (found != state.remoteThreads.end())
    {
        state.remoteThreads.erase(found);
    }
    CloseHandleIfSet(thread);
}

void ReleaseRemoteBuffer(ProcessState& state, void* buffer)
{
    const auto found = std::find(state.remoteBuffers.begin(), state.remoteBuffers.end(), buffer);
    if (found != state.remoteBuffers.end())
    {
        state.remoteBuffers.erase(found);
    }
    if (state.process != nullptr && buffer != nullptr)
    {
        VirtualFreeEx(state.process, buffer, 0, MEM_RELEASE);
    }
}

std::string Utf8(std::wstring_view value)
{
    if (value.empty())
        return {};
    const int length = WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
    if (length == 0)
        return {};
    std::string result(length, '\0');
    if (WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), result.data(), length, nullptr, nullptr) == 0)
    {
        return {};
    }
    return result;
}

void PrintError(std::string_view code, bool created, DWORD processId = 0, std::string_view detail = {})
{
    if (created)
    {
        std::printf("ERROR %.*s target_terminated pid=%lu",
                    static_cast<int>(code.size()),
                    code.data(),
                    processId);
        std::fflush(stdout);
        if (!detail.empty())
        {
            std::printf(" %.*s", static_cast<int>(detail.size()), detail.data());
        }
        std::printf("\n");
        std::fflush(stdout);
    }
    else
    {
        std::printf("ERROR %.*s no_target_created\n", static_cast<int>(code.size()), code.data());
        std::fflush(stdout);
    }
}

void PrintTerminationFailure(std::string_view cause, DWORD processId)
{
    std::printf("ERROR TargetTerminationFailed termination_unconfirmed pid=%lu cause=%.*s\n",
                processId,
                static_cast<int>(cause.size()),
                cause.data());
    std::fflush(stdout);
}

int HexDigit(wchar_t c)
{
    if (c >= L'0' && c <= L'9')
        return c - L'0';
    if (c >= L'a' && c <= L'f')
        return c - L'a' + 10;
    if (c >= L'A' && c <= L'F')
        return c - L'A' + 10;
    return -1;
}

bool ParseHex(std::wstring_view text, std::vector<unsigned char>& output)
{
    if (text.empty() || text.size() > 128 || (text.size() % 2) != 0)
    {
        return false;
    }

    output.resize(text.size() / 2);
    for (std::size_t i = 0; i < output.size(); ++i)
    {
        const int high = HexDigit(text[i * 2]);
        const int low  = HexDigit(text[i * 2 + 1]);
        if (high < 0 || low < 0)
        {
            return false;
        }
        output[i] = static_cast<unsigned char>((high << 4) | low);
    }
    return true;
}

// --patch <rva>:<bytes>: one to eight hex digits of image RVA, then one to 64 hex bytes.
bool ParsePatch(std::wstring_view text, ImagePatch& patch)
{
    const std::size_t separator = text.find(L':');
    if (separator == std::wstring_view::npos || separator == 0 || separator > 8)
    {
        return false;
    }
    DWORD rva = 0;
    for (const wchar_t c : text.substr(0, separator))
    {
        const int digit = HexDigit(c);
        if (digit < 0)
        {
            return false;
        }
        rva = (rva << 4) | static_cast<DWORD>(digit);
    }
    patch.rva = rva;
    return ParseHex(text.substr(separator + 1), patch.bytes);
}

bool ReadFileBytes(const std::wstring& path, std::vector<unsigned char>& output)
{
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
    {
        return false;
    }

    LARGE_INTEGER size{};
    if (!GetFileSizeEx(file, &size) || size.QuadPart < 0 || size.QuadPart > 64 * 1024 * 1024)
    {
        CloseHandle(file);
        return false;
    }

    output.resize(static_cast<std::size_t>(size.QuadPart));
    DWORD      read = 0;
    const BOOL ok   = output.empty() || ReadFile(file, output.data(), static_cast<DWORD>(output.size()), &read, nullptr);
    CloseHandle(file);
    return ok && read == output.size();
}

bool ComputeSha256(const std::wstring& path, std::array<unsigned char, 32>& output)
{
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
    {
        return false;
    }

    BCRYPT_ALG_HANDLE  algorithm        = nullptr;
    BCRYPT_HASH_HANDLE hash             = nullptr;
    PUCHAR             hashObject       = nullptr;
    DWORD              hashObjectLength = 0;
    DWORD              resultLength     = 0;
    bool               ok               = BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0) == 0 && BCryptGetProperty(algorithm, BCRYPT_OBJECT_LENGTH, reinterpret_cast<PUCHAR>(&hashObjectLength), sizeof(hashObjectLength), &resultLength, 0) == 0;

    if (ok)
    {
        hashObject = new (std::nothrow) UCHAR[hashObjectLength];
        ok         = hashObject != nullptr && BCryptCreateHash(algorithm, &hash, hashObject, hashObjectLength, nullptr, 0, 0) == 0;
    }

    std::array<unsigned char, 64 * 1024> buffer{};
    while (ok)
    {
        DWORD read = 0;
        if (!ReadFile(file, buffer.data(), static_cast<DWORD>(buffer.size()), &read, nullptr))
        {
            ok = false;
            break;
        }
        if (read == 0)
        {
            break;
        }
        ok = BCryptHashData(hash, buffer.data(), read, 0) == 0;
    }

    if (ok)
    {
        ok = BCryptFinishHash(hash, output.data(), static_cast<ULONG>(output.size()), 0) == 0;
    }

    if (hash != nullptr)
        BCryptDestroyHash(hash);
    if (algorithm != nullptr)
        BCryptCloseAlgorithmProvider(algorithm, 0);
    delete[] hashObject;
    CloseHandle(file);
    return ok;
}

std::string Hex(const std::array<unsigned char, 32>& bytes)
{
    constexpr char digits[] = "0123456789abcdef";
    std::string    result;
    result.reserve(64);
    for (const unsigned char value : bytes)
    {
        result.push_back(digits[value >> 4]);
        result.push_back(digits[value & 0x0f]);
    }
    return result;
}

bool IsStubIdentity(const std::wstring& path)
{
    const std::wstring filename = std::filesystem::path(path).filename().wstring();
    if (_wcsicmp(filename.c_str(), L"bahamut-test-client.exe") != 0)
    {
        return false;
    }

    std::vector<unsigned char> bytes;
    if (!ReadFileBytes(path, bytes))
    {
        return false;
    }
    const auto marker = reinterpret_cast<const unsigned char*>(
        bahamut_runtime_contract::kStubMarker);
    const auto markerLength = std::strlen(bahamut_runtime_contract::kStubMarker);
    for (std::size_t i = 0; i + markerLength <= bytes.size(); ++i)
    {
        if (std::memcmp(bytes.data() + i, marker, markerLength) == 0)
        {
            return true;
        }
    }
    return false;
}

bool IsRecordedIdentity(const std::wstring& path)
{
    const std::wstring filename = std::filesystem::path(path).filename().wstring();
    if (_wcsicmp(filename.c_str(), L"ffxivgame.exe") != 0)
    {
        return false;
    }

    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
    {
        return false;
    }
    LARGE_INTEGER size{};
    const bool    lengthMatches = GetFileSizeEx(file, &size) && size.QuadPart == bahamut_runtime_contract::kClientByteLength;
    CloseHandle(file);
    if (!lengthMatches)
    {
        return false;
    }

    std::array<unsigned char, 32> digest{};
    if (!ComputeSha256(path, digest) || Hex(digest) != bahamut_runtime_contract::kClientSha256)
    {
        return false;
    }

    const std::filesystem::path versionPath = std::filesystem::path(path).parent_path() / L"game.ver";
    std::ifstream               version(versionPath);
    std::string                 versionText;
    std::getline(version, versionText);
    return versionText == bahamut_runtime_contract::kGameVersion;
}

bool ValidateIdentity(const std::wstring& path)
{
    return IsRecordedIdentity(path) || IsStubIdentity(path);
}

struct ImageSections
{
    const IMAGE_NT_HEADERS32*   nt       = nullptr;
    const IMAGE_SECTION_HEADER* sections = nullptr;
};

std::optional<ImageSections> ParseImageSections(const std::vector<unsigned char>& bytes)
{
    if (bytes.size() < sizeof(IMAGE_DOS_HEADER))
    {
        return std::nullopt;
    }
    const auto* dos = reinterpret_cast<const IMAGE_DOS_HEADER*>(bytes.data());
    if (dos->e_magic != IMAGE_DOS_SIGNATURE || dos->e_lfanew < 0)
    {
        return std::nullopt;
    }
    const std::size_t ntOffset = static_cast<std::size_t>(dos->e_lfanew);
    if (ntOffset > bytes.size() || bytes.size() - ntOffset < sizeof(DWORD) + sizeof(IMAGE_FILE_HEADER))
    {
        return std::nullopt;
    }
    const auto* nt = reinterpret_cast<const IMAGE_NT_HEADERS32*>(bytes.data() + ntOffset);
    if (nt->Signature != IMAGE_NT_SIGNATURE || nt->FileHeader.SizeOfOptionalHeader < sizeof(IMAGE_OPTIONAL_HEADER32) || bytes.size() - ntOffset < offsetof(IMAGE_NT_HEADERS32, OptionalHeader) + nt->FileHeader.SizeOfOptionalHeader || nt->OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR32_MAGIC)
    {
        return std::nullopt;
    }
    const std::size_t sectionOffset = ntOffset + offsetof(IMAGE_NT_HEADERS32, OptionalHeader) + nt->FileHeader.SizeOfOptionalHeader;
    if (nt->FileHeader.NumberOfSections == 0 || nt->FileHeader.NumberOfSections > (std::numeric_limits<std::size_t>::max() - sectionOffset) / sizeof(IMAGE_SECTION_HEADER) || sectionOffset + static_cast<std::size_t>(nt->FileHeader.NumberOfSections) * sizeof(IMAGE_SECTION_HEADER) > bytes.size())
    {
        return std::nullopt;
    }
    return ImageSections{ nt, reinterpret_cast<const IMAGE_SECTION_HEADER*>(bytes.data() + sectionOffset) };
}

std::optional<DWORD> RvaForExport(const std::wstring& path, const char* requestedName)
{
    if (requestedName == nullptr || *requestedName == '\0')
    {
        return std::nullopt;
    }
    std::vector<unsigned char> bytes;
    if (!ReadFileBytes(path, bytes))
    {
        return std::nullopt;
    }
    const auto image = ParseImageSections(bytes);
    if (!image)
    {
        return std::nullopt;
    }
    const auto* nt        = image->nt;
    const auto  directory = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_EXPORT];
    if (directory.VirtualAddress == 0 || directory.Size == 0)
    {
        return std::nullopt;
    }

    const auto* sections    = image->sections;
    auto        rvaToOffset = [&](DWORD rva) -> std::optional<std::size_t>
    {
        for (WORD index = 0; index < nt->FileHeader.NumberOfSections; ++index)
        {
            const auto& section = sections[index];
            const DWORD size    = std::max(section.Misc.VirtualSize, section.SizeOfRawData);
            if (size != 0 && rva >= section.VirtualAddress && rva - section.VirtualAddress < size)
            {
                const std::size_t delta = static_cast<std::size_t>(rva - section.VirtualAddress);
                if (delta >= section.SizeOfRawData || section.PointerToRawData > bytes.size() || delta > bytes.size() - section.PointerToRawData)
                {
                    return std::nullopt;
                }
                return static_cast<std::size_t>(section.PointerToRawData) + delta;
            }
        }
        return std::nullopt;
    };

    auto rangeOffset = [&](DWORD rva, std::uint64_t length) -> std::optional<std::size_t>
    {
        for (WORD index = 0; index < nt->FileHeader.NumberOfSections; ++index)
        {
            const auto& section     = sections[index];
            const DWORD sectionSize = std::max(section.Misc.VirtualSize, section.SizeOfRawData);
            if (sectionSize == 0 || rva < section.VirtualAddress || rva - section.VirtualAddress >= sectionSize)
            {
                continue;
            }
            const std::size_t delta = static_cast<std::size_t>(rva - section.VirtualAddress);
            if (delta >= section.SizeOfRawData || length > static_cast<std::uint64_t>(section.SizeOfRawData - delta) || section.PointerToRawData > bytes.size())
            {
                return std::nullopt;
            }
            const std::size_t offset = static_cast<std::size_t>(section.PointerToRawData) + delta;
            if (offset > bytes.size() || length > bytes.size() - offset)
            {
                return std::nullopt;
            }
            return offset;
        }
        return std::nullopt;
    };

    if (!rangeOffset(directory.VirtualAddress, directory.Size))
    {
        return std::nullopt;
    }
    const auto exportOffset = rangeOffset(directory.VirtualAddress, sizeof(IMAGE_EXPORT_DIRECTORY));
    if (!exportOffset)
    {
        return std::nullopt;
    }
    const auto* exports         = reinterpret_cast<const IMAGE_EXPORT_DIRECTORY*>(bytes.data() + *exportOffset);
    const auto  namesOffset     = rangeOffset(exports->AddressOfNames,
                                              static_cast<std::uint64_t>(exports->NumberOfNames) * sizeof(DWORD));
    const auto  ordinalsOffset  = rangeOffset(exports->AddressOfNameOrdinals,
                                              static_cast<std::uint64_t>(exports->NumberOfNames) * sizeof(WORD));
    const auto  functionsOffset = rangeOffset(exports->AddressOfFunctions,
                                              static_cast<std::uint64_t>(exports->NumberOfFunctions) * sizeof(DWORD));
    if (!namesOffset || !ordinalsOffset || !functionsOffset)
    {
        return std::nullopt;
    }

    const auto* names     = reinterpret_cast<const DWORD*>(bytes.data() + *namesOffset);
    const auto* ordinals  = reinterpret_cast<const WORD*>(bytes.data() + *ordinalsOffset);
    const auto* functions = reinterpret_cast<const DWORD*>(bytes.data() + *functionsOffset);
    for (DWORD i = 0; i < exports->NumberOfNames; ++i)
    {
        if (ordinals[i] >= exports->NumberOfFunctions)
        {
            return std::nullopt;
        }
        const auto nameOffset = rvaToOffset(names[i]);
        if (!nameOffset || *nameOffset >= bytes.size())
            continue;
        const char*       name      = reinterpret_cast<const char*>(bytes.data() + *nameOffset);
        const std::size_t remaining = bytes.size() - *nameOffset;
        if (std::memchr(name, '\0', remaining) == nullptr)
        {
            return std::nullopt;
        }
        if (std::strncmp(name, requestedName, remaining) == 0)
        {
            if (functions[ordinals[i]] == 0)
            {
                return std::nullopt;
            }
            return functions[ordinals[i]];
        }
    }
    return std::nullopt;
}

bool PatchesFitImage(const std::wstring& path, const std::vector<ImagePatch>& patches)
{
    if (patches.empty())
    {
        return true;
    }
    std::vector<unsigned char> bytes;
    if (!ReadFileBytes(path, bytes))
    {
        return false;
    }
    const auto image = ParseImageSections(bytes);
    if (!image)
    {
        return false;
    }
    return std::all_of(patches.begin(), patches.end(), [&](const ImagePatch& patch)
                       {
                           for (WORD index = 0; index < image->nt->FileHeader.NumberOfSections; ++index)
                           {
                               const auto&         section = image->sections[index];
                               const std::uint64_t extent  = std::max(section.Misc.VirtualSize, section.SizeOfRawData);
                               if (patch.rva >= section.VirtualAddress && static_cast<std::uint64_t>(patch.rva - section.VirtualAddress) + patch.bytes.size() <= extent)
                               {
                                   return true;
                               }
                           }
                           return false;
                       });
}

std::optional<std::uintptr_t> RemoteModuleBase(DWORD processId, const wchar_t* moduleName)
{
    HANDLE process = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, FALSE, processId);
    if (process != nullptr)
    {
        std::array<HMODULE, 256> modules{};
        DWORD                    bytesNeeded = 0;
        if (EnumProcessModulesEx(process, modules.data(), static_cast<DWORD>(modules.size() * sizeof(HMODULE)), &bytesNeeded, LIST_MODULES_ALL))
        {
            const DWORD count = std::min<DWORD>(bytesNeeded / sizeof(HMODULE), modules.size());
            for (DWORD index = 0; index < count; ++index)
            {
                wchar_t name[MAX_PATH]{};
                if (GetModuleBaseNameW(process, modules[index], name, ARRAYSIZE(name)) != 0 && _wcsicmp(name, moduleName) == 0)
                {
                    const auto result = reinterpret_cast<std::uintptr_t>(modules[index]);
                    CloseHandle(process);
                    return result;
                }
            }
        }
        CloseHandle(process);
    }
    HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE32, processId);
    if (snapshot == INVALID_HANDLE_VALUE)
    {
        return std::nullopt;
    }
    MODULEENTRY32W entry{};
    entry.dwSize = sizeof(entry);
    std::optional<std::uintptr_t> result;
    if (Module32FirstW(snapshot, &entry))
    {
        do
        {
            if (_wcsicmp(entry.szModule, moduleName) == 0)
            {
                result = reinterpret_cast<std::uintptr_t>(entry.modBaseAddr);
                break;
            }
        } while (Module32NextW(snapshot, &entry));
    }
    CloseHandle(snapshot);

    // A process held at its initial suspended boundary can temporarily reject
    // module enumeration with ERROR_PARTIAL_COPY. VirtualQueryEx and
    // GetMappedFileNameW are documented process queries that still expose the
    // already mapped image without running its primary thread.
    HANDLE queryProcess = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, FALSE, processId);
    if (queryProcess == nullptr)
    {
        return std::nullopt;
    }
    SYSTEM_INFO systemInfo{};
    GetSystemInfo(&systemInfo);
    auto       address = reinterpret_cast<std::uintptr_t>(systemInfo.lpMinimumApplicationAddress);
    const auto maximum = reinterpret_cast<std::uintptr_t>(systemInfo.lpMaximumApplicationAddress);
    while (address < maximum)
    {
        MEMORY_BASIC_INFORMATION memory{};
        if (VirtualQueryEx(queryProcess, reinterpret_cast<const void*>(address), &memory, sizeof(memory)) == 0)
        {
            break;
        }
        if (memory.State == MEM_COMMIT && memory.Type == MEM_IMAGE)
        {
            wchar_t mappedPath[32768]{};
            if (GetMappedFileNameW(queryProcess, memory.BaseAddress, mappedPath, ARRAYSIZE(mappedPath)) != 0)
            {
                const wchar_t* finalSlash = wcsrchr(mappedPath, L'\\');
                const wchar_t* mappedName = finalSlash == nullptr ? mappedPath : finalSlash + 1;
                if (_wcsicmp(mappedName, moduleName) == 0)
                {
                    const auto mappedBase = reinterpret_cast<std::uintptr_t>(memory.AllocationBase);
                    CloseHandle(queryProcess);
                    return mappedBase;
                }
            }
        }
        const auto next = address + memory.RegionSize;
        if (next <= address)
            break;
        address = next;
    }
    CloseHandle(queryProcess);
    return result;
}

std::optional<std::wstring> NativePathForComparison(const std::wstring& path)
{
    HANDLE file = CreateFileW(path.c_str(), FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file != INVALID_HANDLE_VALUE)
    {
        std::wstring native(32768, L'\0');
        const DWORD  length = GetFinalPathNameByHandleW(file, native.data(), static_cast<DWORD>(native.size()), VOLUME_NAME_NT);
        CloseHandle(file);
        if (length != 0 && length < native.size())
        {
            native.resize(length);
            constexpr std::wstring_view prefix = L"\\\\?\\";
            if (native.rfind(prefix, 0) == 0)
            {
                native.erase(0, prefix.size());
            }
            return native;
        }
    }
    std::error_code             error;
    const std::filesystem::path absolute = std::filesystem::absolute(path, error);
    if (error)
    {
        return std::nullopt;
    }
    const std::wstring value = absolute.lexically_normal().wstring();
    if (value.size() < 2 || value[1] != L':')
    {
        return value;
    }
    wchar_t            device[512]{};
    const std::wstring drive  = value.substr(0, 2);
    const DWORD        length = QueryDosDeviceW(drive.c_str(), device, ARRAYSIZE(device));
    if (length == 0 || length >= ARRAYSIZE(device))
    {
        return std::nullopt;
    }
    return std::wstring(device, length) + value.substr(2);
}

std::optional<std::wstring> DosPathForComparison(const std::wstring& path)
{
    HANDLE file = CreateFileW(path.c_str(), FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
    {
        return std::nullopt;
    }
    std::wstring dos(32768, L'\0');
    const DWORD  length = GetFinalPathNameByHandleW(file, dos.data(), static_cast<DWORD>(dos.size()), VOLUME_NAME_DOS);
    CloseHandle(file);
    if (length == 0 || length >= dos.size())
    {
        return std::nullopt;
    }
    dos.resize(length);
    constexpr std::wstring_view prefix = L"\\\\?\\";
    if (dos.rfind(prefix, 0) == 0)
    {
        dos.erase(0, prefix.size());
    }
    return dos;
}

bool RemoteImageMatches(HANDLE process, std::uintptr_t base, const std::wstring& expectedPath, bool forceMismatch = false)
{
    if (forceMismatch)
    {
        return false;
    }
    MEMORY_BASIC_INFORMATION memory{};
    if (VirtualQueryEx(process, reinterpret_cast<const void*>(base), &memory, sizeof(memory)) == 0 || memory.State != MEM_COMMIT || memory.Type != MEM_IMAGE || reinterpret_cast<std::uintptr_t>(memory.AllocationBase) != base)
    {
        return false;
    }
    wchar_t mappedPath[32768]{};
    if (GetMappedFileNameW(process, memory.AllocationBase, mappedPath, ARRAYSIZE(mappedPath)) == 0)
    {
        return false;
    }
    const auto expectedNativePath = NativePathForComparison(expectedPath);
    if (expectedNativePath && _wcsicmp(mappedPath, expectedNativePath->c_str()) == 0)
    {
        return true;
    }
    // Wine names a mapped image \??\<drive>:\... where Windows uses a volume device path.
    constexpr std::wstring_view winePrefix = L"\\??\\";
    if (std::wstring_view(mappedPath).rfind(winePrefix, 0) != 0)
    {
        return false;
    }
    const auto expectedDosPath = DosPathForComparison(expectedPath);
    return expectedDosPath && _wcsicmp(mappedPath + winePrefix.size(), expectedDosPath->c_str()) == 0;
}

std::optional<std::uintptr_t> RemoteLoadLibraryAddress(HANDLE process, bool allowDirectFallback)
{
    HMODULE localKernel = GetModuleHandleW(L"kernel32.dll");
    if (localKernel == nullptr)
    {
        return std::nullopt;
    }
    FARPROC resolved = GetProcAddress(localKernel, "LoadLibraryW");
    if (resolved == nullptr)
    {
        return std::nullopt;
    }

    // LoadLibraryW may be a forwarder. Resolve the owner of the address in
    // this process, then repeat the same module-name and export-RVA lookup
    // against the target process instead of reusing a process-local pointer.
    HMODULE owner = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                            reinterpret_cast<LPCWSTR>(resolved),
                            &owner))
    {
        return std::nullopt;
    }
    wchar_t     ownerPath[MAX_PATH]{};
    const DWORD pathLength = GetModuleFileNameW(owner, ownerPath, ARRAYSIZE(ownerPath));
    if (pathLength == 0 || pathLength >= ARRAYSIZE(ownerPath))
    {
        return std::nullopt;
    }
    const auto localOwnerBase  = reinterpret_cast<std::uintptr_t>(owner);
    const auto resolvedAddress = reinterpret_cast<std::uintptr_t>(resolved);
    if (resolvedAddress < localOwnerBase)
    {
        return std::nullopt;
    }
    const auto rva        = resolvedAddress - localOwnerBase;
    const auto remoteBase = RemoteModuleBase(GetProcessId(process),
                                             std::filesystem::path(ownerPath).filename().c_str());
    if (remoteBase)
    {
        return *remoteBase + rva;
    }

    // If module names are unavailable at the initial suspended boundary,
    // identify the remote image by its documented memory mapping and the
    // resolved function's code signature at the owner-relative RVA.
    std::array<unsigned char, 16> signature{};
    std::memcpy(signature.data(), reinterpret_cast<const void*>(resolved), signature.size());
    SYSTEM_INFO systemInfo{};
    GetSystemInfo(&systemInfo);
    auto           address        = reinterpret_cast<std::uintptr_t>(systemInfo.lpMinimumApplicationAddress);
    const auto     maximum        = reinterpret_cast<std::uintptr_t>(systemInfo.lpMaximumApplicationAddress);
    std::uintptr_t lastAllocation = 0;
    while (address < maximum)
    {
        MEMORY_BASIC_INFORMATION memory{};
        if (VirtualQueryEx(process, reinterpret_cast<const void*>(address), &memory, sizeof(memory)) == 0)
        {
            break;
        }
        const auto allocation = reinterpret_cast<std::uintptr_t>(memory.AllocationBase);
        if (memory.State == MEM_COMMIT && memory.Type == MEM_IMAGE && allocation != 0 && allocation != lastAllocation)
        {
            lastAllocation = allocation;
            std::array<unsigned char, 16> remoteSignature{};
            SIZE_T                        read = 0;
            if (ReadProcessMemory(process, reinterpret_cast<const void*>(allocation + rva), remoteSignature.data(), remoteSignature.size(), &read) && read == remoteSignature.size() && remoteSignature == signature)
            {
                return allocation + rva;
            }
        }
        const auto next = address + memory.RegionSize;
        if (next <= address)
            break;
        address = next;
    }
    // The direct address is permitted only for the first call at the initial
    // suspended boundary. The helper validates its exact remote module+RVA
    // mapping after the loader thread exits and before the primary resumes.
    return allowDirectFallback ? std::optional<std::uintptr_t>(resolvedAddress) : std::nullopt;
}

bool WriteAndVerify(HANDLE process, std::uintptr_t address, const std::vector<unsigned char>& bytes, bool forceRestoreFailure = false)
{
    if (bytes.empty())
    {
        return false;
    }
    DWORD oldProtection = 0;
    if (!VirtualProtectEx(process, reinterpret_cast<void*>(address), bytes.size(), PAGE_EXECUTE_READWRITE, &oldProtection))
    {
        return false;
    }
    SIZE_T                     written = 0;
    const bool                 wrote   = WriteProcessMemory(process, reinterpret_cast<void*>(address), bytes.data(), bytes.size(), &written) && written == bytes.size();
    std::vector<unsigned char> check(bytes.size());
    SIZE_T                     read     = 0;
    const bool                 verified = wrote && ReadProcessMemory(process, reinterpret_cast<void*>(address), check.data(), check.size(), &read) && read == check.size() && check == bytes;
    const bool                 flushed  = verified && FlushInstructionCache(process,
                                                                            reinterpret_cast<const void*>(address),
                                                                            bytes.size()) != FALSE;
    DWORD                      ignored  = 0;
    const bool                 restored = !forceRestoreFailure && VirtualProtectEx(process,
                                                                                   reinterpret_cast<void*>(address),
                                                                                   bytes.size(),
                                                                                   oldProtection,
                                                                                   &ignored) != FALSE;
    if (!flushed || !restored)
    {
        return false;
    }
    return true;
}

bool ApplyPatch(HANDLE process, const std::wstring& clientPath, const char* exportName, DWORD retailRva, bool testStub, const std::vector<unsigned char>& bytes, bool forceRestoreFailure = false)
{
    const bool serverPatch = std::strcmp(exportName, "gStubServerUtcPatch") == 0;
    if ((serverPatch && bytes.size() != bahamut_runtime_contract::kServerUtcPatchSize) || (!serverPatch && (bytes.empty() || bytes.size() > bahamut_runtime_contract::kLobbyHostPatchSize)) || (std::strcmp(exportName, "gStubLobbyHostPatch") == 0 && (bytes.empty() || bytes.back() != 0)))
    {
        return false;
    }
    const auto rva = testStub ? RvaForExport(clientPath, exportName)
                              : std::optional<DWORD>(retailRva);
    if (!rva)
    {
        return false;
    }
    auto base = RemoteModuleBase(GetProcessId(process),
                                 std::filesystem::path(clientPath).filename().c_str());
    if (!base && testStub)
    {
        // The in-repo stub is linked fixed-base so the test remains usable
        // before the suspended process has exposed a module snapshot.
        base = bahamut_runtime_contract::kRetailImageBase;
    }
    if (!base && !testStub)
    {
        // The recorded retail image has relocations stripped and is fixed at
        // its PE image base, so this fallback remains identity-gated.
        base = bahamut_runtime_contract::kRetailImageBase;
    }
    if (!base)
    {
        return false;
    }
    return WriteAndVerify(process, *base + *rva, bytes, forceRestoreFailure);
}

bool ApplyImagePatches(HANDLE process, const std::wstring& clientPath, const std::vector<ImagePatch>& patches)
{
    if (patches.empty())
    {
        return true;
    }
    // Patch payloads may embed absolute addresses, so they apply only at the
    // fixed image base that both accepted client identities load at.
    const auto base = RemoteModuleBase(GetProcessId(process),
                                       std::filesystem::path(clientPath).filename().c_str())
                          .value_or(bahamut_runtime_contract::kRetailImageBase);
    if (base != bahamut_runtime_contract::kRetailImageBase)
    {
        return false;
    }
    return std::all_of(patches.begin(), patches.end(), [&](const ImagePatch& patch)
                       {
                           return WriteAndVerify(process, base + patch.rva, patch.bytes);
                       });
}

std::wstring EventName(const std::wstring& prefix, const wchar_t* suffix)
{
    return prefix + suffix;
}

bool SetEnvironment(const wchar_t* name, const std::wstring& value)
{
    return SetEnvironmentVariableW(name, value.c_str()) != FALSE;
}

int RunTestFaultProbe(const Options& options)
{
    if (options.fault == L"malformed-export")
    {
        const bool rejected = !RvaForExport(options.modulePath, "BahamutRuntimeInitialize");
        std::printf("%s malformed_export_rejected\n",
                    rejected ? "SUCCESS" : "ERROR");
        return rejected ? 0 : 1;
    }
    if (options.fault == L"remote-module-lookup")
    {
        wchar_t     modulePath[MAX_PATH]{};
        const DWORD pathLength = GetModuleFileNameW(nullptr, modulePath, ARRAYSIZE(modulePath));
        if (pathLength == 0 || pathLength >= ARRAYSIZE(modulePath))
        {
            return 1;
        }
        LARGE_INTEGER frequency{};
        QueryPerformanceFrequency(&frequency);
        std::optional<std::uintptr_t> module;
        double                        elapsedMs = 1000.0;
        for (int attempt = 0; attempt < 3; ++attempt)
        {
            LARGE_INTEGER started{};
            LARGE_INTEGER finished{};
            QueryPerformanceCounter(&started);
            module = RemoteModuleBase(GetCurrentProcessId(),
                                      std::filesystem::path(modulePath).filename().c_str());
            QueryPerformanceCounter(&finished);
            if (frequency.QuadPart > 0)
            {
                const double attemptMs =
                    static_cast<double>(finished.QuadPart - started.QuadPart) * 1000.0 / static_cast<double>(frequency.QuadPart);
                elapsedMs = std::min(elapsedMs, attemptMs);
            }
        }
        std::printf("%s remote_module_lookup elapsed_ms=%.2f\n",
                    module && elapsedMs < 95.0 ? "SUCCESS" : "ERROR",
                    elapsedMs);
        return module && elapsedMs < 95.0 ? 0 : 1;
    }
    return -1;
}

void WriteTelemetrySnapshot(const BahamutRuntimeTelemetry&            telemetry,
                            std::string_view                          terminalCode = {},
                            const BahamutRuntimeBootstrapDiagnostics* diagnostics  = nullptr)
{
    BahamutRuntimeTimingSnapshot timing{};
    const bool                   timingAvailable = BahamutReadTimingSnapshot(telemetry, timing);
    LARGE_INTEGER                frequency{};
    QueryPerformanceFrequency(&frequency);
    const double averageMicroseconds = timing.overlayFrameCount > 0 && frequency.QuadPart > 0
                                           ? static_cast<double>(timing.overlayTotalCounter.QuadPart) * 1000000.0 / static_cast<double>(frequency.QuadPart) / static_cast<double>(timing.overlayFrameCount)
                                           : 0.0;
    const double maximumMicroseconds = frequency.QuadPart > 0
                                           ? static_cast<double>(timing.overlayMaxCounter.QuadPart) * 1000000.0 / static_cast<double>(frequency.QuadPart)
                                           : 0.0;
    const DWORD  needed              = GetEnvironmentVariableW(L"BAHAMUT_RUNTIME_TELEMETRY_FILE", nullptr, 0);
    if (needed == 0)
        return;
    std::wstring path(needed, L'\0');
    const DWORD  copied = GetEnvironmentVariableW(L"BAHAMUT_RUNTIME_TELEMETRY_FILE",
                                                  path.data(),
                                                  needed);
    if (copied == 0 || copied >= needed)
        return;
    path.resize(copied);
    std::ofstream file(std::filesystem::path(path), std::ios::trunc);
    if (!file)
        return;
    file << "timing_snapshot_valid = " << (timingAvailable ? "true" : "false") << "\n"
         << "frame_count = " << timing.frameCount << "\n"
         << "overlay_frame_count = " << timing.overlayFrameCount << "\n"
         << "overlay_average_microseconds = " << averageMicroseconds << "\n"
         << "overlay_maximum_microseconds = " << maximumMicroseconds << "\n"
         << "reset_count = " << telemetry.resetCount << "\n"
         << "input_capture_count = " << telemetry.inputCaptureCount << "\n"
         << "input_forward_count = " << telemetry.inputForwardCount << "\n"
         << "overlay_toggle_count = " << telemetry.overlayToggleCount << "\n";
    if (!terminalCode.empty())
    {
        file << "terminal = true\n"
             << "terminal_code = " << terminalCode << "\n";
        if (terminalCode == "RenderBoundaryFailed")
        {
            file << "render_boundary_stage = " << telemetry.resetCount << "\n";
            if (diagnostics->renderBoundaryOwnerPath[0] != L'\0')
            {
                file << "render_boundary_owner_path = "
                     << std::quoted(Utf8(diagnostics->renderBoundaryOwnerPath)) << "\n";
            }
        }
    }
}

bool ParseOptions(int argc, wchar_t** argv, Options& options)
{
    for (int i = 1; i < argc; ++i)
    {
        const std::wstring_view arg(argv[i]);
        auto                    getValue = [&](const wchar_t* name) -> std::optional<std::wstring_view>
        {
            if (arg != name || i + 1 >= argc)
                return std::nullopt;
            return std::wstring_view(argv[++i]);
        };

        if (const auto client = getValue(L"--client"))
            options.clientPath = *client;
        else if (const auto module = getValue(L"--module"))
            options.modulePath = *module;
        else if (const auto serverUtc = getValue(L"--server-utc"))
        {
            if (!ParseHex(*serverUtc, options.serverUtc))
                return false;
        }
        else if (const auto lobbyHost = getValue(L"--lobby-host"))
        {
            if (!ParseHex(*lobbyHost, options.lobbyHost))
                return false;
        }
        else if (const auto eventPrefix = getValue(L"--event-prefix"))
            options.eventPrefix = *eventPrefix;
        else if (const auto timeout = getValue(L"--timeout-ms"))
        {
            wchar_t*            end    = nullptr;
            const unsigned long parsed = wcstoul(std::wstring(*timeout).c_str(), &end, 10);
            if (end == nullptr || *end != L'\0' || parsed == 0 || parsed > 60000)
                return false;
            options.timeoutMs = static_cast<DWORD>(parsed);
        }
        else if (const auto launchArgument = getValue(L"--launch-argument"))
        {
            options.launchArguments = *launchArgument;
        }
        else if (const auto fault = getValue(L"--fault"))
        {
            options.fault = *fault;
        }
        else if (const auto patch = getValue(L"--patch"))
        {
            ImagePatch parsed;
            if (options.patches.size() >= kMaxImagePatches || !ParsePatch(*patch, parsed))
                return false;
            options.patches.push_back(std::move(parsed));
        }
        else if (arg == L"--no-signal")
            options.noSignal = true;
        else if (arg == L"--test-stub")
            options.testStub = true;
        else if (arg == L"--wait-for-client")
            options.waitForClient = true;
        else
            return false;
    }
    if (!options.testStub && (!options.fault.empty() || options.noSignal || options.eventPrefix != L"Local\\BahamutRuntime_" || (!options.waitForClient && options.timeoutMs != kDefaultReadyTimeoutMs)))
    {
        return false;
    }
    return !options.clientPath.empty() && !options.modulePath.empty() && options.serverUtc.size() == bahamut_runtime_contract::kServerUtcPatchSize && !options.lobbyHost.empty() && options.lobbyHost.size() <= bahamut_runtime_contract::kLobbyHostPatchSize && options.lobbyHost.back() == 0 && std::find(options.lobbyHost.begin(), options.lobbyHost.end() - 1, 0) == options.lobbyHost.end() - 1;
}

} // namespace

int wmain(int argc, wchar_t** argv)
{
    Options options;
    if (!ParseOptions(argc, argv, options))
    {
        std::printf("ERROR InvalidArguments no_target_created\n");
        std::fflush(stdout);
        return 2;
    }
    if (options.waitForClient)
    {
        // The Wine backends poll redirected stdout while the helper waits for the client.
        // Full buffering plus a flush per protocol line keeps each line to one
        // write; the resumed client shares the redirected stream under Wine.
        std::setvbuf(stdout, nullptr, _IOFBF, 1 << 12);
    }

    std::error_code             pathError;
    const std::filesystem::path absoluteClientPath =
        std::filesystem::absolute(options.clientPath, pathError);
    const std::filesystem::path absoluteModulePath =
        std::filesystem::absolute(options.modulePath, pathError);
    if (pathError || absoluteClientPath.empty() || absoluteModulePath.empty())
    {
        PrintError("PathResolutionFailed", false);
        return 1;
    }
    options.clientPath           = absoluteClientPath.lexically_normal().wstring();
    options.modulePath           = absoluteModulePath.lexically_normal().wstring();
    const DWORD moduleAttributes = GetFileAttributesW(options.modulePath.c_str());
    if (moduleAttributes == INVALID_FILE_ATTRIBUTES || (moduleAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0)
    {
        PrintError("MissingRuntimeDll", false);
        return 1;
    }

    if (!ValidateIdentity(options.clientPath))
    {
        PrintError("WrongClientIdentity", false);
        return 1;
    }
    if (!PatchesFitImage(options.clientPath, options.patches))
    {
        PrintError("PatchApplyFailed", false);
        return 1;
    }

    if (!options.fault.empty() && (options.fault == L"malformed-export" || options.fault == L"remote-module-lookup"))
    {
        return RunTestFaultProbe(options);
    }

    if (options.testStub)
    {
        options.eventPrefix = options.eventPrefix.empty()
                                  ? L"Local\\BahamutRuntime_"
                                  : options.eventPrefix;
    }
    else
    {
        options.eventPrefix = L"Local\\BahamutRuntime_" + std::to_wstring(GetCurrentProcessId()) + L"_";
    }

    const std::wstring readyEventName   = EventName(options.eventPrefix, L"_ready");
    const std::wstring entryEventName   = EventName(options.eventPrefix, L"_entry");
    const std::wstring resumeEventName  = EventName(options.eventPrefix, L"_resume");
    const std::wstring frameEventName   = EventName(options.eventPrefix, L"_frame");
    const std::wstring telemetryName    = EventName(options.eventPrefix, L"_telemetry");
    HANDLE             readyEvent       = CreateEventW(nullptr, TRUE, FALSE, readyEventName.c_str());
    HANDLE             frameEvent       = CreateEventW(nullptr, TRUE, FALSE, frameEventName.c_str());
    HANDLE             telemetryMapping = CreateFileMappingW(INVALID_HANDLE_VALUE, nullptr, PAGE_READWRITE, 0, sizeof(BahamutRuntimeSharedState), telemetryName.c_str());
    auto*              sharedState      = telemetryMapping == nullptr ? nullptr
                                                                      : static_cast<BahamutRuntimeSharedState*>(MapViewOfFile(telemetryMapping,
                                                                                                                              FILE_MAP_ALL_ACCESS,
                                                                                                                              0,
                                                                                                                              0,
                                                                                                                              sizeof(BahamutRuntimeSharedState)));
    auto*              telemetry        = sharedState == nullptr ? nullptr : &sharedState->telemetry;
    HANDLE             entryEvent       = options.testStub
                                              ? CreateEventW(nullptr, TRUE, FALSE, entryEventName.c_str())
                                              : nullptr;
    HANDLE             resumeEvent      = options.testStub
                                              ? CreateEventW(nullptr, TRUE, FALSE, resumeEventName.c_str())
                                              : nullptr;
    if (readyEvent == nullptr || frameEvent == nullptr || telemetryMapping == nullptr || telemetry == nullptr || (options.testStub && (entryEvent == nullptr || resumeEvent == nullptr)))
    {
        if (sharedState != nullptr)
            UnmapViewOfFile(sharedState);
        CloseHandleIfSet(telemetryMapping);
        CloseHandleIfSet(frameEvent);
        CloseHandleIfSet(readyEvent);
        CloseHandleIfSet(entryEvent);
        CloseHandleIfSet(resumeEvent);
        PrintError("EventCreateFailed", false);
        return 1;
    }

    bool environmentReady = SetEnvironment(L"BAHAMUT_RUNTIME_READY_EVENT", readyEventName);
    environmentReady      = SetEnvironment(L"BAHAMUT_RUNTIME_FRAME_EVENT", frameEventName) && SetEnvironment(L"BAHAMUT_RUNTIME_TELEMETRY_MAPPING", telemetryName) && SetEnvironment(L"BAHAMUT_RUNTIME_TEST_STUB", options.testStub ? L"1" : L"0") && environmentReady;
    if (options.testStub)
    {
        environmentReady = SetEnvironment(L"BAHAMUT_RUNTIME_ENTRY_EVENT", entryEventName) && environmentReady;
        environmentReady = SetEnvironment(L"BAHAMUT_STUB_EVENT_PREFIX", options.eventPrefix) && environmentReady;
        environmentReady = SetEnvironment(L"BAHAMUT_STUB_EXPECT_SERVER_UTC", [&]()
                                          {
                                              std::wstring      value;
                                              constexpr wchar_t digits[] = L"0123456789abcdef";
                                              for (const unsigned char byte : options.serverUtc)
                                              {
                                                  value.push_back(digits[byte >> 4]);
                                                  value.push_back(digits[byte & 0x0f]);
                                              }
                                              return value;
                                          }()) &&
                           environmentReady;
        environmentReady = SetEnvironment(L"BAHAMUT_STUB_EXPECT_LOBBY_HOST", [&]()
                                          {
                                              std::wstring      value;
                                              constexpr wchar_t digits[] = L"0123456789abcdef";
                                              for (const unsigned char byte : options.lobbyHost)
                                              {
                                                  value.push_back(digits[byte >> 4]);
                                                  value.push_back(digits[byte & 0x0f]);
                                              }
                                              return value;
                                          }()) &&
                           environmentReady;
    }
    else
    {
        environmentReady = SetEnvironment(L"BAHAMUT_RUNTIME_ENTRY_EVENT", L"") && environmentReady;
        environmentReady = SetEnvironment(L"BAHAMUT_STUB_EVENT_PREFIX", L"") && environmentReady;
    }
    environmentReady = SetEnvironment(L"BAHAMUT_RUNTIME_NO_SIGNAL", options.noSignal ? L"1" : L"0") && environmentReady;
    if (!environmentReady)
    {
        UnmapViewOfFile(sharedState);
        CloseHandleIfSet(telemetryMapping);
        CloseHandleIfSet(frameEvent);
        CloseHandleIfSet(readyEvent);
        CloseHandleIfSet(entryEvent);
        CloseHandleIfSet(resumeEvent);
        PrintError("EnvironmentSetupFailed", false);
        return 1;
    }

    std::wstring commandLine = L"\"" + options.clientPath + L"\"";
    if (!options.launchArguments.empty())
    {
        commandLine += L" ";
        commandLine += options.launchArguments;
    }
    std::vector<wchar_t> mutableCommand(commandLine.begin(), commandLine.end());
    mutableCommand.push_back(L'\0');
    const std::wstring workingDirectory =
        std::filesystem::path(options.clientPath).parent_path().wstring();
    STARTUPINFOW startup{};
    startup.cb = sizeof(startup);
    PROCESS_INFORMATION processInfo{};
    ProcessState        state;
    if (!CreateProcessW(options.clientPath.c_str(), mutableCommand.data(), nullptr, nullptr, FALSE, CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT, nullptr, workingDirectory.c_str(), &startup, &processInfo))
    {
        UnmapViewOfFile(sharedState);
        CloseHandleIfSet(telemetryMapping);
        CloseHandleIfSet(frameEvent);
        CloseHandleIfSet(readyEvent);
        CloseHandleIfSet(entryEvent);
        CloseHandleIfSet(resumeEvent);
        PrintError("TargetCreateFailed", false);
        return 1;
    }
    state.process       = processInfo.hProcess;
    state.primaryThread = processInfo.hThread;

    auto postCreateFailure = [&](std::string_view code,
                                 std::string_view detail = {}) -> int
    {
        const bool terminated = TerminateAndClose(state, 1);
        if (telemetry != nullptr)
        {
            WriteTelemetrySnapshot(*telemetry, code, &sharedState->bootstrap);
        }
        UnmapViewOfFile(sharedState);
        CloseHandleIfSet(telemetryMapping);
        CloseHandleIfSet(frameEvent);
        CloseHandleIfSet(readyEvent);
        CloseHandleIfSet(entryEvent);
        CloseHandleIfSet(resumeEvent);
        if (terminated)
        {
            PrintError(code, true, processInfo.dwProcessId, detail);
        }
        else
        {
            PrintTerminationFailure(code, processInfo.dwProcessId);
        }
        return 1;
    };

    try
    {
        DWORD_PTR processMask = 0;
        DWORD_PTR systemMask  = 0;
        if (!GetProcessAffinityMask(state.process, &processMask, &systemMask) || processMask == 0)
        {
            return postCreateFailure("TargetAffinityQueryFailed");
        }
        const DWORD_PTR limitedMask = bahamut_loader::LimitGameProcessorMask(processMask);
        if (limitedMask != processMask && !SetProcessAffinityMask(state.process, limitedMask))
        {
            return postCreateFailure("TargetAffinitySetFailed");
        }

        if (options.fault == L"post-create-exception")
        {
            throw std::runtime_error("test post-create exception");
        }

        if (!ApplyPatch(state.process, options.clientPath, "gStubServerUtcPatch", bahamut_runtime_contract::kRetailServerUtcRva, options.testStub, options.serverUtc, options.fault == L"patch-restore-failure") || !ApplyPatch(state.process, options.clientPath, "gStubLobbyHostPatch", bahamut_runtime_contract::kRetailLobbyHostRva, options.testStub, options.lobbyHost, options.fault == L"patch-restore-failure"))
        {
            return postCreateFailure("PatchApplyFailed");
        }
        if (!ApplyImagePatches(state.process, options.clientPath, options.patches))
        {
            return postCreateFailure("PatchApplyFailed");
        }

        // Wait mode may relax, never tighten, the native remote-thread deadline.
        const DWORD        remoteTimeoutMs = options.waitForClient
                                                 ? std::max(kRemoteLoadTimeoutMs, options.timeoutMs)
                                                 : kRemoteLoadTimeoutMs;
        const std::wstring absoluteModule  = options.modulePath;
        const SIZE_T       modulePathBytes = (absoluteModule.size() + 1) * sizeof(wchar_t);
        void*              remoteBuffer    = VirtualAllocEx(state.process, nullptr, modulePathBytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        if (remoteBuffer == nullptr)
        {
            return postCreateFailure("RemoteAllocFailed");
        }
        state.remoteBuffers.push_back(remoteBuffer);
        SIZE_T written = 0;
        if (!WriteProcessMemory(state.process, remoteBuffer, absoluteModule.c_str(), modulePathBytes, &written) || written != modulePathBytes)
        {
            ReleaseRemoteBuffer(state, remoteBuffer);
            return postCreateFailure("RemoteWriteFailed");
        }

        const auto loadLibraryAddress = RemoteLoadLibraryAddress(state.process, true);
        if (!loadLibraryAddress)
        {
            ReleaseRemoteBuffer(state, remoteBuffer);
            return postCreateFailure("LoadLibraryAddressFailed");
        }

        HANDLE remoteThread = CreateRemoteThread(state.process, nullptr, 0, reinterpret_cast<LPTHREAD_START_ROUTINE>(*loadLibraryAddress), remoteBuffer, 0, nullptr);
        if (remoteThread == nullptr)
        {
            ReleaseRemoteBuffer(state, remoteBuffer);
            return postCreateFailure("RemoteThreadCreateFailed");
        }
        state.remoteThreads.push_back(remoteThread);
        const DWORD loadWait   = options.fault == L"remote-load-timeout"
                                     ? WAIT_TIMEOUT
                                     : WaitForSingleObject(remoteThread, remoteTimeoutMs);
        DWORD       loadResult = 0;
        if (loadWait == WAIT_OBJECT_0)
        {
            GetExitCodeThread(remoteThread, &loadResult);
        }
        if (loadWait == WAIT_OBJECT_0)
        {
            ReleaseRemoteThread(state, remoteThread);
            ReleaseRemoteBuffer(state, remoteBuffer);
        }
        if (options.fault == L"remote-load-timeout" && loadWait == WAIT_TIMEOUT)
        {
            std::printf("DIAG remote_resources_retained threads=%zu buffers=%zu\n",
                        state.remoteThreads.size(),
                        state.remoteBuffers.size());
            std::fflush(stdout);
        }
        if (loadWait != WAIT_OBJECT_0 || loadResult == 0)
        {
            return postCreateFailure("RuntimeLoadFailed");
        }

        const auto confirmedLoadLibraryAddress = RemoteLoadLibraryAddress(state.process, false);
        if (!confirmedLoadLibraryAddress || *confirmedLoadLibraryAddress != *loadLibraryAddress)
        {
            return postCreateFailure("LoadLibraryAddressMismatch");
        }

        const auto runtimeBase = static_cast<std::uintptr_t>(loadResult);
        if (!RemoteImageMatches(state.process, runtimeBase, absoluteModule, options.fault == L"runtime-identity-mismatch"))
        {
            return postCreateFailure("RuntimeInitializeAddressFailed");
        }
        const auto apiVersionRva = RvaForExport(absoluteModule, "BahamutRuntimeApiVersion");
        if (!apiVersionRva)
        {
            return postCreateFailure("RuntimeApiVersionAddressFailed");
        }
        HANDLE apiVersionThread = CreateRemoteThread(state.process, nullptr, 0, reinterpret_cast<LPTHREAD_START_ROUTINE>(runtimeBase + *apiVersionRva), nullptr, 0, nullptr);
        if (apiVersionThread == nullptr)
        {
            return postCreateFailure("RuntimeApiVersionThreadFailed");
        }
        state.remoteThreads.push_back(apiVersionThread);
        const DWORD apiVersionWait   = WaitForSingleObject(apiVersionThread, remoteTimeoutMs);
        DWORD       apiVersionResult = 0;
        if (apiVersionWait == WAIT_OBJECT_0)
        {
            GetExitCodeThread(apiVersionThread, &apiVersionResult);
            ReleaseRemoteThread(state, apiVersionThread);
        }
        if (options.fault == L"runtime-api-version-mismatch")
        {
            ++apiVersionResult;
        }
        if (apiVersionWait != WAIT_OBJECT_0 || apiVersionResult != kBahamutRuntimeApiVersion)
        {
            return postCreateFailure("RuntimeApiVersionMismatch");
        }

        const auto initializeRva = RvaForExport(absoluteModule, "BahamutRuntimeInitialize");
        if (!initializeRva)
        {
            return postCreateFailure("RuntimeInitializeAddressFailed");
        }
        HANDLE initializeThread = CreateRemoteThread(state.process, nullptr, 0, reinterpret_cast<LPTHREAD_START_ROUTINE>(runtimeBase + *initializeRva), nullptr, 0, nullptr);
        if (initializeThread == nullptr)
        {
            return postCreateFailure("RuntimeInitializeThreadFailed");
        }
        state.remoteThreads.push_back(initializeThread);
        const DWORD initializeWait   = WaitForSingleObject(initializeThread, remoteTimeoutMs);
        DWORD       initializeResult = 0;
        if (initializeWait == WAIT_OBJECT_0)
        {
            GetExitCodeThread(initializeThread, &initializeResult);
            ReleaseRemoteThread(state, initializeThread);
        }
        if (initializeWait != WAIT_OBJECT_0 || initializeResult != 1)
        {
            if (initializeResult == 10)
                return postCreateFailure("RuntimeIdentityFailed");
            if (initializeResult == 11)
                return postCreateFailure("RuntimeTelemetryFailed");
            if (initializeResult == 13)
            {
                std::string       detail  = "line=" + std::to_string(sharedState->bootstrap.startupScriptLine);
                const std::string message = Utf8(
                    sharedState->bootstrap.startupScriptMessage);
                if (!message.empty())
                {
                    detail += " message=\"" + message + "\"";
                }
                return postCreateFailure("RuntimeStartupScriptFailed", detail);
            }
            if (initializeResult == 15)
            {
                std::string       detail;
                const std::string pluginId = Utf8(
                    sharedState->bootstrap.nativePluginId);
                const std::string message = Utf8(
                    sharedState->bootstrap.nativePluginMessage);
                if (!pluginId.empty())
                {
                    detail = "plugin=\"" + pluginId + "\"";
                }
                if (!message.empty())
                {
                    if (!detail.empty())
                        detail += " ";
                    detail += "message=\"" + message + "\"";
                }
                return postCreateFailure("RuntimeNativePluginFailed", detail);
            }
            if (initializeResult == 12)
            {
                std::printf("DIAG render_boundary_stage=%ld\n", telemetry->resetCount);
                std::fflush(stdout);
                std::string       detail    = "stage=" + std::to_string(telemetry->resetCount);
                const std::string ownerPath = Utf8(
                    sharedState->bootstrap.renderBoundaryOwnerPath);
                if (!ownerPath.empty())
                {
                    detail += " owner_path=\"" + ownerPath + "\"";
                }
                return postCreateFailure("RenderBoundaryFailed", detail);
            }
            return postCreateFailure("RuntimeInitializeFailed");
        }

        if (WaitForSingleObject(readyEvent, options.timeoutMs) != WAIT_OBJECT_0)
        {
            return postCreateFailure("RuntimeReadyTimeout");
        }

        if (ResumeThread(state.primaryThread) == static_cast<DWORD>(-1))
        {
            return postCreateFailure("TargetResumeFailed");
        }

        // The stub's entry event is set only after its suspended primary thread
        // runs. The helper's resume event is the same named observation for the
        // parent test process.
        if (options.testStub)
        {
            if (WaitForSingleObject(entryEvent, options.timeoutMs) != WAIT_OBJECT_0 || WaitForSingleObject(resumeEvent, options.timeoutMs) != WAIT_OBJECT_0 || WaitForSingleObject(frameEvent, options.timeoutMs) != WAIT_OBJECT_0)
            {
                return postCreateFailure("TargetEntryTimeout");
            }
        }

        const DWORD processWait    = WaitForSingleObject(state.process,
                                                         options.testStub ? options.timeoutMs : kDefaultReadyTimeoutMs);
        DWORD       targetExitCode = STILL_ACTIVE;
        GetExitCodeProcess(state.process, &targetExitCode);
        LARGE_INTEGER frequency{};
        QueryPerformanceFrequency(&frequency);
        BahamutRuntimeTimingSnapshot timing{};
        const bool                   timingAvailable            = BahamutReadTimingSnapshot(*telemetry, timing);
        const LONGLONG               elapsedTicks               = timing.lastFrameCounter.QuadPart - timing.firstFrameCounter.QuadPart;
        const double                 elapsedSeconds             = frequency.QuadPart > 0
                                                                      ? static_cast<double>(elapsedTicks) / static_cast<double>(frequency.QuadPart)
                                                                      : 0.0;
        const LONG                   frameCount                 = timing.frameCount;
        const LONG                   overlayFrameCount          = timing.overlayFrameCount;
        const LONG                   overlayFailureCount        = telemetry->overlayFailureCount;
        const LONG                   resetCount                 = telemetry->resetCount;
        const LONG                   inputCaptureCount          = telemetry->inputCaptureCount;
        const LONG                   inputForwardCount          = telemetry->inputForwardCount;
        const LONG                   overlayToggleCount         = telemetry->overlayToggleCount;
        const double                 overlayAverageMicroseconds = overlayFrameCount > 0 && frequency.QuadPart > 0
                                                                      ? static_cast<double>(timing.overlayTotalCounter.QuadPart) * 1000000.0 / static_cast<double>(frequency.QuadPart) / static_cast<double>(overlayFrameCount)
                                                                      : 0.0;
        const double                 overlayMaximumMicroseconds = frequency.QuadPart > 0
                                                                      ? static_cast<double>(timing.overlayMaxCounter.QuadPart) * 1000000.0 / static_cast<double>(frequency.QuadPart)
                                                                      : 0.0;
        if (!options.testStub)
        {
            WriteTelemetrySnapshot(*telemetry);
        }
        const auto releasePostCreateResources = [&]()
        {
            UnmapViewOfFile(sharedState);
            CloseHandleIfSet(telemetryMapping);
            CloseHandleIfSet(frameEvent);
            CloseHandleIfSet(readyEvent);
            CloseHandleIfSet(entryEvent);
            CloseHandleIfSet(resumeEvent);
        };
        if (!timingAvailable && options.testStub)
        {
            releasePostCreateResources();
            CloseHandleIfSet(state.primaryThread);
            CloseHandleIfSet(state.process);
            std::printf("ERROR TelemetrySnapshotUnavailable target_terminated pid=%lu\n",
                        processInfo.dwProcessId);
            std::fflush(stdout);
            return 1;
        }
        if (options.testStub)
        {
            if (processWait == WAIT_OBJECT_0 && targetExitCode != 0)
            {
                releasePostCreateResources();
                CloseHandleIfSet(state.primaryThread);
                CloseHandleIfSet(state.process);
                std::printf("ERROR StubValidationFailed target_terminated pid=%lu exit=%lu\n",
                            processInfo.dwProcessId,
                            targetExitCode);
                std::fflush(stdout);
                return 1;
            }
            if (frameCount != 120 || overlayFrameCount <= 0 || overlayFailureCount != 0 || resetCount != 2 || inputForwardCount < 4 || overlayToggleCount != 0)
            {
                releasePostCreateResources();
                CloseHandleIfSet(state.primaryThread);
                CloseHandleIfSet(state.process);
                std::printf("ERROR OverlayValidationFailed target_terminated pid=%lu "
                            "frames=%ld overlay_frames=%ld overlay_failures=%ld resets=%ld captured=%ld "
                            "forwarded=%ld toggles=%ld\n",
                            processInfo.dwProcessId,
                            frameCount,
                            overlayFrameCount,
                            overlayFailureCount,
                            resetCount,
                            inputCaptureCount,
                            inputForwardCount,
                            overlayToggleCount);
                std::fflush(stdout);
                return 1;
            }
        }
        std::string processHandleField;
        if (!options.waitForClient)
        {
            std::uintptr_t transferredProcessHandle = 0;
            if (!TransferProcessHandleToParent(state.process, transferredProcessHandle))
            {
                return postCreateFailure("ProcessHandleTransferFailed");
            }
            processHandleField = " process_handle=" + std::to_string(static_cast<unsigned long long>(transferredProcessHandle));
        }
        releasePostCreateResources();
        CloseHandleIfSet(state.primaryThread);
        if (options.testStub)
        {
            std::printf("SUCCESS pid=%lu%s entry_reached ready_signal resume_observed "
                        "frame_boundary_observed frames=%ld rate=%.2f_fps overlay_frames=%ld "
                        "overlay_avg_us=%.2f overlay_max_us=%.2f resets=%ld "
                        "captured=%ld forwarded=%ld toggles=%ld input_routed overlay_returned "
                        "state_preserved patches_verified\n",
                        processInfo.dwProcessId,
                        processHandleField.c_str(),
                        frameCount,
                        elapsedSeconds > 0.0 ? static_cast<double>(frameCount - 1) / elapsedSeconds : 0.0,
                        overlayFrameCount,
                        overlayAverageMicroseconds,
                        overlayMaximumMicroseconds,
                        resetCount,
                        inputCaptureCount,
                        inputForwardCount,
                        overlayToggleCount);
            std::fflush(stdout);
        }
        else
        {
            std::printf("SUCCESS pid=%lu%s ready_signal resumed patches_verified\n",
                        processInfo.dwProcessId,
                        processHandleField.c_str());
            std::fflush(stdout);
        }
        if (!options.waitForClient)
        {
            CloseHandleIfSet(state.process);
            return 0;
        }
        if (options.fault == L"hold-after-success")
        {
            HANDLE release = CreateEventW(nullptr, TRUE, FALSE, EventName(options.eventPrefix, L"_release").c_str());
            if (release != nullptr)
            {
                WaitForSingleObject(release, options.timeoutMs);
                CloseHandle(release);
            }
        }
        return WaitForClientExit(state.process, processInfo.dwProcessId);
    }
    catch (...)
    {
        return postCreateFailure("BootstrapException");
    }
}
