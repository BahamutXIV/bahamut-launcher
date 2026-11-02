#pragma once

// CrashFilter may run with a corrupt heap: use fixed buffers and Win32 calls,
// never CRT allocation, iostreams, std::string or std::filesystem.

#include <windows.h>

#include <cstddef>
#include <cstdint>
#include <string_view>

namespace crash_log
{

constexpr std::size_t kMaximumModules      = 256;
constexpr std::size_t kStackScanDwords     = 1024;
constexpr std::size_t kModuleNameCapacity  = 48;
constexpr std::size_t kPdbNameCapacity     = 64;
constexpr std::size_t kMaximumAddonsText   = 512;
constexpr std::size_t kMaximumVersionText  = 64;
constexpr std::size_t kMaximumSections     = 96;
constexpr std::size_t kMaximumDebugEntries = 16;
constexpr std::size_t kMaximumPdbPathBytes = 260;

// Output past capacity is dropped.
class TextBuffer
{
public:
    TextBuffer(char* storage, std::size_t capacity) noexcept
    : storage_(storage)
    , capacity_(capacity)
    {
    }

    void AppendChar(char value) noexcept
    {
        if (size_ < capacity_)
        {
            storage_[size_++] = value;
        }
    }

    void Append(std::string_view text) noexcept
    {
        for (const char value : text)
        {
            AppendChar(value);
        }
    }

    // Printable ASCII only; every other byte becomes '?'.
    void AppendAscii(std::string_view text) noexcept
    {
        for (const char value : text)
        {
            AppendChar(value >= 0x20 && value <= 0x7e ? value : '?');
        }
    }

    void AppendWide(const wchar_t* text, std::size_t length) noexcept
    {
        for (std::size_t index = 0; index < length; ++index)
        {
            AppendChar(text[index] >= 0x20 && text[index] <= 0x7e ? static_cast<char>(text[index]) : '?');
        }
    }

    // Uppercase hex, zero-padded to at least minimumDigits (at most 8).
    void AppendHex(std::uint32_t value, unsigned minimumDigits = 1) noexcept
    {
        char     digits[8];
        unsigned count = 0;
        do
        {
            digits[count++] = "0123456789ABCDEF"[value & 0xfu];
            value >>= 4;
        } while (value != 0);
        for (unsigned pad = count; pad < minimumDigits; ++pad)
        {
            AppendChar('0');
        }
        while (count != 0)
        {
            AppendChar(digits[--count]);
        }
    }

    // Zero-padded to at least minimumDigits (at most 10).
    void AppendDecimal(std::uint32_t value, unsigned minimumDigits = 1) noexcept
    {
        char     digits[10];
        unsigned count = 0;
        do
        {
            digits[count++] = static_cast<char>('0' + value % 10u);
            value /= 10u;
        } while (value != 0);
        for (unsigned pad = count; pad < minimumDigits; ++pad)
        {
            AppendChar('0');
        }
        while (count != 0)
        {
            AppendChar(digits[--count]);
        }
    }

    void Clear() noexcept
    {
        size_ = 0;
    }

    [[nodiscard]] const char* data() const noexcept
    {
        return storage_;
    }

    [[nodiscard]] std::size_t size() const noexcept
    {
        return size_;
    }

    [[nodiscard]] std::string_view view() const noexcept
    {
        return { storage_, size_ };
    }

private:
    char*       storage_;
    std::size_t capacity_;
    std::size_t size_ = 0;
};

struct UtcTime
{
    unsigned year;
    unsigned month;
    unsigned day;
    unsigned hour;
    unsigned minute;
    unsigned second;
};

// 2026-01-02T03:04:05Z
void AppendIsoTime(TextBuffer& out, const UtcTime& time) noexcept;

// game-20260102T030405Z-<pid>: the stem of the .log file name.
void AppendFileStem(TextBuffer& out, const UtcTime& time, std::uint32_t processId) noexcept;

// Short lowercase name for common exception codes; empty when unknown.
[[nodiscard]] std::string_view ExceptionName(std::uint32_t code) noexcept;

// ExceptionInformation[0] of an access violation: read, write or execute.
[[nodiscard]] std::string_view AccessKindName(std::uintptr_t kind) noexcept;

// Reads size bytes at address; false when any byte is unreadable.
struct MemoryReader
{
    bool (*read)(void* context, std::uintptr_t address, void* out, std::size_t size) = nullptr;
    void* context                                                                    = nullptr;

