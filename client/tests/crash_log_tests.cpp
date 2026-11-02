#include "crash_log.h"

#include <windows.h>

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <exception>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <memory>
#include <regex>
#include <string>
#include <string_view>
#include <thread>
#include <vector>

namespace
{

namespace fs = std::filesystem;
using namespace crash_log;

int g_failures = 0;

bool Expect(bool condition, const char* message)
{
    if (!condition)
    {
        std::cerr << "FAIL: " << message << '\n';
        ++g_failures;
    }
    return condition;
}

void ExpectText(std::string_view actual, std::string_view expected, const char* message)
{
    if (actual != expected)
    {
        std::cerr << "FAIL: " << message << "\n--- expected ---\n"
                  << expected << "\n--- actual ---\n"
                  << actual << '\n';
        ++g_failures;
    }
}

// Memory made of explicit regions at fake addresses; a read must fit in one.
class FakeMemory
{
public:
    void Add(std::uintptr_t base, std::vector<std::uint8_t> bytes)
    {
        regions_.push_back({ base, std::move(bytes) });
    }

    void AddDwords(std::uintptr_t base, std::initializer_list<std::uint32_t> values)
    {
        std::vector<std::uint8_t> bytes(values.size() * 4);
        std::size_t               index = 0;
        for (const std::uint32_t value : values)
        {
            std::memcpy(bytes.data() + index * 4, &value, 4);
            ++index;
        }
        Add(base, std::move(bytes));
    }

    [[nodiscard]] MemoryReader Reader()
    {
        return MemoryReader{ &FakeMemory::Read, this };
    }

private:
    struct Region
    {
        std::uintptr_t            base;
        std::vector<std::uint8_t> bytes;
    };

    static bool Read(void* context, std::uintptr_t address, void* out, std::size_t size)
    {
        for (const Region& region : static_cast<FakeMemory*>(context)->regions_)
        {
            if (address >= region.base && address - region.base + size <= region.bytes.size())
            {
                std::memcpy(out, region.bytes.data() + (address - region.base), size);
                return true;
            }
        }
        return false;
    }

    std::vector<Region> regions_;
};

bool Contains(const std::string& text, std::string_view needle)
{
    return text.find(needle) != std::string::npos;
}

void TestTextBuffer()
{
    char       storage[8];
    TextBuffer out(storage, sizeof(storage));
    out.AppendHex(0xabcu, 8);
    ExpectText(out.view(), "00000ABC", "AppendHex pads to the minimum digits in uppercase");
    out.AppendChar('x');
    ExpectText(out.view(), "00000ABC", "output past the capacity is dropped");

    char       wide[32];
    TextBuffer text(wide, sizeof(wide));
    text.AppendHex(0);
    text.AppendChar(' ');
    text.AppendDecimal(0);
    text.AppendChar(' ');
    text.AppendDecimal(4294967295u);
    text.AppendChar(' ');
    text.AppendDecimal(7, 4);
    ExpectText(text.view(), "0 0 4294967295 0007", "hex and decimal formatting");

    char       ascii[32];
    TextBuffer clean(ascii, sizeof(ascii));
    clean.AppendAscii(std::string_view("a\tb\x80z", 5));
    const wchar_t unicode[] = { L'o', L'k', 0x00e9, L'!' };
    clean.AppendWide(unicode, 4);
    ExpectText(clean.view(), "a?b?zok?!", "non-printable bytes become question marks");
}

void TestNames()
{
    struct Case
    {
        std::uint32_t    code;
        std::string_view name;
    };

    const Case cases[] = {
        { 0xc0000005, "access violation" },
        { 0xc00000fd, "stack overflow" },
        { 0xc000001d, "illegal instruction" },
        { 0xc0000094, "integer divide by zero" },
        { 0xc0000096, "privileged instruction" },
        { 0xc0000006, "in-page error" },
        { 0xc0000374, "heap corruption" },
        { 0xc0000409, "stack buffer overrun" },
        { 0xe06d7363, "unhandled C++ exception" },
        { 0x12345678, "" },
    };
    for (const Case& entry : cases)
    {
        ExpectText(ExceptionName(entry.code), entry.name, "exception code name");
    }
    ExpectText(AccessKindName(0), "read", "read access kind");
    ExpectText(AccessKindName(1), "write", "write access kind");
    ExpectText(AccessKindName(8), "execute", "execute access kind");
    ExpectText(AccessKindName(5), "unknown", "other access kind");
}

void TestStem()
{
    const UtcTime time{ 2026, 1, 2, 3, 4, 5 };
    char          storage[64];
    TextBuffer    stem(storage, sizeof(storage));
    AppendFileStem(stem, time, 1234);
    ExpectText(stem.view(), "game-20260102T030405Z-1234", "file stem uses UTC digits and the process id");
}

template <std::size_t Capacity>
void SetText(char (&destination)[Capacity], std::string_view text)
{
    std::memcpy(destination, text.data(), text.size());
    destination[text.size()] = '\0';
}

std::unique_ptr<ModuleTable> SampleModules()
{
    auto  table                 = std::make_unique<ModuleTable>();
    auto& game                  = table->entries[0];
    game.base                   = 0x00400000;
    game.sizeOfImage            = 0x00a00000;
    game.timeDateStamp          = 0x5f000000;
    game.codeBegin              = 0x1000;
    game.codeEnd                = 0x800000;
    game.hasPdb                 = true;
    const std::uint8_t guid[16] = { 0x44, 0x33, 0x22, 0x11, 0x66, 0x55, 0x88, 0x77, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00 };
    std::memcpy(game.pdbGuid, guid, sizeof(guid));
    game.pdbAge = 3;
    SetText(game.name, "game.exe");
    SetText(game.pdbName, "game.pdb");
    auto& d3d         = table->entries[1];
    d3d.base          = 0x6c000000;
    d3d.sizeOfImage   = 0x00200000;
    d3d.timeDateStamp = 0x4a000001;
    d3d.codeBegin     = 0x1000;
    d3d.codeEnd       = 0x100000;
    SetText(d3d.name, "d3d9.dll");
    table->count = 2;
    return table;
}

void TestModuleLookup()
{
    const auto modules = SampleModules();
    char       storage[64];
    const auto offset = [&](std::uintptr_t address)
    {
        TextBuffer out(storage, sizeof(storage));
        AppendModuleOffset(out, *modules, address);
        return std::string(out.view());
    };
    ExpectText(offset(0x00400000), "game.exe+0x0", "module base is offset zero");
    ExpectText(offset(0x00401234), "game.exe+0x1234", "offset inside the first module");
    ExpectText(offset(0x00dfffff), "game.exe+0x9FFFFF", "last byte of the image");
    ExpectText(offset(0x00e00000), "?", "first byte past the image");
    ExpectText(offset(0x003fffff), "?", "byte before the image");
    ExpectText(offset(0x6c001000), "d3d9.dll+0x1000", "offset inside the second module");
    ExpectText(offset(0), "?", "null address");
    Expect(IsCodeAddress(*modules, 0x00401000), "start of the executable range is code");
    Expect(!IsCodeAddress(*modules, 0x00400fff), "header bytes below the code range are not code");
    Expect(!IsCodeAddress(*modules, 0x00c00000), "end of the executable range is exclusive");
    Expect(!IsCodeAddress(*modules, 0x7fff0000), "an address outside every module is not code");
}

// A mapped 32-bit image: headers, three sections, and a debug directory with
// a non-CodeView entry followed by an RSDS record.
class SyntheticImage
{
public:
    static constexpr std::uintptr_t kBase = 0x10000000;

