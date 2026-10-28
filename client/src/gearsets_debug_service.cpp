#include "gearsets_debug_service.h"

#include "equipment_state.h"
#include "runtime_contract.h"

#include <windows.h>

#include <algorithm>
#include <iterator>
#include <string>

namespace
{

std::optional<std::wstring> EnvironmentValue(const wchar_t* name)
{
    std::wstring value(32768u, L'\0');
    const DWORD  count = GetEnvironmentVariableW(name, value.data(), static_cast<DWORD>(value.size()));
    if (count == 0 || count >= value.size())
    {
        return std::nullopt;
    }
    value.resize(count);
    return value;
}

void LogStatus(const wchar_t* message, DWORD error)
{
    std::wstring text = L"Bahamut gearsets: ";
    text += message;
    text += L" error=" + std::to_wstring(error) + L"\n";
    OutputDebugStringW(text.c_str());
}

} // namespace

namespace bahamut_client
{

GearsetsDebugSwitches ReadGearsetsDebugSwitchesFromEnvironment()
{
    return ParseGearsetsDebugSwitches(EnvironmentValue(kGearsetsSnapshotFileEnvironment),
                                      EnvironmentValue(kGearsetsEquipEnvironment));
}

GearsetsDebugService::GearsetsDebugService(EquipmentStateService* equipment)
: equipment_(equipment)
{
}

GearsetsDebugService::~GearsetsDebugService()
{
    FinishAtShutdown();
}

void GearsetsDebugService::Configure(const GearsetsDebugSwitches& switches)
{
    std::lock_guard lock(mutex_);
    switches_      = switches;
    receiptWanted_ = !switches_.snapshotFile.empty();
}

void GearsetsDebugService::SetInvoker(Invoker invoker, void* context)
{
    std::lock_guard lock(mutex_);
    invoker_        = invoker;
    invokerContext_ = context;
}

void GearsetsDebugService::SetMetadataProvider(std::function<void(EquipmentSnapshot&)> provider)
{
    std::lock_guard lock(mutex_);
    metadata_ = std::move(provider);
}

void GearsetsDebugService::ObserveGameFrame(void* context, const void* container)
{
    auto* service = static_cast<GearsetsDebugService*>(context);
    if (service == nullptr || !service->active_.load(std::memory_order_acquire))
    {
        return;
    }
    service->OnGameFrame(ResolvePlayerFromContainer(container), GetTickCount64());
}

template <typename Format>
void GearsetsDebugService::QueueRecordLocked(Format format)
{
    if (!receiptWanted_)
    {
        return;
    }
    if (records_ >= kMaximumReceiptRecords || pending_.size() >= kMaximumPendingRecords)
    {
        ++dropped_;
        return;
    }
    pending_.push_back(format(records_ + 1u));
    ++records_;
}

void GearsetsDebugService::OnGameFrame(const void* player, std::uint64_t nowMs)
{
    if (!active_.load(std::memory_order_acquire))
    {
        return;
    }
    std::lock_guard lock(mutex_);
    // Rechecked under the lock: deactivation may have run while this waited.
    if (equipment_ == nullptr || !active_.load(std::memory_order_acquire))
    {
        return;
    }
    ++frame_;
    const bool playerChanged = player != lastPlayer_;
    if (playerChanged)
    {
        // A different MyPlayer control, or none, is a different session:
        // queued work from the previous one must not reach it, and its
        // snapshot is replaced or cleared now rather than after the interval.
        lastPlayer_ = player;
        ++generation_;
        equipment_->Clear();
    }
    const bool transacting = machine_.Active();
    if (!transacting && !playerChanged && lastReadMs_ && nowMs - *lastReadMs_ < kSnapshotIntervalMs)
    {
        return;
    }
    lastReadMs_ = nowMs;

    std::optional<EquipmentSnapshot> published;
    if (auto snapshot = player != nullptr ? ReadEquipmentInventoryFromPlayer(player) : std::nullopt)
    {
        if (metadata_)
            metadata_(snapshot->equipment);
        equipment_->PublishInventory(*snapshot);
        published = equipment_->Snapshot();
        if (published && (!available_.value_or(false) || published->generation != recordedGeneration_))
        {
            recordedGeneration_ = published->generation;
            QueueRecordLocked([&](std::uint64_t sequence)
                              {
                                  return FormatGearsetsSnapshotRecord(sequence, frame_, *published);
                              });
        }
        available_ = true;
    }
    else
    {
        equipment_->InvalidateSnapshot();
        if (available_.value_or(true))
        {
            QueueRecordLocked([&](std::uint64_t sequence)
                              {
                                  return FormatGearsetsUnavailableRecord(sequence, frame_);
                              });
        }
        available_ = false;
    }

    if (!transacting)
    {
        return;
    }
    GearsetsFrameView view;
    view.frame      = frame_;
    view.nowMs      = nowMs;
    view.generation = generation_;
    view.snapshot   = published ? &*published : nullptr;
    view.readBag    = [player](std::uint16_t bagSlot)
    {
        return player != nullptr ? ReadBagItemFromPlayer(player, bagSlot) : std::nullopt;
    };
    if (invoker_ != nullptr && player != nullptr)
    {
        view.invoke = [this, player](std::uint16_t bagSlot, std::uint8_t equipSlot)
        {
            return invoker_(invokerContext_, player, bagSlot, equipSlot);
        };
    }
    if (machine_.Step(view))
    {
        const auto status = machine_.Status();
        QueueRecordLocked([&](std::uint64_t sequence)
                          {
                              return FormatGearsetsEquipRecord(sequence, frame_, *status);
                          });
    }
}

bool GearsetsDebugService::EquipEnabled() const
{
    std::lock_guard lock(mutex_);
    return switches_.equip;
}

void GearsetsDebugService::OnAddonStateChanged(bool active)
{
    std::lock_guard lock(mutex_);
    // Every transition starts a new session, so a request queued by a
    // previous addon instance can never run in the next one.
    ++generation_;
    if (machine_.EndSession(frame_))
    {
        const auto status = machine_.Status();
        QueueRecordLocked([&](std::uint64_t sequence)
                          {
                              return FormatGearsetsEquipRecord(sequence, frame_, *status);
                          });
    }
    if (!switches_.Any())
    {
        return;
    }
    if (active)
    {
        receiptStarted_ = receiptStarted_ || receiptWanted_;
        active_.store(true, std::memory_order_release);
        return;
    }
    active_.store(false, std::memory_order_release);
    if (equipment_ != nullptr)
    {
        equipment_->Clear();
    }
    available_.reset();
    lastPlayer_ = nullptr;
    lastReadMs_.reset();
    // The receipt covers the first addon session. The render thread writes
    // the footer; this may run inside a detour.
    if (receiptStarted_ && receiptWanted_)
    {
        receiptWanted_ = false;
        finishPending_ = true;
    }
}

void GearsetsDebugService::OnRenderFrame()
{
    bool open   = false;
    bool finish = false;
    {
        std::lock_guard lock(mutex_);
        open           = receiptStarted_ && !receiptOpened_;
        finish         = finishPending_;
        finishPending_ = false;
    }
    if (open)
    {
        OpenReceipt();
    }
    if (finish)
    {
        Finish("addon-inactive");
        return;
    }
    if (WritePending(kMaximumWritesPerUpdate))
    {
        Finish("limit");
    }
}

GearsetsSubmitResult GearsetsDebugService::SubmitDebugEquip(std::uint16_t bagSlot, std::uint8_t equipSlot, std::optional<std::uint64_t> uid)
{
    std::lock_guard            lock(mutex_);
    const GearsetsSubmitResult result = machine_.Submit({ generation_, bagSlot, equipSlot, uid },
                                                        switches_.equip && active_.load(std::memory_order_acquire),
                                                        frame_);
    if (result.accepted)
    {
        const auto status = machine_.Status();
        QueueRecordLocked([&](std::uint64_t sequence)
                          {
                              return FormatGearsetsEquipRecord(sequence, frame_, *status);
                          });
    }
    return result;
}

std::optional<GearsetsEquipStatus> GearsetsDebugService::DebugEquipStatus() const
{
    std::lock_guard lock(mutex_);
    return machine_.Status();
}

void GearsetsDebugService::OpenReceipt()
{
    std::filesystem::path path;
    {
        std::lock_guard lock(mutex_);
        path = switches_.snapshotFile;
    }
    receiptOpened_ = true;
    HANDLE file    = CreateFileW(path.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
    {
        LogStatus(L"receipt-open-failed", GetLastError());
        std::lock_guard lock(mutex_);
        receiptWanted_ = false;
        pending_.clear();
        return;
    }
    file_ = file;
    if (!WriteText(FormatGearsetsReceiptHeader(bahamut_runtime_contract::kClientSha256, GetCurrentProcessId())))
    {
        Finish("write-failed");
    }
}

bool GearsetsDebugService::WriteText(const std::string& text)
{
    if (file_ == nullptr)
    {
        return false;
    }
    DWORD written = 0;
    if (!WriteFile(static_cast<HANDLE>(file_), text.data(), static_cast<DWORD>(text.size()), &written, nullptr) || written != text.size())
    {
        LogStatus(L"receipt-write-failed", GetLastError());
        CloseHandle(static_cast<HANDLE>(file_));
        file_ = nullptr;
        std::lock_guard lock(mutex_);
        receiptWanted_ = false;
        pending_.clear();
        return false;
    }
    return true;
}

bool GearsetsDebugService::WritePending(std::size_t limit)
{
    if (file_ == nullptr)
    {
        return false;
    }
    std::vector<std::string> rows;
    bool                     full = false;
    {
        std::lock_guard   lock(mutex_);
        const std::size_t count = std::min(pending_.size(), limit);
        rows.assign(std::make_move_iterator(pending_.begin()), std::make_move_iterator(pending_.begin() + count));
        pending_.erase(pending_.begin(), pending_.begin() + count);
        full = records_ >= kMaximumReceiptRecords && pending_.empty();
    }
    for (const auto& row : rows)
    {
        if (!WriteText(row))
        {
            return false;
        }
    }
    return full;
}

void GearsetsDebugService::Finish(std::string_view reason)
{
    if (file_ == nullptr)
    {
        return;
    }
    static_cast<void>(WritePending(kMaximumPendingRecords));
    if (file_ == nullptr)
    {
        return;
    }
    std::uint64_t records = 0;
    std::uint64_t dropped = 0;
    {
        std::lock_guard lock(mutex_);
        records        = records_;
        dropped        = dropped_;
        receiptWanted_ = false;
        pending_.clear();
    }
    static_cast<void>(WriteText(FormatGearsetsReceiptFooter(records, dropped, reason)));
    if (file_ != nullptr)
    {
        CloseHandle(static_cast<HANDLE>(file_));
        file_ = nullptr;
    }
}

void GearsetsDebugService::FinishAtShutdown()
{
    if (file_ == nullptr)
    {
        return;
    }
    std::vector<std::string> rows;
    std::uint64_t            records = 0;
    std::uint64_t            dropped = 0;
    {
        std::unique_lock lock(mutex_, std::try_to_lock);
        if (!lock.owns_lock())
        {
            CloseHandle(static_cast<HANDLE>(file_));
            file_ = nullptr;
            return;
        }
        rows.assign(std::make_move_iterator(pending_.begin()), std::make_move_iterator(pending_.end()));
        pending_.clear();
        records        = records_;
        dropped        = dropped_;
        receiptWanted_ = false;
    }
    // A thread that ExitProcess terminated cannot take the lock again, so the
    // relock inside WriteText after a failed write cannot wait forever.
    for (const auto& row : rows)
    {
        if (!WriteText(row))
        {
            return;
        }
    }
    static_cast<void>(WriteText(FormatGearsetsReceiptFooter(records, dropped, "shutdown")));
    if (file_ != nullptr)
    {
        CloseHandle(static_cast<HANDLE>(file_));
        file_ = nullptr;
    }
}

} // namespace bahamut_client
