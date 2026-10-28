#include "login_reader.h"
#include "fault_guard.h"

#include <windows.h>

#include <cstddef>
#include <cstring>

namespace bahamut_client
{
namespace
{

// Retail 1.23b ffxivgame.exe SHA-256
// 9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9, fixed
// image base. Container +0x4BC is the MainElement interface whose scene
// switch FUN_005659E0 stores the current scene at +0xA0 and its id at +0x94.
// Bootup scenes return BootupManager+0x34 (driver vptr 0x00FD25E0).
// FUN_006AF200 stores the phase at manager +0x1C98; phase cells start at
// +0x1C2C. CharacterSelectionPhase (vptr 0x010558AC) is phase cell 5.
constexpr std::size_t   kMainElementOffset  = 0x4BCu;
constexpr std::size_t   kSceneIdOffset      = 0x94u;
constexpr std::size_t   kCurrentSceneOffset = 0xA0u;
constexpr std::uint32_t kBootupDriverVptr   = 0x00FD25E0u;
constexpr std::size_t   kDriverToManager    = 0x34u;
constexpr std::size_t   kPhaseOffset        = 0x1C98u;
constexpr std::size_t   kPhaseCellsOffset   = 0x1C2Cu;
constexpr std::size_t   kRosterPhaseCell    = 5u;
constexpr std::uint32_t kCharacterSelection = 0x010558ACu;
constexpr std::size_t   kClosingOffset      = 0x218u;

// Login requests replay what the client's own Start dialog and character list
// queue on the Bootup manager Q.
constexpr std::size_t   kTitlePhaseCell     = 1u;
constexpr std::uint32_t kTitlePhaseVptr     = 0x010533ACu;
constexpr std::size_t   kPhaseOwnerOffset   = 0x224u; // phase object -> Q
constexpr std::size_t   kTitleOwnerOffset   = 0x4u;
constexpr std::size_t   kStartReadyOffset   = 0x175A4u;
constexpr std::size_t   kStartReadyFlag     = 0x34u;
constexpr std::size_t   kModeFlagsOffset    = 0x1CA0u;
constexpr std::uint8_t  kAlternateModeBit   = 0x20u;
constexpr std::size_t   kRosterBeginOffset  = 0x1BF8u;
constexpr std::size_t   kRosterEndOffset    = 0x1BFCu;
constexpr std::uint32_t kRosterRecordSize   = 0x2E0u;
constexpr std::size_t   kRecordIdOffset     = 0x04u;
constexpr std::size_t   kRecordWorldOffset  = 0x08u;
constexpr std::size_t   kRecordFlagsOffset  = 0x09u;
constexpr std::size_t   kRecordSubBegin     = 0x2D4u;
constexpr std::size_t   kRecordSubEnd       = 0x2D8u;
constexpr std::uint32_t kSubRecordSize      = 0x30u;
constexpr std::size_t   kSubRecordFlags     = 0x0Au;
constexpr std::uint32_t kWarningMaskAddress = 0x012BDEF4u;
constexpr std::uint32_t kVetoMaskAddress    = 0x012D7170u;
constexpr std::uint32_t kStartAccepted      = 0x00886440u; // cdecl (TitlePhase*, int result)
constexpr std::uint32_t kEnqueue            = 0x006AE490u; // thiscall (Q, selector)
constexpr std::uint32_t kEnqueuePayload     = 0x006AE530u; // thiscall (Q, selector, payload, length)

using StartAcceptedFunction  = void(__cdecl*)(void* title, std::int32_t result);
using EnqueueFunction        = void(__thiscall*)(void* manager, std::uint32_t selector);
using EnqueuePayloadFunction = void(__thiscall*)(void* manager, std::uint32_t selector, const void* payload, std::uint32_t length);

SRWLOCK           gLock    = SRWLOCK_INIT;
SRWLOCK           gRunLock = SRWLOCK_INIT; // held while a request runs
LoginScreen       gScreen;
std::uint32_t     gNextId  = 0;
std::uint32_t     gId      = 0;
LoginRequest      gRequest = LoginRequest::None;
std::uint32_t     gSlot    = 0;
LoginRequestState gState   = LoginRequestState::None;
const char*       gReason  = "";

template <typename T>
bool Read(std::uint32_t base, std::size_t offset, T& value)
{
    if (base == 0)
    {
        return false;
    }
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(&value, reinterpret_cast<const void*>(base + offset), sizeof(value));
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

// Reject calls whose function prefixes differ from the supported client.
bool CodeMatches()
{
    static const struct
    {
        std::uint32_t address;
        std::uint8_t  bytes[8];
    } kCode[] = {
        { kStartAccepted, { 0x83, 0x7C, 0x24, 0x08, 0x00, 0x75, 0x70, 0x56 } },
        { kEnqueue, { 0x6A, 0xFF, 0x68, 0xA3, 0x01, 0xE9, 0x00, 0x64 } },
        { kEnqueuePayload, { 0x6A, 0xFF, 0x68, 0xC6, 0x01, 0xE9, 0x00, 0x64 } },
    };

    for (const auto& code : kCode)
    {
        std::uint8_t actual[8]{};
        if (!Read(code.address, 0, actual) || std::memcmp(actual, code.bytes, sizeof(actual)) != 0)
        {
            return false;
        }
    }
    return true;
}

struct RequestOutcome
{
    bool        ran    = false; // false keeps the request pending
    bool        queued = false;
    const char* reason = "";
};

RequestOutcome Rejected(const char* reason)
{
    return { true, false, reason };
}

// Calling the title Start confirmation's accept callback with result 0
// enqueues login.
RequestOutcome RunTitle(std::uint32_t manager)
{
    std::uint32_t title     = 0;
    std::uint32_t titleVptr = 0;
    std::uint32_t titleQ    = 0;
    std::uint32_t owner     = 0;
    if (!Read(manager, kPhaseCellsOffset + kTitlePhaseCell * 4, title) || !Read(title, 0, titleVptr) || titleVptr != kTitlePhaseVptr || !Read(title, kPhaseOwnerOffset, titleQ) || titleQ != manager || !Read(title, kTitleOwnerOffset, owner))
    {
        return {};
    }
    // The Start handler acts only once this flag is set.
    std::uint32_t ready     = 0;
    std::uint8_t  readyFlag = 0;
    if (!Read(owner, kStartReadyOffset, ready) || !Read(ready, kStartReadyFlag, readyFlag) || readyFlag == 0)
    {
        return {};
    }
    BAHAMUT_FAULT_TRY
    {
        reinterpret_cast<StartAcceptedFunction>(kStartAccepted)(reinterpret_cast<void*>(title), 0);
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return Rejected("title start faulted");
    }
    return { true, true, "title start accepted" };
}

// Apply the list's admission checks before queuing its three commands.
RequestOutcome RunSlot(std::uint32_t manager, std::uint32_t slot)
{
    std::uint8_t modeFlags = 0;
    if (!Read(manager, kModeFlagsOffset, modeFlags))
    {
        return Rejected("mode unreadable");
    }
    if ((modeFlags & kAlternateModeBit) != 0)
    {
        return Rejected("list in alternate mode");
    }
    std::uint32_t begin = 0;
    std::uint32_t end   = 0;
    if (!Read(manager, kRosterBeginOffset, begin) || !Read(manager, kRosterEndOffset, end) || begin == 0 || end < begin)
    {
        return Rejected("roster unreadable");
    }
    if (slot == 0 || slot - 1 >= (end - begin) / kRosterRecordSize)
    {
        return Rejected("slot past roster end");
    }
    const std::uint32_t record   = begin + (slot - 1) * kRosterRecordSize;
    std::uint32_t       id       = 0;
    std::uint8_t        world    = 0;
    std::uint8_t        flags    = 0;
    std::uint8_t        warnMask = 0;
    if (!Read(record, kRecordIdOffset, id) || !Read(record, kRecordWorldOffset, world) || !Read(record, kRecordFlagsOffset, flags) || !Read(kWarningMaskAddress, 0, warnMask))
    {
        return Rejected("record unreadable");
    }
    if (id == 0)
    {
        return Rejected("slot is empty");
    }
    if ((flags & warnMask) != 0)
    {
        return Rejected("slot needs a warning dialog");
    }
    std::uint32_t subBegin = 0;
    std::uint32_t subEnd   = 0;
    std::uint16_t vetoMask = 0;
    if (!Read(record, kRecordSubBegin, subBegin) || !Read(record, kRecordSubEnd, subEnd) || !Read(kVetoMaskAddress, 0, vetoMask) || subEnd < subBegin)
    {
        return Rejected("record unreadable");
    }
    for (std::uint32_t sub = subBegin; sub != 0 && subEnd - sub >= kSubRecordSize; sub += kSubRecordSize)
    {
        std::uint16_t subFlags = 0;
        if (!Read(sub, kSubRecordFlags, subFlags))
        {
            return Rejected("record unreadable");
        }
        if ((subFlags & vetoMask) != 0)
        {
            return Rejected("slot vetoed by the list");
        }
    }
    // Native payloads leave padding uninitialized; the consumers read only the
    // fields set here.
    const std::uint8_t select[12] = { 0x01, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x00, 0x00, 0x00 };
    std::uint8_t       submit[8]{};
    submit[0] = world;
    std::memcpy(submit + 4, &id, sizeof(id));
    void* const q = reinterpret_cast<void*>(manager);
    BAHAMUT_FAULT_TRY
    {
        reinterpret_cast<EnqueueFunction>(kEnqueue)(q, 0x13u);
        reinterpret_cast<EnqueuePayloadFunction>(kEnqueuePayload)(q, 0x28u, select, sizeof(select));
        reinterpret_cast<EnqueuePayloadFunction>(kEnqueuePayload)(q, 0x26u, submit, sizeof(submit));
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return Rejected("slot submit faulted");
    }
    return { true, true, "slot submitted" };
}

} // namespace

void CaptureLoginScreen(const void* container)
{
    LoginScreen   next;
    const auto    root       = static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(container));
    std::uint32_t main       = 0;
    std::uint32_t scene      = 0;
    std::uint32_t driverVptr = 0;
    std::uint32_t manager    = 0;
    if (Read(root, kMainElementOffset, main) && Read(main, kSceneIdOffset, next.sceneId) && Read(main, kCurrentSceneOffset, scene) && Read(scene, 0, driverVptr) && driverVptr == kBootupDriverVptr)
    {
        manager                  = scene - kDriverToManager;
        next.bootup              = Read(manager, kPhaseOffset, next.phase);
        std::uint32_t roster     = 0;
        std::uint32_t rosterVptr = 0;
        if (Read(manager, kPhaseCellsOffset + kRosterPhaseCell * 4, roster) && Read(roster, 0, rosterVptr) && rosterVptr == kCharacterSelection)
        {
            next.roster = Read(roster, kClosingOffset, next.closing);
        }
    }
    next.capturedAt = GetTickCount64();

