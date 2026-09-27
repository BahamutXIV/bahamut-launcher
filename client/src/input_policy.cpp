#include "input_policy.h"

int ControllerSelection::Update(
    const std::array<ControllerPresence, kControllerSlotCount>& controllers)
{
    int changedController = -1;
    for (std::size_t index = 0; index < controllers.size(); ++index)
    {
        const bool changed = controllers[index].connected && (!connected_[index] || controllers[index].packetNumber != packetNumbers_[index]);
        if (changed)
        {
            changedController = static_cast<int>(index);
        }
        connected_[index]     = controllers[index].connected;
        packetNumbers_[index] = controllers[index].packetNumber;
    }

    if (changedController != -1)
    {
        active_ = changedController;
    }
    else if (active_ < 0 || !controllers[static_cast<std::size_t>(active_)].connected)
    {
        active_ = -1;
        for (std::size_t index = 0; index < controllers.size(); ++index)
        {
            if (controllers[index].connected)
            {
                active_ = static_cast<int>(index);
                break;
            }
        }
    }
    return active_;
}

int ControllerSelection::Active() const
{
    return active_;
}

void ClientPadMailbox::Publish(const ClientPadState& state, bool connected)
{
    std::lock_guard lock(mutex_);
    const bool      controlsChanged = snapshot_.state.stateDwords[0] != state.stateDwords[0] || snapshot_.state.stateDwords[1] != state.stateDwords[1] || snapshot_.state.stateDwords[2] != state.stateDwords[2] || snapshot_.state.stateDwords[3] != state.stateDwords[3] || snapshot_.state.stateDwords[4] != state.stateDwords[4];
    if (snapshot_.connected != connected || (connected && controlsChanged))
    {
        ++snapshot_.packetNumber;
    }
    snapshot_.state     = state;
    snapshot_.connected = connected;
}

ClientPadSnapshot ClientPadMailbox::Snapshot() const
{
    std::lock_guard lock(mutex_);
    return snapshot_;
}

InputReleaseRoute InputReleasePolicy::RouteKey(
    std::size_t key, InputMessageKind message)
{
    if (message == InputMessageKind::Character || key >= capturedKeys_.size())
    {
        return InputReleaseRoute::None;
    }
    if (capturedKeys_[key])
    {
        if (message == InputMessageKind::VirtualKeyUp)
        {
            capturedKeys_[key] = false;
        }
        return InputReleaseRoute::ImGui;
    }
    if (forwardedKeys_[key])
    {
        if (message == InputMessageKind::VirtualKeyUp)
        {
            forwardedKeys_[key] = false;
        }
        return InputReleaseRoute::Forward;
    }
    return InputReleaseRoute::None;
}

InputReleaseRoute InputReleasePolicy::RouteMouse(
    std::size_t button, MouseMessageKind message)
{
    if (button >= capturedMouse_.size())
    {
        return InputReleaseRoute::None;
    }
    if (capturedMouse_[button])
    {
        if (message == MouseMessageKind::ButtonUp)
        {
            capturedMouse_[button] = false;
        }
        return InputReleaseRoute::ImGui;
    }
    if (forwardedMouse_[button])
    {
        if (message == MouseMessageKind::ButtonUp)
        {
            forwardedMouse_[button] = false;
        }
        return InputReleaseRoute::Forward;
    }
    return InputReleaseRoute::None;
}

void InputReleasePolicy::CaptureKey(std::size_t key)
{
    if (key < capturedKeys_.size())
    {
        capturedKeys_[key]  = true;
        forwardedKeys_[key] = false;
    }
}

void InputReleasePolicy::ForwardKey(std::size_t key)
{
    if (key < forwardedKeys_.size())
    {
        forwardedKeys_[key] = true;
        capturedKeys_[key]  = false;
    }
}

void InputReleasePolicy::CaptureMouse(std::size_t button)
{
    if (button < capturedMouse_.size())
    {
        capturedMouse_[button]  = true;
        forwardedMouse_[button] = false;
    }
}

void InputReleasePolicy::ForwardMouse(std::size_t button)
{
    if (button < forwardedMouse_.size())
    {
        forwardedMouse_[button] = true;
        capturedMouse_[button]  = false;
    }
}

InputLockSnapshot InputReleasePolicy::Clear()
{
    InputLockSnapshot snapshot{
        capturedKeys_, forwardedKeys_, capturedMouse_, forwardedMouse_
    };
    capturedKeys_   = {};
    forwardedKeys_  = {};
    capturedMouse_  = {};
    forwardedMouse_ = {};
    return snapshot;
}
