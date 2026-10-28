#pragma once

#include <cstdint>
#include <span>

namespace bahamut_client
{

inline constexpr std::uintptr_t kActionMovementSetterVa       = 0x007B0700u;
inline constexpr std::uintptr_t kActionMovementWrapperVa      = 0x0065ED30u;
inline constexpr std::uintptr_t kActionMovementProducerCallVa = 0x008284A9u;

// Scene actor id at actor+0xDC (Capstone 5.0.7, 0x007CF062: MOV [ESI+0xDC], EDX) in ffxivgame.exe
// SHA-256 9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9; it shares the
// container+0x1783C active id's space (actor and element maps), see
// https://github.com/XIVLegacy/xivl-decomp/blob/824dda2f99bace4bf1364b2ec981165603edd9db/docs/resource/attribute-resource-and-scene-binding.md#position-reporting-and-default-camera-subject
inline constexpr std::uint32_t kActionMovementActorIdOffset = 0xDCu;
// The player-state active-actor sentinel; never a local player id.
inline constexpr std::uint32_t kActionMovementNoActorId = 0xC0000000u;

// Primary CharaActor vtable, its +0x268 gate target, gate bit and alternate timer flag;
// see the native-boundary section of
// https://github.com/XIVLegacy/xivl-client-structs/blob/3227221932dc99a9c1183e388bd6684564f4c732/docs/action-movement-wait.md
inline constexpr std::uint32_t kActionMovementPrimaryActorVtable = 0x00FC0D34u;
inline constexpr std::uint32_t kActionMovementGateTarget         = 0x00669650u;
inline constexpr std::uint32_t kActionMovementGateBit            = 1u << 6;
inline constexpr std::uint32_t kActionMovementAlternateTimerFlag = 0x4u;
// The wrapper's only admitted caller: the return address after the producer call.
inline constexpr std::uintptr_t kActionMovementProducerReturnVa = kActionMovementProducerCallVa + 5u;

// Exact pre-install windows; see the native-boundary and action-like-producer sections:
// https://github.com/XIVLegacy/xivl-client-structs/blob/3227221932dc99a9c1183e388bd6684564f4c732/docs/action-movement-wait.md
[[nodiscard]] bool MatchActionMovementTargetBytes(std::uintptr_t                imageBase,
                                                  std::span<const std::uint8_t> setter,
                                                  std::span<const std::uint8_t> wrapper,
                                                  std::span<const std::uint8_t> producerCall) noexcept;
// Cold host check: exact file/version identity, no test-stub exception, then mapped bytes.
// It does not install a hook or qualify an action, actor, thread or session.
[[nodiscard]] bool VerifyActionMovementRetailTarget();

// Production uses the retail defaults; native fixtures may substitute vtable and gate addresses.
struct ActionMovementIdentity
{
    std::uintptr_t producerReturn = kActionMovementProducerReturnVa;
    std::uintptr_t actorVptr      = kActionMovementPrimaryActorVtable;
    std::uintptr_t gateTarget     = kActionMovementGateTarget;
};

// Copied entry evidence; nothing is dereferenced after capture. Capture fills the actor fields
// and readsComplete; the host supplies the rest. ActionMovementService::ForwardWrapper stamps currentSession.
struct ActionMovementCallContext
{
    bool           targetVerified   = false;
    std::uint64_t  session          = 0;
    std::uint64_t  currentSession   = 0;
    std::uintptr_t callerReturn     = 0;
    std::uintptr_t actor            = 0;
    std::uintptr_t ownerActor       = 0;
    std::uintptr_t controllerActor  = 0;
    std::uintptr_t actorVptr        = 0;
    std::uintptr_t gateTarget       = 0;
    std::uint32_t  actorWord2B70    = 0;
    std::uint32_t  controllerFlags  = 0;
    std::uint32_t  actorId          = 0;
    std::uint32_t  localActorId     = 0;
    std::int32_t   originalDuration = 0;
    bool           readsComplete    = false;
};

enum class ActionMovementCallAdmission
{
    Admitted,
    NonPositive,
    TargetUnavailable,
    SessionUnavailable,
    ReadsIncomplete,
    CallerMismatch,
    IdentityMismatch,
    GateMismatch,
    ScaledNonPositive
};

// Uses copied evidence only; local means the actor id matches the validated player-state active id.
// Unknown evidence or a positive duration that scales to zero or less leaves the call unchanged.
[[nodiscard]] ActionMovementCallAdmission AdmitActionMovementCall(
    const ActionMovementCallContext& context, std::int32_t originalDuration, std::int32_t retainedDuration, const ActionMovementIdentity& identity = {}) noexcept;

// Copies all actor fields with fault-guarded reads before the original call; no virtual calls,
// native writes or later reads. On failure, clears readsComplete without writing actor fields.
// Target, session and local id remain the caller's.
[[nodiscard]] bool CaptureActionMovementCallContext(void* actor, std::uintptr_t callerReturn, std::int32_t duration, ActionMovementCallContext& out) noexcept;

} // namespace bahamut_client
