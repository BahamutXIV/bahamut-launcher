#include "action_movement.h"
#include "action_movement_admission.h"

#include <limits>

namespace bahamut_client
{

std::int32_t ScaleActionMovementDuration(std::int32_t duration, std::uint32_t retainPercent) noexcept
{
    if (duration <= 0 || retainPercent > 100u)
    {
        return duration;
    }
    return static_cast<std::int32_t>((static_cast<std::int64_t>(duration) * retainPercent) / 100);
}

void ForwardActionMovement(void*                receiver,
                           std::int32_t         duration,
                           bool                 eligible,
                           ActionMovementPolicy policy,
                           ActionMovementSetter original)
{
    const std::int32_t forwarded = policy.enabled && eligible
                                       ? ScaleActionMovementDuration(duration, policy.retainPercent)
                                       : duration;
    original(receiver, forwarded);
}

void ActionMovementService::ClearConsumer()
{
    published_.store(0, std::memory_order_release);
    owner_            = 0;
    requestedPercent_ = 0;
}

void ActionMovementService::ClearSession()
{
    publishedSession_.store(0, std::memory_order_release);
    ClearConsumer();
    session_ = 0;
}

void ActionMovementService::SetBoundaryState(ActionMovementBoundaryState state)
{
    const std::lock_guard lock(mutex_);
    boundary_ = state;
    if (state != ActionMovementBoundaryState::Ready)
    {
        ClearSession();
    }
}

std::uint64_t ActionMovementService::BeginSession()
{
    const std::lock_guard lock(mutex_);
    if (boundary_ != ActionMovementBoundaryState::Ready)
    {
        return 0;
    }
    ClearSession();
    if (sessionSerial_ == std::numeric_limits<std::uint64_t>::max())
    {
        return 0;
    }
    session_ = ++sessionSerial_;
    publishedSession_.store(session_, std::memory_order_release);
    return session_;
}

std::uint64_t ActionMovementService::BindConsumer(std::uint64_t session)
{
    const std::lock_guard lock(mutex_);
    if (boundary_ != ActionMovementBoundaryState::Ready || session == 0 || session != session_)
    {
        return 0;
    }
    ClearConsumer();
    if (ownerSerial_ == std::numeric_limits<std::uint64_t>::max())
    {
        return 0;
    }
    owner_ = ++ownerSerial_;
    return owner_;
}

bool ActionMovementService::Configure(std::uint64_t owner, std::uint64_t session, std::uint32_t retainPercent, bool enabled)
{
    const std::lock_guard lock(mutex_);
    if (boundary_ != ActionMovementBoundaryState::Ready || owner == 0 || owner != owner_ ||
        session == 0 || session != session_ || retainPercent > 100u)
    {
        return false;
    }
    requestedPercent_ = retainPercent;
    // Enabled and percentage publish together; readers never combine two requests.
    published_.store(retainPercent | (enabled ? 0x100u : 0u), std::memory_order_release);
    return true;
}

bool ActionMovementService::RevokeConsumer(std::uint64_t owner)
{
    const std::lock_guard lock(mutex_);
    if (owner == 0 || owner != owner_)
    {
        return false;
    }
    ClearConsumer();
    return true;
}

bool ActionMovementService::EndSession(std::uint64_t session)
{
    const std::lock_guard lock(mutex_);
    if (session == 0 || session != session_)
    {
        return false;
    }
    ClearSession();
    return true;
}

void ActionMovementService::Shutdown()
{
    const std::lock_guard lock(mutex_);
    ClearSession();
    if (boundary_ == ActionMovementBoundaryState::Ready)
    {
        boundary_ = ActionMovementBoundaryState::Unavailable;
    }
}

ActionMovementStatus ActionMovementService::Status() const
{
    const std::lock_guard lock(mutex_);
    const std::uint32_t   policy  = published_.load(std::memory_order_acquire);
    const bool            enabled = (policy & 0x100u) != 0;
    return { boundary_, session_, owner_, requestedPercent_, enabled ? (policy & 0xFFu) : 100u, enabled };
}

void ActionMovementService::Forward(void* receiver, std::int32_t duration, bool eligible, ActionMovementSetter original) const
{
    const std::uint32_t policy = published_.load(std::memory_order_acquire);
    ForwardActionMovement(receiver, duration, eligible, { (policy & 0x100u) != 0, policy & 0xFFu }, original);
}

void ActionMovementService::Forward(void* receiver, std::int32_t duration, const ActionMovementCallContext& context, ActionMovementSetter original) const
{
    // Session loads bracket policy so an obsolete context cannot use a replacement's request.
    const std::uint64_t session   = publishedSession_.load(std::memory_order_acquire);
    const std::uint32_t policy    = published_.load(std::memory_order_acquire);
    const bool          current   = session != 0 && context.session == session &&
                                    publishedSession_.load(std::memory_order_acquire) == session;
    const std::int32_t  forwarded = (policy & 0x100u) != 0
                                        ? ScaleActionMovementDuration(duration, policy & 0xFFu)
                                        : duration;
    const bool          admitted  = current && AdmitActionMovementCall(context, receiver, duration, forwarded) == ActionMovementCallAdmission::Admitted;
    original(receiver, admitted ? forwarded : duration);
}

} // namespace bahamut_client
