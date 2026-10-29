#include "xhbar_pad.h"

#include "xhbar_policy.h"

namespace bahamut_client::xhbar
{
namespace
{

// Selected-Xbox buttons: capture SHA-256 f8fd780895995ee11790bb19d310dfa53963bf9c7517097b9fd73469db19fac3.
// Cached XInput triggers: capture SHA-256 f8668bb4004eeebe9bc9168debc538395dff7c66a616f8b788ee151d8556e5be.
// Logical order: Up, Down, Left, Right, A, B, X, Y, LT, RT, RB.
constexpr std::array<std::uint32_t, 11> kRetailMasks{
    0x0001, 0x0002, 0x0004, 0x0008, 0x0020, 0x0080, 0x0040, 0x0010, 0x0200, 0x1000, 0x0800
};

static_assert(kLeftTrigger == (1u << 8) && kRightTrigger == (1u << 9) && kRightShoulder == (1u << 10));

std::uint32_t RetailMask(std::uint16_t controls)
{
    std::uint32_t mask = 0;
    for (std::size_t index = 0; index < kRetailMasks.size(); ++index)
        if (controls & (1u << index))
            mask |= kRetailMasks[index];
    return mask;
}

} // namespace

std::uint16_t DecodeRetailPadControls(const ClientPadState& state)
{
    std::uint16_t controls = 0;
    for (std::size_t index = 0; index < kRetailMasks.size(); ++index)
        if (state.stateDwords[4] & kRetailMasks[index])
            controls |= static_cast<std::uint16_t>(1u << index);
    return controls;
}

ClientPadState FilterRetailPadControls(const ClientPadState& state, std::uint16_t ownedControls)
{
    auto       filtered = state;
    const auto mask     = RetailMask(ownedControls);
    // Captured state words 4-7: held, press, release, repeat.
    for (std::size_t index = 4; index <= 7; ++index)
        filtered.stateDwords[index] &= ~mask;
    return filtered;
}

std::optional<LabelStyle> ParseLabelStyle(std::string_view name)
{
    if (name == "xbox")
        return LabelStyle::Xbox;
    if (name == "playstation")
        return LabelStyle::PlayStation;
    return std::nullopt;
}

std::string_view LabelStyleName(LabelStyle style)
{
    return style == LabelStyle::PlayStation ? "playstation" : "xbox";
}

Settings ReadSettings(std::string_view text)
{
    constexpr std::string_view kLabels  = "labels=";
    constexpr std::string_view kVisible = "visible=";
    Settings                   result;
    while (!text.empty())
    {
        const std::size_t end  = text.find('\n');
        std::string_view  line = text.substr(0, end);
        text                   = end == std::string_view::npos ? std::string_view{} : text.substr(end + 1);
        if (!line.empty() && line.back() == '\r')
            line.remove_suffix(1);
        if (line.substr(0, kLabels.size()) == kLabels)
        {
            if (const auto style = ParseLabelStyle(line.substr(kLabels.size())))
                result.labels = *style;
        }
        else if (line.substr(0, kVisible.size()) == kVisible)
        {
            const std::string_view value = line.substr(kVisible.size());
            if (value == "true" || value == "false")
                result.visible = value == "true";
        }
    }
    return result;
}

std::string WriteSettings(const Settings& settings)
{
    return "labels=" + std::string(LabelStyleName(settings.labels)) + "\nvisible=" + (settings.visible ? "true" : "false") + "\n";
}

} // namespace bahamut_client::xhbar
