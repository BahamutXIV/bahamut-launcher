#include "packet_capture.h"

#include <windows.h>

#include <algorithm>
#include <atomic>
#include <cctype>
#include <charconv>
#include <condition_variable>
#include <cstdio>
#include <cstring>
#include <ctime>
#include <deque>
#include <exception>
#include <fstream>
#include <iterator>
#include <map>
#include <mutex>
#include <random>
#include <set>
#include <string>
#include <string_view>
#include <system_error>
#include <thread>
#include <utility>
#include <vector>

namespace packet_capture
{

namespace
{

using namespace std::chrono_literals;

constexpr std::string_view kHeader            = "capture_unix_ms_utc,direction,size,frame_hex\n";
constexpr std::size_t      kFlushBytes        = 256u * 1024u;
constexpr unsigned int     kMaximumParts      = 9999;
constexpr unsigned int     kMaximumSuffix     = 1000;
constexpr std::size_t      kMaximumNotices    = 8;
constexpr auto             kWakeInterval      = 250ms;
constexpr std::uint64_t    kEmptySessionBytes = 64u * 1024u;
// Room kept in a part for the end row so rotation never exceeds the part cap.
constexpr std::uint64_t kEndRowReserve = 256;
// Retail client build the capture format describes.
constexpr std::string_view kClientVersion = "2012.09.19.0001";

} // namespace

namespace detail
{

enum class ItemKind : std::uint8_t
{
    Frame,
    Zone,
    Split,
    Start,
    Stop,
};

struct CommandSlot
{
    bool done   = false;
    bool result = false;
};

struct QueueItem
{
    ItemKind                              kind      = ItemKind::Frame;
    packet_observer::Direction            direction = packet_observer::Direction::Incoming;
    std::chrono::system_clock::time_point capturedAt;
    std::vector<std::uint8_t>             bytes; // Stub text when `stub` is set.
    bool                                  stub      = false;
    std::size_t                           frameSize = 0; // Original size of a stub frame; zone id for Zone.
    EndReason                             reason    = EndReason::Stop;
    std::shared_ptr<CommandSlot>          slot;
    std::uint64_t                         generation = 0;
};

struct WriterState
{
    std::filesystem::path     directory;
    PacketCaptureLimits       limits;
    std::string               launcher;
    std::uint32_t             runtimeApi = 0;
    std::string               os;
    std::string               wine;
    mutable std::mutex        mutex;
    std::condition_variable   workCv;
    std::condition_variable   doneCv;
    std::deque<QueueItem>     queue;
    std::size_t               queuedFrames = 0;
    std::size_t               queuedBytes  = 0;
    std::atomic<bool>         exit{ false };
    bool                      wake              = false;
    bool                      paused            = false;
    bool                      busy              = false;
    bool                      accepting         = false;
    std::uint64_t             commandGeneration = 0;
    bool                      threadOk          = false;
    PacketCaptureStatus       status;
    std::deque<std::string>   notices;
    std::atomic<int>          injectedFailures{ 0 };
    std::atomic<std::uint8_t> scope{ static_cast<std::uint8_t>(Scope::Standard) };
};

} // namespace detail

namespace
{

using detail::CommandSlot;
using detail::ItemKind;
using detail::QueueItem;
using detail::WriterState;

class FileHandle
{
public:
    FileHandle() = default;

    explicit FileHandle(HANDLE handle)
    : handle_(handle)
    {
    }

    FileHandle(const FileHandle&)            = delete;
    FileHandle& operator=(const FileHandle&) = delete;

    FileHandle(FileHandle&& other) noexcept
    : handle_(std::exchange(other.handle_, INVALID_HANDLE_VALUE))
    {
    }

    FileHandle& operator=(FileHandle&& other) noexcept
    {
        if (this != &other)
        {
            Close();
            handle_ = std::exchange(other.handle_, INVALID_HANDLE_VALUE);
        }
        return *this;
    }

    ~FileHandle()
    {
        Close();
    }

    bool Valid() const
    {
        return handle_ != INVALID_HANDLE_VALUE;
    }

    HANDLE Get() const
    {
        return handle_;
    }

