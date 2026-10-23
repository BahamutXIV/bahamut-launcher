#pragma once

#include "seasonal_catalog.h"
#include "seasonal_policy.h"

#include <filesystem>
#include <string>

namespace bahamut_client
{

// The runtime's exact executable identity gate must precede Install.
class SeasonalWeatherService
{
public:
    bool               Install(bool exactBuild, const std::filesystem::path& logFile = {});
    bool               Shutdown();
    const std::string& LastError() const;

private:
    bool        installed_ = false;
    std::string error_;
};

namespace seasonal
{

bool         VerifySeasonalBindings(ReadMemory read, void* context, std::uintptr_t imageBase) noexcept;
std::uint8_t SeasonalFlag(TimelineRole role, Season season, std::uint8_t original) noexcept;

#ifdef BAHAMUT_SEASONAL_WEATHER_TEST
using ConditionFixture = std::uint8_t (*)(std::uintptr_t property, std::uintptr_t entry, std::uint8_t original);
bool InstallConditionFixture(void* target, ConditionFixture adjuster);
bool RemoveConditionFixture(void* target);
#endif

} // namespace seasonal
} // namespace bahamut_client
