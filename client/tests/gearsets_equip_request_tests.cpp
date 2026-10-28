#include "gearsets_equip_request.h"

#include <cstdio>
#include <iostream>
#include <string>

namespace
{

using namespace bahamut_client;

std::string Hex(std::span<const std::uint8_t> bytes)
{
    std::string text;
    char        digits[3]{};
    for (const std::uint8_t byte : bytes)
    {
        std::snprintf(digits, sizeof(digits), "%02x", static_cast<unsigned int>(byte));
        text += digits;
    }
    return text;
}

bool Expect(bool condition, const char* message)
{
    if (!condition)
    {
        std::cerr << message << '\n';
    }
    return condition;
}

} // namespace

int main()
{
    bool ok = Expect(GearsetsCrc32(std::span<const std::uint8_t>(reinterpret_cast<const std::uint8_t*>("123456789"), 9u)) == 0xCBF43926u, "CRC32 matches the standard check value");

    // A captured Bahamut request (client -> server 0x012D at 91,947 ms):
    // actor 0x12 equips unique ID 0x1200000002 from bag slot 1 into slot 11.
    GearsetsEquipCommandRequest request;
    request.actor      = 0x12u;
    request.owner      = 0xA0F02EE9u;
    request.stateWord  = 0x30400000u;
    request.eventType  = 0u;
    request.bagSlot    = 1u;
    request.equipSlot  = 11u;
    request.uid        = 0x1200000002ull;
    const auto payload = EncodeGearsetsEquipCommandRequest(request);

    const std::string expected = std::string("12000000e92ef0a000004030a3a21f4d00") +
                                 "636f6d6d616e6452657175657374" + std::string(36u, '0') +
                                 "07000000120001000500000000010505050505000000000b09000000120000000200000000000000000f" + std::string(172u, '0') +
                                 std::string(14u, '0');
    ok                         = Expect(expected.size() == kGearsetsCommandRequestSize * 2u, "the expected record is 184 bytes") && ok;
    ok                         = Expect(Hex(payload) == expected, "the encoded request matches the captured request byte for byte") && ok;

    request.actor   = 0x48u;
    request.uid     = 0x4800000002ull;
    const auto next = EncodeGearsetsEquipCommandRequest(request);
    ok              = Expect(Hex(std::span<const std::uint8_t>(next.data() + 0x31u, 6u)) == "070000004800", "the item reference carries the actor") && ok;
    ok              = Expect(Hex(std::span<const std::uint8_t>(next.data() + 0x49u, 9u)) == "090000004800000002", "the unique ID is big-endian") && ok;

    // The same session's unequip request (87,515 ms): actor 0x12 unequips slot 11.
    GearsetsEquipCommandRequest unequip;
    unequip.actor             = 0x12u;
    unequip.owner             = 0xA0F02EE9u;
    unequip.stateWord         = 0x30400000u;
    unequip.equipSlot         = 11u;
    unequip.unequip           = true;
    unequip.bagSlot           = 7u;
    unequip.uid               = 0x1200000002ull;
    const auto        removed = EncodeGearsetsEquipCommandRequest(unequip);
    const std::string r1      = std::string("12000000e92ef0a000004030d0b1cbe400") +
                                "636f6d6d616e6452657175657374" + std::string(36u, '0') +
                                "050500000000010505050505000000000b0f" + std::string(220u, '0') +
                                std::string(14u, '0');
    ok                        = Expect(r1.size() == kGearsetsCommandRequestSize * 2u, "the expected unequip record is 184 bytes") && ok;
    ok                        = Expect(Hex(removed) == r1, "the encoded unequip matches the captured unequip byte for byte, ignoring bag slot and uid") && ok;

    return ok ? 0 : 1;
}