    void Close()
    {
        if (handle_ != INVALID_HANDLE_VALUE)
        {
            CloseHandle(handle_);
            handle_ = INVALID_HANDLE_VALUE;
        }
    }

private:
    HANDLE handle_ = INVALID_HANDLE_VALUE;
};

std::string SessionStem(std::chrono::system_clock::time_point now, unsigned int suffix)
{
    const auto seconds      = std::chrono::system_clock::to_time_t(now);
    const auto milliseconds = std::chrono::duration_cast<std::chrono::milliseconds>(
                                  now.time_since_epoch())
                                  .count() %
                              1000;
    std::tm    utc{};
    char       stamp[32]{};
    if (gmtime_s(&utc, &seconds) != 0 ||
        std::strftime(stamp, sizeof(stamp), "%Y-%m-%dT%H-%M-%S", &utc) == 0)
    {
        return {};
    }
    char name[64]{};
    if (suffix == 0)
    {
        std::snprintf(name, sizeof(name), "capture-%s.%03lldZ", stamp, static_cast<long long>(milliseconds));
    }
    else
    {
        std::snprintf(name, sizeof(name), "capture-%s.%03lldZ-%u", stamp, static_cast<long long>(milliseconds), suffix);
    }
    return name;
}

void AppendNumber(std::string& out, long long value)
{
    char       digits[24];
    const auto result = std::to_chars(digits, digits + sizeof(digits), value);
    out.append(digits, result.ptr);
}

long long Milliseconds(std::chrono::system_clock::time_point at)
{
    return std::chrono::duration_cast<std::chrono::milliseconds>(at.time_since_epoch()).count();
}

// os=windows-<major>.<minor>.<build>. RtlGetVersion is not subject to the
// manifest-based version lie that GetVersionEx applies.
std::string OsField()
{
    using RtlGetVersionFn = LONG(WINAPI*)(OSVERSIONINFOW*);
    OSVERSIONINFOW info{};
    info.dwOSVersionInfoSize = sizeof(info);
    const HMODULE   ntdll    = GetModuleHandleW(L"ntdll.dll");
    FARPROC         proc     = ntdll != nullptr ? GetProcAddress(ntdll, "RtlGetVersion") : nullptr;
    RtlGetVersionFn function = nullptr;
    if (proc != nullptr)
    {
        std::memcpy(&function, &proc, sizeof(function));
    }
    unsigned long major = 0, minor = 0, build = 0;
    if (function != nullptr && function(&info) == 0)
    {
        major = info.dwMajorVersion;
        minor = info.dwMinorVersion;
        build = info.dwBuildNumber;
    }
    char text[48]{};
    std::snprintf(text, sizeof(text), "windows-%lu.%lu.%lu", major, minor, build);
    return text;
}

// wine=<version>|none. Wine's ntdll exports wine_get_version; Windows does not.
std::string WineField()
{
    using WineVersionFn = const char*(__cdecl*)();
    const HMODULE ntdll = GetModuleHandleW(L"ntdll.dll");
    FARPROC       proc  = ntdll != nullptr ? GetProcAddress(ntdll, "wine_get_version") : nullptr;
    if (proc == nullptr)
    {
        return "none";
    }
    WineVersionFn function = nullptr;
    std::memcpy(&function, &proc, sizeof(function));
    const char* version = function();
    std::string text    = version != nullptr ? SanitizeField(version, 64) : std::string();
    return text.empty() ? "none" : text;
}

std::string RandomSessionId()
{
    std::uint64_t value = 0;
    try
    {
        std::random_device device;
        value = (static_cast<std::uint64_t>(device()) << 32u) | static_cast<std::uint64_t>(device());
    }
    catch (const std::exception&)
    {
        LARGE_INTEGER counter{};
        QueryPerformanceCounter(&counter);
        value = static_cast<std::uint64_t>(counter.QuadPart) ^ (GetTickCount64() << 20u);
    }
    char text[24]{};
    std::snprintf(text, sizeof(text), "%08x%08x", static_cast<unsigned int>(value >> 32u), static_cast<unsigned int>(value));
    return text;
}

void AppendMetaRow(std::string& out, long long ms, std::string_view key, std::string_view value)
{
    AppendNumber(out, ms);
    out.append(",meta,0,");
    out.append(key);
    out.push_back('=');
    out.append(value);
    out.push_back('\n');
}

// Header followed by the meta rows that open every part.
void AppendPartPrologue(std::string&                          out,
                        const WriterState&                    state,
                        std::chrono::system_clock::time_point at,
                        std::string_view                      session,
                        unsigned int                          part,
                        Scope                                 scope)
{
    const long long ms = Milliseconds(at);
    out.append(kHeader);
    AppendMetaRow(out, ms, "format", "2");
    AppendMetaRow(out, ms, "session", session);
    AppendMetaRow(out, ms, "part", std::to_string(part));
    AppendMetaRow(out, ms, "launcher", state.launcher);
    AppendMetaRow(out, ms, "runtime_api", std::to_string(state.runtimeApi));
    AppendMetaRow(out, ms, "client", kClientVersion);
    AppendMetaRow(out, ms, "os", state.os);
    AppendMetaRow(out, ms, "wine", state.wine);
    AppendMetaRow(out, ms, "scope", scope == Scope::Full ? "full" : "standard");
}

std::string_view ReasonName(EndReason reason)
{
    switch (reason)
    {
        case EndReason::Stop:
            return "stop";
        case EndReason::Disable:
            return "disable";
        case EndReason::Rotate:
            return "rotate";
        case EndReason::Split:
            return "split";
        case EndReason::Fault:
            return "fault";
    }
    return "stop";
}

void AppendZoneRow(std::string& out, std::chrono::system_clock::time_point at, std::uint32_t zoneId, std::string_view name)
{
    AppendNumber(out, Milliseconds(at));
    out.append(",zone,0,id=");
    AppendNumber(out, static_cast<long long>(zoneId));
    out.append(";name=");
    out.append(name);
    out.push_back('\n');
}

void AppendEndRow(std::string& out, long long ms, EndReason reason, const PacketCaptureStatus& counters)
{
    AppendNumber(out, ms);
    out.append(",end,0,reason=");
    out.append(ReasonName(reason));
    const auto field = [&](const char* name, std::uint64_t value)
    {
        out.push_back(';');
        out.append(name);
        out.push_back('=');
        out.append(std::to_string(value));
    };
    field("captured", counters.captured);
    field("written", counters.written);
    field("skipped", counters.skipped);
    field("dropped", counters.dropped);
    field("queue_high_water", counters.queue_high_water);
    out.push_back('\n');
}

void AppendRow(std::string&                          out,
               packet_observer::Direction            direction,
               std::chrono::system_clock::time_point capturedAt,
               std::span<const std::uint8_t>         frame,
               bool                                  stub,
               std::size_t                           stubFrameSize)
{
    static constexpr char hex[] = "0123456789ABCDEF";
    AppendNumber(out, Milliseconds(capturedAt));
    out.append(direction == packet_observer::Direction::Incoming ? ",incoming," : ",outgoing,");
    AppendNumber(out, static_cast<long long>(stub ? stubFrameSize : frame.size()));
    out.push_back(',');
    if (stub)
    {
        out.append(reinterpret_cast<const char*>(frame.data()), frame.size());
        out.push_back('\n');
        return;
    }
    const std::size_t start = out.size();
    out.resize(start + frame.size() * 2u);
    char* cursor = out.data() + start;
    for (const std::uint8_t byte : frame)
    {
        *cursor++ = hex[byte >> 4u];
        *cursor++ = hex[byte & 0x0fu];
    }
    out.push_back('\n');
}

// Minimal NT declarations: NtCreateFile is the only way to open a child
// relative to a directory handle. It is resolved from ntdll at run time.
using NtStatus = LONG;

struct NtUnicodeString
{
    USHORT Length;
    USHORT MaximumLength;
    PWSTR  Buffer;
};

struct NtObjectAttributes
{
    ULONG            Length;
    HANDLE           RootDirectory;
    NtUnicodeString* ObjectName;
    ULONG            Attributes;
    PVOID            SecurityDescriptor;
    PVOID            SecurityQualityOfService;
};

struct NtIoStatusBlock
{
    ULONG_PTR Status;
    ULONG_PTR Information;
};

using NtCreateFileFunction = NtStatus(NTAPI*)(PHANDLE, ACCESS_MASK, NtObjectAttributes*, NtIoStatusBlock*, PLARGE_INTEGER, ULONG, ULONG, ULONG, ULONG, PVOID, ULONG);

constexpr ULONG kNtFileOpen              = 0x00000001;
constexpr ULONG kNtNonDirectoryFile      = 0x00000040;
constexpr ULONG kNtSynchronousIoNonalert = 0x00000020;
constexpr ULONG kNtOpenReparsePoint      = 0x00200000;
constexpr ULONG kNtCaseInsensitive       = 0x00000040;

NtCreateFileFunction ResolveNtCreateFile()
{
    static const NtCreateFileFunction function = []() -> NtCreateFileFunction
    {
        const HMODULE ntdll = GetModuleHandleW(L"ntdll.dll");
        if (ntdll == nullptr)
        {
            return nullptr;
        }
        return reinterpret_cast<NtCreateFileFunction>(reinterpret_cast<void*>(GetProcAddress(ntdll, "NtCreateFile")));
    }();
    return function;
}

// Holds the capture directory open without FILE_SHARE_DELETE and refuses links.
// Children are opened relative to this handle (OpenChild), so a rename or a
// junction swapped in under the path string can never redirect a delete.
class PinnedDirectory
{
public:
    explicit PinnedDirectory(const std::filesystem::path& directory)
    : handle_(CreateFileW(directory.c_str(), FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, nullptr))
    {
        BY_HANDLE_FILE_INFORMATION info{};
        if (handle_.Valid() &&
            (!GetFileInformationByHandle(handle_.Get(), &info) ||
             (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) == 0 ||
             (info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0))
        {
            handle_.Close();
        }
    }

    bool Valid() const
    {
        return handle_.Valid();
    }

    HANDLE Get() const
    {
        return handle_.Get();
    }

private:
    FileHandle handle_;
};

// Opens a regular file by bare name under the pinned directory without
// following a link. Returns an invalid handle for links, directories and
// anything else that is not a regular file.
FileHandle OpenChild(const PinnedDirectory& directory, std::wstring_view name, ACCESS_MASK access)
{
    const NtCreateFileFunction create = ResolveNtCreateFile();
    if (create == nullptr || !directory.Valid() || name.empty() || name.size() > 255 ||
        name.find_first_of(L"\\/:") != std::wstring_view::npos)
    {
        return {};
    }
    NtUnicodeString    objectName{ static_cast<USHORT>(name.size() * sizeof(wchar_t)), static_cast<USHORT>(name.size() * sizeof(wchar_t)), const_cast<PWSTR>(name.data()) };
    NtObjectAttributes attributes{ sizeof(attributes), directory.Get(), &objectName, kNtCaseInsensitive, nullptr, nullptr };
    NtIoStatusBlock    status{};
    HANDLE             handle = INVALID_HANDLE_VALUE;
    const NtStatus     result = create(&handle, access | SYNCHRONIZE, &attributes, &status, nullptr, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, kNtFileOpen, kNtOpenReparsePoint | kNtNonDirectoryFile | kNtSynchronousIoNonalert, nullptr, 0);
    if (result < 0)
    {
        return {};
    }
    FileHandle                 file(handle);
    BY_HANDLE_FILE_INFORMATION info{};
    if (!GetFileInformationByHandle(file.Get(), &info) ||
        (info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)) != 0)
    {
        return {};
    }
    return file;
}

bool DeleteRegularFile(const PinnedDirectory& directory, std::wstring_view name)
{
    const FileHandle file = OpenChild(directory, name, DELETE | FILE_READ_ATTRIBUTES);
    if (!file.Valid())
    {
        return false;
    }
    // FILE_DISPOSITION_INFO is a single BOOLEAN; its member name collides with the DeleteFile macro.
    static_assert(sizeof(FILE_DISPOSITION_INFO) == sizeof(BOOLEAN));
    BOOLEAN disposition = TRUE;
    return SetFileInformationByHandle(file.Get(), FileDispositionInfo, &disposition, sizeof(disposition)) != 0;
}

// Parts that any writer in this process has open. The set is allocated once
// and never destroyed, so nothing runs for it at DLL detach. Only writer
// threads touch it, and its mutex is never held while waiting on anything
// else, so holding it across a create or a delete is safe.
class ActivePartRegistry
{
public:
    std::unique_lock<std::mutex> Lock()
    {
        return std::unique_lock<std::mutex>(mutex_);
    }

