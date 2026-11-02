#pragma once

// Crash log for the game process. Everything reachable from CrashFilter runs
// while the heap may be corrupt, so it uses only fixed buffers and Win32 calls:
// no CRT allocation, iostreams, std::string or std::filesystem. The formatting
// and parsing helpers below take caller-supplied buffers and memory readers so
// tests can drive them without crashing.

#include <windows.h>

#include <cstddef>
#include <cstdint>
#include <string_view>

namespace crash_log
{

constexpr std::size_t kMaximumModules       = 256;
constexpr std::size_t kMaximumFrames        = 64;
constexpr std::size_t kStackScanDwords      = 1024;
constexpr std::size_t kModuleNameCapacity   = 48;
constexpr std::size_t kPdbNameCapacity      = 64;
constexpr std::size_t kMaximumAddonsText    = 512;
constexpr std::size_t kMaximumVersionText   = 64;
constexpr std::size_t kMaximumSections      = 96;
constexpr std::size_t kMaximumDebugEntries  = 16;
constexpr std::size_t kMaximumPdbPathBytes  = 260;
constexpr std::size_t kMaximumCodeRanges    = 8;
constexpr std::size_t kMaximumStartupAddons = 64;

// Fixed-capacity ASCII appender. Output past the capacity is dropped and
// sets truncated().
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
        else
        {
            truncated_ = true;
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

    // Uppercase hex, zero-padded to at least minimumDigits.
    void AppendHex(std::uint32_t value, unsigned minimumDigits = 1) noexcept
    {
        char     digits[8];
        unsigned count = 0;
        do
        {
            digits[count++] = "0123456789ABCDEF"[value & 0xfu];
            value >>= 4;
        } while (value != 0);
        for (unsigned pad = count; pad < minimumDigits && pad < 8; ++pad)
        {
            AppendChar('0');
        }
        while (count != 0)
        {
            AppendChar(digits[--count]);
        }
    }

    void AppendDecimal(std::uint32_t value, unsigned minimumDigits = 1) noexcept
    {
        char     digits[10];
        unsigned count = 0;
        do
        {
            digits[count++] = static_cast<char>('0' + value % 10u);
            value /= 10u;
        } while (value != 0);
        for (unsigned pad = count; pad < minimumDigits && pad < 10; ++pad)
        {
            AppendChar('0');
        }
        while (count != 0)
        {
            AppendChar(digits[--count]);
        }
    }

    // Ends the line even when the text filled the buffer.
    void AppendNewline() noexcept
    {
        if (size_ < capacity_)
        {
            storage_[size_++] = '\n';
        }
        else if (capacity_ != 0)
        {
            storage_[capacity_ - 1] = '\n';
        }
    }

    void Clear() noexcept
    {
        size_      = 0;
        truncated_ = false;
    }

    [[nodiscard]] const char* data() const noexcept
    {
        return storage_;
    }

    [[nodiscard]] std::size_t size() const noexcept
    {
        return size_;
    }

    [[nodiscard]] bool truncated() const noexcept
    {
        return truncated_;
    }

    [[nodiscard]] std::string_view view() const noexcept
    {
        return { storage_, size_ };
    }

private:
    char*       storage_;
    std::size_t capacity_;
    std::size_t size_      = 0;
    bool        truncated_ = false;
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
        return read != nullptr && read(context, address, out, size);
    }
};

struct CodeRange
{
    std::uint32_t begin = 0;
    std::uint32_t end   = 0;
};

struct ModuleRecord
{
    std::uintptr_t base          = 0;
    std::uint32_t  sizeOfImage   = 0;
    std::uint32_t  timeDateStamp = 0;
    // Executable sections as RVA ranges; sections past the capacity are not
    // recorded.
    CodeRange     code[kMaximumCodeRanges]{};
    std::uint32_t codeCount = 0;
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

// True when address lies in one of the executable sections of a listed module.
[[nodiscard]] bool IsCodeAddress(const ModuleTable& modules, std::uintptr_t address) noexcept;

// "name+0xOFFSET", or "?" when no listed module contains the address.
void AppendModuleOffset(TextBuffer& out, const ModuleTable& modules, std::uintptr_t address) noexcept;

// Fills the PE-derived fields of record from the mapped image at base:
// SizeOfImage, TimeDateStamp, executable range and the CodeView RSDS record.
// The name is left to the caller. False when the headers are not a readable
// 32-bit PE.
[[nodiscard]] bool ReadModuleImage(const MemoryReader& reader, std::uintptr_t base, ModuleRecord& record) noexcept;

// Return addresses along the EBP chain starting at ebp. Each frame pointer
// must be 4-byte aligned, strictly above the previous one and readable; the
// walk ends at a return address below 0x10000. Returns the count stored.
[[nodiscard]] std::size_t WalkFrames(const MemoryReader& reader, std::uintptr_t ebp, std::uintptr_t* returnAddresses, std::size_t capacity) noexcept;

struct ByteSink
{
    // Returns false when the data could not be written; WriteReport then stops
    // forwarding output, so it never continues past a gap.
    bool (*write)(void* context, const char* data, std::size_t size) = nullptr;
    void* context                                                    = nullptr;
};

struct ReportInput
{
    UtcTime          time{};
    std::uint32_t    processId = 0;
    std::uint32_t    threadId  = 0;
    std::string_view launcherVersion;
    // Comma-joined addon IDs selected at startup, written one per line up to
    // kMaximumStartupAddons; later loads and unloads are not tracked.
    std::string_view startupAddons;
    // The caller already dropped addon IDs that did not fit.
    bool startupAddonsTruncated = false;
    // Bytes of the faulting thread's stack, from ESP up, that reader serves.
    std::uint32_t           stackSnapshotBytes = 0;
    const EXCEPTION_RECORD* record             = nullptr;
    const CONTEXT*          context            = nullptr;
    const ModuleTable*      modules            = nullptr;
    MemoryReader            reader{};
};

void WriteReport(const ByteSink& sink, const ReportInput& input) noexcept;

// Fills the table from the loader's module list. A module whose headers are
// unreadable is skipped.
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
    // false forces the in-thread fallback that is used when the capture
    // thread cannot start.
    bool startCaptureThread = true;
    // How long a faulting thread waits for the capture before it chains.
    DWORD captureTimeoutMs = 20000;
};

// Creates the directory, starts the capture thread and installs CrashFilter as
// the unhandled-exception filter, keeping the previous one to chain to. False
// when disabled, already installed, or the directory cannot be created.
bool Install(const InstallOptions& options) noexcept;

// Reinstalls CrashFilter once, on the first call after Install, in case the
// game replaced it. A different filter returned there becomes the one chained
// to. Cheap to call every frame.
void ReassertFilterOnce() noexcept;

// The filter the crash filter chains to, or nullptr.
[[nodiscard]] LPTOP_LEVEL_EXCEPTION_FILTER ChainedFilter() noexcept;

// Copies the first exception and up to 16 KiB of the faulting stack from ESP
// into static storage and hands them to the capture thread, which writes the
// log from the copies, then waits up to the capture timeout (20 s by default). Any other thread entering meanwhile waits the same
// way. A thread that re-enters the filter from a chained filter gets
// EXCEPTION_CONTINUE_SEARCH with no chaining; threads are tracked in a fixed
// array of 64 ids, and one that finds it full waits and chains unmarked. Each
// thread then returns the
// chained filter's result, or EXCEPTION_CONTINUE_SEARCH when there is none.
// Without a capture thread the log is written on the faulting thread, except
// for a stack overflow, which only chains.
LONG WINAPI CrashFilter(EXCEPTION_POINTERS* exception);

} // namespace crash_log
