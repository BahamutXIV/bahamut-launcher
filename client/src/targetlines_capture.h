#pragma once

#include <Windows.h>

#include <filesystem>

namespace bahamut_client
{

struct TargetlinesCaptureFile
{
    HANDLE                handle = INVALID_HANDLE_VALUE;
    std::filesystem::path path;
    DWORD                 error = ERROR_SUCCESS;
};

TargetlinesCaptureFile OpenTargetlinesCapture(const std::filesystem::path& requestedPath);

} // namespace bahamut_client