    void AddLocked(const std::filesystem::path& path)
    {
        paths_.insert(Key(path));
    }

    bool ContainsLocked(const std::filesystem::path& path) const
    {
        return paths_.contains(Key(path));
    }

    void Remove(const std::filesystem::path& path)
    {
        const auto lock = Lock();
        paths_.erase(Key(path));
    }

    bool Contains(const std::filesystem::path& path)
    {
        const auto lock = Lock();
        return ContainsLocked(path);
    }

private:
    static std::wstring Key(const std::filesystem::path& path)
    {
        std::error_code error;
        auto            key = std::filesystem::absolute(path, error).lexically_normal().wstring();
        if (!key.empty())
        {
            CharLowerBuffW(key.data(), static_cast<DWORD>(key.size()));
        }
        return key;
    }

    std::mutex             mutex_;
    std::set<std::wstring> paths_;
};

ActivePartRegistry& ActiveParts()
{
    static ActivePartRegistry* registry = new ActivePartRegistry;
    return *registry;
}

// Deletes a capture file unless a writer in this process has it open; the
// registry check and the delete are one step under the registry lock.
bool DeleteUnregistered(const PinnedDirectory& directory, const std::filesystem::path& path)
{
    const auto lock = ActiveParts().Lock();
    return !ActiveParts().ContainsLocked(path) && DeleteRegularFile(directory, path.filename().wstring());
}

struct CaptureName
{
    std::string  stamp; // Fixed-width UTC stamp, so string order is time order.
    unsigned int suffix = 0;
    unsigned int part   = 1;
};

bool IsDigit(char c)
{
    return c >= '0' && c <= '9';
}

// capture-YYYY-MM-DDTHH-MM-SS.mmmZ[-N][-partNNNN].csv, N 1..999, NNNN 0002..9999.
std::optional<CaptureName> ParseCaptureName(std::wstring_view wide)
{
    std::string name;
    for (const wchar_t character : wide)
    {
        if (character <= 0 || character > 0x7e)
        {
            return std::nullopt;
        }
        name.push_back(static_cast<char>(character));
    }
    constexpr std::string_view prefix  = "capture-";
    constexpr std::string_view pattern = "dddd-dd-ddTdd-dd-dd.dddZ";
    if (!name.starts_with(prefix) || name.size() < prefix.size() + pattern.size())
    {
        return std::nullopt;
    }
    std::size_t pos = prefix.size();
    for (std::size_t index = 0; index < pattern.size(); ++index)
    {
        const char expected = pattern[index];
        const char actual   = name[pos + index];
        if (expected == 'd' ? !IsDigit(actual) : actual != expected)
        {
            return std::nullopt;
        }
    }
    CaptureName result;
    result.stamp = name.substr(pos, pattern.size());
    pos += pattern.size();
    if (pos + 1 < name.size() && name[pos] == '-' && IsDigit(name[pos + 1]))
    {
        ++pos;
        const std::size_t start = pos;
        unsigned int      value = 0;
        while (pos < name.size() && IsDigit(name[pos]) && pos - start < 4)
        {
            value = value * 10u + static_cast<unsigned int>(name[pos] - '0');
            ++pos;
        }
        if (pos - start > 3 || name[start] == '0' || value < 1 || value > 999)
        {
            return std::nullopt;
        }
        result.suffix = value;
    }
    if (name.compare(pos, 5, "-part") == 0)
    {
        pos += 5;
        if (name.size() < pos + 4)
        {
            return std::nullopt;
        }
        unsigned int value = 0;
        for (std::size_t index = 0; index < 4; ++index)
        {
            if (!IsDigit(name[pos + index]))
            {
                return std::nullopt;
            }
            value = value * 10u + static_cast<unsigned int>(name[pos + index] - '0');
        }
        if (value < 2)
        {
            return std::nullopt;
        }
        pos += 4;
        result.part = value;
    }
    if (name.compare(pos, std::string::npos, ".csv") != 0)
    {
        return std::nullopt;
    }
    return result;
}

struct DiskFile
{
    std::filesystem::path path;
    std::uint64_t         size = 0;
    unsigned int          part = 1;
};

struct DiskSession
{
    std::vector<DiskFile> files;
};

using SessionKey = std::pair<std::string, unsigned int>;

// Non-recursive and enumerated from the pinned handle. Directories and
// reparse points are never listed.
bool ScanCaptureDirectory(const PinnedDirectory&             pin,
                          const std::filesystem::path&       directory,
                          std::map<SessionKey, DiskSession>& sessions,
                          std::uint64_t&                     total)
{
    std::vector<std::uint64_t> buffer(8192);
    auto                       infoClass = FileIdBothDirectoryRestartInfo;
    for (;;)
    {
        if (!GetFileInformationByHandleEx(pin.Get(), infoClass, buffer.data(), static_cast<DWORD>(buffer.size() * sizeof(std::uint64_t))))
        {
            return GetLastError() == ERROR_NO_MORE_FILES;
        }
        infoClass          = FileIdBothDirectoryInfo;
        const auto* cursor = reinterpret_cast<const std::uint8_t*>(buffer.data());
        for (;;)
        {
            const auto* entry = reinterpret_cast<const FILE_ID_BOTH_DIR_INFO*>(cursor);
            if ((entry->FileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)) == 0)
            {
                const std::wstring name(entry->FileName, entry->FileNameLength / sizeof(wchar_t));
                if (const auto parsed = ParseCaptureName(name))
                {
                    const auto size = static_cast<std::uint64_t>(entry->EndOfFile.QuadPart);
                    sessions[{ parsed->stamp, parsed->suffix }].files.push_back({ directory / name, size, parsed->part });
                    total += size;
                }
            }
            if (entry->NextEntryOffset == 0)
            {
                break;
            }
            cursor += entry->NextEntryOffset;
        }
    }
}

// Reads at most kEmptySessionBytes through the handle-relative open. Anything
// unreadable counts as data so the file is kept.
bool HasDataRow(const PinnedDirectory& directory, std::wstring_view name)
{
    const FileHandle file = OpenChild(directory, name, FILE_READ_DATA | FILE_READ_ATTRIBUTES);
    if (!file.Valid())
    {
        return true;
    }
    std::string text(kEmptySessionBytes, '\0');
    std::size_t length = 0;
    while (length < text.size())
    {
        DWORD read = 0;
        if (!ReadFile(file.Get(), text.data() + length, static_cast<DWORD>(text.size() - length), &read, nullptr))
        {
            return true;
        }
        if (read == 0)
        {
            break;
        }
        length += read;
    }
    text.resize(length);
    return text.find(",incoming,") != std::string::npos || text.find(",outgoing,") != std::string::npos;
}

// Runs on the detached writer thread. Only the shared state is touched under
// the mutex; file I/O happens outside it.
class WriterThread
{
public:
    explicit WriterThread(std::shared_ptr<WriterState> state)
    : state_(std::move(state))
    {
    }