    SyntheticImage()
    : bytes_(0x5000, 0)
    {
        auto& dos                                                    = At<IMAGE_DOS_HEADER>(0);
        dos.e_magic                                                  = IMAGE_DOS_SIGNATURE;
        dos.e_lfanew                                                 = 0x80;
        auto& nt                                                     = At<IMAGE_NT_HEADERS32>(0x80);
        nt.Signature                                                 = IMAGE_NT_SIGNATURE;
        nt.FileHeader.Machine                                        = IMAGE_FILE_MACHINE_I386;
        nt.FileHeader.NumberOfSections                               = 3;
        nt.FileHeader.TimeDateStamp                                  = 0x5f3e2a10;
        nt.FileHeader.SizeOfOptionalHeader                           = sizeof(IMAGE_OPTIONAL_HEADER32);
        nt.OptionalHeader.Magic                                      = IMAGE_NT_OPTIONAL_HDR32_MAGIC;
        nt.OptionalHeader.SizeOfImage                                = 0x5000;
        nt.OptionalHeader.NumberOfRvaAndSizes                        = IMAGE_NUMBEROF_DIRECTORY_ENTRIES;
        nt.OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_DEBUG] = { 0x4000, 2 * sizeof(IMAGE_DEBUG_DIRECTORY) };

        const std::size_t sections = 0x80 + offsetof(IMAGE_NT_HEADERS32, OptionalHeader) + sizeof(IMAGE_OPTIONAL_HEADER32);
        auto&             text     = At<IMAGE_SECTION_HEADER>(sections);
        std::memcpy(text.Name, ".text", 5);
        text.VirtualAddress   = 0x1000;
        text.Misc.VirtualSize = 0x1800;
        text.SizeOfRawData    = 0x2000;
        text.Characteristics  = IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ;
        auto& rdata           = At<IMAGE_SECTION_HEADER>(sections + sizeof(IMAGE_SECTION_HEADER));
        std::memcpy(rdata.Name, ".rdata", 6);
        rdata.VirtualAddress   = 0x3000;
        rdata.Misc.VirtualSize = 0x1000;
        rdata.Characteristics  = IMAGE_SCN_MEM_READ;
        auto& init             = At<IMAGE_SECTION_HEADER>(sections + 2 * sizeof(IMAGE_SECTION_HEADER));
        std::memcpy(init.Name, ".init", 5);
        init.VirtualAddress  = 0x4800;
        init.SizeOfRawData   = 0x100;
        init.Characteristics = IMAGE_SCN_MEM_EXECUTE;

