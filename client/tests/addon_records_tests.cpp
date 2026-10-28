#include "addon_records.h"

#include <windows.h>

#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <vector>

namespace
{

using namespace bahamut_client;

bool Expect(bool condition, const char* message)
{
    if (!condition)
    {
        std::cerr << message << '\n';
        return false;
    }
    return true;
}

std::string ReadFile(const std::filesystem::path& path)
{
    std::ifstream file(path, std::ios::binary);
    return std::string(std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>());
}

void WriteFile(const std::filesystem::path& path, const std::string& body)
{
    std::filesystem::create_directories(path.parent_path());
    std::ofstream file(path, std::ios::binary | std::ios::trunc);
    file.write(body.data(), static_cast<std::streamsize>(body.size()));
}

bool TestRoundTripAndReplace(const std::filesystem::path& root)
{
    const AddonRecordStore store(root);
    // Records are opaque bytes: NUL, CR, and high bytes survive unchanged.
    const std::string        first("gearset 1\r\n\0\xff end\n", 18);
    std::string              loaded;
    std::vector<std::string> names;
    bool                     ok = Expect(store.Save("profile-a", "set-1", first) == RecordStatus::Ok, "save should succeed");
    ok                          = Expect(store.Load("profile-a", "set-1", loaded) == RecordStatus::Ok && loaded == first, "load should return the saved bytes") && ok;
    ok                          = Expect(store.Save("profile-a", "set-1", "second") == RecordStatus::Ok && store.Load("profile-a", "set-1", loaded) == RecordStatus::Ok && loaded == "second", "a second save should replace the record") && ok;
    ok                          = Expect(store.Save("profile-a", "set-2", "") == RecordStatus::Ok && store.Load("profile-a", "set-2", loaded) == RecordStatus::Ok && loaded.empty(), "an empty record is allowed") && ok;
    ok                          = Expect(store.List("profile-a", names) == RecordStatus::Ok && names == std::vector<std::string>({ "set-1", "set-2" }), "list should return sorted record names") && ok;
    ok                          = Expect(!std::filesystem::exists(root / "profile-a" / "set-1.tmp"), "a successful save should leave no temporary file") && ok;
    ok                          = Expect(std::filesystem::exists(root / "profile-a" / "set-1.lua") && !std::filesystem::exists(root / "profile-a" / "set-1.rec"), "a record is stored as a .lua file") && ok;
    return ok;
}

bool TestKeyValidation(const std::filesystem::path& root)
{
    const AddonRecordStore store(root);
    std::string            loaded;
    bool                   ok = true;
    for (const char* key : { "", "Upper", "a.b", "a/b", "a\\b", "..", "a b", "con", "nul", "com1", "lpt9", "prn", "aux" })
    {
        ok = Expect(!IsRecordKey(key), "a malformed or reserved key should be rejected") && ok;
        ok = Expect(store.Save("profile-v", key, "x") == RecordStatus::Invalid, "save should reject a malformed record name") && ok;
        ok = Expect(store.Save(key, "set-1", "x") == RecordStatus::Invalid, "save should reject a malformed profile") && ok;
        ok = Expect(store.Load("profile-v", key, loaded) == RecordStatus::Invalid, "load should reject a malformed record name") && ok;
    }
    ok = Expect(!IsRecordKey(std::string(65, 'a')) && IsRecordKey(std::string(64, 'a')), "keys are limited to 64 characters") && ok;
    ok = Expect(IsRecordKey("set-12") && IsRecordKey("com10") && IsRecordKey("0123abcd-ef"), "ordinary keys should be accepted") && ok;
    ok = Expect(!std::filesystem::exists(root / "profile-v"), "rejected saves should create nothing") && ok;
    return ok;
}

bool TestProfileKeyComposition()
{
    const auto key = ComposeProfileKey("0123456789abcdef", 0x12u);
    bool       ok  = Expect(key == "0123456789abcdef-00000012" && IsRecordKey(*key), "a profile key joins the server ID and the 8-digit character ID");
    ok             = Expect(ComposeProfileKey("0123456789abcdef", 0xFFFFFFFFu) == "0123456789abcdef-ffffffff", "the largest character ID keeps eight digits") && ok;
    ok             = Expect(ComposeProfileKey("0123456789abcdef", 0x12u) != ComposeProfileKey("0123456789abcdef", 0x13u) &&
                                ComposeProfileKey("0123456789abcdef", 0x12u) != ComposeProfileKey("fedcba9876543210", 0x12u),
                            "another character or server gives another key") &&
                     ok;
    ok             = Expect(!ComposeProfileKey("0123456789abcdef", 0u), "character ID 0 gives no key") && ok;
    for (const char* serverId : { "", "0123456789abcde", "0123456789abcdef0", "0123456789ABCDEF", "0123456789abcdeg", "0123456789abcde-" })
    {
        ok = Expect(!IsServerId(serverId) && !ComposeProfileKey(serverId, 0x12u), "a malformed server ID gives no key") && ok;
    }
    return ok;
}

// Gear Sets files are hand-editable text; the limits are part of the format.
static_assert(kMaximumRecordBytes == 262144u && kMaximumRecordsPerProfile == 64u);

bool TestSizeAndCountLimits(const std::filesystem::path& root)
{
    const AddonRecordStore store(root);
    std::string            loaded;
    bool                   ok = Expect(store.Save("profile-l", "set-1", "prior") == RecordStatus::Ok, "setup save");
    ok                        = Expect(store.Save("profile-l", "set-1", std::string(kMaximumRecordBytes + 1, 'x')) == RecordStatus::TooLarge && store.Load("profile-l", "set-1", loaded) == RecordStatus::Ok && loaded == "prior", "an oversized save should fail and keep the prior record") && ok;
    ok                        = Expect(store.Save("profile-l", "set-1", std::string(kMaximumRecordBytes, 'x')) == RecordStatus::Ok, "the maximum size should be accepted") && ok;
    WriteFile(root / "profile-l" / "set-9.lua", std::string(kMaximumRecordBytes + 1, 'x'));
    ok = Expect(store.Load("profile-l", "set-9", loaded) == RecordStatus::TooLarge, "an oversized stored file should report TooLarge") && ok;

    for (std::size_t index = 0; index < kMaximumRecordsPerProfile; ++index)
    {
        const std::string name = "set-" + std::to_string(100 + index);
        if (store.Save("profile-c", name, "x") != RecordStatus::Ok)
        {
            std::cerr << "filling the profile should succeed up to the limit\n";
            return false;
        }
    }
    ok = Expect(store.Save("profile-c", "set-1", "x") == RecordStatus::Limit, "a new record past the limit should be refused") && ok;
    ok = Expect(store.Save("profile-c", "set-100", "changed") == RecordStatus::Ok, "replacing an existing record at the limit should work") && ok;
    ok = Expect(store.Save("profile-d", "set-1", "x") == RecordStatus::Ok, "the limit is per profile") && ok;
    return ok;
}

bool TestProfileIsolation(const std::filesystem::path& root)
{
    const AddonRecordStore   store(root);
    std::string              loaded;
    std::vector<std::string> names;
    bool                     ok = Expect(store.Save("server-a-char-1", "set-1", "first character") == RecordStatus::Ok, "setup save a");
    ok                          = Expect(store.Save("server-a-char-2", "set-1", "second character") == RecordStatus::Ok && store.Save("server-a-char-2", "set-2", "extra") == RecordStatus::Ok, "setup save b") && ok;
    ok                          = Expect(store.Load("server-a-char-1", "set-1", loaded) == RecordStatus::Ok && loaded == "first character", "profile one should read its own record") && ok;
    ok                          = Expect(store.Load("server-a-char-2", "set-1", loaded) == RecordStatus::Ok && loaded == "second character", "profile two should read its own record") && ok;
    ok                          = Expect(store.Load("server-a-char-1", "set-2", loaded) == RecordStatus::NotFound, "profile one should not see profile two's record") && ok;
    ok                          = Expect(store.List("server-a-char-1", names) == RecordStatus::Ok && names.size() == 1, "profile one should list one record") && ok;
    ok                          = Expect(store.List("server-a-char-2", names) == RecordStatus::Ok && names.size() == 2, "profile two should list two records") && ok;
    ok                          = Expect(store.List("server-b-char-1", names) == RecordStatus::Ok && names.empty(), "an unseen profile should list nothing") && ok;
    ok                          = Expect(store.Load("server-b-char-1", "set-1", loaded) == RecordStatus::NotFound, "an unseen profile should have no records") && ok;
    return ok;
}

bool TestFailedReplaceKeepsPriorRecord(const std::filesystem::path& root)
{
    const AddonRecordStore store(root);
    const auto             record = root / "profile-f" / "set-1.lua";
    std::string            loaded;
    bool                   ok = Expect(store.Save("profile-f", "set-1", "prior") == RecordStatus::Ok, "setup save");

    // A reader that does not share delete access makes the final replace fail
    // after the temporary file was written completely.
    HANDLE held = CreateFileW(record.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    ok          = Expect(held != INVALID_HANDLE_VALUE, "the test should open the record") && ok;
    ok          = Expect(store.Save("profile-f", "set-1", "replacement") == RecordStatus::Io, "a failed replace should report Io") && ok;
    ok          = Expect(store.Delete("profile-f", "set-1") == RecordStatus::Io && ReadFile(record) == "prior", "a failed delete preserves bytes") && ok;
    if (held != INVALID_HANDLE_VALUE)
    {
        CloseHandle(held);
    }
    ok = Expect(ReadFile(record) == "prior", "a failed replace should keep the prior bytes") && ok;
    ok = Expect(!std::filesystem::exists(root / "profile-f" / "set-1.tmp"), "a failed replace should remove its temporary file") && ok;
    ok = Expect(store.Save("profile-f", "set-1", "replacement") == RecordStatus::Ok && store.Load("profile-f", "set-1", loaded) == RecordStatus::Ok && loaded == "replacement", "saving should work again once the record is free") && ok;

    // A temporary file that cannot be created fails before anything changes.
    std::filesystem::create_directories(root / "profile-f" / "set-1.tmp");
    ok = Expect(store.Save("profile-f", "set-1", "third") == RecordStatus::Io, "an unwritable temporary path should report Io") && ok;
    ok = Expect(ReadFile(record) == "replacement", "an unwritable temporary path should keep the prior bytes") && ok;
    std::filesystem::remove(root / "profile-f" / "set-1.tmp");

    // A temporary file left by a crash is neither listed nor trusted.
    WriteFile(root / "profile-f" / "set-1.tmp", "partial");
    std::vector<std::string> names;
    ok = Expect(store.List("profile-f", names) == RecordStatus::Ok && names == std::vector<std::string>({ "set-1" }), "a stale temporary file should not be listed") && ok;
    ok = Expect(store.Save("profile-f", "set-1", "after crash") == RecordStatus::Ok && ReadFile(record) == "after crash" && !std::filesystem::exists(root / "profile-f" / "set-1.tmp"), "a save should replace a stale temporary file") && ok;
    ok = Expect(store.Delete("profile-f", "../set-1") == RecordStatus::Invalid, "delete validates keys") && ok;
    ok = Expect(store.Delete("profile-other", "set-1") == RecordStatus::NotFound && ReadFile(record) == "after crash", "delete is profile scoped") && ok;
    ok = Expect(store.Delete("profile-f", "set-1") == RecordStatus::Ok && !std::filesystem::exists(record), "delete removes only the selected record") && ok;
    return ok;
}

bool TestListIgnoresForeignFiles(const std::filesystem::path& root)
{
    const AddonRecordStore store(root);
    WriteFile(root / "profile-g" / "set-1.lua", "a");
    WriteFile(root / "profile-g" / "notes.txt", "b");
    WriteFile(root / "profile-g" / "UPPER.lua", "c");
    WriteFile(root / "profile-g" / "con.lua", "d");
    WriteFile(root / "profile-g" / "old.rec", "e");
    std::filesystem::create_directories(root / "profile-g" / "set-2.lua");
    std::vector<std::string> names;
    return Expect(store.List("profile-g", names) == RecordStatus::Ok && names == std::vector<std::string>({ "set-1" }), "list should return only valid regular .lua record files") && Expect(store.List("Bad Profile", names) == RecordStatus::Invalid && names.empty(), "list should reject a malformed profile");
}

} // namespace

int wmain(int argc, wchar_t** argv)
{
    if (argc != 2)
    {
        std::cerr << "usage: addon-records-tests <scratch-directory>\n";
        return 2;
    }
    const std::filesystem::path root = std::filesystem::path(argv[1]) / "addon-records";
    std::error_code             error;
    std::filesystem::remove_all(root, error);
    const bool ok = TestRoundTripAndReplace(root / "round") && TestKeyValidation(root / "keys") && TestProfileKeyComposition() && TestSizeAndCountLimits(root / "limits") && TestProfileIsolation(root / "profiles") && TestFailedReplaceKeepsPriorRecord(root / "atomic") && TestListIgnoresForeignFiles(root / "foreign");
    return ok ? 0 : 1;
}