    void Run()
    {
        WriterState& s = *state_;
        for (;;)
        {
            std::deque<QueueItem> batch;
            std::size_t           batchFrames = 0;
            std::size_t           batchBytes  = 0;
            bool                  exiting     = false;
            {
                std::unique_lock lock(s.mutex);
                const auto       ready = [&]
                {
                    return s.exit.load() || (!s.paused && s.wake);
                };
                // Timed even when idle: the destructor notifies without the lock.
                s.workCv.wait_for(lock, kWakeInterval, ready);
                if (s.paused && !s.exit.load())
                {
                    continue;
                }
                exiting = s.exit.load();
                s.wake  = false;
                if (!s.queue.empty())
                {
                    batch.swap(s.queue);
                    s.busy = true;
                }
            }
            if (!batch.empty())
            {
                for (const QueueItem& item : batch)
                {
                    if (item.kind == ItemKind::Frame)
                    {
                        ++batchFrames;
                        batchBytes += item.bytes.size();
                    }
                }
                ProcessBatch(batch);
                Sync(batchFrames, batchBytes, true);
            }
            if (exiting)
            {
                break;
            }
        }
        FlushOrDrop();
        CloseFile();
        active_ = false;
        Sync(0, 0, false);
    }

private:
    void ProcessBatch(std::deque<QueueItem>& batch)
    {
        for (QueueItem& item : batch)
        {
            switch (item.kind)
            {
                case ItemKind::Frame:
                    if (!active_)
                    {
                        ++droppedDelta_;
                    }
                    else
                    {
                        AppendFrame(item);
                    }
                    break;
                case ItemKind::Zone:
                    if (active_)
                    {
                        AppendZone(item);
                    }
                    break;
                case ItemKind::Split:
                {
                    lastGeneration_ = item.generation;
                    bool ok         = active_;
                    if (ok)
                    {
                        ok = Flush() && WriteEndRow(EndReason::Split) && OpenNextPart();
                        if (!ok)
                        {
                            HandleFailure();
                        }
                    }
                    Complete(item.slot, ok);
                    break;
                }
                case ItemKind::Start:
                {
                    lastGeneration_ = item.generation;
                    bool ok         = true;
                    if (!active_)
                    {
                        restarted_ = false;
                        error_.clear();
                        ok = OpenSession();
                        if (!ok)
                        {
                            refuseGeneration_ = lastGeneration_;
                        }
                    }
                    Complete(item.slot, ok);
                    break;
                }
                case ItemKind::Stop:
                {
                    lastGeneration_ = item.generation;
                    const bool was  = active_;
                    if (was)
                    {
                        // A failed flush or end row closes the part with a fault row instead.
                        const bool flushed = Flush() && WriteEndRow(item.reason);
                        if (!flushed)
                        {
                            WriteEndRow(EndReason::Fault);
                        }
                        CloseFile();
                        active_ = false;
                        DiscardEmptySession();
                        if (!flushed)
                        {
                            error_ = "write error";
                            notices_.emplace_back("Packet capture: stopped after a save error");
                        }
                    }
                    Complete(item.slot, was);
                    break;
                }
            }
        }
        if (active_ && !Flush())
        {
            HandleFailure();
        }
    }

