#include "xhbar_pad.h"
#include "xhbar_policy.h"

#include <cstring>
#include <iostream>
#include <stdexcept>

using namespace bahamut_client::xhbar;

namespace
{

void Require(bool condition, const char* reason)
{
    if (!condition)
        throw std::runtime_error(reason);
}

void MappedSlotEdges()
{
    constexpr std::array<std::uint32_t, 8> buttons{ 1, 2, 4, 8, 0x20, 0x80, 0x40, 0x10 };
    for (const auto trigger : { 0x1000u, 0x0200u })
    {
        for (std::size_t slot = 0; slot < buttons.size(); ++slot)
        {
            InputPolicy policy;
            Sample      context{ 0, true, true, false, { 1, 1, 1 } };
            policy.Update(context);
            ClientPadState state;
            state.stateDwords[4] = trigger;
            context.held         = DecodeRetailPadControls(state);
            policy.Update(context);
            state.stateDwords[4] |= buttons[slot];
            state.stateDwords[5] = buttons[slot];
            context.held         = DecodeRetailPadControls(state);
            const auto press     = policy.Update(context);
            Require(press.edgeCount == 1 && press.edges[0].slot == slot + (trigger == 0x1000 ? 8 : 0), "retail slot mapping");
            Require(FilterRetailPadControls(state, press.consume).stateDwords[4] == 0, "captured held controls escaped");
            state.stateDwords[5] = 0;
            state.stateDwords[7] = buttons[slot];
            const auto hold      = policy.Update(context);
            Require(hold.edgeCount == 0 && FilterRetailPadControls(state, hold.consume).stateDwords[7] == 0, "native repeat escaped");
            state.stateDwords[4] = trigger;
            state.stateDwords[6] = buttons[slot];
            context.held         = DecodeRetailPadControls(state);
            const auto release   = policy.Update(context);
            Require(FilterRetailPadControls(state, release.consume).stateDwords[6] == 0, "owned release escaped");
        }
    }
}

void ExactPreservation()
{
    ClientPadState state;
    for (std::size_t word = 0; word < state.stateDwords.size(); ++word)
        state.stateDwords[word] = static_cast<std::uint32_t>(0xffffffffu - word);
    state.stateFlag     = 0x5a;
    state.opaque        = { 0x12, 0x34, 0x56 };
    const auto original = state;
    auto       expected = state;
    for (std::size_t word = 4; word <= 7; ++word)
        expected.stateDwords[word] &= ~0x0220u;
    const auto filtered = FilterRetailPadControls(state, kLeftTrigger | 0x10);
    Require(std::memcmp(&filtered, &expected, sizeof(state)) == 0, "unowned native bytes changed");
    Require(std::memcmp(&state, &original, sizeof(state)) == 0, "source state mutated");
    const auto unknown = FilterRetailPadControls(state, 0xf800);
    Require(std::memcmp(&unknown, &original, sizeof(state)) == 0, "unknown logical controls changed bytes");
}

void UnavailableAndForwardedControls()
{
    InputPolicy policy;
    Sample      context{ 0, true, false, false, { 1, 1, 1 } };
    policy.Update(context);
    ClientPadState state;
    state.stateDwords[4]   = 0x0220;
    context.held           = DecodeRetailPadControls(state);
    const auto unavailable = policy.Update(context);
    const auto unchanged   = FilterRetailPadControls(state, unavailable.consume);
    Require(unavailable.edgeCount == 0 && std::memcmp(&unchanged, &state, sizeof(state)) == 0, "unqualified input consumed");
    context.playable = true;
    Require(policy.Update(context).edgeCount == 0, "unavailable hold invoked after rearm");
    context.held = 0;
    policy.Update(context);
    context.held        = DecodeRetailPadControls(state);
    const auto captured = policy.Update(context);
    Require(captured.edgeCount == 1, "fresh qualified press missing");
    context.playable    = false;
    const auto disabled = policy.Update(context);
    Require(disabled.edgeCount == 0 && FilterRetailPadControls(state, disabled.consume).stateDwords[4] == 0, "captured hold leaked on disable");
    Require(!policy.IsCurrent(captured.edges[0], context), "disabled edge accepted");

    InputPolicy forwarded;
    context = { 0, true, true, false, { 1, 1, 1 } };
    forwarded.Update(context);
    context.held = 0x10;
    forwarded.Update(context);
    context.held |= kLeftTrigger;
    const auto preserved = FilterRetailPadControls(state, forwarded.Update(context).consume);
    Require(preserved.stateDwords[4] == 0x20, "already-forwarded face hold captured");
}

void MappedSetChord()
{
    InputPolicy policy;
    Sample      context{ 0, true, true, true, { 1, 1, 1 } };
    policy.Update(context);
    ClientPadState state;
    state.stateDwords[4] = 0x0801;
    state.stateDwords[5] = 0x0801;
    context.held         = DecodeRetailPadControls(state);
    const auto chord     = policy.Update(context);
    const auto filtered  = FilterRetailPadControls(state, chord.consume);
    Require(chord.cycle == -1 && chord.edgeCount == 0 && filtered.stateDwords[4] == 0 && filtered.stateDwords[5] == 0, "mapped RB/Up chord leaked");
}

void OverlaySettings()
{
    Require(ParseLabelStyle("xbox") == LabelStyle::Xbox && ParseLabelStyle("playstation") == LabelStyle::PlayStation, "label names");
    Require(!ParseLabelStyle("PlayStation") && !ParseLabelStyle("ps") && !ParseLabelStyle(""), "only exact names parse");
    Require(LabelStyleName(LabelStyle::Xbox) == "xbox" && LabelStyleName(LabelStyle::PlayStation) == "playstation", "label style names");
    const auto read = [](std::string_view text, LabelStyle labels, bool visible)
    {
        const Settings settings = ReadSettings(text);
        return settings.labels == labels && settings.visible == visible;
    };
    Require(read("", LabelStyle::Xbox, true), "missing file reads as defaults");
    Require(read("labels=playstation\nvisible=false\n", LabelStyle::PlayStation, false), "saved lines");
    Require(read("labels=playstation\r\nvisible=false\r\n", LabelStyle::PlayStation, false), "CRLF lines");
    Require(read("other=1\nvisible=false", LabelStyle::Xbox, false), "last line without a newline");
    Require(read("labels=playstation\nlabels=xbox\nvisible=false\nvisible=true\n", LabelStyle::Xbox, true), "last valid line wins");
    Require(read("labels=playstation\nlabels=ps\nvisible=false\nvisible=no\n", LabelStyle::PlayStation, false), "invalid values ignored");
    Require(read(" labels=playstation\nvisible = false\n", LabelStyle::Xbox, true), "spaced keys ignored");
    for (const Settings settings : { Settings{}, Settings{ LabelStyle::PlayStation, false } })
    {
        const Settings again = ReadSettings(WriteSettings(settings));
        Require(again.labels == settings.labels && again.visible == settings.visible, "written settings read back");
    }
    Require(WriteSettings({ LabelStyle::PlayStation, false }) == "labels=playstation\nvisible=false\n", "written form");
}

} // namespace

int main()
{
    try
    {
        MappedSlotEdges();
        ExactPreservation();
        UnavailableAndForwardedControls();
        MappedSetChord();
        OverlaySettings();
    }
    catch (const std::exception& error)
    {
        std::cerr << error.what() << '\n';
        return 1;
    }
    std::cout << "selected-Xbox mapping, sixteen slot edges, owned cycles, native byte preservation, and overlay settings passed\n";
    return 0;
}
