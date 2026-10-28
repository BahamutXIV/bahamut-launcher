#include "autologin_machine.h"

#include <cstdio>
#include <utility>

namespace bahamut_client
{
namespace
{

constexpr std::uint32_t kBootupTitlePhase     = 1u;
constexpr std::uint32_t kBootupCharacterPhase = 5u;
constexpr std::uint64_t kPreTitleTimeoutMs    = 300000u;
constexpr std::uint64_t kTitleSettleMs        = 4000u;
constexpr std::uint64_t kListTimeoutMs        = 60000u;
constexpr std::uint64_t kListSettleMs         = 3000u;
constexpr std::uint64_t kSnapshotStaleMs      = 1000u;
constexpr std::uint64_t kWorldTimeoutMs       = 60000u;
constexpr std::uint32_t kMinSlot              = 1u;
constexpr std::uint32_t kMaxSlot              = 8u;

// Treat a cross-thread timestamp slightly ahead of "now" as zero elapsed time.
std::uint64_t Since(std::uint64_t now, std::uint64_t then)
{
    return now >= then ? now - then : 0;
}

bool Finished(AutologinPhase phase)
{
    return phase == AutologinPhase::Idle || phase == AutologinPhase::Done || phase == AutologinPhase::Cancelled || phase == AutologinPhase::NeedsUser;
}

bool Fresh(const LoginScreen& screen, std::uint64_t now)
{
    return screen.capturedAt != 0 && Since(now, screen.capturedAt) <= kSnapshotStaleMs;
}

bool InWorld(const LoginScreen& screen)
{
    return !screen.bootup && screen.sceneId == kAutologinWorldScene;
}

const char* RequestStateName(LoginRequestState state)
{
    switch (state)
    {
        case LoginRequestState::Pending:
            return "pending";
        case LoginRequestState::Queued:
            return "queued";
        case LoginRequestState::Rejected:
            return "rejected";
        case LoginRequestState::None:
            break;
    }
    return "none";
}

} // namespace

AutologinMachine::AutologinMachine(AutologinRequest request, AutologinLog log)
: request_(std::move(request))
, log_(std::move(log))
{
}

void AutologinMachine::Enter(AutologinPhase phase, std::uint64_t now, const char* reason)
{
    if (log_)
    {
        char line[128];
        std::snprintf(line, sizeof(line), "state %s -> %s (%s)", AutologinPhaseName(status_.phase), AutologinPhaseName(phase), reason);
        log_(line);
    }
    status_.phase = phase;
    std::snprintf(status_.reason, sizeof(status_.reason), "%s", reason);
    since_ = now;
    if (Finished(phase))
    {
        // Drop a request the frame-tick thread has not run yet.
        Request(LoginRequest::None, 0);
    }
}

void AutologinMachine::Request(LoginRequest request, std::uint32_t slot)
{
    requestId_   = request_ ? request_(request, slot) : 0;
    lastRequest_ = LoginRequestState::Pending;
}

AutologinStatus AutologinMachine::Start(std::uint32_t slot, std::uint64_t now, const LoginScreen& screen)
{
    if (started_)
    {
        return status_;
    }
    started_     = true;
    status_.slot = slot;
    if (slot < kMinSlot || slot > kMaxSlot)
    {
        Enter(AutologinPhase::NeedsUser, now, "slot must be 1-8");
    }
    else if (Fresh(screen, now) && InWorld(screen))
    {
        Enter(AutologinPhase::Idle, now, "already in world");
    }
    else
    {
        Enter(AutologinPhase::WaitTitle, now, "armed");
    }
    return status_;
}

AutologinStatus AutologinMachine::Cancel(std::uint64_t now)
{
    if (Running())
    {
        Enter(AutologinPhase::Cancelled, now, "cancelled");
    }
    return status_;
}

AutologinStatus AutologinMachine::Status() const
{
    return status_;
}

bool AutologinMachine::Running() const
{
    return started_ && !Finished(status_.phase);
}

void AutologinMachine::Step(const AutologinInputs& inputs)
{
    const std::uint64_t now    = inputs.now;
    const LoginScreen&  screen = inputs.screen;
    if (!Running())
    {
        return;
    }
    // Ignore outcomes from captures of other requests.
    if (requestId_ != 0 && screen.requestId == requestId_ && screen.request != lastRequest_)
    {
        lastRequest_ = screen.request;
        if (log_)
        {
            char line[128];
            std::snprintf(line, sizeof(line), "request %s (%s)", RequestStateName(screen.request), screen.requestReason != nullptr ? screen.requestReason : "");
            log_(line);
        }
        if (screen.request == LoginRequestState::Rejected)
        {
            Enter(AutologinPhase::NeedsUser, now, screen.requestReason != nullptr ? screen.requestReason : "request rejected");
            return;
        }
    }
    if (status_.phase == AutologinPhase::Submitted)
    {
        if (!screen.bootup && screen.sceneId != 0u && Fresh(screen, now))
        {
            if (screen.sceneId == kAutologinWorldScene)
            {
                Enter(AutologinPhase::Done, now, "entered world");
            }
            else
            {
                Enter(AutologinPhase::NeedsUser, now, "unexpected scene after submit");
            }
        }
        else if (Since(now, since_) > kWorldTimeoutMs)
        {
            Enter(AutologinPhase::NeedsUser, now, "no world entry");
        }
        return;
    }
    if (!Fresh(screen, now))
    {
        const std::uint64_t limit = status_.phase == AutologinPhase::WaitTitle ? kPreTitleTimeoutMs : kListTimeoutMs;
        if (Since(now, since_) > limit)
        {
            Enter(AutologinPhase::NeedsUser, now, "stale screen state");
        }
        return;
    }
    const bool title = screen.bootup && screen.phase == kBootupTitlePhase;
    const bool list  = screen.bootup && screen.phase == kBootupCharacterPhase && screen.roster && screen.closing == 0;
    switch (status_.phase)
    {
        case AutologinPhase::WaitTitle:
            if (InWorld(screen))
            {
                Enter(AutologinPhase::Idle, now, "already in world");
                return;
            }
            if (!title)
            {
                titleSince_ = 0;
                if (screen.bootup)
                {
                    Enter(AutologinPhase::NeedsUser, now, "unexpected phase before title");
                }
                else if (Since(now, since_) > kPreTitleTimeoutMs)
                {
                    Enter(AutologinPhase::NeedsUser, now, "no title");
                }
                return;
            }
            if (titleSince_ == 0)
            {
                titleSince_ = now;
            }
            if (Since(now, titleSince_) >= kTitleSettleMs)
            {
                Enter(AutologinPhase::TitleStart, now, "title settled");
                Request(LoginRequest::Title, 0);
            }
            return;
        case AutologinPhase::TitleStart:
            // Login can pass through other Bootup phases before the list opens.
            if (list)
            {
                listSince_ = now;
                Enter(AutologinPhase::ListSettle, now, "list ready");
            }
            else if (Since(now, since_) > kListTimeoutMs)
            {
                Enter(AutologinPhase::NeedsUser, now, "no character list");
            }
            return;
        case AutologinPhase::ListSettle:
            if (!list)
            {
                Enter(AutologinPhase::NeedsUser, now, "left character list");
                return;
            }
            // The list ignored an Enter sent 750 ms after opening in a live run;
            // allow it to settle before submitting.
            if (Since(now, listSince_) >= kListSettleMs)
            {
                Enter(AutologinPhase::Submitted, now, "list settled");
                Request(LoginRequest::Slot, status_.slot);
            }
            return;
        default:
            return;
    }
}

} // namespace bahamut_client
