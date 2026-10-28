#pragma once

#include "equipment_state.h"

#include <cstdint>
#include <filesystem>
#include <functional>
#include <optional>
#include <string>
#include <string_view>

namespace bahamut_client
{

// Debug switches for the gearsets addon. Both are off unless set, and neither
// does anything unless the gearsets addon is loaded and healthy.
inline constexpr wchar_t kGearsetsSnapshotFileEnvironment[] = L"BAHAMUT_TEST_GEARSETS_SNAPSHOT_FILE";
inline constexpr wchar_t kGearsetsEquipEnvironment[]        = L"BAHAMUT_TEST_GEARSETS_EQUIP";

struct GearsetsDebugSwitches
{
    std::filesystem::path snapshotFile; // empty: no receipt
    bool                  equip = false;

    [[nodiscard]] bool Any() const
    {
        return !snapshotFile.empty() || equip;
    }
};

// The receipt path must be absolute; the equip switch must be exactly "1".
// Any other value leaves that switch off.
GearsetsDebugSwitches ParseGearsetsDebugSwitches(const std::optional<std::wstring>& snapshotFile,
                                                 const std::optional<std::wstring>& equip);

// Receipt text. Schema 1 is a header of '#' lines, CSV records, and a footer.
inline constexpr unsigned int kGearsetsReceiptSchema = 1u;

enum class GearsetsEquipPhase : std::uint8_t
{
    Queued,   // accepted, waiting for the game thread
    Resolved, // bag uid captured; invoked only after a later recheck
    Invoked,  // the native call returned true; waiting for a snapshot
    Complete,
    Rejected,
    Unconfirmed,
    Refused, // the native invocation is not available in this build
};

enum class GearsetsEquipReason : std::uint8_t
{
    None,
    Disabled,
    Busy,
    InvalidSlot,
    InvalidUid,
    StaleGeneration,
    BagUnavailable,
    BagEmpty,
    UidMismatch,
    AlreadyEquipped,
    InvokeUnavailable,
    BindingFalse,
    SessionEnded,
    Confirmed,
    Timeout,
};

std::string_view GearsetsEquipPhaseName(GearsetsEquipPhase phase);
std::string_view GearsetsEquipReasonName(GearsetsEquipReason reason);
bool             IsTerminalGearsetsEquipPhase(GearsetsEquipPhase phase);

// equipSlot is the 1-based wire slot of the Equip command request; it must
// be one of kEquipmentSlots plus one.
bool IsSupportedGearsetsEquipSlot(std::uint8_t equipSlot);

struct GearsetsEquipRequest
{
    std::uint64_t                generation = 0;
    std::uint16_t                bagSlot    = 0;
    std::uint8_t                 equipSlot  = 0;
    std::optional<std::uint64_t> expectedUid;
};

struct GearsetsEquipStatus
{
    std::uint32_t                id         = 0;
    GearsetsEquipPhase           phase      = GearsetsEquipPhase::Queued;
    GearsetsEquipReason          reason     = GearsetsEquipReason::None;
    std::uint64_t                generation = 0;
    std::uint16_t                bagSlot    = 0;
    std::uint8_t                 equipSlot  = 0;
    std::optional<std::uint64_t> expectedUid;
    std::uint32_t                catalogId   = 0;
    std::uint32_t                invocations = 0;
    std::optional<bool>          returnValue;
    std::uint64_t                submitFrame = 0;
    std::uint64_t                invokeFrame = 0;
    std::uint64_t                finishFrame = 0;
    std::uint64_t                deadlineMs  = 0;
};

struct GearsetsSubmitResult
{
    bool                accepted = false;
    GearsetsEquipReason reason   = GearsetsEquipReason::None;
    std::uint32_t       id       = 0;
};

enum class GearsetsInvokeResult : std::uint8_t
{
    ReturnedTrue,
    ReturnedFalse,
};

// One game-thread frame as the transaction sees it. readBag performs the
// recheck read; an empty invoke means no native invocation exists.
struct GearsetsFrameView
{
    std::uint64_t                                                    frame      = 0;
    std::uint64_t                                                    nowMs      = 0;
    std::uint64_t                                                    generation = 0;
    std::function<std::optional<BagItem>(std::uint16_t)>             readBag;
    const EquipmentSnapshot*                                         snapshot = nullptr;
    std::function<GearsetsInvokeResult(std::uint16_t, std::uint8_t)> invoke;
};

// Single explicit equip transaction. Not thread-safe; the owner serializes.
class GearsetsEquipMachine
{
public:
    // Bahamut replied 146-282 ms after each request in the G0 capture and
    // the store applies changes on later frames, so 5 s bounds confirmation.
    static constexpr std::uint64_t kConfirmTimeoutMs = 5000u;

    GearsetsSubmitResult Submit(const GearsetsEquipRequest& request, bool enabled, std::uint64_t frame);
    // Advances one game frame; returns true when the status changed.
    bool Step(const GearsetsFrameView& view);
    // Session end, unload, or fault: an unsent request is rejected and an
    // invoked one becomes Unconfirmed. Returns true when the status changed.
    bool EndSession(std::uint64_t frame);

    [[nodiscard]] bool                               Active() const;
    [[nodiscard]] std::optional<GearsetsEquipStatus> Status() const;

private:
    bool Finish(GearsetsEquipPhase phase, GearsetsEquipReason reason, std::uint64_t frame);

    std::optional<GearsetsEquipStatus> status_;
    std::uint32_t                      nextId_ = 1;
};

std::string FormatGearsetsReceiptHeader(std::string_view clientSha256, std::uint32_t processId);
std::string FormatGearsetsSnapshotRecord(std::uint64_t sequence, std::uint64_t frame, const EquipmentSnapshot& snapshot);
std::string FormatGearsetsUnavailableRecord(std::uint64_t sequence, std::uint64_t frame);
std::string FormatGearsetsEquipRecord(std::uint64_t sequence, std::uint64_t frame, const GearsetsEquipStatus& status);
std::string FormatGearsetsReceiptFooter(std::uint64_t records, std::uint64_t dropped, std::string_view reason);
std::string FormatGearsetsUid(std::uint64_t uid);

// The gearsets debug surface the addon host consumes. Implemented by
// GearsetsDebugService; tests may supply their own.
class GearsetsDebugPort
{
public:
    virtual ~GearsetsDebugPort() = default;

    [[nodiscard]] virtual bool EquipEnabled() const = 0;
    // Called on every load, reload, unload, and fault of the gearsets addon,
    // from the thread that changed it, which may be inside a game detour.
    // Implementations must not block on file I/O here.
    virtual void OnAddonStateChanged(bool active) = 0;
    // Render thread, once per addon host update.
    virtual void                                             OnRenderFrame()                                                                                   = 0;
    virtual GearsetsSubmitResult                             SubmitDebugEquip(std::uint16_t bagSlot, std::uint8_t equipSlot, std::optional<std::uint64_t> uid) = 0;
    [[nodiscard]] virtual std::optional<GearsetsEquipStatus> DebugEquipStatus() const                                                                          = 0;
};

} // namespace bahamut_client
