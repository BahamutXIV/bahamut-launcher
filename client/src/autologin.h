#pragma once

#include <cstdint>

namespace bahamut_client
{

enum class AutologinPhase
{
    Idle,
    WaitTitle,
    TitleStart,
    ListSettle,
    Submitted,
    Done,
    Cancelled,
    NeedsUser,
};

struct AutologinStatus
{
    AutologinPhase phase = AutologinPhase::Idle;
    std::uint32_t  slot  = 0;
    char           reason[64]{};
};

// Lowercase hyphenated name, for example "wait-title" or "needs-user".
inline const char* AutologinPhaseName(AutologinPhase phase)
{
    switch (phase)
    {
        case AutologinPhase::Idle:
            return "idle";
        case AutologinPhase::WaitTitle:
            return "wait-title";
        case AutologinPhase::TitleStart:
            return "title-start";
        case AutologinPhase::ListSettle:
            return "list-settle";
        case AutologinPhase::Submitted:
            return "submitted";
        case AutologinPhase::Done:
            return "done";
        case AutologinPhase::Cancelled:
            return "cancelled";
        case AutologinPhase::NeedsUser:
            return "needs-user";
    }
    return "unknown";
}

// Methods are safe from any thread. One run per game process: Start arms only
// the first time; later calls return the current status unchanged.
class AutologinService
{
public:
    virtual ~AutologinService()                       = default;
    virtual AutologinStatus Start(std::uint32_t slot) = 0;
    virtual AutologinStatus Cancel()                  = 0;
};

AutologinService& ProductionAutologinService();

// Called once per rendered frame on the render thread.
void StepAutologin();

} // namespace bahamut_client