    // Runs after the client's update on this thread. The run lock makes a
    // replacement or clear wait until a running request has finished.
    AcquireSRWLockExclusive(&gRunLock);
    AcquireSRWLockExclusive(&gLock);
    const LoginRequest  request = gState == LoginRequestState::Pending ? gRequest : LoginRequest::None;
    const std::uint32_t slot    = gSlot;
    ReleaseSRWLockExclusive(&gLock);

    RequestOutcome outcome;
    if (request != LoginRequest::None && next.bootup)
    {
        if (!CodeMatches())
        {
            outcome = Rejected("client code mismatch");
        }
        else if (request == LoginRequest::Title && next.phase == 1u)
        {
            outcome = RunTitle(manager);
        }
        else if (request == LoginRequest::Slot && next.phase == kRosterPhaseCell && next.roster && next.closing == 0)
        {
            outcome = RunSlot(manager, slot);
        }
    }

    AcquireSRWLockExclusive(&gLock);
    if (outcome.ran)
    {
        gState  = outcome.queued ? LoginRequestState::Queued : LoginRequestState::Rejected;
        gReason = outcome.reason;
    }
    next.requestId     = gId;
    next.request       = gState;
    next.requestReason = gReason;
    gScreen            = next;
    ReleaseSRWLockExclusive(&gLock);
    ReleaseSRWLockExclusive(&gRunLock);
}

std::uint32_t RequestLogin(LoginRequest request, std::uint32_t slot)
{
    AcquireSRWLockExclusive(&gRunLock);
    AcquireSRWLockExclusive(&gLock);
    std::uint32_t id = 0;
    if (request != LoginRequest::None)
    {
        id = ++gNextId == 0 ? ++gNextId : gNextId;
    }
    gId      = id;
    gRequest = request;
    gSlot    = slot;
    gState   = request == LoginRequest::None ? LoginRequestState::None : LoginRequestState::Pending;
    gReason  = "";
    ReleaseSRWLockExclusive(&gLock);
    ReleaseSRWLockExclusive(&gRunLock);
    return id;
}

LoginScreen ReadLoginScreen()
{
    AcquireSRWLockShared(&gLock);
    const LoginScreen screen = gScreen;
    ReleaseSRWLockShared(&gLock);
    return screen;
}

} // namespace bahamut_client
