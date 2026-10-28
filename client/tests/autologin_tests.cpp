#include "autologin_machine.h"

#include <cstring>
#include <iostream>
#include <stdexcept>
#include <utility>
#include <vector>

namespace
{

using namespace bahamut_client;

constexpr std::uint32_t kLoadingScene = 7;

void Check(bool condition, const char* message)
{
    if (!condition)
    {
        throw std::runtime_error(message);
    }
}

using RequestCall = std::pair<LoginRequest, std::uint32_t>;

LoginScreen TitleScreen()
{
    LoginScreen screen;
    screen.sceneId = 3;
    screen.bootup  = true;
    screen.phase   = 1;
    return screen;
}

LoginScreen ListScreen()
{
    LoginScreen screen;
    screen.sceneId = 3;
    screen.bootup  = true;
    screen.phase   = 5;
    screen.roster  = true;
    return screen;
}

LoginScreen SceneScreen(std::uint32_t sceneId)
{
    LoginScreen screen;
    screen.sceneId = sceneId;
    return screen;
}

// Steps refresh the capture timestamp unless stale; tests supply request outcomes.
struct Rig
{
    std::vector<RequestCall> requests;
    std::uint64_t            now    = 1000;
    LoginScreen              screen = SceneScreen(0);
    bool                     stale  = false;
    std::uint32_t            lastId = 0;
    AutologinMachine         machine{ [this](LoginRequest request, std::uint32_t slot)
                                      {
                                  requests.emplace_back(request, slot);
                                  return request == LoginRequest::None ? 0u : ++lastId;
                              },
                              AutologinLog{} };

    AutologinStatus Start(std::uint32_t slot)
    {
        screen.capturedAt = now;
        return machine.Start(slot, now, screen);
    }

    void Advance(std::uint64_t ms)
    {
        now += ms;
        if (!stale)
        {
            screen.capturedAt = now;
        }
        machine.Step({ screen, now });
    }

    void Run(std::uint64_t ms)
    {
        for (std::uint64_t elapsed = 0; elapsed < ms; elapsed += 100)
        {
            Advance(100);
        }
    }

    bool RunUntil(AutologinPhase phase, std::uint64_t limitMs)
    {
        for (std::uint64_t elapsed = 0; elapsed < limitMs && machine.Status().phase != phase; elapsed += 100)
        {
            Advance(100);
        }
        return machine.Status().phase == phase;
    }

    std::size_t Count(LoginRequest request) const
    {
        std::size_t count = 0;
        for (const RequestCall& call : requests)
        {
            count += call.first == request ? 1u : 0u;
        }
        return count;
    }

