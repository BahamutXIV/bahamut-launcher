#include "action_movement_admission.h"

#include "runtime_contract.h"

#include <algorithm>

namespace bahamut_client
{
namespace
{

constexpr std::uint8_t kSetter[] = {
    0xF3, 0x0F, 0x2A, 0x44, 0x24, 0x04, 0xF3, 0x0F, 0x11, 0x81, 0xCC, 0x01, 0x00, 0x00, 0xE8, 0x7D, 0xF6, 0xFF, 0xFF, 0xC2, 0x04, 0x00
};
constexpr std::uint8_t kWrapper[] = {
    0x56, 0x8B, 0x74, 0x24, 0x08, 0x85, 0xF6, 0x74, 0x20, 0x8B, 0x06, 0x8B, 0x90, 0x68, 0x02, 0x00, 0x00, 0x8B, 0xCE, 0xFF, 0xD2, 0x85, 0xC0, 0x75, 0x10, 0x8B, 0x44, 0x24, 0x0C, 0x50, 0x8D, 0x8E, 0xF0, 0x0B, 0x00, 0x00, 0xE8, 0xA7, 0x19, 0x15, 0x00, 0x5E, 0xC2, 0x08, 0x00
};
constexpr std::uint8_t kProducerCall[] = { 0xE8, 0x82, 0x68, 0xE3, 0xFF };

template <std::size_t Size>
bool Matches(std::span<const std::uint8_t> bytes, const std::uint8_t (&expected)[Size]) noexcept
{
    return bytes.size() == Size && std::equal(bytes.begin(), bytes.end(), expected);
}

} // namespace

bool MatchActionMovementTargetBytes(std::uintptr_t imageBase, std::span<const std::uint8_t> setter, std::span<const std::uint8_t> wrapper, std::span<const std::uint8_t> producerCall) noexcept
{
    return imageBase == bahamut_runtime_contract::kRetailImageBase &&
           Matches(setter, kSetter) && Matches(wrapper, kWrapper) && Matches(producerCall, kProducerCall);
}

ActionMovementCallAdmission AdmitActionMovementCall(const ActionMovementCallContext& context,
                                                    std::int32_t                     originalDuration,
                                                    std::int32_t                     retainedDuration,
                                                    const ActionMovementIdentity&    identity) noexcept
{
    using Result = ActionMovementCallAdmission;
    if (originalDuration <= 0)
        return Result::NonPositive;
    if (!context.targetVerified)
        return Result::TargetUnavailable;
    if (context.session == 0 || context.session != context.currentSession)
        return Result::SessionUnavailable;
    if (!context.readsComplete)
        return Result::ReadsIncomplete;
    if (context.callerReturn != identity.producerReturn || context.originalDuration != originalDuration)
        return Result::CallerMismatch;
    if (context.actor == 0 || context.actor != context.ownerActor || context.actor != context.controllerActor ||
        context.actorVptr != identity.actorVptr || context.gateTarget != identity.gateTarget ||
        context.actorId == 0 || context.actorId == kActionMovementNoActorId || context.actorId != context.localActorId)
        return Result::IdentityMismatch;
    if ((context.actorWord2B70 & kActionMovementGateBit) != 0 || (context.controllerFlags & kActionMovementAlternateTimerFlag) != 0)
        return Result::GateMismatch;
    if (retainedDuration <= 0)
        return Result::ScaledNonPositive;
    return Result::Admitted;
}

} // namespace bahamut_client
