#pragma once

#include "packet_observer.h"

#include <array>
#include <cstdint>
#include <string>
#include <string_view>

namespace packet_capture
{

enum class Scope : std::uint8_t
{
    Standard,
    Full,
};

struct FrameDecision
{
    bool        write = false;
    std::string stub; // Empty means a full hex row.
};

// Game message opcodes written as stub rows (opcode only, no payload).
// Opcode integers are not globally unique, so each direction has its own list.
// XIVLegacy/xivl-opcodes:opcodes.json, outgoing entries:
//   MapServerbound 0x0003 ChatMessagePacket,
//   WorldServerbound 0x00C8 TellChatMessagePacket,
//   WorldServerbound 0x00C9 SelectedGroupChatMessagePacket,
//   MapServerbound 0x01D5 GMSupportTicketPacket (mail-compose text block),
//   MapServerbound 0x01DC _0x01DC (player-search profile update text).
inline constexpr std::array<std::uint16_t, 5> kOutgoingStubOpcodes = {
    0x0003u,
    0x00C8u,
    0x00C9u,
    0x01D5u,
    0x01DCu,
};

// XIVLegacy/xivl-opcodes:opcodes.json, incoming entries:
//   MapClientbound and WorldClientbound 0x0003 SendMessagePacket,
//   WorldClientbound 0x00C8 TellChatMessagePacket,
//   WorldClientbound 0x00C9 SelectedGroupChatMessagePacket,
//   MapClientbound 0x01E0 _0x01E0 (search results with comment text).
inline constexpr std::array<std::uint16_t, 4> kIncomingStubOpcodes = {
    0x0003u,
    0x00C8u,
    0x00C9u,
    0x01E0u,
};

namespace detail
{

inline constexpr std::uint16_t kGameMessageType = 0x0003u;

inline bool IsStubOpcode(packet_observer::Direction direction, std::uint16_t opcode) noexcept
{
    if (direction == packet_observer::Direction::Outgoing)
    {
        for (const std::uint16_t denied : kOutgoingStubOpcodes)
        {
            if (denied == opcode)
            {
                return true;
            }
        }
        return false;
    }
    for (const std::uint16_t denied : kIncomingStubOpcodes)
    {
        if (denied == opcode)
        {
            return true;
        }
    }
    return false;
}

inline void AppendStubItem(std::string& out, const char* prefix, std::uint16_t value)
{
    static constexpr char kDigits[] = "0123456789ABCDEF";
    if (!out.empty())
    {
        out.push_back(';');
    }
    out.append(prefix);
    for (int shift = 12; shift >= 0; shift -= 4)
    {
        out.push_back(kDigits[(value >> shift) & 0xFu]);
    }
}

} // namespace detail

inline Scope ParseScope(std::string_view value) noexcept
{
    return value == "full" ? Scope::Full : Scope::Standard;
}

// Standard scope writes only parsed game-world frames and replaces any frame that
// carries a non-game-message subpacket or a stub opcode with an item-only stub.
inline FrameDecision Classify(Scope scope, const packet_observer::FrameInfo& frame)
{
    FrameDecision decision;
    if (scope == Scope::Full)
    {
        decision.write = true;
        return decision;
    }
    if (frame.stream != packet_observer::StreamKind::Game || !frame.parsed)
    {
        return decision;
    }
    decision.write = true;

    bool stubbed = false;
    for (const packet_observer::Subpacket& subpacket : frame.subpackets)
    {
        if (subpacket.type != detail::kGameMessageType ||
            detail::IsStubOpcode(frame.direction, subpacket.opcode))
        {
            stubbed = true;
            break;
        }
    }
    if (!stubbed)
    {
        return decision;
    }

    decision.stub = "stub:";
    std::string items;
    for (const packet_observer::Subpacket& subpacket : frame.subpackets)
    {
        if (subpacket.type == detail::kGameMessageType)
        {
            detail::AppendStubItem(items, "op=", subpacket.opcode);
        }
        else
        {
            detail::AppendStubItem(items, "t=", subpacket.type);
        }
    }
    decision.stub += items;
    return decision;
}

} // namespace packet_capture