        auto& other               = At<IMAGE_DEBUG_DIRECTORY>(0x4000);
        other.Type                = IMAGE_DEBUG_TYPE_MISC;
        auto&      codeView       = At<IMAGE_DEBUG_DIRECTORY>(0x4000 + sizeof(IMAGE_DEBUG_DIRECTORY));
        const char pdb[]          = "C:\\build\\out\\Game.pdb";
        codeView.Type             = IMAGE_DEBUG_TYPE_CODEVIEW;
        codeView.AddressOfRawData = 0x4100;
        codeView.SizeOfData       = 24 + sizeof(pdb);
        std::memcpy(&bytes_[0x4100], "RSDS", 4);
        const std::uint8_t guid[16] = { 0x44, 0x33, 0x22, 0x11, 0x66, 0x55, 0x88, 0x77, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00 };
        std::memcpy(&bytes_[0x4104], guid, sizeof(guid));
        const std::uint32_t age = 7;
        std::memcpy(&bytes_[0x4114], &age, sizeof(age));
        std::memcpy(&bytes_[0x4118], pdb, sizeof(pdb));
    }

    template <typename Value>
    Value& At(std::size_t offset)
    {
        return *reinterpret_cast<Value*>(&bytes_[offset]);
    }

    [[nodiscard]] MemoryReader Reader()
    {
        return MemoryReader{ &SyntheticImage::Read, this };
    }

    std::vector<std::uint8_t>& Bytes()
    {
        return bytes_;
    }

private:
    static bool Read(void* context, std::uintptr_t address, void* out, std::size_t size)
    {
        auto& image = *static_cast<SyntheticImage*>(context);
        if (address < kBase || address - kBase + size > image.bytes_.size())
        {
            return false;
        }
        std::memcpy(out, &image.bytes_[address - kBase], size);
        return true;
    }

    std::vector<std::uint8_t> bytes_;
};

void TestModuleImage()
{
    {
        SyntheticImage image;
        ModuleRecord   record;
        Expect(ReadModuleImage(image.Reader(), SyntheticImage::kBase, record), "a valid image parses");
        Expect(record.sizeOfImage == 0x5000, "SizeOfImage is read");
        Expect(record.timeDateStamp == 0x5f3e2a10, "TimeDateStamp is read");
        Expect(record.codeBegin == 0x1000 && record.codeEnd == 0x4900, "executable sections form one span and a zero VirtualSize falls back to the raw size");
        Expect(record.hasPdb, "the CodeView entry after a non-CodeView entry is found");
        Expect(record.pdbAge == 7, "PDB age");
        ExpectText(record.pdbName, "Game.pdb", "PDB file name without its directory");
        const std::uint8_t guid[16] = { 0x44, 0x33, 0x22, 0x11, 0x66, 0x55, 0x88, 0x77, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00 };
        Expect(std::memcmp(record.pdbGuid, guid, sizeof(guid)) == 0, "PDB GUID bytes");
    }
    {
        SyntheticImage image;
        image.At<IMAGE_NT_HEADERS32>(0x80).OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_DEBUG] = {};
        ModuleRecord record;
        Expect(ReadModuleImage(image.Reader(), SyntheticImage::kBase, record) && !record.hasPdb, "an image without a debug directory has no PDB");
    }
    {
        SyntheticImage image;
        std::memcpy(&image.Bytes()[0x4100], "NB10", 4);
        ModuleRecord record;
        Expect(ReadModuleImage(image.Reader(), SyntheticImage::kBase, record) && !record.hasPdb, "a debug record that is not RSDS is ignored");
    }
    {
        SyntheticImage image;
        image.At<IMAGE_DOS_HEADER>(0).e_magic = 0;
        ModuleRecord record;
        Expect(!ReadModuleImage(image.Reader(), SyntheticImage::kBase, record), "a missing MZ signature is rejected");
    }
    {
        SyntheticImage image;
        image.At<IMAGE_NT_HEADERS32>(0x80).OptionalHeader.Magic = IMAGE_NT_OPTIONAL_HDR64_MAGIC;
        ModuleRecord record;
        Expect(!ReadModuleImage(image.Reader(), SyntheticImage::kBase, record), "a 64-bit optional header is rejected");
    }
    {
        SyntheticImage image;
        ModuleRecord   record;
        Expect(!ReadModuleImage(image.Reader(), SyntheticImage::kBase + 0x10000, record), "an unreadable base is rejected");
    }
}

