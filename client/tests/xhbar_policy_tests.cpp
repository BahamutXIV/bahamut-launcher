#include "xhbar_policy.h"

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

struct Driver
{
    InputPolicy policy;
    Sample      context{ 0, true, true, true, { 1, 1, 1 } };

    Driver()
    {
        policy.Update(context);
    }

    Routing Hold(std::uint16_t held)
    {
        context.held = held;
        return policy.Update(context);
    }
};

void AllSlotsAndNoRepeat()
{
    for (const auto trigger : { kLeftTrigger, kRightTrigger })
    {
        for (std::size_t slot = 0; slot < 8; ++slot)
        {
            Driver d;
            d.Hold(trigger);
            const auto button = static_cast<std::uint16_t>(1u << slot);
            const auto press  = d.Hold(trigger | button | 0x8000);
            Require(press.edgeCount == 1 && press.edges[0].slot == slot + (trigger == kRightTrigger ? 8u : 0u), "slot routing");
            Require(press.consume == (trigger | button), "unrelated control consumed");
            Require(d.Hold(trigger | button).edgeCount == 0, "held repeat");
            Require(d.Hold(trigger).consume == (trigger | button), "captured release escaped");
            Require(d.Hold(trigger | button).edgeCount == 1, "fresh press lost");
        }
    }
}

void TriggerOwnershipAndForwardedHolds()
{
    Driver d;
    d.Hold(4); // Pre-trigger holds remain native until release.
    auto result = d.Hold(kLeftTrigger | 4);
    Require(result.edgeCount == 0 && !(result.consume & 4), "forwarded hold stolen");
    Require(!(d.Hold(kLeftTrigger).consume & 4), "forwarded release stolen");
    Require(d.Hold(kLeftTrigger | 4).edgeCount == 1, "released button did not rearm");
    result = d.Hold(kLeftTrigger | kRightTrigger | 4);
    Require(result.side == Side::LeftThenRight && result.edgeCount == 0 && (result.consume & 4), "combo replayed or leaked hold");
    result = d.Hold(kRightTrigger | 4);
    Require(result.side == Side::Right && result.edgeCount == 0, "side change replayed hold");
    d.Hold(kRightTrigger);
    Require(d.Hold(kRightTrigger | 4).edges[0].slot == 10, "new side not selected");

    Driver simultaneous;
    result = simultaneous.Hold(kLeftTrigger | kRightTrigger | 4);
    Require(result.side == Side::RightThenLeft && result.edgeCount == 1 && result.edges[0].slot == 18, "same-frame triggers not RT then LT");
    Require(simultaneous.Hold(kLeftTrigger | 4).edgeCount == 0, "combo release replayed hold");
    simultaneous.Hold(kLeftTrigger);
    Require(simultaneous.Hold(kLeftTrigger | 4).edgeCount == 1, "combo release did not rearm");
}

void TriggerCombos()
{
    constexpr std::uint16_t kY = 0x80;
    Driver                  d;
    d.Hold(kLeftTrigger);
    Require(d.Hold(kLeftTrigger | kRightTrigger).side == Side::LeftThenRight, "LT then RT not selected");
    auto result = d.Hold(kLeftTrigger | kRightTrigger | kUp);
    Require(result.edgeCount == 1 && result.edges[0].slot == 24, "LT then RT Up off its slot");
    result = d.Hold(kLeftTrigger | kRightTrigger | kUp | kY);
    Require(result.edgeCount == 1 && result.edges[0].slot == 31, "LT then RT Y off its slot");
    result = d.Hold(kLeftTrigger | kUp | kY);
    Require(result.side == Side::Left && result.edgeCount == 0 && (result.consume & (kUp | kY)) == (kUp | kY), "combo release replayed or leaked");
    Require((d.Hold(kLeftTrigger).consume & (kUp | kY)) == (kUp | kY), "combo press release escaped");
    Require(d.Hold(kLeftTrigger | kUp).edges[0].slot == 0, "first trigger bar not restored");

    Driver r;
    r.Hold(kRightTrigger);
    Require(r.Hold(kRightTrigger | kLeftTrigger).side == Side::RightThenLeft, "RT then LT not selected");
    result = r.Hold(kRightTrigger | kLeftTrigger | kY);
    Require(result.edgeCount == 1 && result.edges[0].slot == 23, "RT then LT Y off its slot");
    result = r.Hold(kLeftTrigger | kY);
    Require(result.side == Side::Left && result.edgeCount == 0 && (result.consume & kY), "first trigger release replayed or leaked");
    Require(r.Hold(kLeftTrigger | kRightTrigger | kY).side == Side::LeftThenRight, "held trigger did not become first");
}

