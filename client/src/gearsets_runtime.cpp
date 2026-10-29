#include "gearsets_runtime.h"

#include "equipment_state.h"

#include <windows.h>

namespace bahamut_client
{

GearsetsRuntime::GearsetsRuntime(EquipmentStateService* equipment, GearsetsExecutor& executor)
: equipment_(equipment)
, executor_(executor)
{
}

void GearsetsRuntime::SetMetadataProvider(std::function<void(EquipmentSnapshot&)> provider)
{
    std::lock_guard lock(mutex_);
    metadata_ = std::move(provider);
}

void GearsetsRuntime::SetGameFrameObserver(void (*observer)(void* context, const void* container), void* context)
{
    std::lock_guard lock(mutex_);
    next_        = observer;
    nextContext_ = context;
}

void GearsetsRuntime::ObserveGameFrame(void* context, const void* container)
{
    auto* runtime = static_cast<GearsetsRuntime*>(context);
    if (runtime == nullptr)
        return;
    runtime->OnGameFrame(container, runtime->active_.load(std::memory_order_acquire) ? ResolvePlayerFromContainer(container) : nullptr, GetTickCount64());
}

void GearsetsRuntime::OnGameFrame(const void* container, const void* player, std::uint64_t nowMs)
{
    void (*next)(void*, const void*) = nullptr;
    void* nextContext                = nullptr;
    {
        std::lock_guard lock(mutex_);
        next        = next_;
        nextContext = nextContext_;
        // Checked under the lock: deactivation may have run while this waited.
        if (equipment_ != nullptr && active_.load(std::memory_order_acquire))
            FrameLocked(container, player, nowMs);
    }
    if (next != nullptr)
        next(nextContext, container);
}

void GearsetsRuntime::FrameLocked(const void* container, const void* player, std::uint64_t nowMs)
{
    const bool playerChanged = player != lastPlayer_;
    if (playerChanged)
    {
        // A different MyPlayer control, or none, is a different session.
        lastPlayer_ = player;
        equipment_->Clear();
    }
    const bool transacting = transaction_.Active();
    if (!transacting && !playerChanged && lastReadMs_ && nowMs - *lastReadMs_ < kSnapshotIntervalMs)
        return;
    lastReadMs_ = nowMs;

    auto snapshot = player != nullptr ? ReadEquipmentInventoryFromPlayer(player) : std::nullopt;
    if (snapshot)
    {
        if (metadata_)
            metadata_(snapshot->equipment);
        equipment_->PublishInventory(*snapshot);
        lastGoodReadMs_ = nowMs;
    }
    else
    {
        equipment_->InvalidateSnapshot();
    }
    if (!transacting)
        return;

    GearsetsView view;
    view.nowMs = nowMs;
    if (const auto inventory = snapshot ? equipment_->Inventory() : std::nullopt)
    {
        view.inventory = *inventory;
        view.session   = inventory->equipment.session;
        view.classId   = inventory->equipment.classId.value_or(0);
        view.jobId     = inventory->equipment.jobId.value_or(0);
    }
    else
    {
        if (!playerChanged && lastGoodReadMs_ && nowMs - *lastGoodReadMs_ < kReadFailureMs)
            return;
        // An incomplete view stops the transaction.
        view.session = equipment_->SessionGeneration();
    }
    executor_.BeginFrame(container, player);
    transaction_.Step(view, executor_);
}

void GearsetsRuntime::SetActive(bool active)
{
    std::lock_guard lock(mutex_);
    transaction_.EndSession();
    active_.store(active, std::memory_order_release);
    if (active)
        return;
    if (equipment_ != nullptr)
        equipment_->Clear();
    lastPlayer_ = nullptr;
    lastReadMs_.reset();
    lastGoodReadMs_.reset();
}

std::optional<GearsetsResult> GearsetsRuntime::Begin(const GearsetIntent& intent, const GearsetsView& view)
{
    std::lock_guard lock(mutex_);
    if (!transaction_.Begin(intent, view, executor_))
        return std::nullopt;
    return transaction_.Result();
}

void GearsetsRuntime::Cancel()
{
    std::lock_guard lock(mutex_);
    transaction_.Cancel();
}

GearsetsResult GearsetsRuntime::Result() const
{
    std::lock_guard lock(mutex_);
    return transaction_.Result();
}

} // namespace bahamut_client