    void ReachSubmitted(std::uint32_t slot)
    {
        screen = TitleScreen();
        Start(slot);
        Check(RunUntil(AutologinPhase::TitleStart, 6000), "title start not requested");
        screen = ListScreen();
        Check(RunUntil(AutologinPhase::Submitted, 5000), "slot not requested");
    }
};

void title_settles_then_requests_start_once()
{
    Rig rig;
    rig.screen = TitleScreen();
    rig.Start(1);
    rig.Run(3800);
    Check(rig.requests.empty(), "start requested before the title settled");
    rig.Run(400);
    Check(rig.machine.Status().phase == AutologinPhase::TitleStart, "title not settled");
    rig.Run(5000);
    Check(rig.requests.size() == 1 && rig.requests[0] == RequestCall{ LoginRequest::Title, 0u }, "title start not requested exactly once");
}

void list_settles_then_requests_the_slot()
{
    Rig rig;
    rig.screen = TitleScreen();
    rig.Start(3);
    Check(rig.RunUntil(AutologinPhase::TitleStart, 6000), "title start not requested");
    rig.screen.phase = 2;
    rig.Run(5000);
    Check(rig.machine.Status().phase == AutologinPhase::TitleStart, "intermediate phase stopped the run");
    rig.screen = ListScreen();
    rig.Advance(100);
    Check(rig.machine.Status().phase == AutologinPhase::ListSettle, "list not entered");
    rig.Run(2800);
    Check(rig.Count(LoginRequest::Slot) == 0, "slot requested before the list settled");
    rig.Run(300);
    Check(rig.machine.Status().phase == AutologinPhase::Submitted, "slot not requested after settle");
    Check(rig.requests.back() == RequestCall{ LoginRequest::Slot, 3u }, "wrong slot requested");
}

void closing_list_does_not_count_as_ready()
{
    Rig rig;
    rig.screen = TitleScreen();
    rig.Start(1);
    Check(rig.RunUntil(AutologinPhase::TitleStart, 6000), "title start not requested");
    rig.screen         = ListScreen();
    rig.screen.closing = 1;
    rig.Run(5000);
    Check(rig.machine.Status().phase == AutologinPhase::TitleStart, "closing list accepted");
}

void rejected_request_needs_user_with_its_reason()
{
    Rig rig;
    rig.ReachSubmitted(2);
    rig.screen.requestId     = rig.lastId;
    rig.screen.request       = LoginRequestState::Rejected;
    rig.screen.requestReason = "slot is empty";
    rig.Advance(100);
    const AutologinStatus status = rig.machine.Status();
    Check(status.phase == AutologinPhase::NeedsUser && std::strcmp(status.reason, "slot is empty") == 0, "rejection not reported");
    Check(rig.requests.back().first == LoginRequest::None, "pending request not cleared");
}

void queued_request_keeps_waiting()
{
    Rig rig;
    rig.ReachSubmitted(1);
    rig.screen.requestId     = rig.lastId;
    rig.screen.request       = LoginRequestState::Queued;
    rig.screen.requestReason = "slot submitted";
    rig.Run(1000);
    Check(rig.machine.Status().phase == AutologinPhase::Submitted, "queued request ended the wait");
}

void state_of_another_request_is_ignored()
{
    Rig rig;
    rig.ReachSubmitted(1);
    // Id 1 was the title start; the slot submit is id 2.
    rig.screen.requestId     = 1;
    rig.screen.request       = LoginRequestState::Rejected;
    rig.screen.requestReason = "old";
    rig.Run(500);
    Check(rig.machine.Status().phase == AutologinPhase::Submitted, "another request's state was applied");
}

void no_list_after_title_start_times_out()
{
    Rig rig;
    rig.screen = TitleScreen();
    rig.Start(1);
    Check(rig.RunUntil(AutologinPhase::TitleStart, 6000), "title start not requested");
    rig.Run(59000);
    Check(rig.machine.Status().phase == AutologinPhase::TitleStart, "gave up early");
    rig.Run(2000);
    Check(rig.machine.Status().phase == AutologinPhase::NeedsUser && std::strcmp(rig.machine.Status().reason, "no character list") == 0, "missing list not reported");
}

void leaving_the_list_before_submit_needs_user()
{
    Rig rig;
    rig.screen = TitleScreen();
    rig.Start(1);
    Check(rig.RunUntil(AutologinPhase::TitleStart, 6000), "title start not requested");
    rig.screen = ListScreen();
    rig.Advance(100);
    rig.screen = TitleScreen();
    rig.Advance(100);
    Check(rig.machine.Status().phase == AutologinPhase::NeedsUser, "left list accepted");
    Check(rig.Count(LoginRequest::Slot) == 0, "slot requested after leaving the list");
}

void already_in_world_at_arming_requests_nothing()
{
    Rig rig;
    rig.screen                   = SceneScreen(kAutologinWorldScene);
    const AutologinStatus status = rig.Start(1);
    Check(status.phase == AutologinPhase::Idle && std::strcmp(status.reason, "already in world") == 0, "world not detected at arming");
    rig.Run(10000);
    Check(rig.Count(LoginRequest::Title) == 0 && rig.Count(LoginRequest::Slot) == 0, "requests sent from the world");
}

void slot_outside_one_to_eight_needs_user()
{
    for (const std::uint32_t slot : { 0u, 9u })
    {
        Rig rig;
        rig.screen                   = TitleScreen();
        const AutologinStatus status = rig.Start(slot);
        Check(status.phase == AutologinPhase::NeedsUser && status.slot == slot, "invalid slot accepted");
        rig.Run(10000);
        Check(rig.Count(LoginRequest::Title) == 0, "title requested for an invalid slot");
    }
}

void non_world_scene_after_submit_needs_user()
{
    Rig rig;
    rig.ReachSubmitted(1);
    rig.screen = SceneScreen(kLoadingScene);
    rig.Run(200);
    const AutologinStatus status = rig.machine.Status();
    Check(status.phase == AutologinPhase::NeedsUser && std::strcmp(status.reason, "unexpected scene after submit") == 0, "foreign scene accepted");
}

void world_scene_after_submit_is_done()
{
    Rig rig;
    rig.ReachSubmitted(1);
    rig.Run(300);
    Check(rig.machine.Status().phase == AutologinPhase::Submitted, "left submitted while still in bootup");
    rig.screen = SceneScreen(kAutologinWorldScene);
    rig.Run(200);
    Check(rig.machine.Status().phase == AutologinPhase::Done, "world scene not done");
}

void cancel_clears_the_request_and_does_not_rearm()
{
    Rig rig;
    rig.screen = TitleScreen();
    rig.Start(1);
    Check(rig.RunUntil(AutologinPhase::TitleStart, 6000), "title start not requested");
    rig.machine.Cancel(rig.now);
    Check(rig.requests.back().first == LoginRequest::None, "cancel left the request pending");
    const AutologinStatus again = rig.Start(2);
    Check(again.phase == AutologinPhase::Cancelled && again.slot == 1, "second start replaced the run");
    rig.Run(10000);
    Check(rig.Count(LoginRequest::Title) == 1 && rig.Count(LoginRequest::Slot) == 0, "cancelled run requested more");
}

void stale_screen_times_out_without_title()
{
    Rig rig;
    rig.stale = true;
    rig.Start(1);
    rig.Run(299000);
    Check(rig.machine.Status().phase == AutologinPhase::WaitTitle, "gave up early");
    rig.Run(2000);
    Check(rig.machine.Status().phase == AutologinPhase::NeedsUser && rig.Count(LoginRequest::Title) == 0, "stale screen not reported");
}

void phase_names_are_lowercase_hyphenated()
{
    Check(std::strcmp(AutologinPhaseName(AutologinPhase::WaitTitle), "wait-title") == 0 && std::strcmp(AutologinPhaseName(AutologinPhase::TitleStart), "title-start") == 0 && std::strcmp(AutologinPhaseName(AutologinPhase::NeedsUser), "needs-user") == 0, "phase name differs");
}

} // namespace

