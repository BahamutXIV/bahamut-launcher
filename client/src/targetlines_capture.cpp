#include "targetlines_capture.h"

#include <string>

namespace
{

constexpr unsigned int kMaximumRetryPaths = 64u;

std::filesystem::path RetryPath(const std::filesystem::path& requestedPath,
                                DWORD                        processId,
                                unsigned int                 retry)
{
    const std::wstring filename  = requestedPath.filename().wstring();
    const std::wstring extension = requestedPath.extension().wstring();
    const std::wstring stem      = extension.empty() ? filename : filename.substr(0u, filename.size() - extension.size());
    return requestedPath.parent_path() /
           (stem + L".retry-" + std::to_wstring(processId) + L"-" + std::to_wstring(retry) + extension);
}

bool IsCollision(DWORD error)
{
    return error == ERROR_FILE_EXISTS || error == ERROR_ALREADY_EXISTS;
}

bahamut_client::TargetlinesCaptureFile OpenNew(const std::filesystem::path& path)
{
    bahamut_client::TargetlinesCaptureFile result;
    result.handle = CreateFileW(path.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (result.handle != INVALID_HANDLE_VALUE)
    {
        result.path = path;
        return result;
    }
    result.error = GetLastError();
    return result;
}

} // namespace

namespace bahamut_client
{

TargetlinesCaptureFile OpenTargetlinesCapture(const std::filesystem::path& requestedPath)
{
    TargetlinesCaptureFile result = OpenNew(requestedPath);
    if (result.handle != INVALID_HANDLE_VALUE || !IsCollision(result.error))
    {
        SetLastError(result.error);
        return result;
    }

    const DWORD processId = GetCurrentProcessId();
    for (unsigned int retry = 0u; retry < kMaximumRetryPaths; ++retry)
    {
        result = OpenNew(RetryPath(requestedPath, processId, retry));
        if (result.handle != INVALID_HANDLE_VALUE || !IsCollision(result.error))
        {
            SetLastError(result.error);
            return result;
        }
    }

    SetLastError(ERROR_FILE_EXISTS);
    result.error = ERROR_FILE_EXISTS;
    return result;
}

} // namespace bahamut_client
