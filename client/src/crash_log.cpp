#include "crash_log.h"

#include "fault_guard.h"

#include <winternl.h>

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstring>

static_assert(sizeof(void*) == 4, "the crash log reads x86 contexts and loader structures");

namespace crash_log
{

namespace
{

constexpr DWORD         kCaptureStackBytes    = 256 * 1024;
constexpr std::size_t   kMaximumActiveThreads = 64;
constexpr std::size_t   kStackSnapshotBytes   = kStackScanDwords * sizeof(std::uint32_t);
constexpr DWORD         kReadableProtection   = PAGE_READONLY | PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY;
constexpr std::size_t   kLineCapacity         = 384;
constexpr std::size_t   kSinkCapacity         = 8192;
constexpr std::size_t   kDirectoryCapacity    = 400;
constexpr std::size_t   kPathCapacity         = kDirectoryCapacity + 64;
constexpr std::size_t   kMaximumLoaderEntries = 512;
constexpr std::uint32_t kRsdsSignature        = 0x53445352; // "RSDS"
constexpr std::uint32_t kCppExceptionCode     = 0xe06d7363;

// x86 LDR_DATA_TABLE_ENTRY up to the fields read; winternl.h leaves the
// middle of the structure opaque.
struct LoaderEntry
{
    LIST_ENTRY     inLoadOrderLinks;
    LIST_ENTRY     inMemoryOrderLinks;
    LIST_ENTRY     inInitializationOrderLinks;
    void*          dllBase;
    void*          entryPoint;
    ULONG          sizeOfImage;
    UNICODE_STRING fullDllName;
};

static_assert(offsetof(LoaderEntry, inMemoryOrderLinks) == 8, "x86 LDR_DATA_TABLE_ENTRY layout");
static_assert(offsetof(LoaderEntry, dllBase) == 24, "x86 LDR_DATA_TABLE_ENTRY layout");
static_assert(offsetof(LoaderEntry, fullDllName) == 36, "x86 LDR_DATA_TABLE_ENTRY layout");

struct FileSink
{
    HANDLE      file   = INVALID_HANDLE_VALUE;
    std::size_t used   = 0;
    bool        failed = false;
    char        buffer[kSinkCapacity];
};

// Everything the filter touches is static storage prepared at install time.
struct State
{
    std::atomic<bool> installed{ false };
    // Both filters stay inert until the events and capture thread exist.
    std::atomic<bool>                         ready{ false };
    std::atomic<LPTOP_LEVEL_EXCEPTION_FILTER> chained{ nullptr };
    // Only the game thread's ReassertFilterOnce touches this.
    bool reasserted = false;
    // Nonzero once a thread owns the capture.
    volatile LONG claimed = 0;
    // Prevent chained-filter recursion without TLS or heap storage; 0 marks a free slot.
    volatile LONG activeThreads[kMaximumActiveThreads]{};
    DWORD         captureTimeoutMs = 20000;
    HANDLE        requestEvent     = nullptr;
    HANDLE        doneEvent        = nullptr;
    // Copy before signaling; capture must not read the faulting thread's records.
    EXCEPTION_RECORD record{};
    CONTEXT          context{};
    bool             hasRecord      = false;
    bool             hasContext     = false;
    volatile LONG    faultingThread = 0;
    // The faulting thread's stack from stackBase up, copied before signaling.
    std::uintptr_t stackBase  = 0;
    std::size_t    stackBytes = 0;
    std::uint8_t   stack[kStackSnapshotBytes]{};
    bool           addonsTruncated = false;
    wchar_t        directory[kDirectoryCapacity]{};
    std::size_t    directoryLength = 0;
    wchar_t        scratch[kDirectoryCapacity]{};
    wchar_t        path[kPathCapacity]{};
    char           version[kMaximumVersionText]{};
    std::size_t    versionLength = 0;
    char           addons[kMaximumAddonsText]{};
    std::size_t    addonsLength = 0;
    ModuleTable    modules;
    FileSink       sink;
};

State gState;

bool IsSeparator(wchar_t value) noexcept
{
    return value == L'\\' || value == L'/';
}

template <typename Value>
bool ReadValue(const MemoryReader& reader, std::uintptr_t address, Value& value) noexcept
{
    return reader.Read(address, &value, sizeof(Value));
}

void WriteLine(const ByteSink& sink, TextBuffer& line) noexcept
{
    line.AppendChar('\n');
    sink.write(sink.context, line.data(), line.size());
    line.Clear();
}

void AppendHex32(TextBuffer& out, std::uint32_t value) noexcept
{
    out.Append("0x");
    out.AppendHex(value, 8);
}

void AppendAddressWithModule(TextBuffer& out, const ModuleTable& modules, std::uintptr_t address) noexcept
{
    AppendHex32(out, static_cast<std::uint32_t>(address));
    out.AppendChar(' ');
    AppendModuleOffset(out, modules, address);
}

void AppendGuid(TextBuffer& out, const std::uint8_t (&guid)[16]) noexcept
{
    // GUID in memory is Data1 and Data2/Data3 little-endian, then 8 bytes.
    const std::uint32_t data1 = static_cast<std::uint32_t>(guid[0]) | static_cast<std::uint32_t>(guid[1]) << 8 | static_cast<std::uint32_t>(guid[2]) << 16 | static_cast<std::uint32_t>(guid[3]) << 24;
    out.AppendHex(data1, 8);
    out.AppendChar('-');
    out.AppendHex(static_cast<std::uint32_t>(guid[4]) | static_cast<std::uint32_t>(guid[5]) << 8, 4);
    out.AppendChar('-');
    out.AppendHex(static_cast<std::uint32_t>(guid[6]) | static_cast<std::uint32_t>(guid[7]) << 8, 4);
    out.AppendChar('-');
    out.AppendHex(guid[8], 2);
    out.AppendHex(guid[9], 2);
    out.AppendChar('-');
    for (int index = 10; index < 16; ++index)
    {
        out.AppendHex(guid[index], 2);
    }
}

void CopyBaseName(char* destination, std::size_t capacity, const char* text, std::size_t length) noexcept
{
    std::size_t start = 0;
    for (std::size_t index = 0; index < length; ++index)
    {
        if (text[index] == '\\' || text[index] == '/')
        {
            start = index + 1;
        }
    }
    TextBuffer out(destination, capacity - 1);
    out.AppendAscii(std::string_view(text + start, length - start));
    destination[out.size()] = '\0';
}

bool ReadDebugRecord(const MemoryReader& reader, std::uintptr_t base, const IMAGE_DEBUG_DIRECTORY& entry, ModuleRecord& record) noexcept
{
    constexpr std::size_t kRsdsHeaderBytes = 24;
    if (entry.Type != IMAGE_DEBUG_TYPE_CODEVIEW || entry.AddressOfRawData == 0 || entry.SizeOfData <= kRsdsHeaderBytes)
    {
        return false;
    }
    const std::uintptr_t address = base + entry.AddressOfRawData;
    std::uint8_t         header[kRsdsHeaderBytes];
    if (!reader.Read(address, header, sizeof(header)))
    {
        return false;
    }
    std::uint32_t signature = 0;
    std::memcpy(&signature, header, sizeof(signature));
    if (signature != kRsdsSignature)
    {
        return false;
    }
    char        pathBytes[kMaximumPdbPathBytes];
    std::size_t pathLength = entry.SizeOfData - kRsdsHeaderBytes;
    if (pathLength > sizeof(pathBytes))
    {
        pathLength = sizeof(pathBytes);
    }
    if (!reader.Read(address + kRsdsHeaderBytes, pathBytes, pathLength))
    {
        return false;
    }
    std::size_t terminator = 0;
    while (terminator < pathLength && pathBytes[terminator] != '\0')
    {
        ++terminator;
    }
    std::memcpy(record.pdbGuid, header + 4, sizeof(record.pdbGuid));
    std::memcpy(&record.pdbAge, header + 20, sizeof(record.pdbAge));
    CopyBaseName(record.pdbName, sizeof(record.pdbName), pathBytes, terminator);
    record.hasPdb = true;
    return true;
}

bool ProcessRead(void*, std::uintptr_t address, void* out, std::size_t size) noexcept
{
    if (address + size < address)
    {
        return false;
    }
    const std::uintptr_t end    = address + size;
    std::uintptr_t       cursor = address;
    while (cursor < end)
    {
        MEMORY_BASIC_INFORMATION info{};
        if (VirtualQuery(reinterpret_cast<const void*>(cursor), &info, sizeof(info)) != sizeof(info))
        {
            return false;
        }
        if (info.State != MEM_COMMIT || (info.Protect & PAGE_GUARD) != 0 || (info.Protect & kReadableProtection) == 0)
        {
            return false;
        }
        const std::uintptr_t regionEnd = reinterpret_cast<std::uintptr_t>(info.BaseAddress) + info.RegionSize;
        if (regionEnd <= cursor)
        {
            return false;
        }
        cursor = regionEnd;
    }
    // The pages can still change between the query and the copy.
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(out, reinterpret_cast<const void*>(address), size);
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

void FlushSink(FileSink& sink) noexcept
{
    std::size_t written = 0;
    while (written < sink.used && !sink.failed)
    {
        DWORD count = 0;
        if (!WriteFile(sink.file, sink.buffer + written, static_cast<DWORD>(sink.used - written), &count, nullptr) || count == 0)
        {
            sink.failed = true;
            break;
        }
        written += count;
    }
    sink.used = 0;
}

// Stop after a failed write so an incomplete log cannot have an end marker.
void SinkWrite(void* context, const char* data, std::size_t size) noexcept
{
    auto& sink = *static_cast<FileSink*>(context);
    while (size != 0 && !sink.failed)
    {
        if (sink.used == sizeof(sink.buffer))
        {
            FlushSink(sink);
            continue;
        }
        std::size_t chunk = sizeof(sink.buffer) - sink.used;
        if (chunk > size)
        {
            chunk = size;
        }
        std::memcpy(sink.buffer + sink.used, data, chunk);
        sink.used += chunk;
        data += chunk;
        size -= chunk;
    }
}

// The final attribute check decides success even if CreateDirectoryW fails.
bool EnsureDirectory(const wchar_t* path, std::size_t length) noexcept
{
    wchar_t* scratch = gState.scratch;
    std::memcpy(scratch, path, length * sizeof(wchar_t));
    scratch[length] = L'\0';
    for (std::size_t index = 3; index < length; ++index)
    {
        if (IsSeparator(scratch[index]))
        {
            const wchar_t saved = scratch[index];
            scratch[index]      = L'\0';
            CreateDirectoryW(scratch, nullptr);
            scratch[index] = saved;
        }
    }
    CreateDirectoryW(scratch, nullptr);
    const DWORD attributes = GetFileAttributesW(scratch);
    return attributes != INVALID_FILE_ATTRIBUTES && (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
}

// directory\stem.log into gState.path; the longest directory leaves room.
void BuildPath(const char* stem, std::size_t stemLength) noexcept
{
    std::size_t length = gState.directoryLength;
    std::memcpy(gState.path, gState.directory, length * sizeof(wchar_t));
    gState.path[length++] = L'\\';
    for (std::size_t index = 0; index < stemLength; ++index)
    {
        gState.path[length++] = static_cast<wchar_t>(static_cast<unsigned char>(stem[index]));
    }
    constexpr wchar_t kExtension[] = L".log";
    std::memcpy(gState.path + length, kExtension, sizeof(kExtension));
}

HANDLE CreateCrashFile() noexcept
{
    HANDLE file = CreateFileW(gState.path, GENERIC_WRITE, FILE_SHARE_READ, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE && GetLastError() == ERROR_PATH_NOT_FOUND && EnsureDirectory(gState.directory, gState.directoryLength))
    {
        file = CreateFileW(gState.path, GENERIC_WRITE, FILE_SHARE_READ, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr);
    }
    return file;
}

// Reads outside the snapshot return false, ending the stack scan.
bool SnapshotRead(void*, std::uintptr_t address, void* out, std::size_t size) noexcept
{
    if (address < gState.stackBase || address - gState.stackBase > gState.stackBytes || size > gState.stackBytes - (address - gState.stackBase))
    {
        return false;
    }
    std::memcpy(out, gState.stack + (address - gState.stackBase), size);
    return true;
}

// The capture thread reads only copies of the exception and stack.
void WriteCrashFiles(DWORD faultingThread) noexcept
{
    SYSTEMTIME now{};
    GetSystemTime(&now);
    const UtcTime       time{ now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond };
    const std::uint32_t processId = GetCurrentProcessId();

    char       stemStorage[kModuleNameCapacity];
    TextBuffer stem(stemStorage, sizeof(stemStorage));
    AppendFileStem(stem, time, processId);
    BuildPath(stem.data(), stem.size());
    FileSink& sink = gState.sink;
    sink.used      = 0;
    sink.failed    = false;
    sink.file      = CreateCrashFile();
    if (sink.file == INVALID_HANDLE_VALUE)
    {
        return;
    }

    CaptureModules(gState.modules, ProcessMemoryReader());

    ReportInput input;
    input.time                   = time;
    input.processId              = processId;
    input.threadId               = faultingThread;
    input.launcherVersion        = std::string_view(gState.version, gState.versionLength);
    input.startupAddons          = std::string_view(gState.addons, gState.addonsLength);
    input.startupAddonsTruncated = gState.addonsTruncated;
    input.stackSnapshotBytes     = static_cast<std::uint32_t>(gState.stackBytes);
    input.record                 = gState.hasRecord ? &gState.record : nullptr;
    input.context                = gState.hasContext ? &gState.context : nullptr;
    input.modules                = &gState.modules;
    input.reader                 = MemoryReader{ &SnapshotRead, nullptr };
    WriteReport(ByteSink{ &SinkWrite, &sink }, input);
    FlushSink(sink);
    CloseHandle(sink.file);
    sink.file = INVALID_HANDLE_VALUE;
}

void WaitForCapture() noexcept
{
    WaitForSingleObject(gState.doneEvent, gState.captureTimeoutMs);
}

// Own stack, so a capture works after a stack overflow while the faulter waits at most the timeout.
DWORD WINAPI CaptureThreadMain(LPVOID)
{
    WaitForSingleObject(gState.requestEvent, INFINITE);
    BAHAMUT_FAULT_TRY
    {
        WriteCrashFiles(static_cast<DWORD>(gState.faultingThread));
    }
    BAHAMUT_FAULT_EXCEPT
    {
    }
    SetEvent(gState.doneEvent);
    return 0;
}

// Copy the record and context before capture can outlive the faulting stack.
void StoreException(const EXCEPTION_POINTERS* exception) noexcept
{
    gState.hasRecord  = false;
    gState.hasContext = false;
    BAHAMUT_FAULT_TRY
    {
        if (exception->ExceptionRecord != nullptr)
        {
            gState.record    = *exception->ExceptionRecord;
            gState.hasRecord = true;
        }
        if (exception->ContextRecord != nullptr)
        {
            gState.context    = *exception->ContextRecord;
            gState.hasContext = true;
        }
    }
    BAHAMUT_FAULT_EXCEPT
    {
    }
}

// Snapshot from ESP to the scan limit or region end before the thread resumes.
// One VirtualQuery and a copy use little stack, permitting capture after stack overflow.
void StoreStack() noexcept
{
    gState.stackBase  = 0;
    gState.stackBytes = 0;
    if (!gState.hasContext)
    {
        return;
    }
    const std::uintptr_t     esp = gState.context.Esp;
    MEMORY_BASIC_INFORMATION info{};
    if (VirtualQuery(reinterpret_cast<const void*>(esp), &info, sizeof(info)) != sizeof(info) || info.State != MEM_COMMIT || (info.Protect & PAGE_GUARD) != 0 || (info.Protect & kReadableProtection) == 0)
    {
        return;
    }
    const std::uintptr_t regionEnd = reinterpret_cast<std::uintptr_t>(info.BaseAddress) + info.RegionSize;
    if (regionEnd <= esp)
    {
        return;
    }
    std::size_t bytes = regionEnd - esp;
    if (bytes > kStackSnapshotBytes)
    {
        bytes = kStackSnapshotBytes;
    }
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(gState.stack, reinterpret_cast<const void*>(esp), bytes);
        gState.stackBase  = esp;
        gState.stackBytes = bytes;
    }
    BAHAMUT_FAULT_EXCEPT
    {
    }
}

void RunCapture(const EXCEPTION_POINTERS* exception, DWORD thread) noexcept
{
    StoreException(exception);
    StoreStack();
    gState.faultingThread = static_cast<LONG>(thread);
    SetEvent(gState.requestEvent);
    WaitForCapture();
}

constexpr int kReentered = -1;
constexpr int kNoSlot    = -2;

// The slot now holding this thread's id, kReentered when the id is already
// present (a chained filter led back here), or kNoSlot when the array is full.
int EnterFilter(DWORD thread) noexcept
{
    const LONG id = static_cast<LONG>(thread);
    for (std::size_t index = 0; index < kMaximumActiveThreads; ++index)
    {
        if (gState.activeThreads[index] == id)
        {
            return kReentered;
        }
    }
    for (std::size_t index = 0; index < kMaximumActiveThreads; ++index)
    {
        if (InterlockedCompareExchange(&gState.activeThreads[index], id, 0) == 0)
        {
            return static_cast<int>(index);
        }
    }
    return kNoSlot;
}

} // namespace

void AppendIsoTime(TextBuffer& out, const UtcTime& time) noexcept
{
    out.AppendDecimal(time.year, 4);
    out.AppendChar('-');
    out.AppendDecimal(time.month, 2);
    out.AppendChar('-');
    out.AppendDecimal(time.day, 2);
    out.AppendChar('T');
    out.AppendDecimal(time.hour, 2);
    out.AppendChar(':');
    out.AppendDecimal(time.minute, 2);
    out.AppendChar(':');
    out.AppendDecimal(time.second, 2);
    out.AppendChar('Z');
}

void AppendFileStem(TextBuffer& out, const UtcTime& time, std::uint32_t processId) noexcept
{
    out.Append("game-");
    out.AppendDecimal(time.year, 4);
    out.AppendDecimal(time.month, 2);
    out.AppendDecimal(time.day, 2);
    out.AppendChar('T');
    out.AppendDecimal(time.hour, 2);
    out.AppendDecimal(time.minute, 2);
    out.AppendDecimal(time.second, 2);
    out.AppendChar('Z');
    out.AppendChar('-');
    out.AppendDecimal(processId);
}

std::string_view ExceptionName(std::uint32_t code) noexcept
{
    switch (code)
    {
        case 0xc0000005:
            return "access violation";
        case 0xc00000fd:
            return "stack overflow";
        case 0xc000001d:
            return "illegal instruction";
        case 0xc0000094:
            return "integer divide by zero";
        case 0xc0000096:
            return "privileged instruction";
        case 0xc0000006:
            return "in-page error";
        case 0xc0000374:
            return "heap corruption";
        case 0xc0000409:
            return "stack buffer overrun";
        case kCppExceptionCode:
            return "unhandled C++ exception";
        default:
            return {};
    }
}

std::string_view AccessKindName(std::uintptr_t kind) noexcept
{
    switch (kind)
    {
        case 0:
            return "read";
        case 1:
            return "write";
        case 8:
            return "execute";
        default:
            return "unknown";
    }
}

const ModuleRecord* FindModule(const ModuleTable& modules, std::uintptr_t address) noexcept
{
    for (std::size_t index = 0; index < modules.count; ++index)
    {
        const ModuleRecord& module = modules.entries[index];
        if (address >= module.base && address - module.base < module.sizeOfImage)
        {
            return &module;
        }
    }
    return nullptr;
}

bool IsCodeAddress(const ModuleTable& modules, std::uintptr_t address) noexcept
{
    const ModuleRecord* module = FindModule(modules, address);
    if (module == nullptr)
    {
        return false;
    }
    const std::uintptr_t offset = address - module->base;
    return offset >= module->codeBegin && offset < module->codeEnd;
}

void AppendModuleOffset(TextBuffer& out, const ModuleTable& modules, std::uintptr_t address) noexcept
{
    const ModuleRecord* module = FindModule(modules, address);
    if (module == nullptr)
    {
        out.AppendChar('?');
        return;
    }
    out.Append(module->name);
    out.Append("+0x");
    out.AppendHex(static_cast<std::uint32_t>(address - module->base));
}

bool ReadModuleImage(const MemoryReader& reader, std::uintptr_t base, ModuleRecord& record) noexcept
{
    IMAGE_DOS_HEADER dos{};
    if (!ReadValue(reader, base, dos) || dos.e_magic != IMAGE_DOS_SIGNATURE || dos.e_lfanew < static_cast<LONG>(sizeof(dos)) || dos.e_lfanew > 0x1000)
    {
        return false;
    }
    const std::uintptr_t ntAddress = base + static_cast<std::uintptr_t>(dos.e_lfanew);
    IMAGE_NT_HEADERS32   nt{};
    if (!ReadValue(reader, ntAddress, nt) || nt.Signature != IMAGE_NT_SIGNATURE || nt.OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR32_MAGIC || nt.OptionalHeader.SizeOfImage == 0)
    {
        return false;
    }
    record.sizeOfImage   = nt.OptionalHeader.SizeOfImage;
    record.timeDateStamp = nt.FileHeader.TimeDateStamp;

    std::uintptr_t section = ntAddress + offsetof(IMAGE_NT_HEADERS32, OptionalHeader) + nt.FileHeader.SizeOfOptionalHeader;
    std::size_t    count   = nt.FileHeader.NumberOfSections;
    if (count > kMaximumSections)
    {
        count = kMaximumSections;
    }
    for (std::size_t index = 0; index < count; ++index, section += sizeof(IMAGE_SECTION_HEADER))
    {
        IMAGE_SECTION_HEADER header{};
        if (!ReadValue(reader, section, header))
        {
            break;
        }
        if ((header.Characteristics & IMAGE_SCN_MEM_EXECUTE) == 0)
        {
            continue;
        }
        const std::uint32_t length = header.Misc.VirtualSize != 0 ? header.Misc.VirtualSize : header.SizeOfRawData;
        const std::uint32_t begin  = header.VirtualAddress;
        const std::uint32_t end    = begin + length;
        if (length == 0 || end < begin)
        {
            continue;
        }
        if (record.codeEnd == 0 || begin < record.codeBegin)
        {
            record.codeBegin = begin;
        }
        if (end > record.codeEnd)
        {
            record.codeEnd = end;
        }
    }

    const IMAGE_DATA_DIRECTORY& debug = nt.OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_DEBUG];
    if (debug.VirtualAddress != 0 && debug.Size >= sizeof(IMAGE_DEBUG_DIRECTORY))
    {
        std::size_t entries = debug.Size / sizeof(IMAGE_DEBUG_DIRECTORY);
        if (entries > kMaximumDebugEntries)
        {
            entries = kMaximumDebugEntries;
        }
        for (std::size_t index = 0; index < entries; ++index)
        {
            IMAGE_DEBUG_DIRECTORY entry{};
            if (!ReadValue(reader, base + debug.VirtualAddress + index * sizeof(entry), entry))
            {
                break;
            }
            if (ReadDebugRecord(reader, base, entry, record))
            {
                break;
            }
        }
    }
    return true;
}

void WriteReport(const ByteSink& sink, const ReportInput& input) noexcept
{
    char               storage[kLineCapacity];
    TextBuffer         line(storage, sizeof(storage));
    const ModuleTable& modules = *input.modules;

    line.Append("Bahamut game crash log");
    WriteLine(sink, line);

    line.Append("time_utc: ");
    AppendIsoTime(line, input.time);
    WriteLine(sink, line);

    line.Append("launcher_version: ");
    if (input.launcherVersion.empty())
    {
        line.Append("unknown");
    }
    else
    {
        line.Append(input.launcherVersion);
    }
    WriteLine(sink, line);

    line.Append("process_id: ");
    line.AppendDecimal(input.processId);
    WriteLine(sink, line);

    line.Append("thread_id: ");
    line.AppendDecimal(input.threadId);
    WriteLine(sink, line);

    for (std::size_t start = 0; start < input.startupAddons.size();)
    {
        std::size_t end = input.startupAddons.find(',', start);
        if (end == std::string_view::npos)
        {
            end = input.startupAddons.size();
        }
        line.Append("startup_addon: ");
        line.Append(input.startupAddons.substr(start, end - start));
        WriteLine(sink, line);
        start = end + 1;
    }
    if (input.startupAddonsTruncated)
    {
        line.Append("startup_addons_truncated: yes");
        WriteLine(sink, line);
    }

    if (input.record != nullptr)
    {
        const EXCEPTION_RECORD& record = *input.record;
        const std::uint32_t     code   = static_cast<std::uint32_t>(record.ExceptionCode);
        line.Append("exception_code: 0x");
        line.AppendHex(code, 8);
        if (const std::string_view name = ExceptionName(code); !name.empty())
        {
            line.Append(" (");
            line.Append(name);
            line.AppendChar(')');
        }
        WriteLine(sink, line);

        const bool pageFault = code == static_cast<std::uint32_t>(EXCEPTION_ACCESS_VIOLATION) || code == static_cast<std::uint32_t>(EXCEPTION_IN_PAGE_ERROR);
        if (pageFault && record.NumberParameters >= 2)
        {
            line.Append("exception_access: ");
            line.Append(AccessKindName(record.ExceptionInformation[0]));
            line.Append(" at ");
            AppendHex32(line, static_cast<std::uint32_t>(record.ExceptionInformation[1]));
            WriteLine(sink, line);
        }
        if (code == static_cast<std::uint32_t>(EXCEPTION_IN_PAGE_ERROR) && record.NumberParameters >= 3)
        {
            line.Append("exception_status: ");
            AppendHex32(line, static_cast<std::uint32_t>(record.ExceptionInformation[2]));
            WriteLine(sink, line);
        }

        line.Append("fault_address: ");
        AppendAddressWithModule(line, modules, reinterpret_cast<std::uintptr_t>(record.ExceptionAddress));
        WriteLine(sink, line);
    }

    if (input.context != nullptr)
    {
        const CONTEXT& context = *input.context;

        const struct
        {
            const char*   name;
            std::uint32_t value;
        } registers[] = {
            { "eax", context.Eax },
            { "ebx", context.Ebx },
            { "ecx", context.Ecx },
            { "edx", context.Edx },
            { "esi", context.Esi },
            { "edi", context.Edi },
            { "ebp", context.Ebp },
            { "esp", context.Esp },
            { "eip", context.Eip },
            { "eflags", context.EFlags },
            { "cs", context.SegCs },
            { "ss", context.SegSs },
        };

        for (const auto& entry : registers)
        {
            line.Append(entry.name);
            line.Append(": ");
            AppendHex32(line, entry.value);
            WriteLine(sink, line);
        }

        line.Append("stack_snapshot_bytes: ");
        line.AppendDecimal(input.stackSnapshotBytes);
        WriteLine(sink, line);

        for (std::uint32_t index = 0; index < kStackScanDwords; ++index)
        {
            std::uint32_t value = 0;
            if (!input.reader.Read(static_cast<std::uintptr_t>(context.Esp) + index * 4u, &value, sizeof(value)))
            {
                break;
            }
            if (!IsCodeAddress(modules, value))
            {
                continue;
            }
            line.Append("stack esp+0x");
            line.AppendHex(index * 4u, 4);
            line.Append(": ");
            AppendAddressWithModule(line, modules, value);
            WriteLine(sink, line);
        }
    }

    for (std::size_t index = 0; index < modules.count; ++index)
    {
        const ModuleRecord& module = modules.entries[index];
        line.Append("module ");
        line.Append(module.name);
        line.Append(" base=");
        AppendHex32(line, static_cast<std::uint32_t>(module.base));
        line.Append(" size_of_image=");
        AppendHex32(line, module.sizeOfImage);
        line.Append(" time_date_stamp=");
        AppendHex32(line, module.timeDateStamp);
        if (module.hasPdb)
        {
            line.Append(" pdb_guid=");
            AppendGuid(line, module.pdbGuid);
            line.Append(" pdb_age=");
            line.AppendDecimal(module.pdbAge);
            line.Append(" pdb=");
            line.Append(module.pdbName);
        }
        WriteLine(sink, line);
    }

    line.Append("end of crash log");
    WriteLine(sink, line);
}

void CaptureModules(ModuleTable& modules, const MemoryReader& reader) noexcept
{
    modules.count = 0;
    BAHAMUT_FAULT_TRY
    {
        const PEB*        peb  = NtCurrentTeb()->ProcessEnvironmentBlock;
        const LIST_ENTRY* head = &peb->Ldr->InMemoryOrderModuleList;
        const LIST_ENTRY* node = head->Flink;
        for (std::size_t visited = 0; node != nullptr && node != head && visited < kMaximumLoaderEntries && modules.count < kMaximumModules; ++visited, node = node->Flink)
        {
            const auto*  entry  = CONTAINING_RECORD(node, LoaderEntry, inMemoryOrderLinks);
            ModuleRecord record = {};
            record.base         = reinterpret_cast<std::uintptr_t>(entry->dllBase);
            if (record.base == 0 || !ReadModuleImage(reader, record.base, record))
            {
                continue;
            }
            char        narrow[260];
            TextBuffer  full(narrow, sizeof(narrow));
            std::size_t characters = entry->fullDllName.Length / sizeof(wchar_t);
            if (characters > sizeof(narrow))
            {
                characters = sizeof(narrow);
            }
            full.AppendWide(entry->fullDllName.Buffer, characters);
            CopyBaseName(record.name, sizeof(record.name), full.data(), full.size());
            modules.entries[modules.count++] = record;
        }
    }
    BAHAMUT_FAULT_EXCEPT
    {
    }
}

MemoryReader ProcessMemoryReader() noexcept
{
    return MemoryReader{ &ProcessRead, nullptr };
}

bool Install(const InstallOptions& options) noexcept
{
    if (options.directory.empty() || options.directory.size() >= kDirectoryCapacity - 1)
    {
        return false;
    }
    bool expected = false;
    if (!gState.installed.compare_exchange_strong(expected, true))
    {
        return false;
    }

    std::size_t length = options.directory.size();
    while (length > 3 && IsSeparator(options.directory[length - 1]))
    {
        --length;
    }
    std::memcpy(gState.directory, options.directory.data(), length * sizeof(wchar_t));
    gState.directory[length] = L'\0';
    gState.directoryLength   = length;
    if (!EnsureDirectory(gState.directory, length))
    {
        gState.installed.store(false);
        return false;
    }

    TextBuffer version(gState.version, sizeof(gState.version));
    version.AppendAscii(options.launcherVersion);
    gState.versionLength = version.size();
    // Whole IDs only: cut at the last comma that fits and say so in the log.
    std::string_view addonText = options.addons;
    gState.addonsTruncated     = false;
    if (addonText.size() > sizeof(gState.addons))
    {
        const std::size_t cut  = addonText.substr(0, sizeof(gState.addons) + 1).rfind(',');
        addonText              = addonText.substr(0, cut == std::string_view::npos ? 0 : cut);
        gState.addonsTruncated = true;
    }
    TextBuffer addons(gState.addons, sizeof(gState.addons));
    addons.AppendAscii(addonText);
    gState.addonsLength = addons.size();

    gState.captureTimeoutMs = options.captureTimeoutMs;
    gState.requestEvent     = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    gState.doneEvent        = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    HANDLE thread           = nullptr;
    if (gState.requestEvent != nullptr && gState.doneEvent != nullptr)
    {
        thread = CreateThread(nullptr, kCaptureStackBytes, &CaptureThreadMain, nullptr, STACK_SIZE_PARAM_IS_A_RESERVATION, nullptr);
    }
    if (thread == nullptr)
    {
        for (HANDLE* event : { &gState.requestEvent, &gState.doneEvent })
        {
            if (*event != nullptr)
            {
                CloseHandle(*event);
                *event = nullptr;
            }
        }
        gState.installed.store(false);
        return false;
    }
    CloseHandle(thread);

    gState.ready.store(true, std::memory_order_release);
    gState.chained.store(SetUnhandledExceptionFilter(&CrashFilter));
    return true;
}

void ReassertFilterOnce() noexcept
{
    if (gState.reasserted || !gState.ready.load(std::memory_order_acquire))
    {
        return;
    }
    gState.reasserted                           = true;
    const LPTOP_LEVEL_EXCEPTION_FILTER previous = SetUnhandledExceptionFilter(&CrashFilter);
    if (previous != &CrashFilter)
    {
        gState.chained.store(previous);
    }
}

LONG WINAPI CrashFilter(EXCEPTION_POINTERS* exception)
{
    const int slot = EnterFilter(GetCurrentThreadId());
    if (slot == kReentered)
    {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if (exception != nullptr && gState.ready.load(std::memory_order_acquire))
    {
        if (InterlockedCompareExchange(&gState.claimed, 1, 0) == 0)
        {
            RunCapture(exception, GetCurrentThreadId());
        }
        else
        {
            // Another thread is capturing; ending the process now would cut it off.
            WaitForCapture();
        }
    }
    LONG                               result = EXCEPTION_CONTINUE_SEARCH;
    const LPTOP_LEVEL_EXCEPTION_FILTER next   = gState.chained.load();
    if (next != nullptr && next != &CrashFilter)
    {
        result = next(exception);
    }
    if (slot >= 0)
    {
        InterlockedExchange(&gState.activeThreads[slot], 0);
    }
    return result;
}

} // namespace crash_log
