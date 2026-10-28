#pragma once

#include "action_movement.h"

#include <MinHook.h>

namespace bahamut_client
{

struct ActionMovementHookApi
{
    decltype(&MH_CreateHook)  create  = &MH_CreateHook;
    decltype(&MH_EnableHook)  enable  = &MH_EnableHook;
    decltype(&MH_DisableHook) disable = &MH_DisableHook;
    decltype(&MH_RemoveHook)  remove  = &MH_RemoveHook;
};

enum class ActionMovementHookStage
{
    None,
    Target,
    Create,
    Trampoline,
    Enable,
    Disable,
    Remove
};

enum class ActionMovementHookShutdown
{
    Complete,
    AwaitingCalls,
    Failed
};

struct ActionMovementHookStatus
{
    void*                   target         = nullptr;
    bool                    disabled       = true;
    ActionMovementHookStage installFailure = ActionMovementHookStage::None;
    MH_STATUS               installError   = MH_OK;
    ActionMovementHookStage cleanupFailure = ActionMovementHookStage::None;
    MH_STATUS               cleanupError   = MH_OK;
};

class ActionMovementHook
{
public:
    explicit ActionMovementHook(ActionMovementService& service, ActionMovementHookApi api = {});
    ActionMovementHook(const ActionMovementHook&)            = delete;
    ActionMovementHook& operator=(const ActionMovementHook&) = delete;

    // One owner per service. The host owns MinHook initialization, target qualification,
    // serialized lifecycle calls and detour routing; all api entries must be callable.
    // The trampoline supplies the original wrapper; image and caller checks belong to the runtime.
    [[nodiscard]] bool Install(void* target, void* detour, bool qualified);
    // callsQuiescent is external proof that detour/trampoline calls have finished.
    // Retain this owner, service and detour code until Complete; destruction does no cleanup.
    [[nodiscard]] ActionMovementHookShutdown Shutdown(bool callsQuiescent);
    [[nodiscard]] ActionMovementWrapper      Original() const;
    [[nodiscard]] ActionMovementHookStatus   Status() const;

private:
    static_assert(std::atomic<ActionMovementWrapper>::is_always_lock_free);
    ActionMovementService&             service_;
    ActionMovementHookApi              api_;
    ActionMovementHookStatus           status_;
    std::atomic<ActionMovementWrapper> original_ = nullptr;
};

} // namespace bahamut_client