    [[nodiscard]] bool Read(std::uintptr_t address, void* out, std::size_t size) const noexcept
    {
        return read(context, address, out, size);
    }
};

struct ModuleRecord
{
    std::uintptr_t base          = 0;
    std::uint32_t  sizeOfImage   = 0;
    std::uint32_t  timeDateStamp = 0;
    // Span of the executable sections as RVAs: [codeBegin, codeEnd).
    std::uint32_t codeBegin = 0;
    std::uint32_t codeEnd   = 0;
    bool          hasPdb    = false;
    std::uint8_t  pdbGuid[16]{};
    std::uint32_t pdbAge = 0;
    char          name[kModuleNameCapacity]{};
    char          pdbName[kPdbNameCapacity]{};
};

struct ModuleTable
{
    ModuleRecord entries[kMaximumModules];
    std::size_t  count = 0;
};

// The loaded module whose image contains address, or nullptr.
[[nodiscard]] const ModuleRecord* FindModule(const ModuleTable& modules, std::uintptr_t address) noexcept;

// True when address lies in the executable span of a listed module.
[[nodiscard]] bool IsCodeAddress(const ModuleTable& modules, std::uintptr_t address) noexcept;

// "name+0xOFFSET", or "?" when no listed module contains the address.
void AppendModuleOffset(TextBuffer& out, const ModuleTable& modules, std::uintptr_t address) noexcept;

// Read SizeOfImage, TimeDateStamp, executable span and CodeView RSDS from a 32-bit PE.
// Requires a fresh record; the caller sets its name. False for unreadable or invalid headers.
[[nodiscard]] bool ReadModuleImage(const MemoryReader& reader, std::uintptr_t base, ModuleRecord& record) noexcept;

struct ByteSink
{
    // Receives one line per call; after failure, the sink must drop all later lines.
    void (*write)(void* context, const char* data, std::size_t size) = nullptr;
    void* context                                                    = nullptr;
};

struct ReportInput
{
    UtcTime          time{};
    std::uint32_t    processId = 0;
    std::uint32_t    threadId  = 0;
    std::string_view launcherVersion;
    // Comma-joined IDs selected at startup, printable ASCII; later loads are not tracked.
    std::string_view startupAddons;
    // Install dropped IDs to fit its capacity.
    bool startupAddonsTruncated = false;
    // Bytes of the faulting thread's stack, from ESP up, that reader serves.
    std::uint32_t           stackSnapshotBytes = 0;
    const EXCEPTION_RECORD* record             = nullptr;
    const CONTEXT*          context            = nullptr;
    const ModuleTable*      modules            = nullptr; // required
    MemoryReader            reader{};
};

void WriteReport(const ByteSink& sink, const ReportInput& input) noexcept;

// Read the loader's module list, skipping unreadable headers.
void CaptureModules(ModuleTable& modules, const MemoryReader& reader) noexcept;

// VirtualQuery-validated reads of the current process.
[[nodiscard]] MemoryReader ProcessMemoryReader() noexcept;

struct InstallOptions
{
    // Absolute directory for the crash files; empty leaves crash logging off.
    std::wstring_view directory;
    std::string_view  launcherVersion;
    // Comma-joined addon IDs selected at startup; may be empty.
    std::string_view addons;
    // How long a faulting thread waits for the capture before it chains.
    DWORD captureTimeoutMs = 20000;
};

// Create the directory and capture thread; install CrashFilter and chain the previous filter.
// False if disabled, already installed, or directory or thread creation fails.
bool Install(const InstallOptions& options) noexcept;

// Reinstall once after Install, chaining any replacement filter; cheap to call every frame.
void ReassertFilterOnce() noexcept;

// Capture the first exception, wait, then chain; see ../README.md#crash-logs.
// Re-entry returns EXCEPTION_CONTINUE_SEARCH; a full thread array still waits and chains.
LONG WINAPI CrashFilter(EXCEPTION_POINTERS* exception);

} // namespace crash_log
