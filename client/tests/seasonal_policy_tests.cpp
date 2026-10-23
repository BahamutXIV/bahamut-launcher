#include "seasonal_policy.h"

#include <cstdint>
#include <iostream>
#include <optional>

namespace
{

using bahamut_client::seasonal::Admission;
using bahamut_client::seasonal::CityRoot;
using bahamut_client::seasonal::Decision;
using bahamut_client::seasonal::Season;
using bahamut_client::seasonal::SeasonalPolicy;
using bahamut_client::seasonal::SelectedWeatherKey;

constexpr std::uintptr_t kActor = 0x1000;

Admission Ready(std::uint32_t region = 101)
{
    return Admission{ kActor, region, 0, 0, true, true, false, false, false, true };
}

bool IsDecision(const Decision& decision, bool forward, std::uint32_t selector, std::uint32_t value, bool clear = false)
{
    return decision.forward == forward && decision.clearWeather == clear && decision.selector == selector && decision.value == value;
}

bool KeysAndRoots()
{
    return CityRoot(101) == 101 && CityRoot(103) == 103 && CityRoot(104) == 104 && CityRoot(106) == 103 && CityRoot(115) == 103 && CityRoot(122) == 103 && CityRoot(123) == 103 && CityRoot(999) == 0 && SelectedWeatherKey(Season::Halloween, 101) == std::optional<std::uint32_t>{ 0x29d9001f } && SelectedWeatherKey(Season::Halloween, 103) == std::optional<std::uint32_t>{ 0x29d9001f } && SelectedWeatherKey(Season::Halloween, 104) == std::optional<std::uint32_t>{ 0x615a0023 } && SelectedWeatherKey(Season::Christmas, 101) == std::optional<std::uint32_t>{ 0x29b00020 } && SelectedWeatherKey(Season::Christmas, 104) == std::optional<std::uint32_t>{ 0x29b00020 } && !SelectedWeatherKey(Season::Ordinary, 101).has_value() && !SelectedWeatherKey(Season::Halloween, 999).has_value();
}

bool LogicalIdsAndKeys()
{
    SeasonalPolicy policy;
    Admission      admission = Ready(104);
    admission.currentId      = 8027;
    if (!IsDecision(policy.Request(0x0c, 8027, admission), true, 0x0c, 8027, true) || policy.Mode(kActor) != Season::Halloween)
    {
        return false;
    }
    if (!IsDecision(policy.Request(0x0c, 8027, admission), true, 0x0c, 8027) || policy.Mode(kActor) != Season::Halloween)
    {
        return false;
    }
    if (!IsDecision(policy.Request(0x0c, 8033, admission), true, 0x0c, 8027, true) || policy.Mode(kActor) != Season::Christmas)
    {
        return false;
    }
    if (!IsDecision(policy.Request(0x0c, 8032, admission), true, 0x0c, 8032) || policy.Mode(kActor) != Season::Ordinary)
    {
        return false;
    }
    if (!IsDecision(policy.Request(0x0c, 8033, Ready(999)), true, 0x0c, 8033) || policy.Mode(kActor) != Season::Ordinary)
    {
        return false;
    }

    const std::uint32_t halloweenKey = SelectedWeatherKey(Season::Halloween, 104).value();
    if (!IsDecision(policy.Request(0x0d, 8033, admission), true, 0x0d, 8033) || policy.Mode(kActor) != Season::Ordinary)
    {
        return false;
    }
    if (!IsDecision(policy.Request(0x0d, halloweenKey, admission), true, 0x0d, halloweenKey) || policy.Mode(kActor) != Season::Ordinary)
    {
        return false;
    }
    if (!IsDecision(policy.Request(0x0c, 8027, admission), true, 0x0c, 8027, true) || !IsDecision(policy.Request(0x0d, halloweenKey, admission), true, 0x0d, halloweenKey) || policy.Mode(kActor) != Season::Halloween)
    {
        return false;
    }
    if (!IsDecision(policy.Request(0x0d, 0xdeadbeef, admission), true, 0x0d, 0xdeadbeef) || policy.Mode(kActor) != Season::Ordinary)
    {
        return false;
    }
    return true;
}

bool SameIdClearAndGuards()
{
    SeasonalPolicy policy;
    Admission      admission = Ready();
    if (!IsDecision(policy.Request(0x0c, 8027, admission), true, 0x0c, 8027))
    {
        return false;
    }
    const auto blockedBy = [&](auto change)
    {
        Admission candidate = admission;
        change(candidate);
        return !policy.Request(0x0c, 8033, candidate).forward;
    };
    if (!blockedBy([](Admission& candidate)
                   {
                       candidate.regionValid = false;
                   }) ||
        !blockedBy([](Admission& candidate)
                   {
                       candidate.weatherEnabled = false;
                   }) ||
        !blockedBy([](Admission& candidate)
                   {
                       candidate.ownerReady = false;
                   }))
    {
        return false;
    }

    if (!blockedBy([](Admission& candidate)
                   {
                       candidate.actor = 0;
                   }) ||
        !blockedBy([](Admission& candidate)
                   {
                       candidate.region = 0;
                   }) ||
        !blockedBy([](Admission& candidate)
                   {
                       candidate.stateThree = true;
                   }) ||
        !blockedBy([](Admission& candidate)
                   {
                       candidate.pendingRegion = true;
                   }) ||
        !blockedBy([](Admission& candidate)
                   {
                       candidate.pendingWeather = true;
                   }))
    {
        return false;
    }
    return true;
}

bool DeferredReplayAndLatestWins()
{
    SeasonalPolicy policy;
    Admission      admission = Ready(101);
    Admission      blocked   = admission;
    blocked.pendingWeather   = true;
    if (policy.Request(0x0c, 8033, blocked).forward || policy.Mode(kActor) != Season::Ordinary)
    {
        return false;
    }
    Admission replayAdmission            = Ready(104);
    replayAdmission.currentId            = 8027;
    const std::optional<Decision> replay = policy.Replay(replayAdmission);
    if (!replay.has_value() || !IsDecision(replay.value(), true, 0x0c, 8027, true) || policy.Mode(kActor) != Season::Christmas)
    {
        return false;
    }
    if (policy.Replay(replayAdmission).has_value())
    {
        return false;
    }

    SeasonalPolicy latestPolicy;
    Admission      latestAdmission = Ready(101);
    latestAdmission.currentId      = 8027;
    if (!IsDecision(latestPolicy.Request(0x0c, 8027, latestAdmission), true, 0x0c, 8027, true))
    {
        return false;
    }
    blocked                = latestAdmission;
    blocked.pendingWeather = true;
    if (latestPolicy.Request(0x0c, 8033, blocked).forward || latestPolicy.Request(0x0c, 8032, blocked).forward)
    {
        return false;
    }
    const std::optional<Decision> latestReplay = latestPolicy.Replay(Ready(104));
    if (!latestReplay.has_value() || !IsDecision(latestReplay.value(), true, 0x0c, 8032) || latestPolicy.Mode(kActor) != Season::Ordinary || latestPolicy.Replay(Ready(104)).has_value())
    {
        return false;
    }

    blocked               = admission;
    blocked.pendingRegion = true;
    if (latestPolicy.Request(0x0c, 8027, blocked).forward)
    {
        return false;
    }
    Admission actorReplacement = Ready(103);
    actorReplacement.actor     = 0x2000;
    if (!IsDecision(latestPolicy.Request(0x0c, 8033, actorReplacement), true, 0x0c, 8027) || latestPolicy.Mode(0x2000) != Season::Christmas || latestPolicy.Mode(kActor) != Season::Ordinary)
    {
        return false;
    }
    return !latestPolicy.Replay(actorReplacement).has_value();
}

bool RawKeyAcrossRegions()
{
    for (const auto origin : { 101u, 104u })
    {
        const auto     destination = origin == 101 ? 104u : 103u;
        const auto     oldKey      = SelectedWeatherKey(Season::Halloween, origin).value();
        const auto     newKey      = SelectedWeatherKey(Season::Halloween, destination).value();
        SeasonalPolicy direct;
        static_cast<void>(direct.Request(0x0c, 8027, Ready(origin)));
        if (!IsDecision(direct.Request(0x0d, oldKey, Ready(destination)), true, 0x0d, newKey) || direct.Mode(kActor) != Season::Halloween)
        {
            return false;
        }
        SeasonalPolicy deferred;
        static_cast<void>(deferred.Request(0x0c, 8027, Ready(origin)));
        auto blocked          = Ready(origin);
        blocked.pendingRegion = true;
        if (deferred.Request(0x0d, oldKey, blocked).forward)
        {
            return false;
        }
        const auto replay = deferred.Replay(Ready(destination));
        if (!replay || !IsDecision(*replay, true, 0x0d, newKey) || deferred.Mode(kActor) != Season::Halloween)
        {
            return false;
        }
        if (!IsDecision(deferred.Request(0x0d, 0xDEADBEEF, Ready(destination)), true, 0x0d, 0xDEADBEEF) || deferred.Mode(kActor) != Season::Ordinary)
        {
            return false;
        }
    }
    SeasonalPolicy unknownRoot;
    static_cast<void>(unknownRoot.Request(0x0c, 8027, Ready(101)));
    if (!IsDecision(unknownRoot.Request(0x0d, 0x29D9001F, Ready(999)), true, 0x0d, 0x29D9001F) || unknownRoot.Mode(kActor) != Season::Ordinary)
    {
        return false;
    }
    SeasonalPolicy otherSeason;
    static_cast<void>(otherSeason.Request(0x0c, 8033, Ready(101)));
    return IsDecision(otherSeason.Request(0x0d, 0x29D9001F, Ready(104)), true, 0x0d, 0x29D9001F) && otherSeason.Mode(kActor) == Season::Ordinary;
}

bool ResetAndOtherSelectors()
{
    SeasonalPolicy policy;
    Admission      admission = Ready();
    if (!IsDecision(policy.Request(0x0c, 8027, admission), true, 0x0c, 8027) || !IsDecision(policy.Request(0x01, 8033, admission), true, 0x01, 8033) || policy.Mode(kActor) != Season::Halloween)
    {
        return false;
    }
    policy.Reset(kActor + 1);
    if (policy.Mode(kActor) != Season::Halloween)
    {
        return false;
    }
    policy.Reset(kActor);
    if (policy.Mode(kActor) != Season::Ordinary)
    {
        return false;
    }
    admission.pendingRegion = true;
    if (policy.Request(0x0c, 8033, admission).forward)
    {
        return false;
    }
    policy.Reset();
    return !policy.Replay(Ready()).has_value() && policy.Mode(kActor) == Season::Ordinary;
}

} // namespace

int main()
{
    if (!KeysAndRoots())
        return 1;
    if (!LogicalIdsAndKeys())
        return 2;
    if (!SameIdClearAndGuards())
        return 3;
    if (!DeferredReplayAndLatestWins())
        return 4;
    if (!ResetAndOtherSelectors())
        return 5;
    if (!RawKeyAcrossRegions())
        return 6;

    std::cout << "seasonal policy tests passed: keys and roots, logical IDs and raw keys, guards, replay, actor replacement and reset\n";
    return 0;
}