void TestReports()
{
    const auto modules = SampleModules();
    FakeMemory memory;
    memory.AddDwords(0x0019fe00, { 0x00000010, 0x00402000, 0x6c001000, 0x6c000800 });

    EXCEPTION_RECORD record{};
    record.ExceptionCode           = EXCEPTION_ACCESS_VIOLATION;
    record.ExceptionAddress        = reinterpret_cast<PVOID>(0x00401234);
    record.NumberParameters        = 2;
    record.ExceptionInformation[0] = 1;
    record.ExceptionInformation[1] = 0x10;
    CONTEXT context{};
    context.Eax    = 1;
    context.Ebx    = 2;
    context.Ecx    = 3;
    context.Edx    = 4;
    context.Esi    = 5;
    context.Edi    = 6;
    context.Ebp    = 0x0019ff00;
    context.Esp    = 0x0019fe00;
    context.Eip    = 0x00401234;
    context.EFlags = 0x00010246;
    context.SegCs  = 0x23;
    context.SegSs  = 0x2b;

    std::string out;
    const auto  append = [](void* target, const char* data, std::size_t size)
    {
        static_cast<std::string*>(target)->append(data, size);
    };

    ReportInput input;
    input.time               = UtcTime{ 2026, 1, 2, 3, 4, 5 };
    input.processId          = 1234;
    input.threadId           = 5678;
    input.launcherVersion    = "1.2.3";
    input.startupAddons      = "fps,pos";
    input.record             = &record;
    input.context            = &context;
    input.modules            = modules.get();
    input.reader             = memory.Reader();
    input.stackSnapshotBytes = 4080;
    WriteReport(ByteSink{ append, &out }, input);
    ExpectText(out,
               "Bahamut game crash log\n"
               "time_utc: 2026-01-02T03:04:05Z\n"
               "launcher_version: 1.2.3\n"
               "process_id: 1234\n"
               "thread_id: 5678\n"
               "startup_addon: fps\n"
               "startup_addon: pos\n"
               "exception_code: 0xC0000005 (access violation)\n"
               "exception_access: write at 0x00000010\n"
               "fault_address: 0x00401234 game.exe+0x1234\n"
               "eax: 0x00000001\n"
               "ebx: 0x00000002\n"
               "ecx: 0x00000003\n"
               "edx: 0x00000004\n"
               "esi: 0x00000005\n"
               "edi: 0x00000006\n"
               "ebp: 0x0019FF00\n"
               "esp: 0x0019FE00\n"
               "eip: 0x00401234\n"
               "eflags: 0x00010246\n"
               "cs: 0x00000023\n"
               "ss: 0x0000002B\n"
               "stack_snapshot_bytes: 4080\n"
               "stack esp+0x0004: 0x00402000 game.exe+0x2000\n"
               "stack esp+0x0008: 0x6C001000 d3d9.dll+0x1000\n"
               "module game.exe base=0x00400000 size_of_image=0x00A00000 time_date_stamp=0x5F000000 pdb_guid=11223344-5566-7788-99AA-BBCCDDEEFF00 pdb_age=3 pdb=game.pdb\n"
               "module d3d9.dll base=0x6C000000 size_of_image=0x00200000 time_date_stamp=0x4A000001\n"
               "end of crash log\n",
               "full report with modules and stack scan");

    EXCEPTION_RECORD paging{};
    paging.ExceptionCode           = EXCEPTION_IN_PAGE_ERROR;
    paging.ExceptionAddress        = reinterpret_cast<PVOID>(0x10);
    paging.NumberParameters        = 3;
    paging.ExceptionInformation[0] = 0;
    paging.ExceptionInformation[1] = 0x1000;
    paging.ExceptionInformation[2] = 0xc000009c;
    ReportInput minimal;
    minimal.time      = UtcTime{ 2026, 12, 31, 23, 59, 58 };
    minimal.processId = 7;
    minimal.threadId  = 8;
    minimal.record    = &paging;
    const auto none   = std::make_unique<ModuleTable>();
    minimal.modules   = none.get();
    out.clear();
    WriteReport(ByteSink{ append, &out }, minimal);
    ExpectText(out,
               "Bahamut game crash log\n"
               "time_utc: 2026-12-31T23:59:58Z\n"
               "launcher_version: unknown\n"
               "process_id: 7\n"
               "thread_id: 8\n"
               "exception_code: 0xC0000006 (in-page error)\n"
               "exception_access: read at 0x00001000\n"
               "exception_status: 0xC000009C\n"
               "fault_address: 0x00000010 ?\n"
               "end of crash log\n",
               "report without listed modules, context or version");

    EXCEPTION_RECORD unknown{};
    unknown.ExceptionCode = 0x80000003;
    minimal.record        = &unknown;
    out.clear();
    WriteReport(ByteSink{ append, &out }, minimal);
    Expect(out.find("exception_code: 0x80000003\n") != std::string::npos && out.find("exception_access") == std::string::npos, "an unnamed code prints bare hex and no access line");

    minimal.record                 = nullptr;
    minimal.startupAddons          = "a,b";
    minimal.startupAddonsTruncated = true;
    out.clear();
    WriteReport(ByteSink{ append, &out }, minimal);
    Expect(Contains(out, "startup_addon: a\nstartup_addon: b\nstartup_addons_truncated: yes\n"), "each addon gets a line and a truncation is marked");
}

