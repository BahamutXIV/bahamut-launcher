#pragma once

#include <cstdint>
#include <functional>
#include <memory>
#include <span>
#include <vector>

namespace packet_observer
{

enum class Direction : std::uint8_t
{
    Incoming,
    Outgoing,
};

struct GameMessage
{
    Direction     direction = Direction::Incoming;
    std::uint16_t opcode    = 0;
    std::uint32_t sourceId  = 0;
    std::uint32_t targetId  = 0;
    // Raw GameMessageHeader timestamp: uint32 Unix-epoch seconds on retail.
    std::uint32_t             timestamp = 0;
    std::vector<std::uint8_t> payload;
};

class PacketObserver
{
public:
    using Sink      = std::function<void(const GameMessage&)>;
    using FrameSink = std::function<void(Direction, std::span<const std::uint8_t>)>;

    PacketObserver();
    ~PacketObserver();

    PacketObserver(const PacketObserver&)            = delete;
    PacketObserver& operator=(const PacketObserver&) = delete;

    void SetSink(Sink sink);
    void SetFrameSink(FrameSink sink);

    // MinHook is initialized by the render boundary before installation.
    // These hooks inspect synchronous Winsock completions only.
    bool InstallHook();
    bool Shutdown();

private:
    friend struct ObserverImplementation;

    struct Impl;
    std::unique_ptr<Impl> impl_;
};

} // namespace packet_observer
