#include "command_boundary.h"
#include "fault_guard.h"

#include "addon_host.h"
#include "retail_identity.h"
#include "runtime_contract.h"

#include <MinHook.h>

#include <algorithm>
#include <array>
#include <cstring>
#include <fstream>
#include <intrin.h>
#include <string_view>

namespace
{

// Retail exact-build disassembly calls this lookup at 0x0056E69F after the
// parser verifies a slash token.
constexpr std::array<unsigned char, 40> kLookupSignature = {
    0x6A,
    0xFF,
    0x68,
    0x74,
    0xF6,
    0xE6,
    0x00,
    0x64,
    0xA1,
    0x00,
    0x00,
    0x00,
    0x00,
    0x50,
    0x83,
    0xEC,
    0x54,
    0x53,
    0x56,
    0x57,
    0xA1,
    0xB0,
    0xA8,
    0x2E,
    0x01,
    0x33,
    0xC4,
    0x50,
    0x8D,
    0x44,
    0x24,
    0x64,
    0x64,
    0xA3,
    0x00,
    0x00,
    0x00,
    0x00,
    0x8B,
    0xF1,
};
CommandBoundary* gInstalledBoundary = nullptr;

bool HasValue(const wchar_t* name)
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(name, value, ARRAYSIZE(value));
    return count != 0 && count < ARRAYSIZE(value) && value[0] != L'0';
}

bool IsAsciiSpace(unsigned char value)
{
    return value == ' ' || value == '\t' || value == '\r' || value == '\n' || value == '\v' || value == '\f';
}

constexpr std::size_t kCommandTokenSize          = 0x54;
constexpr std::size_t kMaximumCommandTokenLength = 128;
constexpr std::size_t kMaximumCommandTokenCount  = 16;
constexpr std::size_t kMaximumCommandLength      = 512;

// The retail parser's token vector begins at parser-frame [ESP+0x28] and ends
// at [ESP+0x2C]. Lookup entry is 0x0C below that frame baseline.
constexpr std::size_t kCallerTokenBeginOffset = 0x34;
constexpr std::size_t kCallerTokenEndOffset   = 0x38;

