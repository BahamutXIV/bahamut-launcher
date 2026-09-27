#pragma once

#include <array>
#include <cstdint>
#include <mutex>

constexpr std::size_t kXInputControllerCount    = 4;
constexpr std::size_t kControllerSlotCount      = kXInputControllerCount + 1;
constexpr std::size_t kClientPadStateDwordCount = 10;

struct ControllerPresence
{
    bool          connected    = false;
    std::uint32_t packetNumber = 0;
};

class ControllerSelection
{
public:
    int Update(const std::array<ControllerPresence, kControllerSlotCount>& controllers);
    int Active() const;

private:
    std::array<std::uint32_t, kControllerSlotCount> packetNumbers_{};
    std::array<bool, kControllerSlotCount>          connected_{};
    int                                             active_ = -1;
};

struct ClientPadState
{
    std::array<std::uint32_t, kClientPadStateDwordCount> stateDwords{};
    std::uint8_t                                         stateFlag = 0;
    std::array<std::uint8_t, 3>                          opaque{};
};

static_assert(sizeof(ClientPadState) == 0x2c);

struct ClientPadSnapshot
{
    ClientPadState state{};
    bool           connected    = false;
    std::uint32_t  packetNumber = 0;
};

class ClientPadMailbox
{
public:
    void              Publish(const ClientPadState& state, bool connected);
    ClientPadSnapshot Snapshot() const;

private:
    mutable std::mutex mutex_;
    ClientPadSnapshot  snapshot_{};
};

enum class InputMessageKind
{
    VirtualKeyDown,
    VirtualKeyUp,
    Character,
};

enum class InputReleaseRoute
{
    None,
    ImGui,
    Forward,
};

enum class MouseMessageKind
{
    ButtonDown,
    ButtonUp,
};

struct InputLockSnapshot
{
    std::array<bool, 256> capturedKeys{};
    std::array<bool, 256> forwardedKeys{};
    std::array<bool, 5>   capturedMouse{};
    std::array<bool, 5>   forwardedMouse{};
};

class InputReleasePolicy
{
public:
    InputReleaseRoute RouteKey(std::size_t key, InputMessageKind message);
    InputReleaseRoute RouteMouse(std::size_t button, MouseMessageKind message);
    void              CaptureKey(std::size_t key);
    void              ForwardKey(std::size_t key);
    void              CaptureMouse(std::size_t button);
    void              ForwardMouse(std::size_t button);
    InputLockSnapshot Clear();

private:
    std::array<bool, 256> capturedKeys_{};
    std::array<bool, 256> forwardedKeys_{};
    std::array<bool, 5>   capturedMouse_{};
    std::array<bool, 5>   forwardedMouse_{};
};
