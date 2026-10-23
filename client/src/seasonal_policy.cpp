#include "seasonal_policy.h"

namespace bahamut_client::seasonal
{

namespace
{

constexpr std::uint32_t kLogicalWeatherSelector = 0x0c;
constexpr std::uint32_t kRawWeatherSelector     = 0x0d;
constexpr std::uint32_t kHalloweenId            = 8027;
constexpr std::uint32_t kChristmasId            = 8033;
constexpr std::uint32_t kChristmasNativeId      = 8027;

bool IsWeatherSelector(std::uint32_t selector)
{
    return selector == kLogicalWeatherSelector || selector == kRawWeatherSelector;
}

bool IsSeasonalKey(Season season, std::uint32_t value)
{
    for (const auto root : { 101u, 103u, 104u })
    {
        const auto key = SelectedWeatherKey(season, root);
        if (key && *key == value)
        {
            return true;
        }
    }
    return false;
}

Season LogicalSeason(std::uint32_t value, std::uint32_t root)
{
    if (root == 0)
    {
        return Season::Ordinary;
    }
    if (value == kHalloweenId)
    {
        return Season::Halloween;
    }
    if (value == kChristmasId)
    {
        return Season::Christmas;
    }
    return Season::Ordinary;
}

std::uint32_t NativeId(std::uint32_t value, Season season, std::uint32_t root)
{
    if (root != 0 && season == Season::Christmas && value == kChristmasId)
    {
        return kChristmasNativeId;
    }
    return value;
}

} // namespace

std::uint32_t CityRoot(std::uint32_t region)
{
    switch (region)
    {
        case 101:
        case 103:
        case 104:
            return region;
        case 106:
        case 115:
        case 122:
        case 123:
            return 103;
        default:
            return 0;
    }
}

std::optional<std::uint32_t> SelectedWeatherKey(Season season, std::uint32_t region)
{
    switch (CityRoot(region))
    {
        case 101:
        case 103:
            if (season == Season::Halloween)
            {
                return 0x29d9001f;
            }
            if (season == Season::Christmas)
            {
                return 0x29b00020;
            }
            break;
        case 104:
            if (season == Season::Halloween)
            {
                return 0x615a0023;
            }
            if (season == Season::Christmas)
            {
                return 0x29b00020;
            }
            break;
        default:
            break;
    }
    return std::nullopt;
}

void SeasonalPolicy::ReplaceActor(std::uintptr_t actor)
{
    if (actor_ == actor)
    {
        return;
    }
    actor_   = actor;
    mode_    = Season::Ordinary;
    pending_ = std::nullopt;
}

bool SeasonalPolicy::CanSubmit(const Admission& admission) const
{
    return admission.actor != 0 && admission.regionValid && admission.region != 0 && admission.weatherEnabled && !admission.stateThree && !admission.pendingRegion && !admission.pendingWeather && admission.ownerReady;
}

Decision SeasonalPolicy::Compute(std::uint32_t selector, std::uint32_t value, const Admission& admission)
{
    Decision            decision{ true, false, selector, value };
    const std::uint32_t root         = CityRoot(admission.region);
    const Season        previousMode = mode_;
    Season              nextMode     = Season::Ordinary;

    if (selector == kLogicalWeatherSelector)
    {
        nextMode              = LogicalSeason(value, root);
        decision.value        = NativeId(value, nextMode, root);
        decision.clearWeather = decision.value == kHalloweenId && admission.currentId == kHalloweenId && previousMode != nextMode;
    }
    else if (selector == kRawWeatherSelector)
    {
        const std::optional<std::uint32_t> activeKey = SelectedWeatherKey(mode_, admission.region);
        if (activeKey.has_value() && IsSeasonalKey(mode_, value))
        {
            // A queued mode-byte replay can retain the previous city's key.
            nextMode       = mode_;
            decision.value = activeKey.value();
        }
    }

    mode_ = nextMode;
    return decision;
}

Decision SeasonalPolicy::Request(std::uint32_t selector, std::uint32_t value, const Admission& admission)
{
    ReplaceActor(admission.actor);

    if (!IsWeatherSelector(selector))
    {
        return Decision{ true, false, selector, value };
    }

    if (!CanSubmit(admission))
    {
        pending_ = Pending{ selector, value };
        return Decision{ false, false, selector, value };
    }

    pending_ = std::nullopt;
    return Compute(selector, value, admission);
}

std::optional<Decision> SeasonalPolicy::Replay(const Admission& admission)
{
    ReplaceActor(admission.actor);
    if (!pending_.has_value() || !CanSubmit(admission))
    {
        return std::nullopt;
    }

    const Pending pending = pending_.value();
    pending_              = std::nullopt;
    return Compute(pending.selector, pending.value, admission);
}

void SeasonalPolicy::Reset(std::uintptr_t actor)
{
    if (actor != 0 && actor_ != actor)
    {
        return;
    }
    actor_   = 0;
    mode_    = Season::Ordinary;
    pending_ = std::nullopt;
}

Season SeasonalPolicy::Mode(std::uintptr_t actor) const
{
    if (actor == 0 || actor != actor_)
    {
        return Season::Ordinary;
    }
    return mode_;
}

} // namespace bahamut_client::seasonal
