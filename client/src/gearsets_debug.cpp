#include "gearsets_debug.h"

#include <algorithm>
#include <cstdio>
#include <string>

namespace
{

using namespace bahamut_client;

bool SlotHolds(const EquipmentSnapshot& snapshot, std::uint8_t equipSlot, std::uint64_t uid)
{
    const std::uint8_t position = static_cast<std::uint8_t>(equipSlot - 1u);
    return std::any_of(snapshot.slots.begin(), snapshot.slots.end(), [&](const EquipmentSlot& slot)
                       {
                           return slot.slot == position && slot.state == EquipmentSlotState::Item && slot.uid == uid;
                       });
}

std::string_view SlotStateName(EquipmentSlotState state)
{
    switch (state)
    {
        case EquipmentSlotState::Empty:
            return "empty";
        case EquipmentSlotState::Item:
            return "item";
        case EquipmentSlotState::Unknown:
            break;
    }
    return "unknown";
}

std::string Decimal(std::uint64_t value)
{
    return std::to_string(value);
}

} // namespace

namespace bahamut_client
{

GearsetsDebugSwitches ParseGearsetsDebugSwitches(const std::optional<std::wstring>& snapshotFile,
                                                 const std::optional<std::wstring>& equip)
{
    GearsetsDebugSwitches switches;
    if (snapshotFile && !snapshotFile->empty())
    {
        const std::filesystem::path path(*snapshotFile);
        if (path.is_absolute() && path.has_filename())
        {
            switches.snapshotFile = path;
        }
    }
    switches.equip = equip && *equip == L"1";
    return switches;
}

std::string_view GearsetsEquipPhaseName(GearsetsEquipPhase phase)
{
    switch (phase)
    {
        case GearsetsEquipPhase::Queued:
            return "queued";
        case GearsetsEquipPhase::Resolved:
            return "resolved";
        case GearsetsEquipPhase::Invoked:
            return "invoked";
        case GearsetsEquipPhase::Complete:
            return "complete";
        case GearsetsEquipPhase::Rejected:
            return "rejected";
        case GearsetsEquipPhase::Unconfirmed:
            return "unconfirmed";
        case GearsetsEquipPhase::Refused:
            return "refused";
    }
    return "unknown";
}

std::string_view GearsetsEquipReasonName(GearsetsEquipReason reason)
{
    switch (reason)
    {
        case GearsetsEquipReason::None:
            return "none";
        case GearsetsEquipReason::Disabled:
            return "disabled";
        case GearsetsEquipReason::Busy:
            return "busy";
        case GearsetsEquipReason::InvalidSlot:
            return "invalid-slot";
        case GearsetsEquipReason::InvalidUid:
            return "invalid-uid";
        case GearsetsEquipReason::StaleGeneration:
            return "stale-generation";
        case GearsetsEquipReason::BagUnavailable:
            return "bag-unavailable";
        case GearsetsEquipReason::BagEmpty:
            return "bag-empty";
        case GearsetsEquipReason::UidMismatch:
            return "uid-mismatch";
        case GearsetsEquipReason::AlreadyEquipped:
            return "already-equipped";
        case GearsetsEquipReason::InvokeUnavailable:
            return "invoke-unavailable";
        case GearsetsEquipReason::BindingFalse:
            return "binding-false";
        case GearsetsEquipReason::SessionEnded:
            return "session-ended";
        case GearsetsEquipReason::Confirmed:
            return "confirmed";
        case GearsetsEquipReason::Timeout:
            return "timeout";
    }
    return "unknown";
}

bool IsTerminalGearsetsEquipPhase(GearsetsEquipPhase phase)
{
    return phase == GearsetsEquipPhase::Complete || phase == GearsetsEquipPhase::Rejected ||
           phase == GearsetsEquipPhase::Unconfirmed || phase == GearsetsEquipPhase::Refused;
}

bool IsSupportedGearsetsEquipSlot(std::uint8_t equipSlot)
{
    return equipSlot != 0u && std::find(kEquipmentSlots.begin(), kEquipmentSlots.end(), static_cast<std::uint8_t>(equipSlot - 1u)) != kEquipmentSlots.end();
}

GearsetsSubmitResult GearsetsEquipMachine::Submit(const GearsetsEquipRequest& request, bool enabled, std::uint64_t frame)
{
    if (!enabled)
    {
        return { false, GearsetsEquipReason::Disabled, 0 };
    }
    if (!IsSupportedGearsetsEquipSlot(request.equipSlot))
    {
        return { false, GearsetsEquipReason::InvalidSlot, 0 };
    }
    if (request.expectedUid && *request.expectedUid == 0u)
    {
        return { false, GearsetsEquipReason::InvalidUid, 0 };
    }
    if (Active())
    {
        return { false, GearsetsEquipReason::Busy, 0 };
    }
    GearsetsEquipStatus status;
    status.id          = nextId_++;
    status.generation  = request.generation;
    status.bagSlot     = request.bagSlot;
    status.equipSlot   = request.equipSlot;
    status.expectedUid = request.expectedUid;
    status.submitFrame = frame;
    status_            = status;
    return { true, GearsetsEquipReason::None, status.id };
}

bool GearsetsEquipMachine::Step(const GearsetsFrameView& view)
{
    if (!Active())
    {
        return false;
    }
    GearsetsEquipStatus& status = *status_;
    if (status.generation != view.generation)
    {
        return status.phase == GearsetsEquipPhase::Invoked
                   ? Finish(GearsetsEquipPhase::Unconfirmed, GearsetsEquipReason::SessionEnded, view.frame)
                   : Finish(GearsetsEquipPhase::Rejected, GearsetsEquipReason::StaleGeneration, view.frame);
    }
    if (status.phase == GearsetsEquipPhase::Invoked)
    {
        if (view.snapshot != nullptr && SlotHolds(*view.snapshot, status.equipSlot, *status.expectedUid))
        {
            return Finish(GearsetsEquipPhase::Complete, GearsetsEquipReason::Confirmed, view.frame);
        }
        if (view.nowMs >= status.deadlineMs)
        {
            return Finish(GearsetsEquipPhase::Unconfirmed, GearsetsEquipReason::Timeout, view.frame);
        }
        return false;
    }

    // The bag position is read again on the frame that would invoke, so a
    // stale position never reaches the native call.
    const std::optional<BagItem> bag = view.readBag ? view.readBag(status.bagSlot) : std::nullopt;
    if (!bag || bag->state == EquipmentSlotState::Unknown)
    {
        return Finish(GearsetsEquipPhase::Rejected, GearsetsEquipReason::BagUnavailable, view.frame);
    }
    if (bag->state == EquipmentSlotState::Empty)
    {
        return Finish(GearsetsEquipPhase::Rejected, GearsetsEquipReason::BagEmpty, view.frame);
    }
    if (status.expectedUid && *status.expectedUid != bag->uid)
    {
        return Finish(GearsetsEquipPhase::Rejected, GearsetsEquipReason::UidMismatch, view.frame);
    }
    status.catalogId = bag->catalogId;
    if (!status.expectedUid)
    {
        status.expectedUid = bag->uid;
        status.phase       = GearsetsEquipPhase::Resolved;
        return true;
    }
    if (view.snapshot != nullptr && SlotHolds(*view.snapshot, status.equipSlot, bag->uid))
    {
        return Finish(GearsetsEquipPhase::Complete, GearsetsEquipReason::AlreadyEquipped, view.frame);
    }
    if (!view.invoke)
    {
        return Finish(GearsetsEquipPhase::Refused, GearsetsEquipReason::InvokeUnavailable, view.frame);
    }
    ++status.invocations;
    status.invokeFrame = view.frame;
    status.returnValue = view.invoke(status.bagSlot, status.equipSlot) == GearsetsInvokeResult::ReturnedTrue;
    if (!*status.returnValue)
    {
        return Finish(GearsetsEquipPhase::Rejected, GearsetsEquipReason::BindingFalse, view.frame);
    }
    status.phase      = GearsetsEquipPhase::Invoked;
    status.deadlineMs = view.nowMs + kConfirmTimeoutMs;
    return true;
}

bool GearsetsEquipMachine::EndSession(std::uint64_t frame)
{
    if (!Active())
    {
        return false;
    }
    return status_->phase == GearsetsEquipPhase::Invoked
               ? Finish(GearsetsEquipPhase::Unconfirmed, GearsetsEquipReason::SessionEnded, frame)
               : Finish(GearsetsEquipPhase::Rejected, GearsetsEquipReason::SessionEnded, frame);
}

bool GearsetsEquipMachine::Active() const
{
    return status_ && !IsTerminalGearsetsEquipPhase(status_->phase);
}

std::optional<GearsetsEquipStatus> GearsetsEquipMachine::Status() const
{
    return status_;
}

bool GearsetsEquipMachine::Finish(GearsetsEquipPhase phase, GearsetsEquipReason reason, std::uint64_t frame)
{
    status_->phase       = phase;
    status_->reason      = reason;
    status_->finishFrame = frame;
    return true;
}

std::string FormatGearsetsUid(std::uint64_t uid)
{
    char text[17]{};
    std::snprintf(text, sizeof(text), "%016llx", static_cast<unsigned long long>(uid));
    return text;
}

std::string FormatGearsetsReceiptHeader(std::string_view clientSha256, std::uint32_t processId)
{
    std::string header = "# bahamut-gearsets-receipt schema=" + std::to_string(kGearsetsReceiptSchema) + "\n";
    header += "# client_sha256=";
    header.append(clientSha256);
    header += "\n# process_id=" + std::to_string(processId) + "\n";
    header += "# snapshot: record,sequence,frame,generation,complete,slot,state,uid,catalog\n";
    header += "# unavailable: record,sequence,frame\n";
    header += "# equip: record,sequence,frame,request,phase,reason,generation,bag_slot,equip_slot,uid,catalog,invocations,return_value,submit_frame,invoke_frame,finish_frame\n";
    return header;
}

std::string FormatGearsetsSnapshotRecord(std::uint64_t sequence, std::uint64_t frame, const EquipmentSnapshot& snapshot)
{
    const std::string prefix = "snapshot," + Decimal(sequence) + "," + Decimal(frame) + "," + Decimal(snapshot.generation) + "," + (snapshot.complete ? "1" : "0") + ",";
    std::string       rows;
    for (const auto& slot : snapshot.slots)
    {
        rows += prefix;
        rows += Decimal(slot.slot) + ",";
        rows.append(SlotStateName(slot.state));
        rows += ",";
        if (slot.state == EquipmentSlotState::Item)
        {
            rows += FormatGearsetsUid(slot.uid) + "," + Decimal(slot.catalogId);
        }
        else
        {
            rows += ",";
        }
        rows += "\n";
    }
    return rows;
}

std::string FormatGearsetsUnavailableRecord(std::uint64_t sequence, std::uint64_t frame)
{
    return "unavailable," + Decimal(sequence) + "," + Decimal(frame) + "\n";
}

std::string FormatGearsetsEquipRecord(std::uint64_t sequence, std::uint64_t frame, const GearsetsEquipStatus& status)
{
    std::string row = "equip," + Decimal(sequence) + "," + Decimal(frame) + "," + Decimal(status.id) + ",";
    row.append(GearsetsEquipPhaseName(status.phase));
    row += ",";
    row.append(GearsetsEquipReasonName(status.reason));
    row += "," + Decimal(status.generation) + "," + Decimal(status.bagSlot) + "," + Decimal(status.equipSlot) + ",";
    row += status.expectedUid ? FormatGearsetsUid(*status.expectedUid) : std::string();
    row += "," + (status.catalogId != 0u ? Decimal(status.catalogId) : std::string()) + "," + Decimal(status.invocations) + ",";
    row += status.returnValue ? (*status.returnValue ? "true" : "false") : "";
    row += "," + Decimal(status.submitFrame) + "," + Decimal(status.invokeFrame) + "," + Decimal(status.finishFrame) + "\n";
    return row;
}

std::string FormatGearsetsReceiptFooter(std::uint64_t records, std::uint64_t dropped, std::string_view reason)
{
    std::string footer = "# end records=" + Decimal(records) + " dropped=" + Decimal(dropped) + " reason=";
    footer.append(reason);
    footer += "\n";
    return footer;
}

} // namespace bahamut_client
