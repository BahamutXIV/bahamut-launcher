#include "targetlines_capture.h"

#include <Windows.h>

#include <array>
#include <filesystem>
#include <iostream>
#include <string>
#include <vector>

namespace
{

struct TemporaryDirectory
{
    std::filesystem::path              path;
    std::vector<std::filesystem::path> files;

    ~TemporaryDirectory()
    {
        for (const auto& file : files)
        {
            DeleteFileW(file.c_str());
        }
        RemoveDirectoryW(path.c_str());
    }
};

bool Write(HANDLE file, const std::string& bytes)
{
    DWORD written = 0u;
    return WriteFile(file, bytes.data(), static_cast<DWORD>(bytes.size()), &written, nullptr) != FALSE && written == bytes.size();
}

std::string Read(const std::filesystem::path& path)
{
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
    {
        return {};
    }
    std::array<char, 64> bytes{};
    DWORD                read = 0u;
    const bool           ok   = ReadFile(file, bytes.data(), static_cast<DWORD>(bytes.size()), &read, nullptr) != FALSE;
    CloseHandle(file);
    return ok ? std::string(bytes.data(), read) : std::string{};
}

TemporaryDirectory MakeTemporaryDirectory()
{
    wchar_t            buffer[32768]{};
    const DWORD        length = GetTempPathW(static_cast<DWORD>(std::size(buffer)), buffer);
    TemporaryDirectory result;
    if (length == 0u || length >= std::size(buffer))
    {
        return result;
    }
    result.path = std::filesystem::path(buffer) /
                  (L"bahamut-targetlines-capture-" + std::to_wstring(GetCurrentProcessId()) + L"-" + std::to_wstring(GetTickCount64()));
    if (!CreateDirectoryW(result.path.c_str(), nullptr))
    {
        result.path.clear();
    }
    return result;
}

} // namespace

int main()
{
    TemporaryDirectory temporary = MakeTemporaryDirectory();
    if (temporary.path.empty())
    {
        std::cerr << "could not create temporary directory\n";
        return 1;
    }

    const auto freshPath = temporary.path / L"fresh.btlp";
    temporary.files.push_back(freshPath);
    auto fresh = bahamut_client::OpenTargetlinesCapture(freshPath);
    if (fresh.handle == INVALID_HANDLE_VALUE || fresh.path != freshPath || !Write(fresh.handle, "fresh"))
    {
        std::cerr << "new capture did not use the requested writable filename\n";
        return 2;
    }
    CloseHandle(fresh.handle);

    const auto requestedPath = temporary.path / L"retry.btlp";
    temporary.files.push_back(requestedPath);
    HANDLE original = CreateFileW(requestedPath.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (original == INVALID_HANDLE_VALUE || !Write(original, "original"))
    {
        std::cerr << "could not create the pre-existing capture\n";
        return 3;
    }
    CloseHandle(original);

    HANDLE fixedFilenameRetry = CreateFileW(requestedPath.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (fixedFilenameRetry != INVALID_HANDLE_VALUE || GetLastError() != ERROR_FILE_EXISTS)
    {
        if (fixedFilenameRetry != INVALID_HANDLE_VALUE)
        {
            CloseHandle(fixedFilenameRetry);
        }
        std::cerr << "fixed filename collision was not reproduced\n";
        return 4;
    }

    auto first = bahamut_client::OpenTargetlinesCapture(requestedPath);
    if (first.handle == INVALID_HANDLE_VALUE || first.path == requestedPath || !Write(first.handle, "retry-one"))
    {
        std::cerr << "first collision retry did not create a writable sibling\n";
        return 5;
    }
    temporary.files.push_back(first.path);
    CloseHandle(first.handle);

    auto second = bahamut_client::OpenTargetlinesCapture(requestedPath);
    if (second.handle == INVALID_HANDLE_VALUE || second.path == requestedPath || second.path == first.path || !Write(second.handle, "retry-two"))
    {
        std::cerr << "repeated collision retry reused a capture path\n";
        return 6;
    }
    temporary.files.push_back(second.path);
    CloseHandle(second.handle);

    if (Read(requestedPath) != "original")
    {
        std::cerr << "pre-existing capture was modified\n";
        return 7;
    }
    return 0;
}
