#pragma once

#include "equipment_state.h"

#include <array>
#include <optional>
#include <string>
#include <vector>

namespace bahamut_client
{

struct GearsetIntent
{
    std::uint64_t                                  session = 0;
    std::array<EquipmentSlot, kEquipmentSlotCount> slots{};
    // Job after the main hand: ID, 0 for none, or nullopt to leave it alone.
    std::optional<std::uint8_t> job;
};

// Returns nullopt for non-job IDs.
std::optional<std::uint8_t> GearsetsJobClass(std::uint8_t job);

struct GearsetsView
{
    std::uint64_t              session = 0;
    std::uint64_t              nowMs   = 0;
    std::uint16_t              classId = 0;
    std::uint16_t              jobId   = 0;
    EquipmentInventorySnapshot inventory;
};

struct GearsetsOperation
{
    EquipmentSlot                target;
    std::optional<std::uint16_t> bagSlot;
    std::optional<std::uint8_t>  sourceSlot;
};

struct GearsetsFailure
{
    std::string                 reason;
    std::optional<std::uint8_t> slot;
};

struct GearsetsPlan
{
    // A qualified executor supplies ordering and a bounded confirmation deadline.
    // The unavailable executor has neither and returns no plan.
    std::vector<std::uint8_t> order;
    std::uint64_t             confirmationMs = 0;
    // Wait after main-hand confirmation or a late class/job change for dependent
    // slots to settle. Observe the resulting class, never predict it, then
    // re-evaluate the remaining slots.
    std::uint64_t settleMs = 0;
};

enum class GearsetsSubmission
{
    Accepted,
    Rejected,
    Unavailable,
    // Nothing was sent because the native route is not ready yet; the step is
    // retried until the confirmation deadline.
    Deferred
};

class GearsetsExecutor
{
public:
    virtual ~GearsetsExecutor()                                                 = default;
    virtual std::optional<GearsetsPlan> Plan(const GearsetIntent& intent) const = 0;

    // Game thread, before each step: the frame's player-state container and
    // MyPlayer control, either possibly null.
    virtual void BeginFrame(const void* /*container*/, const void* /*player*/)
    {
    }

    // Checks the full destination, including Empty operations and restrictions
    // that can be established before a class transition. Called again afterward.
    virtual std::optional<GearsetsFailure> Preflight(const GearsetIntent& intent, const GearsetsView& view) const = 0;
    virtual GearsetsSubmission             Submit(const GearsetsOperation& operation)                             = 0;
    // Change Job mode 1 takes the current class's job, mode 2 leaves it.
    virtual GearsetsSubmission SubmitJob(std::uint8_t mode) = 0;
};

class UnavailableGearsetsExecutor final : public GearsetsExecutor
{
public:
    std::optional<GearsetsPlan> Plan(const GearsetIntent&) const override
    {
        return std::nullopt;
    }

    std::optional<GearsetsFailure> Preflight(const GearsetIntent&, const GearsetsView&) const override
    {
        return GearsetsFailure{ "executor-unavailable", {} };
    }

    GearsetsSubmission Submit(const GearsetsOperation&) override
    {
        return GearsetsSubmission::Unavailable;
    }

    GearsetsSubmission SubmitJob(std::uint8_t) override
    {
        return GearsetsSubmission::Unavailable;
    }
};

enum class GearsetsPhase
{
    Checking,
    Applying,
    Complete,
    Rejected,
    Partial,
    Unconfirmed,
    Cancelled
};
std::string_view GearsetsPhaseName(GearsetsPhase phase);

struct GearsetsResult
{
    GearsetsPhase                         phase = GearsetsPhase::Rejected;
    std::string                           reason;
    std::optional<std::uint8_t>           slot;
    std::array<bool, kEquipmentSlotCount> changed{};
    std::uint32_t                         submissions = 0;
};

// The owner serializes calls. Begin copies intent; sent requests are never
// retried or rolled back.
class GearsetsTransaction
{
public:
    bool Begin(const GearsetIntent& intent, const GearsetsView& view, GearsetsExecutor& executor);
    void Step(const GearsetsView& view, GearsetsExecutor& executor);
    void Cancel();
    void EndSession();
    bool Active() const;

    const GearsetsResult& Result() const
    {
        return result_;
    }

private:
    std::optional<GearsetsFailure> Preflight(const GearsetsView& view, const GearsetsExecutor& executor) const;
    void                           Stop(std::string reason, std::optional<std::uint8_t> slot = {});
    void                           ObserveChanges(const GearsetsView& view);
    bool                           ExpectedChanges(const GearsetsView& view, std::optional<std::uint8_t>& unexpected) const;
    bool                           LateClassChange(const GearsetsView& view) const;
    bool                           WearsIntent(const GearsetsView& view) const;
    bool                           TowardJob(const GearsetsView& view) const;
    void                           SubmitJob(const GearsetsView& view, GearsetsExecutor& executor);

    GearsetIntent                    intent_;
    GearsetsPlan                     plan_;
    GearsetsView                     baseline_;
    GearsetsView                     expected_;
    GearsetsResult                   result_;
    std::optional<GearsetsOperation> pending_;
    std::uint64_t                    submittedMs_     = 0;
    bool                             cancelRequested_ = false;
    // A late class change seen while another request was pending settles once that request confirms.
    bool                         lateSettle_ = false;
    std::optional<std::uint64_t> settleUntilMs_;
    std::optional<std::uint64_t> deferredSinceMs_;
    // Job refusals have no reply; retry only after further slot submissions.
    bool                         jobPending_     = false;
    bool                         jobChanged_     = false; // counts toward a partial outcome
    std::uint32_t                jobAttempts_    = 0;
    std::uint32_t                jobSubmissions_ = 0; // submissions as of the last attempt
    std::optional<std::uint64_t> classSinceMs_;
};

} // namespace bahamut_client