bool ReadBoundedInput(const unsigned char* input, char* output, std::size_t capacity, std::size_t& length)
{
    if (input == nullptr)
    {
        return false;
    }
    BAHAMUT_FAULT_TRY
    {
        for (std::size_t index = 0; index != capacity; ++index)
        {
            const unsigned char value = input[index];
            if (value == '\0')
            {
                output[index] = '\0';
                length        = index;
                return true;
            }
            output[index] = static_cast<char>(value);
        }
        return false;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool ReadAsciiCommandBoundary(const unsigned char* input, unsigned char& boundary)
{
    if (input == nullptr)
    {
        return false;
    }
    std::array<char, kMaximumCommandTokenLength + 1> storage{};
    std::size_t                                      length = 0;
    if (!ReadBoundedInput(input, storage.data(), storage.size(), length))
    {
        return false;
    }
    const std::string_view text(storage.data(), length);
    for (const std::string_view command : {
             "/pos", "/fps", "/wiki", "/distance", "/targethp", "/packetlogger", "/combatparser" })
    {
        if (text.starts_with(command))
        {
            boundary = text.size() == command.size()
                           ? '\0'
                           : static_cast<unsigned char>(text[command.size()]);
            return true;
        }
    }
    return false;
}

const unsigned char* ReadTokenText(const void* token)
{
    if (token == nullptr)
    {
        return nullptr;
    }
    BAHAMUT_FAULT_TRY
    {
        // FUN_00445060 returns the byte pointer in the command token's first
        // field; FUN_0056E380 checks that byte for '/' before this lookup.
        return *static_cast<const unsigned char* const*>(token);
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return nullptr;
    }
}

bool MatchesAddonCommandToken(const void* token)
{
    const auto*   text          = ReadTokenText(token);
    unsigned char asciiBoundary = 0;
    return ReadAsciiCommandBoundary(text, asciiBoundary) && (asciiBoundary == '\0' || IsAsciiSpace(asciiBoundary));
}

bool ReadCallerTokenRange(const void*  returnAddressSlot,
                          const void*& begin,
                          const void*& end)
{
    const auto* frame = static_cast<const unsigned char*>(returnAddressSlot);
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(&begin, frame + kCallerTokenBeginOffset, sizeof(begin));
        std::memcpy(&end, frame + kCallerTokenEndOffset, sizeof(end));
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
    return true;
}

bool ReadCommandToken(const void* token, std::string_view& text, std::array<char, kMaximumCommandTokenLength + 1>& storage)
{
    std::size_t length = 0;
    if (!ReadBoundedInput(ReadTokenText(token), storage.data(), storage.size(), length))
    {
        return false;
    }
    text = std::string_view(storage.data(), length);
    return true;
}

bool ReadCanonicalAddonCommand(const void* token, const void* tokenRangeBegin, const void* tokenRangeEnd, std::string& canonical)
{
    canonical.clear();
    const auto begin = reinterpret_cast<std::uintptr_t>(tokenRangeBegin);
    const auto end   = reinterpret_cast<std::uintptr_t>(tokenRangeEnd);
    if (tokenRangeBegin != token || begin == 0 || end < begin)
    {
        return false;
    }
    const std::uintptr_t span = end - begin;
    if (span == 0 || span % kCommandTokenSize != 0)
    {
        return false;
    }
    const std::size_t count = span / kCommandTokenSize;
    if (count == 0 || count > kMaximumCommandTokenCount)
    {
        return false;
    }

    std::array<std::array<char, kMaximumCommandTokenLength + 1>,
               kMaximumCommandTokenCount>
                                                            storage{};
    std::array<std::string_view, kMaximumCommandTokenCount> tokens{};
    for (std::size_t index = 0; index != count; ++index)
    {
        const auto* element = reinterpret_cast<const void*>(
            begin + index * kCommandTokenSize);
        if (!ReadCommandToken(element, tokens[index], storage[index]))
        {
            return false;
        }
    }
    if ((tokens[0] == "/fps" || tokens[0] == "/distance" || tokens[0] == "/targethp") &&
        count == 3 && (tokens[1] == "color" || tokens[1] == "size"))
    {
        canonical.assign(tokens[0]);
        canonical.push_back(' ');
        canonical.append(tokens[1]);
        canonical.push_back(' ');
        canonical.append(tokens[2]);
        return true;
    }
    if (tokens[0] == "/fps")
    {
        if (count == 1)
        {
            canonical = "/fps";
            return true;
        }
        if (count == 2 && tokens[1] == "help")
        {
            canonical = "/fps help";
            return true;
        }
        if (count == 2 && tokens[1] == "lock")
        {
            canonical = "/fps lock";
            return true;
        }
        return false;
    }
    if (tokens[0] == "/pos")
    {
        if (count == 1)
        {
            canonical = "/pos";
            return true;
        }
        if (count == 2 && (tokens[1] == "help" || tokens[1] == "lock"))
        {
            canonical = tokens[1] == "help" ? "/pos help" : "/pos lock";
            return true;
        }
        return false;
    }
    for (const std::string_view command : {
             "/distance", "/targethp" })
    {
        if (tokens[0] == command)
        {
            if (count == 2 && (tokens[1] == "lock" ||
                               (tokens[1] == "help" &&
                                (command == "/distance" || command == "/targethp"))))
            {
                canonical.assign(command);
                canonical += tokens[1] == "lock" ? " lock" : " help";
                return true;
            }
            return false;
        }
    }
    if (tokens[0] == "/packetlogger")
    {
        if (count == 1)
        {
            canonical = "/packetlogger";
            return true;
        }
        if (count == 2 && (tokens[1] == "start" || tokens[1] == "stop" || tokens[1] == "status"))
        {
            canonical = "/packetlogger ";
            canonical += tokens[1];
            return true;
        }
        return false;
    }
    if (tokens[0] == "/combatparser")
    {
        if (count == 1)
        {
            canonical = "/combatparser";
            return true;
        }
        if (count == 2 && (tokens[1] == "mode" || tokens[1] == "reset" || tokens[1] == "lock" ||
                           tokens[1] == "help"))
        {
            canonical = "/combatparser ";
            canonical += tokens[1];
            return true;
        }
        return false;
    }
    if (tokens[0] != "/wiki")
    {
        return false;
    }
    if (count == 1)
    {
        canonical = "/wiki";
        return true;
    }
    if (count == 2 && tokens[1] == "help")
    {
        canonical = "/wiki help";
        return true;
    }
    canonical.assign(tokens[0].data(), tokens[0].size());
    for (std::size_t index = 1; index != count; ++index)
    {
        if (canonical.size() + 1 + tokens[index].size() > kMaximumCommandLength)
        {
            canonical.clear();
            return false;
        }
        canonical.push_back(' ');
        canonical.append(tokens[index].data(), tokens[index].size());
    }
    return true;
}

bool Matches(const unsigned char* address,
             const unsigned char* signature,
             std::size_t          size)
{
    BAHAMUT_FAULT_TRY
    {
        return std::memcmp(address, signature, size) == 0;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

template <std::size_t N>
std::size_t CountExecutableMatches(HMODULE                             image,
                                   const std::array<unsigned char, N>& signature,
                                   const unsigned char*                expectedAddress)
{
    if (image == nullptr)
    {
        return 0;
    }

    BAHAMUT_FAULT_TRY
    {
        const auto* base = reinterpret_cast<const unsigned char*>(image);
        const auto* dos  = reinterpret_cast<const IMAGE_DOS_HEADER*>(base);
        if (dos->e_magic != IMAGE_DOS_SIGNATURE || dos->e_lfanew <= 0)
        {
            return 0;
        }
        const auto* nt = reinterpret_cast<const IMAGE_NT_HEADERS32*>(
            base + static_cast<std::size_t>(dos->e_lfanew));
        if (nt->Signature != IMAGE_NT_SIGNATURE || nt->OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR32_MAGIC)
        {
            return 0;
        }
        const DWORD imageSize    = nt->OptionalHeader.SizeOfImage;
        const WORD  sectionCount = nt->FileHeader.NumberOfSections;
        if (imageSize == 0 || sectionCount == 0 || sectionCount > 96)
        {
            return 0;
        }
        const auto* section = IMAGE_FIRST_SECTION(nt);
        std::size_t matches = 0;
        for (WORD index = 0; index != sectionCount; ++index)
        {
            if ((section[index].Characteristics & IMAGE_SCN_MEM_EXECUTE) == 0)
            {
                continue;
            }
            const DWORD address     = section[index].VirtualAddress;
            const DWORD sectionSize = std::max(section[index].Misc.VirtualSize,
                                               section[index].SizeOfRawData);
            if (address >= imageSize || sectionSize > imageSize - address || sectionSize < N)
            {
                continue;
            }
            const auto* begin = base + address;
            const auto* end   = begin + sectionSize - N + 1;
            for (const auto* cursor = begin; cursor != end; ++cursor)
            {
                if (Matches(cursor, signature.data(), N))
                {
                    ++matches;
                    if (cursor != expectedAddress && matches > 1)
                    {
                        return matches;
                    }
                }
            }
        }
        return matches;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return 0;
    }
}

bool IsExactRetailBoundaryTarget(void* address, std::uintptr_t rva)
{
    const auto image = GetModuleHandleW(nullptr);
    if (image == nullptr || reinterpret_cast<std::uintptr_t>(image) != bahamut_runtime_contract::kRetailImageBase || reinterpret_cast<std::uintptr_t>(address) != bahamut_runtime_contract::kRetailImageBase + rva)
    {
        return false;
    }
    MEMORY_BASIC_INFORMATION memory{};
    if (VirtualQuery(address, &memory, sizeof(memory)) == 0 || memory.State != MEM_COMMIT || (memory.Protect & 0xff) != PAGE_EXECUTE_READ)
    {
        return false;
    }
    return true;
}

void* ResolveLookupTarget()
{
    if (HasValue(L"BAHAMUT_RUNTIME_TEST_STUB") || !VerifyRuntimeHostIdentity())
    {
        return nullptr;
    }
    const auto base   = bahamut_runtime_contract::kRetailImageBase;
    auto*      lookup = reinterpret_cast<void*>(base + kTextCommandLookupRva);
    if (!IsExactRetailBoundaryTarget(lookup, kTextCommandLookupRva))
    {
        return nullptr;
    }
    if (!CommandBoundaryLookupSignatureMatches(lookup))
    {
        return nullptr;
    }
    const auto image = GetModuleHandleW(nullptr);
    if (CountExecutableMatches(image, kLookupSignature, static_cast<const unsigned char*>(lookup)) != 1)
    {
        return nullptr;
    }
    return lookup;
}

int __fastcall HookedLookup(void* parser, void*, const void* token, void* auxiliaryOutput)
{
    if (gInstalledBoundary == nullptr)
    {
        return -2;
    }
    const void* returnAddressSlot = _AddressOfReturnAddress();
    const auto  returnAddress     = reinterpret_cast<std::uintptr_t>(_ReturnAddress());
    const bool  topLevelCall      = returnAddress == bahamut_runtime_contract::kRetailImageBase + kTopLevelTextCommandLookupReturnRva;
    const void* tokenRangeBegin   = nullptr;
    const void* tokenRangeEnd     = nullptr;
    if (topLevelCall)
    {
        static_cast<void>(ReadCallerTokenRange(returnAddressSlot,
                                               tokenRangeBegin,
                                               tokenRangeEnd));
    }
    return gInstalledBoundary->HandleLookup(nullptr, parser, token, auxiliaryOutput, topLevelCall, tokenRangeBegin, tokenRangeEnd);
}

bool DispatchToAddonHost(void* context, std::string_view command)
{
    auto* host = static_cast<AddonHost*>(context);
    return host != nullptr && host->DispatchCommand(command);
}

CommandBoundary& ProductionBoundary(AddonHost* addonHost)
{
    static CommandBoundary boundary(addonHost);
    return boundary;
}

int ForwardThiscallLookup(void* context, void* parser, const void* token, void* auxiliaryOutput)
{
    const auto original = reinterpret_cast<CommandLookupFunction>(context);
    return original == nullptr ? -2 : original(parser, token, auxiliaryOutput);
}

int ForwardProbeLookup(void* context, void* parser, const void* token, void* auxiliaryOutput)
{
    const auto original = reinterpret_cast<CommandLookupProbeFunction>(context);
    return original == nullptr ? -2 : original(parser, token, auxiliaryOutput);
}

void WriteDiagnosticSnapshot(const CommandBoundarySnapshot& snapshot)
{
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(
        L"BAHAMUT_TEST_COMMAND_BOUNDARY_RESULT_FILE", path, ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    try
    {
        std::ofstream file(path, std::ios::trunc);
        if (!file)
        {
            return;
        }
        file << "lookup_entries=" << snapshot.lookupEntries
             << " lookup_recognized=" << snapshot.lookupRecognized
             << " dispatch_calls=" << snapshot.dispatchCalls
             << " dispatch_handled=" << snapshot.dispatchHandled
             << " dispatch_exceptions=" << snapshot.dispatchExceptions
             << " lookup_forwarded=" << snapshot.lookupForwarded
             << " lookup_synthetic_misses=" << snapshot.lookupSyntheticMisses
             << " lookup_thread=" << snapshot.lastLookupThreadId
             << " lookup_result=" << snapshot.lastLookupResult
             << " sequence=" << snapshot.sequence << '\n';
    }
    catch (...)
    {
    }
}

} // namespace

bool CommandBoundaryLookupSignatureMatches(const void* address)
{
    if (address == nullptr)
    {
        return false;
    }
    return Matches(static_cast<const unsigned char*>(address),
                   kLookupSignature.data(),
                   kLookupSignature.size());
}

CommandBoundary::CommandBoundary(AddonHost* addonHost)
: context_(addonHost)
, addonHost_(addonHost)
, dispatch_(&DispatchToAddonHost)
{
}

CommandBoundary::CommandBoundary(void* context, CommandDispatchFunction dispatch)
: context_(context)
, dispatch_(dispatch)
{
}

bool CommandBoundary::RecognizesAddonCommand(const void* token)
{
    return MatchesAddonCommandToken(token);
}

void CommandBoundary::RecordEvent(volatile LONG* counter)
{
    InterlockedIncrement(&sequence_);
    InterlockedIncrement(counter);
}

void CommandBoundary::RecordDiagnosticSnapshot() const
{
    WriteDiagnosticSnapshot(Snapshot());
}

int CommandBoundary::HandleLookup(CommandLookupFunction original, void* parser, const void* token, void* auxiliaryOutput, bool topLevelCall, const void* tokenRangeBegin, const void* tokenRangeEnd)
{
    if (original == nullptr)
    {
        original = originalLookup_;
    }
    return HandleLookupCore(&ForwardThiscallLookup,
                            reinterpret_cast<void*>(original),
                            parser,
                            token,
                            auxiliaryOutput,
                            topLevelCall,
                            tokenRangeBegin,
                            tokenRangeEnd);
}

int CommandBoundary::HandleLookupForTest(CommandLookupProbeFunction original,
                                         void*                      parser,
                                         const void*                token,
                                         void*                      auxiliaryOutput,
                                         bool                       topLevelCall,
                                         const void*                tokenRangeBegin,
                                         const void*                tokenRangeEnd)
{
    return HandleLookupCore(&ForwardProbeLookup,
                            reinterpret_cast<void*>(original),
                            parser,
                            token,
                            auxiliaryOutput,
                            topLevelCall,
                            tokenRangeBegin,
                            tokenRangeEnd);
}

int CommandBoundary::HandleLookupCore(
    int (*forward)(void* context, void* parser, const void* token, void* auxiliaryOutput),
    void*       forwardContext,
    void*       parser,
    const void* token,
    void*       auxiliaryOutput,
    bool        topLevelCall,
    const void* tokenRangeBegin,
    const void* tokenRangeEnd)
{
    RecordEvent(&lookupEntries_);
    InterlockedExchange(&lastLookupThreadId_,
                        static_cast<LONG>(GetCurrentThreadId()));

    const bool recognized = MatchesAddonCommandToken(token);
    if (!topLevelCall || !recognized || dispatch_ == nullptr)
    {
        const int result = forward(forwardContext, parser, token, auxiliaryOutput);
        RecordEvent(&lookupForwarded_);
        InterlockedExchange(&lastLookupResult_, result);
        RecordDiagnosticSnapshot();
        return result;
    }
    RecordEvent(&lookupRecognized_);

    std::string command;
    if (!ReadCanonicalAddonCommand(token, tokenRangeBegin, tokenRangeEnd, command))
    {
        const int result = forward(forwardContext, parser, token, auxiliaryOutput);
        RecordEvent(&lookupForwarded_);
        InterlockedExchange(&lastLookupResult_, result);
        RecordDiagnosticSnapshot();
        return result;
    }

    RecordEvent(&dispatchCalls_);
    bool handled = false;
    try
    {
        handled = dispatch_(context_, command);
    }
    catch (...)
    {
        RecordEvent(&dispatchExceptions_);
    }
    if (handled)
    {
        RecordEvent(&dispatchHandled_);
        RecordEvent(&lookupSyntheticMisses_);
        InterlockedExchange(&lastLookupResult_, -2);
        RecordDiagnosticSnapshot();
        return -2;
    }

    const int result = forward(forwardContext, parser, token, auxiliaryOutput);
    RecordEvent(&lookupForwarded_);
    InterlockedExchange(&lastLookupResult_, result);
    RecordDiagnosticSnapshot();
    return result;
}

CommandBoundaryInstallResult CommandBoundary::Install()
{
    if (installed_)
    {
        return gInstalledBoundary == this
                   ? CommandBoundaryInstallResult::Installed
                   : CommandBoundaryInstallResult::RollbackFailed;
    }
    if (addonHost_ == nullptr || (!addonHost_->IsLoaded("pos") && !addonHost_->IsLoaded("fps") && !addonHost_->IsLoaded("wiki") &&
                                  !addonHost_->IsLoaded("distance") && !addonHost_->IsLoaded("targethp") &&
                                  !addonHost_->IsLoaded("combatparser")))
    {
        return CommandBoundaryInstallResult::Disabled;
    }
    if (gInstalledBoundary != nullptr)
    {
        return CommandBoundaryInstallResult::RollbackFailed;
    }
    void* lookup = ResolveLookupTarget();
    if (lookup == nullptr)
    {
        return CommandBoundaryInstallResult::Disabled;
    }

    CommandLookupFunction originalLookup = nullptr;
    const MH_STATUS       lookupCreated  = MH_CreateHook(lookup,
                                                         reinterpret_cast<LPVOID>(&HookedLookup),
                                                         reinterpret_cast<LPVOID*>(&originalLookup));
    if (lookupCreated != MH_OK)
    {
        return CommandBoundaryInstallResult::Disabled;
    }
    lookupTarget_      = lookup;
    originalLookup_    = originalLookup;
    installed_         = true;
    gInstalledBoundary = this;

    if (MH_EnableHook(lookup) != MH_OK)
    {
        return Shutdown() ? CommandBoundaryInstallResult::Disabled
                          : CommandBoundaryInstallResult::RollbackFailed;
    }
    return CommandBoundaryInstallResult::Installed;
}

bool CommandBoundary::Shutdown()
{
    if (!installed_)
    {
        return true;
    }
    static_cast<void>(MH_DisableHook(lookupTarget_));
    const MH_STATUS removed       = MH_RemoveHook(lookupTarget_);
    const bool      lookupRemoved = removed == MH_OK || removed == MH_ERROR_NOT_CREATED;
    if (!lookupRemoved)
    {
        return false;
    }
    if (gInstalledBoundary == this)
    {
        gInstalledBoundary = nullptr;
    }
    lookupTarget_   = nullptr;
    originalLookup_ = nullptr;
    installed_      = false;
    return true;
}

CommandBoundarySnapshot CommandBoundary::Snapshot() const
{
    CommandBoundarySnapshot snapshot;
    snapshot.lookupEntries = InterlockedCompareExchange(
        const_cast<volatile LONG*>(&lookupEntries_), 0, 0);
    snapshot.lookupRecognized = InterlockedCompareExchange(
        const_cast<volatile LONG*>(&lookupRecognized_), 0, 0);
    snapshot.dispatchCalls = InterlockedCompareExchange(
        const_cast<volatile LONG*>(&dispatchCalls_), 0, 0);
    snapshot.dispatchHandled = InterlockedCompareExchange(
        const_cast<volatile LONG*>(&dispatchHandled_), 0, 0);
    snapshot.dispatchExceptions = InterlockedCompareExchange(
        const_cast<volatile LONG*>(&dispatchExceptions_), 0, 0);
    snapshot.lookupForwarded = InterlockedCompareExchange(
        const_cast<volatile LONG*>(&lookupForwarded_), 0, 0);
    snapshot.lookupSyntheticMisses = InterlockedCompareExchange(
        const_cast<volatile LONG*>(&lookupSyntheticMisses_), 0, 0);
    snapshot.lastLookupThreadId = static_cast<DWORD>(InterlockedCompareExchange(
        const_cast<volatile LONG*>(&lastLookupThreadId_), 0, 0));
    snapshot.lastLookupResult   = InterlockedCompareExchange(
        const_cast<volatile LONG*>(&lastLookupResult_), 0, 0);
    snapshot.sequence = InterlockedCompareExchange(
        const_cast<volatile LONG*>(&sequence_), 0, 0);
    return snapshot;
}

void CommandBoundary::ResetDiagnostics()
{
    InterlockedExchange(&lookupEntries_, 0);
    InterlockedExchange(&lookupRecognized_, 0);
    InterlockedExchange(&dispatchCalls_, 0);
    InterlockedExchange(&dispatchHandled_, 0);
    InterlockedExchange(&dispatchExceptions_, 0);
    InterlockedExchange(&lookupForwarded_, 0);
    InterlockedExchange(&lookupSyntheticMisses_, 0);
    InterlockedExchange(&lastLookupThreadId_, 0);
    InterlockedExchange(&lastLookupResult_, 0);
    InterlockedExchange(&sequence_, 0);
}

CommandBoundaryInstallResult InstallCommandBoundary(AddonHost* addonHost)
{
    return ProductionBoundary(addonHost).Install();
}

bool ShutdownCommandBoundary()
{
    return gInstalledBoundary == nullptr || gInstalledBoundary->Shutdown();
}
