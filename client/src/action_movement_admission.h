#pragma once

#include <cstdint>
#include <span>

namespace bahamut_client
{

inline constexpr std::uintptr_t kActionMovementSetterVa       = 0x007B0700u;
inline constexpr std::uintptr_t kActionMovementWrapperVa      = 0x0065ED30u;
inline constexpr std::uintptr_t kActionMovementProducerCallVa = 0x008284A9u;

// Scene actor id at actor+0xDC: ffxivgame.exe SHA-256
// 9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9, Capstone 5.0.7
// disassembly at 0x007CF062: MOV [ESI+0xDC], EDX. The actor map and the element map
// behind the container+0x1783C active id share that id space; see
// https://github.com/XIVLegacy/xivl-decomp/blob/824dda2f99bace4bf1364b2ec981165603edd9db/docs/resource/attribute-resource-and-scene-binding.md#position-reporting-and-default-camera-subject
inline constexpr std::uint32_t kActionMovementActorIdOffset = 0xDCu;
// The player-state active-actor sentinel; never a local player id.
inline constexpr std::uint32_t kActionMovementNoActorId = 0xC0000000u;

// Exact pre-install windows; see the native-boundary and action-like-producer sections:
// https://github.com/XIVLegacy/xivl-client-structs/blob/3227221932dc99a9c1183e388bd6684564f4c732/docs/action-movement-wait.md
[[nodiscard]] bool MatchActionMovementTargetBytes(std::uintptr_t                imageBase,
                                                  std::span<const std::uint8_t> setter,
                                                  std::span<const std::uint8_t> wrapper,
                                                  std::span<const std::uint8_t> producerCall) noexcept;
// Cold host check: exact file/version identity, no test-stub exception, then mapped bytes.
// It does not install a hook or qualify an action, actor, thread or session.
[[nodiscard]] bool VerifyActionMovementRetailTarget();

struct ActionMovementCallContext
{
    bool           targetVerified            = false;
    bool           recoveryQualified         = false;
    bool           borrowedLifetimeQualified = false;
    bool           zeroResultQualified       = false;
    std::uint64_t  session                   = 0;
    std::uint64_t  currentSession            = 0;
    std::uint32_t  thread                    = 0;
    std::uint32_t  borrowedThread            = 0;
    std::uintptr_t actor                     = 0;
    std::uintptr_t ownerActor                = 0;
    std::uint32_t  actorId                   = 0;
    std::uint32_t  localActorId              = 0;
    std::uintptr_t actorVptr                 = 0;
    std::uintptr_t gateTarget                = 0;
    std::uintptr_t producerReturn            = 0;
    std::uintptr_t setterReturn              = 0;
    std::uint32_t  gateResult                = 1;
    std::uint32_t  controllerFlags           = 0;
    std::int32_t   originalDuration          = 0;
};

enum class ActionMovementCallAdmission
{
    Admitted,
    NonPositive,
    TargetUnavailable,
    ContextUnqualified,
    SessionUnavailable,
    BorrowUnavailable,
    IdentityMismatch,
    CallerMismatch,
    GateMismatch,
    ZeroResultUnqualified
};

// Native-owner evidence copied in one synchronous borrowed call, never a pointer cache.
// The caller must establish the qualification flags; pointer equality supplies no proof.
// The local player is the actor whose id equals the validated player-state active id.
// No retail evidence supplier exists yet. Unknown evidence leaves the call unchanged.
[[nodiscard]] ActionMovementCallAdmission AdmitActionMovementCall(
    const ActionMovementCallContext& context, const void* receiver, std::int32_t originalDuration, std::int32_t retainedDuration) noexcept;

} // namespace bahamut_client
