#include "action_movement_admission.h"

#include "fault_guard.h"

#include <limits>

namespace bahamut_client
{
namespace
{

// Wrapper-entry layout; the actor controller is inline at +0xBF0. See kActionMovementPrimaryActorVtable.
constexpr std::size_t kOwnerOffset           = 0x118u;
constexpr std::size_t kOwnerActorOffset      = 0xD0u;
constexpr std::size_t kControllerOffset      = 0xBF0u;
constexpr std::size_t kControllerFlagsOffset = 4u;
constexpr std::size_t kGateTargetOffset      = 0x268u;
constexpr std::size_t kActorWordOffset       = 0x2B70u;

template <class T>
T Read(std::uintptr_t base, std::size_t offset)
{
    return *reinterpret_cast<const volatile T*>(base + offset);
}

} // namespace

bool CaptureActionMovementCallContext(void* actor, std::uintptr_t callerReturn, std::int32_t duration, ActionMovementCallContext& out) noexcept
{
    out.readsComplete = false;
    const auto token  = reinterpret_cast<std::uintptr_t>(actor);
    if (token == 0 || token > std::numeric_limits<std::uintptr_t>::max() - kActorWordOffset - sizeof(std::uint32_t))
        return false;
    BAHAMUT_FAULT_TRY
    {
        const auto controller = token + kControllerOffset;
        const auto vptr       = Read<std::uintptr_t>(token, 0);
        const auto owner      = Read<std::uintptr_t>(token, kOwnerOffset);
        if (vptr == 0 || vptr > std::numeric_limits<std::uintptr_t>::max() - kGateTargetOffset - sizeof(std::uintptr_t) ||
            owner == 0 || owner > std::numeric_limits<std::uintptr_t>::max() - kOwnerActorOffset - sizeof(std::uintptr_t))
            return false;
        const auto controllerActor = Read<std::uintptr_t>(controller, 0);
        const auto gateTarget      = Read<std::uintptr_t>(vptr, kGateTargetOffset);
        const auto ownerActor      = Read<std::uintptr_t>(owner, kOwnerActorOffset);
        const auto actorId         = Read<std::uint32_t>(token, kActionMovementActorIdOffset);
        const auto actorWord       = Read<std::uint32_t>(token, kActorWordOffset);
        const auto controllerFlags = Read<std::uint8_t>(controller, kControllerFlagsOffset);
        out.callerReturn           = callerReturn;
        out.originalDuration       = duration;
        out.actor                  = token;
        out.ownerActor             = ownerActor;
        out.controllerActor        = controllerActor;
        out.actorVptr              = vptr;
        out.gateTarget             = gateTarget;
        out.actorWord2B70          = actorWord;
        out.controllerFlags        = controllerFlags;
        out.actorId                = actorId;
        out.readsComplete          = true;
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

} // namespace bahamut_client
