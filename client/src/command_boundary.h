#pragma once

#include <windows.h>

#include <cstdint>
#include <string_view>

class AddonHost;

inline constexpr std::uintptr_t kTextCommandLookupRva               = 0x0016D3C0u;
inline constexpr std::uintptr_t kTopLevelTextCommandLookupReturnRva = 0x0016E6A4u;

enum class CommandBoundaryInstallResult
{
    Disabled,
    Installed,
    RollbackFailed,
};

using CommandLookupFunction      = int(__thiscall*)(void*       parser,
                                                    const void* token,
                                                    void*       auxiliaryOutput);
using CommandLookupProbeFunction = int (*)(void*       parser,
                                           const void* token,
                                           void*       auxiliaryOutput);
using CommandDispatchFunction    = bool (*)(void*            context,
                                            std::string_view command);

struct CommandBoundarySnapshot
{
    LONG  lookupEntries         = 0;
    LONG  lookupRecognized      = 0;
    LONG  dispatchCalls         = 0;
    LONG  dispatchHandled       = 0;
    LONG  dispatchExceptions    = 0;
    LONG  lookupForwarded       = 0;
    LONG  lookupSyntheticMisses = 0;
    DWORD lastLookupThreadId    = 0;
    LONG  lastLookupResult      = 0;
    LONG  sequence              = 0;
};

class CommandBoundary
{
public:
    explicit CommandBoundary(AddonHost* addonHost);
    CommandBoundary(void* context, CommandDispatchFunction dispatch);

    CommandBoundary(const CommandBoundary&)            = delete;
    CommandBoundary& operator=(const CommandBoundary&) = delete;

    static bool RecognizesAddonCommand(const void* token);

    int HandleLookup(CommandLookupFunction original, void* parser, const void* token, void* auxiliaryOutput, bool topLevelCall = true, const void* tokenRangeBegin = nullptr, const void* tokenRangeEnd = nullptr);
    int HandleLookupForTest(CommandLookupProbeFunction original, void* parser, const void* token, void* auxiliaryOutput, bool topLevelCall = true, const void* tokenRangeBegin = nullptr, const void* tokenRangeEnd = nullptr);

    CommandBoundaryInstallResult Install();
    bool                         Shutdown();

    CommandBoundarySnapshot Snapshot() const;
    void                    ResetDiagnostics();

private:
    void RecordEvent(volatile LONG* counter);
    void RecordDiagnosticSnapshot() const;
    int  HandleLookupCore(int (*forward)(void* context, void* parser, const void* token, void* auxiliaryOutput), void* forwardContext, void* parser, const void* token, void* auxiliaryOutput, bool topLevelCall, const void* tokenRangeBegin, const void* tokenRangeEnd);

    void*                   context_        = nullptr;
    AddonHost*              addonHost_      = nullptr;
    CommandDispatchFunction dispatch_       = nullptr;
    void*                   lookupTarget_   = nullptr;
    CommandLookupFunction   originalLookup_ = nullptr;
    bool                    installed_      = false;

    volatile LONG lookupEntries_         = 0;
    volatile LONG lookupRecognized_      = 0;
    volatile LONG dispatchCalls_         = 0;
    volatile LONG dispatchHandled_       = 0;
    volatile LONG dispatchExceptions_    = 0;
    volatile LONG lookupForwarded_       = 0;
    volatile LONG lookupSyntheticMisses_ = 0;
    volatile LONG lastLookupThreadId_    = 0;
    volatile LONG lastLookupResult_      = 0;
    volatile LONG sequence_              = 0;
};

bool                         CommandBoundaryLookupSignatureMatches(const void* address);
CommandBoundaryInstallResult InstallCommandBoundary(AddonHost* addonHost);
bool                         ShutdownCommandBoundary();