    void AppendFrame(const QueueItem& item)
    {
        row_.clear();
        AppendRow(row_, item.direction, item.capturedAt, item.bytes, item.stub, item.frameSize);
        if (!MakeRoom(row_.size()))
        {
            ++droppedDelta_;
            return;
        }
        buffer_.append(row_);
        partBytes_ += row_.size();
        ++rowsInPart_;
        ++bufferRows_;
        if (buffer_.size() >= kFlushBytes && !Flush() && !HandleFailure())
        {
            return;
        }
    }

    // Zone rows are not frames: they are not counted and never make a session
    // non-empty, but they fill the part and so take part in rotation.
    void AppendZone(const QueueItem& item)
    {
        row_.clear();
        AppendZoneRow(row_, item.capturedAt, static_cast<std::uint32_t>(item.frameSize), std::string_view(reinterpret_cast<const char*>(item.bytes.data()), item.bytes.size()));
        if (!MakeRoom(row_.size()))
        {
            return;
        }
        buffer_.append(row_);
        partBytes_ += row_.size();
        ++rowsInPart_;
    }

    // Rotates when the row plus the end row would pass the part cap. Returns
    // false when capture could not continue and the row must be dropped.
    bool MakeRoom(std::size_t rowSize)
    {
        if (rowsInPart_ == 0 || partBytes_ + rowSize + kEndRowReserve <= state_->limits.maxPartBytes)
        {
            return true;
        }
        if (!Flush())
        {
            return HandleFailure();
        }
        if (!WriteEndRow(EndReason::Rotate))
        {
            return HandleFailure();
        }
        return OpenNextPart() || HandleFailure();
    }

    // One end row per part. Returns false when the write failed, which the
    // caller treats as a write error. The fault row is best effort and exempt
    // from injected failures, so recovery cannot recurse.
    bool WriteEndRow(EndReason reason)
    {
        if (endWritten_ || !file_.Valid())
        {
            return true;
        }
        PacketCaptureStatus counters;
        {
            std::scoped_lock lock(state_->mutex);
            counters = state_->status;
        }
        counters.written += writtenDelta_;
        counters.dropped += droppedDelta_;
        std::string row;
        AppendEndRow(row, Milliseconds(std::chrono::system_clock::now()), reason, counters);
        if (!WriteAll(row, reason != EndReason::Fault))
        {
            return false;
        }
        endWritten_ = true;
        partBytes_ += row.size();
        return true;
    }

    bool ConsumeInjectedFailure()
    {
        int pending = state_->injectedFailures.load();
        while (pending > 0)
        {
            if (state_->injectedFailures.compare_exchange_weak(pending, pending - 1))
            {
                return true;
            }
        }
        return false;
    }

    bool WriteAll(std::string_view data, bool injectable = true)
    {
        if (!file_.Valid() || (injectable && ConsumeInjectedFailure()))
        {
            return false;
        }
        while (!data.empty())
        {
            const DWORD chunk   = static_cast<DWORD>(std::min<std::size_t>(data.size(), 1u << 20));
            DWORD       written = 0;
            if (!WriteFile(file_.Get(), data.data(), chunk, &written, nullptr) || written == 0)
            {
                return false;
            }
            data.remove_prefix(written);
            bytesOnDisk_ += written;
        }
        return true;
    }

    // On failure the buffered rows are counted as dropped.
    bool Flush()
    {
        if (buffer_.empty())
        {
            return true;
        }
        const bool ok = WriteAll(buffer_);
        (ok ? writtenDelta_ : droppedDelta_) += bufferRows_;
        if (ok)
        {
            sessionRows_ += bufferRows_;
        }
        buffer_.clear();
        bufferRows_ = 0;
        return ok;
    }

    void FlushOrDrop()
    {
        if (active_)
        {
            Flush();
        }
    }

    // Creating and registering one step: no other writer's retention can see the new part unregistered.
    FileHandle CreateNew(const std::filesystem::path& path, DWORD& error)
    {
        const auto lock   = ActiveParts().Lock();
        HANDLE     handle = CreateFileW(path.c_str(), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_DELETE, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr);
        error             = handle == INVALID_HANDLE_VALUE ? GetLastError() : ERROR_SUCCESS;
        if (handle != INVALID_HANDLE_VALUE)
        {
            ActiveParts().AddLocked(path);
        }
        return FileHandle(handle);
    }

