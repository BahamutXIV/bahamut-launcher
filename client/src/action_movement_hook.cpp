#include "action_movement_hook.h"

namespace bahamut_client
{

ActionMovementHook::ActionMovementHook(ActionMovementService& service, ActionMovementHookApi api)
: service_(service)
, api_(api)
{
}

bool ActionMovementHook::Install(void* target, void* detour, bool qualified)
{
    if (status_.target != nullptr)
    {
        return false;
    }
    status_ = {};
    service_.SetBoundaryState(ActionMovementBoundaryState::Unavailable);
    if (!qualified || target == nullptr || detour == nullptr)
    {
        status_.installFailure = ActionMovementHookStage::Target;
        service_.SetBoundaryState(ActionMovementBoundaryState::TargetMismatch);
        return false;
    }
    void*           trampoline = nullptr;
    const MH_STATUS created    = api_.create(target, detour, &trampoline);
    if (created != MH_OK)
    {
        // In particular, ALREADY_CREATED belongs to another owner and is not ours to remove.
        status_.installFailure = ActionMovementHookStage::Create;
        status_.installError   = created;
        service_.SetBoundaryState(ActionMovementBoundaryState::InstallFailed);
        return false;
    }
    status_.target = target;
    original_.store(reinterpret_cast<ActionMovementWrapper>(trampoline), std::memory_order_release);
    if (trampoline == nullptr)
    {
        status_.installFailure = ActionMovementHookStage::Trampoline;
        status_.installError   = MH_UNKNOWN;
        service_.SetBoundaryState(ActionMovementBoundaryState::InstallFailed);
        // Create leaves the hook disabled; no detour has been enabled by this owner.
        static_cast<void>(Shutdown(true));
        return false;
    }
    // A failed enable can be reported after side effects; rollback must disable it.
    status_.disabled        = false;
    const MH_STATUS enabled = api_.enable(target);
    if (enabled != MH_OK)
    {
        status_.installFailure = ActionMovementHookStage::Enable;
        status_.installError   = enabled;
        service_.SetBoundaryState(ActionMovementBoundaryState::InstallFailed);
        static_cast<void>(Shutdown(false));
        return false;
    }
    service_.SetBoundaryState(ActionMovementBoundaryState::Ready);
    return true;
}

ActionMovementHookShutdown ActionMovementHook::Shutdown(bool callsQuiescent)
{
    service_.Shutdown();
    if (status_.target == nullptr)
    {
        return ActionMovementHookShutdown::Complete;
    }
    if (!status_.disabled)
    {
        const MH_STATUS disabled = api_.disable(status_.target);
        if (disabled != MH_OK && disabled != MH_ERROR_DISABLED)
        {
            status_.cleanupFailure = ActionMovementHookStage::Disable;
            status_.cleanupError   = disabled;
            service_.SetBoundaryState(ActionMovementBoundaryState::CleanupFailed);
            return ActionMovementHookShutdown::Failed;
        }
        status_.disabled = true;
    }
    if (!callsQuiescent)
    {
        return ActionMovementHookShutdown::AwaitingCalls;
    }
    const MH_STATUS removed = api_.remove(status_.target);
    if (removed != MH_OK && removed != MH_ERROR_NOT_CREATED)
    {
        status_.cleanupFailure = ActionMovementHookStage::Remove;
        status_.cleanupError   = removed;
        service_.SetBoundaryState(ActionMovementBoundaryState::CleanupFailed);
        return ActionMovementHookShutdown::Failed;
    }
    original_.store(nullptr, std::memory_order_release);
    status_.target         = nullptr;
    status_.cleanupFailure = ActionMovementHookStage::None;
    status_.cleanupError   = MH_OK;
    service_.SetBoundaryState(status_.installFailure == ActionMovementHookStage::None
                                  ? ActionMovementBoundaryState::Unavailable
                                  : ActionMovementBoundaryState::InstallFailed);
    return ActionMovementHookShutdown::Complete;
}

ActionMovementWrapper ActionMovementHook::Original() const
{
    return original_.load(std::memory_order_acquire);
}

ActionMovementHookStatus ActionMovementHook::Status() const
{
    return status_;
}

} // namespace bahamut_client
