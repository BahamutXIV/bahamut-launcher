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

// Per-socket classification shared by both directions of a socket.
enum class StreamKind : std::uint8_t
{
    Unclassified,
    Game,
    Other,
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

struct Subpacket
{
    std::uint16_t type   = 0;
    std::uint16_t opcode = 0; // Set only for type 0x0003.
};

struct FrameInfo
{
    Direction                     direction = Direction::Incoming;
    StreamKind                    stream    = StreamKind::Unclassified;
    bool                          parsed    = false; // Inflate and subpacket parse succeeded.
    std::span<const std::uint8_t> bytes;             // Original frame bytes.
    std::span<const Subpacket>    subpackets;        // Empty unless parsed.
};

class PacketObserver
{
public:
    using Sink      = std::function<void(const GameMessage&)>;
    using FrameSink = std::function<void(const FrameInfo&)>;

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