    bool OpenSession()
    {
        CloseFile();
        active_ = false;
        buffer_.clear();
        bufferRows_ = 0;
        std::error_code directoryError;
        std::filesystem::create_directories(state_->directory, directoryError);
        const auto now = std::chrono::system_clock::now();
        for (unsigned int suffix = 0; suffix < kMaximumSuffix; ++suffix)
        {
            const std::string stem = SessionStem(now, suffix);
            if (stem.empty())
            {
                return false;
            }
            const std::filesystem::path path   = state_->directory / (stem + ".csv");
            DWORD                       error  = ERROR_SUCCESS;
            FileHandle                  handle = CreateNew(path, error);
            if (!handle.Valid())
            {
                if (error == ERROR_FILE_EXISTS || error == ERROR_ALREADY_EXISTS)
                {
                    continue;
                }
                return false;
            }
            file_ = std::move(handle);
            RegisterOpenFile(path);
            stem_        = stem;
            path_        = path;
            part_        = 1;
            partBytes_   = 0;
            rowsInPart_  = 0;
            sessionRows_ = 0;
            endWritten_  = false;
            session_     = RandomSessionId();
            std::string prologue;
            AppendPartPrologue(prologue, *state_, now, session_, part_, static_cast<Scope>(state_->scope.load()));
            if (!WriteAll(prologue))
            {
                CloseFile();
                if (const PinnedDirectory pin(state_->directory); pin.Valid())
                {
                    DeleteUnregistered(pin, path);
                }
                stem_.clear();
                return false;
            }
            partBytes_ = prologue.size();
            active_    = true;
            ApplyRetention();
            return true;
        }
        return false;
    }

    void RegisterOpenFile(const std::filesystem::path& path)
    {
        registeredPath_ = path;
    }

    void CloseFile()
    {
        file_.Close();
        if (!registeredPath_.empty())
        {
            ActiveParts().Remove(registeredPath_);
            registeredPath_.clear();
        }
    }

    // The caller has flushed the buffer.
    bool OpenNextPart()
    {
        if (part_ >= kMaximumParts)
        {
            return false;
        }
        char suffix[32]{};
        std::snprintf(suffix, sizeof(suffix), "-part%04u.csv", part_ + 1u);
        const std::filesystem::path path   = state_->directory / (stem_ + suffix);
        DWORD                       error  = ERROR_SUCCESS;
        FileHandle                  handle = CreateNew(path, error);
        if (!handle.Valid())
        {
            return false;
        }
        CloseFile();
        file_ = std::move(handle);
        RegisterOpenFile(path);
        path_ = path;
        ++part_;
        partBytes_  = 0;
        rowsInPart_ = 0;
        endWritten_ = false;
        std::string prologue;
        AppendPartPrologue(prologue, *state_, std::chrono::system_clock::now(), session_, part_, static_cast<Scope>(state_->scope.load()));
        if (!WriteAll(prologue))
        {
            return false;
        }
        partBytes_ = prologue.size();
        ApplyRetention();
        return true;
    }

    // Deletes the session's own parts when it never wrote a data row.
    void DiscardEmptySession()
    {
        if (sessionRows_ != 0 || stem_.empty())
        {
            return;
        }
        CloseFile();
        const PinnedDirectory pin(state_->directory);
        for (unsigned int part = 1; pin.Valid() && part <= part_; ++part)
        {
            char suffix[32]{};
            std::snprintf(suffix, sizeof(suffix), "-part%04u", part);
            const auto      path = state_->directory / (stem_ + (part == 1 ? "" : suffix) + ".csv");
            std::error_code error;
            const auto      size = std::filesystem::file_size(path, error);
            if (!error && DeleteUnregistered(pin, path))
            {
                bytesOnDisk_ -= std::min<std::uint64_t>(size, bytesOnDisk_);
            }
        }
        stem_.clear();
    }

    static bool SessionIsOpen(const DiskSession& session)
    {
        return std::any_of(session.files.begin(), session.files.end(), [](const DiskFile& file)
                           {
                               return ActiveParts().Contains(file.path);
                           });
    }

    bool DeleteCapture(const PinnedDirectory& pin, const DiskFile& file, std::uint64_t& total)
    {
        if (!DeleteUnregistered(pin, file.path))
        {
            if (ActiveParts().Contains(file.path))
            {
                return false;
            }
            ++pruneFailures_;
            return false;
        }
        total -= std::min(file.size, total);
        return true;
    }

    // Deletes only grammar-matching capture files; never the active part.
    void ApplyRetention()
    {
        const PacketCaptureLimits&        limits = state_->limits;
        std::map<SessionKey, DiskSession> sessions;
        std::uint64_t                     total = 0;
        const PinnedDirectory             pin(state_->directory);
        if (!pin.Valid() || !ScanCaptureDirectory(pin, state_->directory, sessions, total))
        {
            return;
        }
        const auto active = ParseCaptureName(path_.filename().wstring());
        if (!active)
        {
            bytesOnDisk_ = total;
            return;
        }
        const SessionKey activeKey{ active->stamp, active->suffix };
        const auto       activeName = path_.filename().wstring();
        // Directory listings lag behind an open file; the writer knows its own size.
        for (DiskFile& file : sessions[activeKey].files)
        {
            if (file.path.filename().wstring() == activeName)
            {
                total -= std::min(file.size, total);
                file.size = partBytes_;
                total += file.size;
            }
        }

        for (auto it = sessions.begin(); it != sessions.end();)
        {
            const auto& files = it->second.files;
            if (it->first != activeKey && files.size() == 1 && files[0].size <= kEmptySessionBytes &&
                !ActiveParts().Contains(files[0].path) && !HasDataRow(pin, files[0].path.filename().wstring()))
            {
                DiskFile file = files[0];
                if (DeleteCapture(pin, file, total))
                {
                    it = sessions.erase(it);
                    continue;
                }
            }
            ++it;
        }

        for (auto it = sessions.begin(); it != sessions.end() &&
                                         (sessions.size() > limits.maxSessions || total > limits.maxTotalBytes);)
        {
            if (it->first == activeKey || SessionIsOpen(it->second))
            {
                ++it;
                continue;
            }
            auto& files = it->second.files;
            for (auto file = files.begin(); file != files.end();)
            {
                file = DeleteCapture(pin, *file, total) ? files.erase(file) : file + 1;
            }
            it = files.empty() ? sessions.erase(it) : std::next(it);
        }

        if (total > limits.maxTotalBytes)
        {
            auto& files = sessions[activeKey].files;
            std::sort(files.begin(), files.end(), [](const DiskFile& a, const DiskFile& b)
                      {
                          return a.part < b.part;
                      });
            for (const DiskFile& file : files)
            {
                if (total <= limits.maxTotalBytes)
                {
                    break;
                }
                if (file.path.filename().wstring() != activeName)
                {
                    DeleteCapture(pin, file, total);
                }
            }
        }
        bytesOnDisk_ = total;
    }

