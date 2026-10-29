#include "addon_records.h"

#include <windows.h>

#include <algorithm>
#include <array>
#include <cstdint>
#include <cstdio>
#include <fstream>
#include <system_error>
#include <utility>

namespace bahamut_client
{

namespace
{

constexpr std::size_t kMaximumKeyLength = 64;

bool IsReservedDeviceName(std::string_view value)
{
    constexpr std::array<std::string_view, 4> kBase = { "con", "prn", "aux", "nul" };
    if (std::find(kBase.begin(), kBase.end(), value) != kBase.end())
    {
        return true;
    }
    return value.size() == 4 && (value.starts_with("com") || value.starts_with("lpt")) && value[3] >= '1' && value[3] <= '9';
}

std::filesystem::path RecordPath(const std::filesystem::path& directory, std::string_view name, const char* extension)
{
    return directory / (std::filesystem::path(std::string(name)) += extension);
}

std::size_t CountRecords(const std::filesystem::path& directory)
{
    std::error_code error;
    std::size_t     count = 0;
    for (std::filesystem::directory_iterator it(directory, error), end; !error && it != end; it.increment(error))
    {
        const auto& path = it->path();
        if (path.extension() == ".lua" && IsRecordKey(path.stem().string()) && it->is_regular_file(error))
        {
            ++count;
        }
    }
    return count;
}

bool WriteFlushed(const std::filesystem::path& path, std::string_view body)
{
    HANDLE file = CreateFileW(path.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
    {
        return false;
    }
    bool        ok      = true;
    std::size_t written = 0;
    while (ok && written < body.size())
    {
        DWORD       chunk = 0;
        const DWORD want  = static_cast<DWORD>(std::min<std::size_t>(body.size() - written, 1u << 20));
        ok                = WriteFile(file, body.data() + written, want, &chunk, nullptr) != FALSE && chunk != 0;
        written += chunk;
    }
    ok = ok && FlushFileBuffers(file) != FALSE;
    ok = CloseHandle(file) != FALSE && ok;
    return ok;
}

} // namespace

std::string_view RecordStatusName(RecordStatus status)
{
    switch (status)
    {
        case RecordStatus::Ok:
            return "ok";
        case RecordStatus::NotFound:
            return "not_found";
        case RecordStatus::Invalid:
            return "invalid";
        case RecordStatus::Limit:
            return "limit";
        case RecordStatus::TooLarge:
            return "too_large";
        case RecordStatus::Io:
            return "io";
    }
    return "io";
}

bool IsRecordKey(std::string_view value)
{
    if (value.empty() || value.size() > kMaximumKeyLength || IsReservedDeviceName(value))
    {
        return false;
    }
    return std::all_of(value.begin(), value.end(), [](unsigned char character)
                       {
                           return (character >= 'a' && character <= 'z') || (character >= '0' && character <= '9') || character == '-';
                       });
}

bool IsServerId(std::string_view value)
{
    return value.size() == 16 && std::all_of(value.begin(), value.end(), [](unsigned char character)
                                             {
                                                 return (character >= 'a' && character <= 'f') || (character >= '0' && character <= '9');
                                             });
}

std::optional<std::string> ComposeProfileKey(std::string_view serverId, std::uint32_t characterId)
{
    if (!IsServerId(serverId) || characterId == 0)
    {
        return std::nullopt;
    }
    char suffix[10]{};
    std::snprintf(suffix, sizeof(suffix), "-%08x", static_cast<unsigned int>(characterId));
    return std::string(serverId) + suffix;
}

AddonRecordStore::AddonRecordStore(std::filesystem::path root)
: root_(std::move(root))
{
}

RecordStatus AddonRecordStore::Save(std::string_view profile, std::string_view name, std::string_view body) const
{
    if (!IsRecordKey(profile) || !IsRecordKey(name))
    {
        return RecordStatus::Invalid;
    }
    if (body.size() > kMaximumRecordBytes)
    {
        return RecordStatus::TooLarge;
    }
    const std::filesystem::path directory = root_ / std::filesystem::path(std::string(profile));
    std::error_code             error;
    std::filesystem::create_directories(directory, error);
    if (error)
    {
        return RecordStatus::Io;
    }
    const std::filesystem::path record = RecordPath(directory, name, ".lua");
    const std::filesystem::path temp   = RecordPath(directory, name, ".tmp");
    const bool                  exists = std::filesystem::exists(record, error);
    if (error)
    {
        return RecordStatus::Io;
    }
    if (!exists && CountRecords(directory) >= kMaximumRecordsPerProfile)
    {
        return RecordStatus::Limit;
    }
    if (!WriteFlushed(temp, body))
    {
        DeleteFileW(temp.c_str());
        return RecordStatus::Io;
    }
    if (!MoveFileExW(temp.c_str(), record.c_str(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH))
    {
        DeleteFileW(temp.c_str());
        return RecordStatus::Io;
    }
    return RecordStatus::Ok;
}

RecordStatus AddonRecordStore::Load(std::string_view profile, std::string_view name, std::string& body) const
{
    body.clear();
    if (!IsRecordKey(profile) || !IsRecordKey(name))
    {
        return RecordStatus::Invalid;
    }
    const std::filesystem::path record = RecordPath(root_ / std::filesystem::path(std::string(profile)), name, ".lua");
    std::error_code             error;
    const auto                  kind = std::filesystem::status(record, error);
    if (kind.type() == std::filesystem::file_type::not_found)
    {
        return RecordStatus::NotFound;
    }
    if (error || kind.type() != std::filesystem::file_type::regular)
    {
        return RecordStatus::Io;
    }
    const std::uintmax_t size = std::filesystem::file_size(record, error);
    if (error)
    {
        return RecordStatus::Io;
    }
    if (size > kMaximumRecordBytes)
    {
        return RecordStatus::TooLarge;
    }
    std::ifstream file(record, std::ios::binary);
    if (!file)
    {
        return RecordStatus::Io;
    }
    std::string contents(static_cast<std::size_t>(size), '\0');
    file.read(contents.data(), static_cast<std::streamsize>(contents.size()));
    if (!file || file.gcount() != static_cast<std::streamsize>(contents.size()))
    {
        return RecordStatus::Io;
    }
    body = std::move(contents);
    return RecordStatus::Ok;
}

RecordStatus AddonRecordStore::Delete(std::string_view profile, std::string_view name) const
{
    if (!IsRecordKey(profile) || !IsRecordKey(name))
    {
        return RecordStatus::Invalid;
    }
    const auto record = RecordPath(root_ / std::filesystem::path(std::string(profile)), name, ".lua");
    if (DeleteFileW(record.c_str()))
    {
        return RecordStatus::Ok;
    }
    const DWORD error = GetLastError();
    return error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND ? RecordStatus::NotFound : RecordStatus::Io;
}

RecordStatus AddonRecordStore::List(std::string_view profile, std::vector<std::string>& names) const
{
    names.clear();
    if (!IsRecordKey(profile))
    {
        return RecordStatus::Invalid;
    }
    const std::filesystem::path directory = root_ / std::filesystem::path(std::string(profile));
    std::error_code             error;
    if (!std::filesystem::exists(directory, error))
    {
        return error ? RecordStatus::Io : RecordStatus::Ok;
    }
    for (std::filesystem::directory_iterator it(directory, error), end; !error && it != end; it.increment(error))
    {
        const auto& path = it->path();
        if (path.extension() == ".lua" && IsRecordKey(path.stem().string()) && it->is_regular_file(error))
        {
            names.push_back(path.stem().string());
        }
    }
    if (error)
    {
        names.clear();
        return RecordStatus::Io;
    }
    std::sort(names.begin(), names.end());
    if (names.size() > kMaximumRecordsPerProfile)
    {
        names.resize(kMaximumRecordsPerProfile);
        return RecordStatus::Limit;
    }
    return RecordStatus::Ok;
}

} // namespace bahamut_client
