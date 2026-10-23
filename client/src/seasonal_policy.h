#pragma once

#include <cstdint>
#include <optional>

namespace bahamut_client::seasonal
{

enum class Season
{
    Ordinary,
    Halloween,
    Christmas,
};

struct Admission
{
    std::uintptr_t actor          = 0;
    std::uint32_t  region         = 0;
    std::uint32_t  currentId      = 0;
    std::uint32_t  currentKey     = 0;
    bool           regionValid    = false;
    bool           weatherEnabled = false;
    bool           stateThree     = false;
    bool           pendingRegion  = false;
    bool           pendingWeather = false;
    bool           ownerReady     = false;
};

struct Decision
{
    bool          forward      = false;
    bool          clearWeather = false;
    std::uint32_t selector     = 0;
    std::uint32_t value        = 0;
};

class SeasonalPolicy
{
public:
    Decision                Request(std::uint32_t selector, std::uint32_t value, const Admission& admission);
    std::optional<Decision> Replay(const Admission& admission);
    void                    Reset(std::uintptr_t actor = 0);
    Season                  Mode(std::uintptr_t actor) const;

private:
    struct Pending
    {
        std::uint32_t selector = 0;
        std::uint32_t value    = 0;
    };

    void     ReplaceActor(std::uintptr_t actor);
    bool     CanSubmit(const Admission& admission) const;
    Decision Compute(std::uint32_t selector, std::uint32_t value, const Admission& admission);

    std::uintptr_t         actor_ = 0;
    Season                 mode_  = Season::Ordinary;
    std::optional<Pending> pending_;
};

std::optional<std::uint32_t> SelectedWeatherKey(Season season, std::uint32_t region);
std::uint32_t                CityRoot(std::uint32_t region);

} // namespace bahamut_client::seasonal