    // Returns whether capture is still active in a fresh session.
    bool HandleFailure()
    {
        buffer_.clear();
        bufferRows_ = 0;
        WriteEndRow(EndReason::Fault);
        CloseFile();
        active_ = false;
        buffer_.clear();
        bufferRows_ = 0;
        DiscardEmptySession();
        if (!restarted_)
        {
            restarted_ = true;
            if (OpenSession())
            {
                notices_.emplace_back("Packet capture: recording again after a save error");
                Sync(0, 0, false);
                return true;
            }
        }
        error_            = "write error";
        refuseGeneration_ = lastGeneration_;
        notices_.emplace_back("Packet capture: stopped after a save error");
        Sync(0, 0, false);
        return false;
    }

    // Caller holds the mutex.
    void ApplyLocked()
    {
        WriterState& s          = *state_;
        s.status.recording      = active_;
        s.status.file           = path_.filename().string();
        s.status.session        = session_;
        s.status.part           = part_;
        s.status.error          = error_;
        s.status.bytes_on_disk  = bytesOnDisk_;
        s.status.prune_failures = pruneFailures_;
        s.status.written += writtenDelta_;
        s.status.dropped += droppedDelta_;
        writtenDelta_ = 0;
        droppedDelta_ = 0;
        // A command queued after the one whose session failed decides acceptance.
        if (refuseGeneration_ && *refuseGeneration_ == s.commandGeneration)
        {
            s.accepting = false;
        }
        refuseGeneration_.reset();
        for (std::string& notice : notices_)
        {
            if (s.notices.size() >= kMaximumNotices)
            {
                s.notices.pop_front();
            }
            s.notices.push_back(std::move(notice));
        }
        notices_.clear();
    }

    void Sync(std::size_t doneFrames, std::size_t doneBytes, bool finishBatch)
    {
        WriterState&     s = *state_;
        std::scoped_lock lock(s.mutex);
        ApplyLocked();
        s.queuedFrames -= doneFrames;
        s.queuedBytes -= doneBytes;
        if (finishBatch)
        {
            s.busy = false;
        }
        s.doneCv.notify_all();
    }

    void Complete(const std::shared_ptr<CommandSlot>& slot, bool result)
    {
        WriterState&     s = *state_;
        std::scoped_lock lock(s.mutex);
        ApplyLocked();
        if (slot != nullptr)
        {
            slot->result = result;
            slot->done   = true;
        }
        s.doneCv.notify_all();
    }

    std::shared_ptr<WriterState> state_;
    FileHandle                   file_;
    std::filesystem::path        path_;
    std::filesystem::path        registeredPath_;
    std::string                  stem_;
    std::string                  buffer_;
    std::string                  row_;
    std::string                  error_;
    std::string                  session_;
    std::vector<std::string>     notices_;
    std::uint64_t                partBytes_     = 0;
    std::uint64_t                rowsInPart_    = 0;
    std::uint64_t                bufferRows_    = 0;
    std::uint64_t                writtenDelta_  = 0;
    std::uint64_t                droppedDelta_  = 0;
    std::uint64_t                sessionRows_   = 0;
    std::uint64_t                bytesOnDisk_   = 0;
    std::uint64_t                pruneFailures_ = 0;
    unsigned int                 part_          = 0;
    bool                         active_        = false;
    bool                         restarted_     = false;
    bool                         endWritten_    = false;
    std::optional<std::uint64_t> refuseGeneration_;
    std::uint64_t                lastGeneration_ = 0;
};

} // namespace

namespace detail
{

bool DeleteCaptureFileForTest(const std::filesystem::path& directory, std::wstring_view name)
{
    const PinnedDirectory pin(directory);
    return pin.Valid() && DeleteRegularFile(pin, name);
}

bool HasDataRowForTest(const std::filesystem::path& directory, std::wstring_view name)
{
    const PinnedDirectory pin(directory);
    return pin.Valid() && HasDataRow(pin, name);
}

} // namespace detail

std::string SanitizeField(std::string_view value, std::size_t maxLength)
{
    std::string out;
    out.reserve(std::min(value.size(), maxLength));
    for (const char raw : value)
    {
        const auto byte = static_cast<unsigned char>(raw);
        out.push_back(byte >= 0x20u && byte <= 0x7eu && byte != ',' ? raw : ' ');
    }
    const std::size_t first = out.find_first_not_of(' ');
    if (first == std::string::npos)
    {
        return {};
    }
    out.erase(0, first);
    out.resize(std::min(out.size(), maxLength));
    out.erase(out.find_last_not_of(' ') + 1u);
    return out;
}

PacketCaptureWriter::PacketCaptureWriter(std::filesystem::path directory,
                                         PacketCaptureLimits   limits,
                                         PacketCaptureIdentity identity)
: state_(std::make_shared<detail::WriterState>())
{
    state_->directory  = std::move(directory);
    state_->limits     = limits;
    state_->launcher   = SanitizeField(identity.launcher);
    state_->runtimeApi = identity.runtimeApi;
    if (state_->launcher.empty())
    {
        state_->launcher = "unknown";
    }
    state_->os   = OsField();
    state_->wine = WineField();
    try
    {
        std::thread([state = state_]
                    {
                        WriterThread(state).Run();
                    })
            .detach();
        state_->threadOk = true;
    }
    catch (const std::system_error&)
    {
        state_->threadOk = false;
    }
}

PacketCaptureWriter::~PacketCaptureWriter()
{
    // Runs under the loader lock at DLL detach, after process exit may have
    // killed the writer thread while it held the mutex: never lock or wait.
    state_->exit.store(true);
    state_->workCv.notify_all();
}

bool PacketCaptureWriter::Start(std::chrono::milliseconds wait)
{
    detail::WriterState& s    = *state_;
    auto                 slot = std::make_shared<detail::CommandSlot>();
    {
        std::scoped_lock lock(s.mutex);
        if (!s.threadOk || s.exit.load())
        {
            return false;
        }
        s.accepting = true;
        QueueItem item;
        item.kind       = detail::ItemKind::Start;
        item.slot       = slot;
        item.generation = ++s.commandGeneration;
        s.queue.push_back(std::move(item));
        s.wake = true;
    }
    s.workCv.notify_all();
    if (wait <= 0ms)
    {
        return true;
    }
    std::unique_lock lock(s.mutex);
    s.doneCv.wait_for(lock, wait, [&]
                      {
                          return slot->done;
                      });
    return slot->done && slot->result;
}

bool PacketCaptureWriter::Stop(std::chrono::milliseconds wait)
{
    return Stop(EndReason::Stop, wait);
}