void ContextAndGenerations()
{
    for (int transition = 0; transition < 7; ++transition)
    {
        Driver d;
        d.Hold(kLeftTrigger);
        const auto edge = d.Hold(kLeftTrigger | 4).edges[0];
        if (transition == 0)
            ++d.context.generation.session;
        if (transition == 1)
            ++d.context.generation.layout;
        if (transition == 2)
            ++d.context.generation.actions;
        if (transition >= 3 && transition <= 5)
            d.context.playable = false; // focus, editor/menu, disable/fault
        if (transition == 6)
            d.context.connected = false;
        auto result = d.Hold(kLeftTrigger | 4);
        Require(result.edgeCount == 0 && !d.policy.IsCurrent(edge, d.context), "stale intent accepted");
        d.context.playable  = true;
        d.context.connected = true;
        result              = d.Hold(kLeftTrigger | 4);
        Require(!d.policy.IsCurrent(edge, d.context), "stale intent revived after resume");
        Require(result.edgeCount == 0 && (result.consume & 4), "captured hold leaked on resume");
        const auto fresh = d.Hold(kLeftTrigger | 8);
        Require(fresh.edgeCount == (transition == 1 || transition == 2 ? 1u : 0u), "layout/context trigger rearm mismatch");
        d.Hold(0);
        Require(d.Hold(kLeftTrigger | 4).edgeCount == 1, "neutral did not rearm");
    }
    Sample      held{ static_cast<std::uint16_t>(kLeftTrigger | 4), true, true, true, { 7, 1, 1 } };
    InputPolicy policy;
    Require(policy.Update(held).edgeCount == 0 && policy.Update(held).consume == 0, "initial native hold captured");

    for (const bool changeLayout : { false, true })
    {
        Driver changed;
        changed.Hold(kLeftTrigger);
        if (changeLayout)
            ++changed.context.generation.layout;
        else
            ++changed.context.generation.actions;
        auto result = changed.Hold(kLeftTrigger | 4);
        Require(result.edgeCount == 0 && (result.consume & 4), "generation transition leaked fresh slot");
        Require(changed.Hold(kLeftTrigger).consume & 4, "transition slot release escaped");
        Require(changed.Hold(kLeftTrigger | 4).edgeCount == 1, "transition slot failed to rearm");
        ++changed.context.generation.layout;
        Require(changed.Hold(0).side == Side::None, "generation transition kept released side");
    }
    Driver chordChanged;
    Require(chordChanged.Hold(kRightShoulder | kUp).cycle == -1, "initial chord lost");
    ++chordChanged.context.generation.layout;
    const auto chord = chordChanged.Hold(kRightShoulder | kDown);
    Require(chord.cycle == 0 && (chord.consume & kDown), "generation transition leaked chord direction");
    Require(chordChanged.Hold(kRightShoulder).consume & kDown, "transition chord release escaped");
    Require(chordChanged.Hold(kRightShoulder | kDown).cycle == 1, "transition chord failed to rearm");
}

void SetChordPriority()
{
    Driver d;
    d.Hold(kLeftTrigger);
    auto result = d.Hold(kLeftTrigger | kRightShoulder | kDown | 4);
    Require(result.cycle == 1 && result.edgeCount == 0, "set chord also invoked slot");
    Require(d.Hold(kLeftTrigger | kRightShoulder | kDown | 4).cycle == 0, "set chord repeated");
    result = d.Hold(kLeftTrigger | 4);
    Require(result.edgeCount == 0 && (result.consume & (kRightShoulder | kDown)), "chord release escaped or slot replayed");
    d.Hold(0);
    result = d.Hold(kRightShoulder | kUp | kDown);
    Require(result.cycle == 0 && result.edgeCount == 0, "ambiguous set direction");

    Driver unavailable;
    unavailable.context.selectionChordAvailable = false;
    result                                      = unavailable.Hold(kRightShoulder | kDown);
    Require(result.consume == 0 && result.cycle == 0, "unproved shoulder reserved");
    Driver forwarded;
    forwarded.Hold(kUp);
    result = forwarded.Hold(kRightShoulder | kUp);
    Require(result.cycle == 0 && !(result.consume & kUp), "forwarded direction stolen");

    Driver shoulderPrefix;
    Require(shoulderPrefix.Hold(kRightShoulder).consume == 0, "ordinary shoulder stolen");
    result = shoulderPrefix.Hold(kRightShoulder | kUp);
    Require(result.cycle == 0 && result.consume == 0, "forwarded shoulder prefix reused");
    Require(shoulderPrefix.Hold(0).consume == 0, "forwarded shoulder release stolen");

    Driver staggered;
    Require(staggered.Hold(kRightShoulder | kUp).cycle == -1, "fresh chord lost");
    result = staggered.Hold(kRightShoulder | kUp | kDown);
    Require(result.cycle == 0 && (result.consume & kDown), "staggered directions cycled ambiguously");
}

void ShortcutTargets()
{
    constexpr std::array<ShortcutTarget, kSlotEdges> expected = { {
        { 2, 3 },
        { 2, 5 },
        { 2, 6 },
        { 2, 4 },
        { 2, 1 },
        { 1, 10 },
        { 2, 2 },
        { 1, 9 },
        { 1, 5 },
        { 1, 7 },
        { 1, 8 },
        { 1, 6 },
        { 1, 3 },
        { 1, 2 },
        { 1, 4 },
        { 1, 1 },
        { 3, 1 },
        { 3, 3 },
        { 3, 4 },
        { 3, 2 },
        { 2, 9 },
        { 2, 8 },
        { 2, 10 },
        { 2, 7 },
        { 3, 9 },
        { 0, 0 },
        { 0, 0 },
        { 3, 10 },
        { 3, 7 },
        { 3, 6 },
        { 3, 8 },
        { 3, 5 },
    } };
    for (std::size_t slot = 0; slot < expected.size(); ++slot)
    {
        const ShortcutTarget target = Shortcut(slot);
        Require(target.page == expected[slot].page && target.button == expected[slot].button, "slot off its clockwise position");
    }
    Require(Shortcut(kSlotEdges).page == 0 && Shortcut(kSlotEdges).button == 0, "slot past the last set dispatched");
}

} // namespace

int main()
{
    try
    {
        AllSlotsAndNoRepeat();
        TriggerOwnershipAndForwardedHolds();
        TriggerCombos();
        ContextAndGenerations();
        SetChordPriority();
        ShortcutTargets();
    }
    catch (const std::exception& error)
    {
        std::cerr << error.what() << '\n';
        return 1;
    }
    std::cout << "xhbar logical input policy passed; no retail adapter exercised\n";
    return 0;
}