std::wstring BaseName(const std::wstring& path)
{
    return path.substr(path.find_last_of(L"\\/") + 1);
}

std::string NarrowAscii(const std::wstring& text)
{
    std::string narrow;
    for (const wchar_t value : text)
    {
        narrow.push_back(static_cast<char>(value));
    }
    return narrow;
}

void TestProcessReader()
{
    const MemoryReader reader  = ProcessMemoryReader();
    const char         local[] = "readable";
    char               copy[sizeof(local)]{};
    Expect(reader.Read(reinterpret_cast<std::uintptr_t>(local), copy, sizeof(local)) && std::strcmp(copy, local) == 0, "readable memory is copied");
    Expect(!reader.Read(0, copy, 1), "the null page is unreadable");
    Expect(reader.Read(reinterpret_cast<std::uintptr_t>(local), copy, 0), "an empty read succeeds");

    SYSTEM_INFO system{};
    GetSystemInfo(&system);
    const SIZE_T page = system.dwPageSize;
    auto* const  base = static_cast<std::uint8_t*>(VirtualAlloc(nullptr, page * 4, MEM_RESERVE, PAGE_NOACCESS));
    if (!Expect(base != nullptr, "reserve test pages"))
    {
        return;
    }
    VirtualAlloc(base, page, MEM_COMMIT, PAGE_READWRITE);
    VirtualAlloc(base + page, page, MEM_COMMIT, PAGE_READWRITE | PAGE_GUARD);
    VirtualAlloc(base + page * 2, page, MEM_COMMIT, PAGE_NOACCESS);
    std::uint8_t byte[8]{};
    const auto   address = reinterpret_cast<std::uintptr_t>(base);
    Expect(reader.Read(address, byte, 1), "a committed read-write page is readable");
    Expect(reader.Read(address + page - 4, byte, 4), "a read ending at the page end is readable");
    Expect(!reader.Read(address + page - 2, byte, 4), "a read crossing into a guard page fails");
    Expect(!reader.Read(address + page, byte, 1), "a guard page is unreadable and keeps its guard");
    Expect(!reader.Read(address + page * 2, byte, 1), "a no-access page is unreadable");
    Expect(!reader.Read(address + page * 3, byte, 1), "a reserved page is unreadable");
    VirtualFree(base, 0, MEM_RELEASE);
}

void TestCaptureModules()
{
    const MemoryReader reader  = ProcessMemoryReader();
    const auto         modules = std::make_unique<ModuleTable>();
    CaptureModules(*modules, reader);
    Expect(modules->count >= 3, "the loader list holds the executable, ntdll and kernel32");

    wchar_t executable[MAX_PATH]{};
    GetModuleFileNameW(nullptr, executable, MAX_PATH);
    const std::string   exeName = NarrowAscii(BaseName(executable));
    const auto          exeBase = reinterpret_cast<std::uintptr_t>(GetModuleHandleW(nullptr));
    const ModuleRecord* self    = FindModule(*modules, exeBase);
    if (Expect(self != nullptr, "the executable is in the captured list"))
    {
        ExpectText(self->name, exeName, "the executable name is the loader base name");
        Expect(self->base == exeBase && self->sizeOfImage != 0 && self->timeDateStamp != 0, "the executable has an image size and a timestamp");
        Expect(IsCodeAddress(*modules, reinterpret_cast<std::uintptr_t>(&TestCaptureModules)), "this function lies in the executable's code range");
        char       storage[96];
        TextBuffer out(storage, sizeof(storage));
        AppendModuleOffset(out, *modules, reinterpret_cast<std::uintptr_t>(&TestCaptureModules));
        Expect(out.view().substr(0, exeName.size() + 3) == exeName + "+0x", "a code address in the executable resolves to name+0xOFFSET");
    }

    const auto          kernel   = reinterpret_cast<std::uintptr_t>(GetModuleHandleW(L"kernel32.dll"));
    const ModuleRecord* kernel32 = FindModule(*modules, kernel);
    if (Expect(kernel32 != nullptr, "kernel32 is in the captured list"))
    {
        Expect(_stricmp(kernel32->name, "kernel32.dll") == 0, "the loader base name is the module file name");
    }
}

std::string ReadFile(const fs::path& path)
{
    std::ifstream stream(path, std::ios::binary);
    return std::string(std::istreambuf_iterator<char>(stream), std::istreambuf_iterator<char>());
}

std::atomic<int>  g_previousCalls{ 0 };
std::atomic<int>  g_gameCalls{ 0 };
std::atomic<int>  g_nestedChained{ 0 };
std::atomic<bool> g_callNested{ true };
std::atomic<int>  g_incompleteAtChain{ 0 };
HANDLE            g_release = nullptr;
fs::path          g_logDirectory;