int main()
{
    const struct
    {
        const char* name;
        void (*run)();
    } tests[] = {
        { "title_settles_then_requests_start_once", title_settles_then_requests_start_once },
        { "list_settles_then_requests_the_slot", list_settles_then_requests_the_slot },
        { "closing_list_does_not_count_as_ready", closing_list_does_not_count_as_ready },
        { "rejected_request_needs_user_with_its_reason", rejected_request_needs_user_with_its_reason },
        { "queued_request_keeps_waiting", queued_request_keeps_waiting },
        { "state_of_another_request_is_ignored", state_of_another_request_is_ignored },
        { "no_list_after_title_start_times_out", no_list_after_title_start_times_out },
        { "leaving_the_list_before_submit_needs_user", leaving_the_list_before_submit_needs_user },
        { "already_in_world_at_arming_requests_nothing", already_in_world_at_arming_requests_nothing },
        { "slot_outside_one_to_eight_needs_user", slot_outside_one_to_eight_needs_user },
        { "non_world_scene_after_submit_needs_user", non_world_scene_after_submit_needs_user },
        { "world_scene_after_submit_is_done", world_scene_after_submit_is_done },
        { "cancel_clears_the_request_and_does_not_rearm", cancel_clears_the_request_and_does_not_rearm },
        { "stale_screen_times_out_without_title", stale_screen_times_out_without_title },
        { "phase_names_are_lowercase_hyphenated", phase_names_are_lowercase_hyphenated },
    };

    for (const auto& test : tests)
    {
        try
        {
            test.run();
        }
        catch (const std::exception& error)
        {
            std::cerr << test.name << ": " << error.what() << '\n';
            return 1;
        }
    }
    std::cout << "autologin fixtures passed\n";
    return 0;
}
