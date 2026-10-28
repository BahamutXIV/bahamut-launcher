#pragma once

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace bahamut_client
{

// Bounded whole-record storage for one addon: <root>/<profile>/<name>.lua.
// Contents are opaque bytes; the caller owns the format and version.
inline constexpr std::size_t kMaximumRecordBytes       = 256u * 1024u;
inline constexpr std::size_t kMaximumRecordsPerProfile = 64u;

enum class RecordStatus
{
    Ok,
    NotFound,
    Invalid,  // profile or record name is not a valid key
    Limit,    // too many records for the profile
    TooLarge, // body exceeds kMaximumRecordBytes
    Io,
};

// Stable lowercase names returned to Lua.
std::string_view RecordStatusName(RecordStatus status);

// Profile and record names are 1-64 characters from [a-z0-9-] and are not a
// reserved Windows device name.
bool IsRecordKey(std::string_view value);

// A server ID is the launcher's ServerProfile::stable_id: 16 lowercase hex digits.
bool IsServerId(std::string_view value);

// "<server id>-<character id as 8 lowercase hex digits>", or nullopt for an
// invalid server ID or a zero character ID.
std::optional<std::string> ComposeProfileKey(std::string_view serverId, std::uint32_t characterId);

class AddonRecordStore
{
public:
    explicit AddonRecordStore(std::filesystem::path root);

    // Writes <name>.tmp beside the record, flushes it, then replaces the
    // record in one operation. Any failure removes the temporary file and
    // leaves the previous record untouched.
    RecordStatus Save(std::string_view profile, std::string_view name, std::string_view body) const;
    RecordStatus Load(std::string_view profile, std::string_view name, std::string& body) const;
    RecordStatus Delete(std::string_view profile, std::string_view name) const;
    // Sorted record names. A directory holding more than the limit returns
    // Limit with the first kMaximumRecordsPerProfile names.
    RecordStatus List(std::string_view profile, std::vector<std::string>& names) const;

private:
    std::filesystem::path root_;
};

} // namespace bahamut_client