std::vector<fs::path> FilesWithExtension(const fs::path& directory, const wchar_t* extension)
{
    std::vector<fs::path> files;
    std::error_code       error;
    for (fs::directory_iterator entry(directory, error), end; !error && entry != end; entry.increment(error))
    {
        if (entry->path().extension() == extension)
        {
            files.push_back(entry->path());
        }
    }
    return files;
}

std::size_t CountFiles(const fs::path& directory)
{
    std::size_t     count = 0;
    std::error_code error;
    for (fs::directory_iterator entry(directory, error), end; !error && entry != end; entry.increment(error))
    {
        ++count;
    }
    return count;
}

bool LogComplete()
{
    const auto logs = FilesWithExtension(g_logDirectory, L".log");
    return logs.size() == 1 && Contains(ReadFile(logs[0]), "end of crash log\n");
}

bool WaitForLog()
{
    for (int attempt = 0; attempt < 1000 && !LogComplete(); ++attempt)
    {
        Sleep(10);
    }
    return LogComplete();
}

LONG WINAPI PreviousFilter(EXCEPTION_POINTERS*)
{
    ++g_previousCalls;
    return EXCEPTION_CONTINUE_SEARCH;
}

// Model a game filter chaining back to CrashFilter; g_release holds the thread inside.
LONG WINAPI GameFilter(EXCEPTION_POINTERS* pointers)
{
    ++g_gameCalls;
    if (!LogComplete())
    {
        ++g_incompleteAtChain;
    }
    if (g_callNested && CrashFilter(pointers) != EXCEPTION_CONTINUE_SEARCH)
    {
        ++g_nestedChained;
    }
    if (g_release != nullptr)
    {
        WaitForSingleObject(g_release, 20000);
    }
    return EXCEPTION_EXECUTE_HANDLER;
}

LONG WINAPI OtherFilter(EXCEPTION_POINTERS*)
{
    return EXCEPTION_CONTINUE_SEARCH;
}

struct Crash
{
    explicit Crash(DWORD code)
    {
        RtlCaptureContext(&context);
        record.ExceptionCode           = code;
        record.ExceptionAddress        = reinterpret_cast<PVOID>(&Crash::Marker);
        record.NumberParameters        = 2;
        record.ExceptionInformation[0] = 1;
        record.ExceptionInformation[1] = 0x10;
        pointers                       = { &record, &context };
    }

    static void Marker()
    {
    }

    EXCEPTION_RECORD   record{};
    CONTEXT            context{};
    EXCEPTION_POINTERS pointers{};
};

fs::path PrepareDirectory(const fs::path& root)
{
    std::error_code error;
    fs::remove_all(root, error);
    g_logDirectory = root / L"nested" / L"logs";
    return g_logDirectory;
}