bool PacketCaptureWriter::Stop(EndReason reason, std::chrono::milliseconds wait)
{
    detail::WriterState& s    = *state_;
    auto                 slot = std::make_shared<detail::CommandSlot>();
    {
        std::scoped_lock lock(s.mutex);
        if (!s.threadOk || s.exit.load())
        {
            return false;
        }
        s.accepting = false;
        QueueItem item;
        item.kind       = detail::ItemKind::Stop;
        item.reason     = reason;
        item.slot       = slot;
        item.generation = ++s.commandGeneration;
        s.queue.push_back(std::move(item));
        s.wake = true;
    }
    s.workCv.notify_all();
    if (wait <= 0ms)
    {
        return true;
    }
    std::unique_lock lock(s.mutex);
    s.doneCv.wait_for(lock, wait, [&]
                      {
                          return slot->done;
                      });
    return slot->done && slot->result;
}

bool PacketCaptureWriter::Split(std::chrono::milliseconds wait)
{
    detail::WriterState& s    = *state_;
    auto                 slot = std::make_shared<detail::CommandSlot>();
    {
        std::scoped_lock lock(s.mutex);
        if (!s.threadOk || s.exit.load() || !s.accepting)
        {
            return false;
        }
        QueueItem item;
        item.kind       = detail::ItemKind::Split;
        item.slot       = slot;
        item.generation = ++s.commandGeneration;
        s.queue.push_back(std::move(item));
        s.wake = true;
    }
    s.workCv.notify_all();
    if (wait <= 0ms)
    {
        return true;
    }
    std::unique_lock lock(s.mutex);
    s.doneCv.wait_for(lock, wait, [&]
                      {
                          return slot->done;
                      });
    return slot->done && slot->result;
}

namespace
{

// Shared by Enqueue and EnqueueStub; the caller holds no lock.
bool EnqueueItem(WriterState&                          s,
                 packet_observer::Direction            direction,
                 std::chrono::system_clock::time_point capturedAt,
                 std::span<const std::uint8_t>         payload,
                 bool                                  stub,
                 std::size_t                           frameSize)
{
    bool wake = false;
    {
        std::scoped_lock lock(s.mutex);
        if (!s.accepting || payload.empty())
        {
            return false;
        }
        ++s.status.captured;
        if (s.queuedFrames >= s.limits.maxQueuedFrames ||
            payload.size() > s.limits.maxQueuedBytes - s.queuedBytes)
        {
            ++s.status.dropped;
            return false;
        }
        QueueItem item;
        item.kind       = ItemKind::Frame;
        item.direction  = direction;
        item.capturedAt = capturedAt;
        item.bytes.assign(payload.begin(), payload.end());
        item.stub      = stub;
        item.frameSize = frameSize;
        s.queue.push_back(std::move(item));
        ++s.queuedFrames;
        s.queuedBytes += payload.size();
        s.status.queue_high_water = std::max(s.status.queue_high_water, s.queuedFrames);
        // Rows are normally batched on the wake interval; a half-full queue
        // is drained early.
        wake = s.queuedFrames * 2u >= s.limits.maxQueuedFrames ||
               s.queuedBytes * 2u >= s.limits.maxQueuedBytes;
        if (wake)
        {
            s.wake = true;
        }
    }
    if (wake)
    {
        s.workCv.notify_all();
    }
    return true;
}

} // namespace

bool PacketCaptureWriter::Enqueue(packet_observer::Direction            direction,
                                  std::chrono::system_clock::time_point capturedAt,
                                  std::span<const std::uint8_t>         frame)
{
    return EnqueueItem(*state_, direction, capturedAt, frame, false, 0);
}

bool PacketCaptureWriter::EnqueueStub(packet_observer::Direction            direction,
                                      std::chrono::system_clock::time_point capturedAt,
                                      std::size_t                           frameSize,
                                      std::string_view                      payload)
{
    return EnqueueItem(*state_,
                       direction,
                       capturedAt,
                       std::span<const std::uint8_t>(reinterpret_cast<const std::uint8_t*>(payload.data()),
                                                     payload.size()),
                       true,
                       frameSize);
}

bool PacketCaptureWriter::EnqueueZone(std::chrono::system_clock::time_point capturedAt,
                                      std::uint32_t                         zoneId,
                                      std::string_view                      name)
{
    const std::string    clean = SanitizeField(name);
    detail::WriterState& s     = *state_;
    std::scoped_lock     lock(s.mutex);
    if (!s.accepting || s.queue.size() >= s.limits.maxQueuedFrames * 2u)
    {
        return false;
    }
    QueueItem item;
    item.kind       = detail::ItemKind::Zone;
    item.capturedAt = capturedAt;
    item.frameSize  = zoneId;
    item.bytes.assign(clean.begin(), clean.end());
    s.queue.push_back(std::move(item));
    return true;
}

void PacketCaptureWriter::CountSkipped()
{
    detail::WriterState& s = *state_;
    std::scoped_lock     lock(s.mutex);
    if (s.accepting)
    {
        ++s.status.skipped;
    }
}

void PacketCaptureWriter::SetScope(Scope scope)
{
    state_->scope.store(static_cast<std::uint8_t>(scope));
}

Scope PacketCaptureWriter::GetScope() const
{
    return static_cast<Scope>(state_->scope.load());
}

bool PacketCaptureWriter::WaitIdle(std::chrono::milliseconds timeout)
{
    detail::WriterState& s = *state_;
    std::unique_lock     lock(s.mutex);
    s.wake = true;
    s.workCv.notify_all();
    return s.doneCv.wait_for(lock, timeout, [&]
                             {
                                 return s.queue.empty() && !s.busy;
                             });
}

PacketCaptureStatus PacketCaptureWriter::Status() const
{
    std::scoped_lock    lock(state_->mutex);
    PacketCaptureStatus status = state_->status;
    status.queued              = state_->queuedFrames;
    return status;
}

std::optional<std::string> PacketCaptureWriter::TakeNotice()
{
    std::scoped_lock lock(state_->mutex);
    if (state_->notices.empty())
    {
        return std::nullopt;
    }
    std::string notice = std::move(state_->notices.front());
    state_->notices.pop_front();
    return notice;
}

void PacketCaptureWriter::SetPausedForTest(bool paused)
{
    {
        std::scoped_lock lock(state_->mutex);
        state_->paused = paused;
        state_->wake   = true;
    }
    state_->workCv.notify_all();
}

void PacketCaptureWriter::FailNextWritesForTest(int count)
{
    state_->injectedFailures.store(count);
}

} // namespace packet_capture
