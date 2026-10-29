#pragma once

#include "input_policy.h"

#include <optional>
#include <string>
#include <string_view>

namespace bahamut_client::xhbar
{

std::uint16_t  DecodeRetailPadControls(const ClientPadState& state);
ClientPadState FilterRetailPadControls(const ClientPadState& state, std::uint16_t ownedControls);

// Labels do not change the control mapping.
enum class LabelStyle : std::uint8_t
{
    Xbox,
    PlayStation,
};

// "xbox" or "playstation"; nullopt otherwise.
std::optional<LabelStyle> ParseLabelStyle(std::string_view name);
std::string_view          LabelStyleName(LabelStyle style);

// Settings lines: labels=<name> and visible=true|false.
struct Settings
{
    LabelStyle labels  = LabelStyle::Xbox;
    bool       visible = true;
};

// Last valid line wins; missing or invalid lines retain defaults.
Settings    ReadSettings(std::string_view text);
std::string WriteSettings(const Settings& settings);

} // namespace bahamut_client::xhbar