void TestInstallAndFilter(const fs::path& root)
{
    const fs::path directory = PrepareDirectory(root);

    Expect(!Install({ std::wstring_view(), "1", "" }), "an empty directory leaves crash logging off");

    const auto original = SetUnhandledExceptionFilter(&PreviousFilter);

    // A failed Install must leave both filters inert and allow a later capture.
    const fs::path blocker = root / L"blocker";
    fs::create_directories(root);
    std::ofstream(blocker).put('x');
    Expect(!Install({ (blocker / L"logs").native(), "9.9.9", "" }), "a directory that cannot be created leaves crash logging off");
    ReassertFilterOnce();
    Expect(SetUnhandledExceptionFilter(&PreviousFilter) == &PreviousFilter, "reassert after a failed Install installs nothing");
    {
        const auto early = std::make_unique<Crash>(EXCEPTION_ACCESS_VIOLATION);
        Expect(CrashFilter(&early->pointers) == EXCEPTION_CONTINUE_SEARCH && !fs::exists(directory), "the filter writes nothing before a successful Install");
    }

    Expect(Install({ directory.native(), "9.9.9", "fps,pos" }), "Install succeeds");
    Expect(fs::is_directory(directory), "Install creates the missing directory");
    Expect(!Install({ directory.native(), "9.9.9", "" }), "a second Install is refused");

    Expect(SetUnhandledExceptionFilter(&GameFilter) == &CrashFilter, "the crash filter is the installed filter");
    ReassertFilterOnce();
    Expect(SetUnhandledExceptionFilter(&OtherFilter) == &CrashFilter, "reassert installs the crash filter again");
    ReassertFilterOnce();
    Expect(SetUnhandledExceptionFilter(&OtherFilter) == &OtherFilter, "a second reassert does not reinstall the filter");

    // Concurrent threads must wait for one capture, chain and survive re-entry.
    constexpr int kThreads = 17;
    g_release              = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    const auto crash       = std::make_unique<Crash>(EXCEPTION_ACCESS_VIOLATION);

    std::vector<DWORD>       ids(kThreads, 0);
    std::vector<LONG>        results(kThreads, 0);
    std::vector<std::thread> threads;
    for (int index = 0; index < kThreads; ++index)
    {
        threads.emplace_back(
            [&, index]
            {
                ids[index]     = GetCurrentThreadId();
                results[index] = CrashFilter(&crash->pointers);
            });
    }
    for (int attempt = 0; attempt < 1000 && g_gameCalls < kThreads; ++attempt)
    {
        Sleep(10);
    }
    Expect(g_gameCalls == kThreads, "every concurrent faulting thread reaches the chained filter");
    SetEvent(g_release);
    for (auto& thread : threads)
    {
        thread.join();
    }

    bool allChained = true;
    for (const LONG result : results)
    {
        allChained = allChained && result == EXCEPTION_EXECUTE_HANDLER;
    }
    Expect(allChained, "every thread returns the chained filter's result");
    Expect(g_previousCalls == 0, "only the current chained filter ran");
    Expect(g_nestedChained == 0, "a chained filter that calls back into the crash filter gets CONTINUE_SEARCH without recursion");
    Expect(g_incompleteAtChain == 0, "every thread chains only after the capture finished");

    const auto logs = FilesWithExtension(directory, L".log");
    Expect(logs.size() == 1, "one log is written");
    Expect(CountFiles(directory) == 1, "the log is the only file");
    if (logs.size() == 1)
    {
        const std::string stem = NarrowAscii(logs[0].stem().wstring());
        Expect(std::regex_match(stem, std::regex("game-[0-9]{8}T[0-9]{6}Z-" + std::to_string(GetCurrentProcessId()))), "the stem is game-<UTC time>-<pid>");

        const std::string log = ReadFile(logs[0]);
        Expect(log.rfind("Bahamut game crash log\n", 0) == 0, "the log starts with its header");
        Expect(Contains(log, "launcher_version: 9.9.9\n") && Contains(log, "startup_addon: fps\nstartup_addon: pos\n"), "install-time version and addons are logged");
        Expect(Contains(log, "process_id: " + std::to_string(GetCurrentProcessId()) + "\n"), "the process id is logged");
        bool faultingThreadLogged = false;
        for (const DWORD id : ids)
        {
            faultingThreadLogged = faultingThreadLogged || Contains(log, "thread_id: " + std::to_string(id) + "\n");
        }
        Expect(faultingThreadLogged, "a faulting thread, not the capture thread, is logged");
        Expect(Contains(log, "exception_code: 0xC0000005 (access violation)\n") && Contains(log, "exception_access: write at 0x00000010\n"), "the exception is logged");
        wchar_t executable[MAX_PATH]{};
        GetModuleFileNameW(nullptr, executable, MAX_PATH);
        const std::string exeName = NarrowAscii(BaseName(executable));
        Expect(Contains(log, "module " + exeName + " base=0x"), "the module list is logged");
    }

    // A full thread array must still wait and chain, not report re-entry.
    constexpr int kManyThreads = 70;
    g_callNested               = false;
    CloseHandle(g_release);
    g_release = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    std::vector<LONG>        manyResults(kManyThreads, 0);
    std::vector<std::thread> manyThreads;
    for (int index = 0; index < kManyThreads; ++index)
    {
        manyThreads.emplace_back([&, index]
                                 {
                                     manyResults[index] = CrashFilter(&crash->pointers);
                                 });
    }
    for (int attempt = 0; attempt < 1000 && g_gameCalls < kThreads + kManyThreads; ++attempt)
    {
        Sleep(10);
    }
    Expect(g_gameCalls == kThreads + kManyThreads, "all 70 threads are inside the chained filter at once, including those without a slot");
    SetEvent(g_release);
    for (auto& thread : manyThreads)
    {
        thread.join();
    }
    bool manyChained = true;
    for (const LONG result : manyResults)
    {
        manyChained = manyChained && result == EXCEPTION_EXECUTE_HANDLER;
    }
    Expect(manyChained, "threads that find the slot array full still return the chained filter's result");
    Expect(g_incompleteAtChain == 0, "no thread chains before the capture finished");
    g_callNested = true;

    CloseHandle(g_release);
    g_release = nullptr;
    Expect(CrashFilter(&crash->pointers) == EXCEPTION_EXECUTE_HANDLER && g_gameCalls == kThreads + kManyThreads + 1, "a later crash still chains");
    Expect(FilesWithExtension(directory, L".log").size() == 1, "only the first crash is written");

    SetUnhandledExceptionFilter(original);
}

void* g_fakeStack = nullptr;

// Simulate stack unwinding by overwriting it just after signaling capture.
LONG WINAPI ClobberFilter(EXCEPTION_POINTERS*)
{
    ++g_previousCalls;
    std::memset(g_fakeStack, 0xdd, 0x8000);
    VirtualFree(g_fakeStack, 0, MEM_RELEASE);
    g_fakeStack = nullptr;
    return EXCEPTION_CONTINUE_SEARCH;
}

