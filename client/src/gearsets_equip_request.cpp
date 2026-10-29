#include "gearsets_equip_request.h"

#include <cstring>

namespace
{

constexpr std::size_t kNameOffset    = 0x11u;
constexpr std::size_t kNameSize      = 0x20u;
constexpr std::size_t kTailOffset    = 0x31u;
constexpr std::size_t kTailSize      = 0x80u;
constexpr char        kCommandName[] = "commandRequest";

void PutLittle32(std::uint8_t* out, std::uint32_t value)
{
    for (int index = 0; index < 4; ++index)
    {
        out[index] = static_cast<std::uint8_t>(value >> (8 * index));
    }
}

class TailWriter
{
public:
    explicit TailWriter(std::uint8_t* out)
    : out_(out)
    {
    }

    void Byte(std::uint8_t value)
    {
        out_[size_++] = value;
    }

    void Big(std::uint64_t value, int bytes)
    {
        for (int index = bytes - 1; index >= 0; --index)
        {
            Byte(static_cast<std::uint8_t>(value >> (8 * index)));
        }
    }

private:
    std::uint8_t* out_;
    std::size_t   size_ = 0;
};

} // namespace

namespace bahamut_client
{

std::uint32_t GearsetsCrc32(std::span<const std::uint8_t> bytes)
{
    std::uint32_t crc = 0xFFFFFFFFu;
    for (const std::uint8_t byte : bytes)
    {
        crc ^= byte;
        for (int bit = 0; bit < 8; ++bit)
        {
            crc = (crc >> 1) ^ (0xEDB88320u & (0u - (crc & 1u)));
        }
    }
    return ~crc;
}

std::array<std::uint8_t, kGearsetsCommandRequestSize> EncodeGearsetsEquipCommandRequest(const GearsetsEquipCommandRequest& request)
{
    std::array<std::uint8_t, kGearsetsCommandRequestSize> payload{};
    std::uint8_t* const                                   tail = payload.data() + kTailOffset;

    TailWriter writer(tail);
    if (request.unequip)
    {
        writer.Byte(0x05u); // nil item
    }
    else
    {
        writer.Byte(0x07u); // item: owner actor, bag slot, package 0
        writer.Big(request.actor, 4);
        writer.Big(request.bagSlot, 2);
        writer.Byte(0x00u);
    }
    writer.Byte(0x05u); // nil
    // Bahamut's client sends category 1 (charaWork.commandCategory); retail
    // sends 0, and Bahamut's EquipCommand.lua ignores it.
    writer.Byte(0x00u);
    writer.Big(1u, 4);
    for (int index = 0; index < 5; ++index)
    {
        writer.Byte(0x05u);
    }
    writer.Byte(0x00u);
    writer.Big(request.equipSlot, 4);
    if (!request.unequip)
    {
        writer.Byte(0x09u); // itemDBIds: item unique ID, then 0
        writer.Big(request.uid, 8);
        writer.Big(0u, 8);
    }
    writer.Byte(0x0Fu);

    PutLittle32(payload.data() + 0x00u, request.actor);
    PutLittle32(payload.data() + 0x04u, request.owner);
    PutLittle32(payload.data() + 0x08u, request.stateWord);
    PutLittle32(payload.data() + 0x0Cu, GearsetsCrc32(std::span<const std::uint8_t>(tail, kTailSize)));
    payload[0x10u] = request.eventType;
    static_assert(sizeof(kCommandName) <= kNameSize);
    std::memcpy(payload.data() + kNameOffset, kCommandName, sizeof(kCommandName) - 1u);
    return payload;
}

} // namespace bahamut_client
