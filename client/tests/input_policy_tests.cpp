#include "input_policy.h"

#include <array>
#include <iostream>

bool input_hook_routes_release_and_text_focus()
{
    InputReleasePolicy input;
    input.CaptureKey(65);
    if (input.RouteKey(65, InputMessageKind::Character) != InputReleaseRoute::None)
    {
        return false;
    }
    if (input.RouteKey(65, InputMessageKind::VirtualKeyUp) != InputReleaseRoute::ImGui || input.RouteKey(65, InputMessageKind::VirtualKeyUp) != InputReleaseRoute::None)
    {
        return false;
    }

    input.CaptureMouse(2);
    if (input.RouteMouse(2, MouseMessageKind::ButtonUp) != InputReleaseRoute::ImGui || input.RouteMouse(2, MouseMessageKind::ButtonUp) != InputReleaseRoute::None)
    {
        return false;
    }

    input.CaptureKey(66);
    input.ForwardKey(67);
    input.CaptureMouse(0);
    input.ForwardMouse(1);
    const InputLockSnapshot cleared = input.Clear();
    if (!cleared.capturedKeys[66] || !cleared.forwardedKeys[67] || !cleared.capturedMouse[0] || !cleared.forwardedMouse[1] || input.RouteKey(66, InputMessageKind::VirtualKeyUp) != InputReleaseRoute::None || input.RouteMouse(0, MouseMessageKind::ButtonUp) != InputReleaseRoute::None)
    {
        return false;
    }

    ControllerSelection                                  selection;
    std::array<ControllerPresence, kControllerSlotCount> simultaneous{};
    simultaneous[0]                      = { true, 1 };
    simultaneous[kXInputControllerCount] = { true, 1 };
    return selection.Update(simultaneous) == static_cast<int>(kXInputControllerCount);
}

bool client_pad_state_is_published_without_mutation()
{
    ClientPadState state{};
    state.stateDwords[4] = 0x2010;
    state.stateDwords[9] = 41;
    state.stateFlag      = 1;
    state.opaque         = { 2, 3, 4 };
    ClientPadMailbox mailbox;
    mailbox.Publish(state, true);
    const ClientPadSnapshot first = mailbox.Snapshot();
    state.stateDwords[9]          = 42;
    mailbox.Publish(state, true);
    const ClientPadSnapshot metadataOnly = mailbox.Snapshot();
    state.stateDwords[4]                 = 0;
    mailbox.Publish(state, true);
    const ClientPadSnapshot released = mailbox.Snapshot();
    if (!first.connected || first.packetNumber == 0 || metadataOnly.packetNumber != first.packetNumber || released.packetNumber == metadataOnly.packetNumber)
    {
        return false;
    }
    mailbox.Publish({}, false);
    const ClientPadSnapshot disconnected = mailbox.Snapshot();
    mailbox.Publish(state, true);
    const ClientPadSnapshot reconnected = mailbox.Snapshot();
    if (disconnected.connected || disconnected.packetNumber == released.packetNumber || !reconnected.connected || reconnected.packetNumber == disconnected.packetNumber)
    {
        return false;
    }

    return first.state.stateDwords[4] == 0x2010 && first.state.stateDwords[9] == 41 && first.state.stateFlag == 1 && first.state.opaque == std::array<std::uint8_t, 3>{ 2, 3, 4 };
}

int main()
{
    ControllerSelection                                  selection;
    std::array<ControllerPresence, kControllerSlotCount> controllers{};
    if (selection.Update(controllers) != -1)
        return 1;

    controllers[2] = { true, 1 };
    if (selection.Update(controllers) != 2)
        return 2;

    controllers[0] = { true, 7 };
    if (selection.Update(controllers) != 0)
        return 3;

    controllers[0].connected = false;
    if (selection.Update(controllers) != 2)
        return 4;

    controllers[2].connected = false;
    if (selection.Update(controllers) != -1)
        return 5;

    controllers[1] = { true, 1 };
    if (selection.Update(controllers) != 1 || selection.Active() != 1)
        return 6;

    if (!input_hook_routes_release_and_text_focus())
        return 7;
    if (!client_pad_state_is_published_without_mutation())
        return 8;

    std::cout << "input policy tests passed: controller selection, "
                 "input_hook_routes_release_and_text_focus, "
                 "client_pad_state_is_published_without_mutation\n";
    return 0;
}