// With no wait, the originals are overwritten during capture; only the snapshot
// ending 4 KiB above ESP must be read.
void TestCaptureReadsCopies(const fs::path& root)
{
    const fs::path directory = PrepareDirectory(root);
    SetUnhandledExceptionFilter(&ClobberFilter);
    std::string addons;
    // Short ids: more than 64 fit inside the install cut.
    for (int index = 0; index < 150; ++index)
    {
        addons += (index == 0 ? "a" : ",a") + std::to_string(1000 + index).substr(1);
    }
    InstallOptions options{ directory.native(), "1.0.0", addons };
    options.captureTimeoutMs = 0;
    Expect(Install(options), "Install succeeds");

    g_fakeStack = VirtualAlloc(nullptr, 0x8000, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (!Expect(g_fakeStack != nullptr, "the fake stack is allocated"))
    {
        return;
    }
    const auto base = reinterpret_cast<std::uintptr_t>(g_fakeStack);
    const auto put  = [&](std::uintptr_t address, std::uint32_t first, std::uint32_t second)
    {
        const std::uint32_t pair[2] = { first, second };
        std::memcpy(reinterpret_cast<void*>(address), pair, sizeof(pair));
    };
    const std::uint32_t marker = static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(&Crash::Marker));
    put(base + 0x20, marker, 0);
    put(base + 0x100c, marker, marker); // last dword of the snapshot, then one past it

    const auto crash   = std::make_unique<Crash>(EXCEPTION_ACCESS_VIOLATION);
    crash->context.Eax = 0x1234abcd;
    crash->context.Esp = static_cast<DWORD>(base + 0x10);
    Expect(CrashFilter(&crash->pointers) == EXCEPTION_CONTINUE_SEARCH && g_previousCalls == 1, "a zero timeout chains without waiting");
    std::memset(&crash->record, 0xdd, sizeof(crash->record));
    std::memset(&crash->context, 0xdd, sizeof(crash->context));
    crash->pointers = { nullptr, nullptr };

    if (Expect(WaitForLog(), "the capture finishes after the faulting thread has left"))
    {
        const std::string log = ReadFile(FilesWithExtension(directory, L".log")[0]);
        Expect(Contains(log, "exception_code: 0xC0000005 (access violation)\n") && Contains(log, "exception_access: write at 0x00000010\n"), "the copied exception record is logged");
        Expect(Contains(log, "eax: 0x1234ABCD\n"), "the copied context is logged");
        Expect(Contains(log, "thread_id: " + std::to_string(GetCurrentThreadId()) + "\n"), "the faulting thread id is logged");
        Expect(Contains(log, "stack_snapshot_bytes: 4096\n"), "the stack snapshot is bounded to 4 KiB and logged");
        Expect(Contains(log, "stack esp+0x0010: 0x") && Contains(log, "stack esp+0x0FFC: 0x"), "the stack scan reads the snapshot after the original stack was destroyed");
        Expect(!Contains(log, "stack esp+0x1000: "), "a dword past the snapshot is not scanned");

        std::size_t lines = 0, whole = 0;
        for (std::size_t at = log.find("startup_addon: "); at != std::string::npos; at = log.find("startup_addon: ", at + 1))
        {
            ++lines;
        }
        const std::regex wholeLine("startup_addon: a[0-9]{3}\n");
        for (std::sregex_iterator match(log.begin(), log.end(), wholeLine), end; match != end; ++match)
        {
            ++whole;
        }
        Expect(lines > 64 && lines < 150 && lines == whole, "every addon inside the cut is logged and the list is cut at a whole id");
        Expect(Contains(log, "startup_addons_truncated: yes\n"), "the cut addon list is marked");
    }
}

} // namespace

int wmain(int argc, wchar_t** argv)
{
    if (argc < 2 || argc > 3)
    {
        std::cerr << "usage: bahamut-crash-log-tests <scratch directory> [copy]\n";
        return 2;
    }
    const std::wstring scenario = argc == 3 ? argv[2] : L"";
    const fs::path     root(argv[1]);
    try
    {
        if (scenario.empty())
        {
            TestTextBuffer();
            TestNames();
            TestStem();
            TestModuleLookup();
            TestModuleImage();
            TestReports();
            TestProcessReader();
            TestCaptureModules();
            TestInstallAndFilter(root);
        }
        else if (scenario == L"copy")
        {
            TestCaptureReadsCopies(root);
        }
        else
        {
            std::cerr << "unknown scenario\n";
            return 2;
        }
    }
    catch (const std::exception& failure)
    {
        std::cerr << "FAIL: unexpected exception: " << failure.what() << '\n';
        return 1;
    }
    if (g_failures != 0)
    {
        std::cerr << g_failures << " crash log check(s) failed\n";
        return 1;
    }
    std::cout << "crash log tests passed\n";
    return 0;
}
