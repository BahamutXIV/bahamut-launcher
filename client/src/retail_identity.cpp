#include "retail_identity.h"
#include "runtime_contract.h"

#include <windows.h>

#include <bcrypt.h>

#include <algorithm>
#include <array>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <new>
#include <string>
#include <vector>

namespace
{

bool HasValue(const wchar_t* name)
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(name, value, ARRAYSIZE(value));
    return count != 0 && count < ARRAYSIZE(value) && value[0] != L'0';
}

bool ReadFileBytes(const std::wstring& path, std::vector<unsigned char>& output)
{
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
    {
        return false;
    }
    LARGE_INTEGER size{};
    const bool    validSize = GetFileSizeEx(file, &size) && size.QuadPart >= 0 && size.QuadPart <= 64 * 1024 * 1024;
    if (!validSize)
    {
        CloseHandle(file);
        return false;
    }
    output.resize(static_cast<std::size_t>(size.QuadPart));
    DWORD      read = 0;
    const bool ok   = output.empty() || (ReadFile(file, output.data(), static_cast<DWORD>(output.size()), &read, nullptr) && read == output.size());
    CloseHandle(file);
    return ok;
}

bool IsTestStub(const std::wstring& path)
{
    if (!HasValue(L"BAHAMUT_RUNTIME_TEST_STUB") || _wcsicmp(std::filesystem::path(path).filename().c_str(),
                                                            L"bahamut-test-client.exe") != 0)
    {
        return false;
    }
    std::vector<unsigned char> bytes;
    if (!ReadFileBytes(path, bytes))
    {
        return false;
    }
    const auto* marker = reinterpret_cast<const unsigned char*>(
        bahamut_runtime_contract::kStubMarker);
    const std::size_t length = std::char_traits<char>::length(
        bahamut_runtime_contract::kStubMarker);
    for (std::size_t index = 0; index + length <= bytes.size(); ++index)
    {
        if (std::equal(marker, marker + length, bytes.data() + index))
        {
            return true;
        }
    }
    return false;
}

bool ComputeSha256(const std::wstring& path, std::array<unsigned char, 32>& output, std::uint64_t& length)
{
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
    {
        return false;
    }
    LARGE_INTEGER size{};
    if (!GetFileSizeEx(file, &size) || size.QuadPart < 0)
    {
        CloseHandle(file);
        return false;
    }
    length = static_cast<std::uint64_t>(size.QuadPart);

    BCRYPT_ALG_HANDLE  algorithm    = nullptr;
    BCRYPT_HASH_HANDLE hash         = nullptr;
    PUCHAR             object       = nullptr;
    DWORD              objectLength = 0;
    DWORD              resultLength = 0;
    bool               ok           = BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0) == 0 && BCryptGetProperty(algorithm, BCRYPT_OBJECT_LENGTH, reinterpret_cast<PUCHAR>(&objectLength), sizeof(objectLength), &resultLength, 0) == 0;
    if (ok)
    {
        object = new (std::nothrow) UCHAR[objectLength];
        ok     = object != nullptr && BCryptCreateHash(algorithm, &hash, object, objectLength, nullptr, 0, 0) == 0;
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
    delete[] object;
    CloseHandle(file);
    return ok;
}

std::string Hex(const std::array<unsigned char, 32>& bytes)
{
    constexpr char digits[] = "0123456789abcdef";
    std::string    output;
    output.reserve(64);
    for (const unsigned char value : bytes)
    {
        output.push_back(digits[value >> 4]);
        output.push_back(digits[value & 0x0f]);
    }
    return output;
}

} // namespace

bool VerifyRuntimeHostIdentity()
{
    wchar_t     path[32768]{};
    const DWORD length = GetModuleFileNameW(nullptr, path, ARRAYSIZE(path));
    if (length == 0 || length >= ARRAYSIZE(path))
    {
        return false;
    }
    if (IsTestStub(path))
    {
        return true;
    }

    std::array<unsigned char, 32> digest{};
    std::uint64_t                 byteLength = 0;
    if (!ComputeSha256(path, digest, byteLength) || byteLength != bahamut_runtime_contract::kClientByteLength || Hex(digest) != bahamut_runtime_contract::kClientSha256)
    {
        return false;
    }
    std::ifstream version(std::filesystem::path(path).parent_path() / L"game.ver");
    std::string   text;
    std::getline(version, text);
    if (!text.empty() && text.back() == '\r')
    {
        text.pop_back();
    }
    return text == bahamut_runtime_contract::kGameVersion;
}
