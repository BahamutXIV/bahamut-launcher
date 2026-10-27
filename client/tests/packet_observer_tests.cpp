#include "packet_capture_policy.h"
#include "packet_observer.h"

#include <MinHook.h>
#include <miniz.h>
#include <winsock2.h>
#include <ws2tcpip.h>

#include <algorithm>
#include <array>
#include <cstdint>
#include <iostream>
#include <string>
#include <utility>
#include <vector>

namespace
{

using packet_observer::Direction;
using packet_observer::StreamKind;

void WriteU16(std::array<std::uint8_t, 51>& bytes, std::size_t offset, std::uint16_t value)
{
    bytes[offset]     = static_cast<std::uint8_t>(value);
    bytes[offset + 1] = static_cast<std::uint8_t>(value >> 8u);
}

void WriteU32(std::array<std::uint8_t, 51>& bytes, std::size_t offset, std::uint32_t value)
{
    for (std::size_t index = 0; index < 4; ++index)
    {
        bytes[offset + index] = static_cast<std::uint8_t>(value >> (index * 8u));
    }
}

int gFailures = 0;

void Expect(bool condition, const char* label)
{
    if (!condition)
    {
        std::cerr << "FAILED: " << label << '\n';
        ++gFailures;
    }
}

struct SubpacketSpec
{
    std::uint16_t type    = 0;
    std::uint16_t opcode  = 0; // Used only for type 3.
    std::size_t   payload = 0;
};

void PutU16(std::vector<std::uint8_t>& bytes, std::size_t offset, std::uint16_t value)
{
    bytes[offset]     = static_cast<std::uint8_t>(value);
    bytes[offset + 1] = static_cast<std::uint8_t>(value >> 8u);
}

std::vector<std::uint8_t> BuildFrame(const std::vector<SubpacketSpec>& specs, bool compressed = false)
{
    std::vector<std::uint8_t> body;
    for (const SubpacketSpec& spec : specs)
    {
        const std::size_t size   = (spec.type == 3 ? 32u : 16u) + spec.payload;
        const std::size_t offset = body.size();
        body.resize(offset + size, 0);
        PutU16(body, offset, static_cast<std::uint16_t>(size));
        PutU16(body, offset + 2, spec.type);
        if (spec.type == 3)
        {
            PutU16(body, offset + 18, spec.opcode);
        }
    }
    if (compressed)
    {
        std::vector<std::uint8_t> packed(body.size() + 128u);
        mz_ulong                  packedSize = static_cast<mz_ulong>(packed.size());
        if (mz_compress(packed.data(), &packedSize, body.data(), static_cast<mz_ulong>(body.size())) != MZ_OK)
        {
            return {};
        }
        packed.resize(packedSize);
        body = std::move(packed);
    }
    std::vector<std::uint8_t> frame(16u + body.size(), 0);
    frame[1] = compressed ? 1u : 0u;
    PutU16(frame, 4, static_cast<std::uint16_t>(frame.size()));
    PutU16(frame, 6, static_cast<std::uint16_t>(specs.size()));
    std::copy(body.begin(), body.end(), frame.begin() + 16);
    return frame;
}

struct Record
{
    Direction                               direction = Direction::Incoming;
    StreamKind                              stream    = StreamKind::Unclassified;
    bool                                    parsed    = false;
    std::vector<std::uint8_t>               bytes;
    std::vector<packet_observer::Subpacket> subpackets;
};

bool SubpacketsAre(const Record& record, const std::vector<std::pair<std::uint16_t, std::uint16_t>>& expected)
{
    if (record.subpackets.size() != expected.size())
    {
        return false;
    }
    for (std::size_t index = 0; index < expected.size(); ++index)
    {
        if (record.subpackets[index].type != expected[index].first ||
            record.subpackets[index].opcode != expected[index].second)
        {
            return false;
        }
    }
    return true;
}

bool SendAll(SOCKET socket, const std::vector<std::uint8_t>& bytes)
{
    return send(socket, reinterpret_cast<const char*>(bytes.data()), static_cast<int>(bytes.size()), 0) ==
           static_cast<int>(bytes.size());
}

bool ReceiveAll(SOCKET socket, std::size_t length)
{
    std::vector<char> buffer(length);
    return recv(socket, buffer.data(), static_cast<int>(length), MSG_WAITALL) == static_cast<int>(length);
}

struct SocketPair
{
    SOCKET a = INVALID_SOCKET;
    SOCKET b = INVALID_SOCKET;
};

SocketPair MakePair(SOCKET listener, const sockaddr_in& address)
{
    SocketPair pair;
    pair.a = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (pair.a == INVALID_SOCKET ||
        connect(pair.a, reinterpret_cast<const sockaddr*>(&address), sizeof(address)) != 0)
    {
        return {};
    }
    pair.b              = accept(listener, nullptr, nullptr);
    const DWORD timeout = 2000;
    setsockopt(pair.a, SOL_SOCKET, SO_RCVTIMEO, reinterpret_cast<const char*>(&timeout), sizeof(timeout));
    setsockopt(pair.b, SOL_SOCKET, SO_RCVTIMEO, reinterpret_cast<const char*>(&timeout), sizeof(timeout));
    return pair;
}

void ClosePair(SocketPair& pair)
{
    closesocket(pair.a);
    closesocket(pair.b);
}

packet_observer::FrameInfo MakeInfo(Direction                                      direction,
                                    StreamKind                                     stream,
                                    bool                                           parsed,
                                    const std::vector<packet_observer::Subpacket>& subpackets)
{
    packet_observer::FrameInfo info;
    info.direction  = direction;
    info.stream     = stream;
    info.parsed     = parsed;
    info.subpackets = subpackets;
    return info;
}

void RunPolicyTests()
{
    using packet_capture::Classify;
    using packet_capture::Scope;
    using packet_observer::Subpacket;

    const std::vector<Subpacket> plain = { { 3, 0x0123 }, { 3, 0x0200 } };

    const auto fullOther = Classify(Scope::Full, MakeInfo(Direction::Incoming, StreamKind::Other, true, plain));
    Expect(fullOther.write && fullOther.stub.empty(), "full writes hex for an Other stream");
    const auto fullRaw = Classify(Scope::Full, MakeInfo(Direction::Outgoing, StreamKind::Unclassified, false, {}));
    Expect(fullRaw.write && fullRaw.stub.empty(), "full writes hex for an unparsed frame");

    Expect(!Classify(Scope::Standard, MakeInfo(Direction::Incoming, StreamKind::Other, true, plain)).write,
           "standard skips Other");
    Expect(!Classify(Scope::Standard, MakeInfo(Direction::Incoming, StreamKind::Unclassified, true, plain)).write,
           "standard skips Unclassified");
    Expect(!Classify(Scope::Standard, MakeInfo(Direction::Incoming, StreamKind::Game, false, {})).write,
           "standard skips unparsed");

    const auto plainGame = Classify(Scope::Standard, MakeInfo(Direction::Incoming, StreamKind::Game, true, plain));
    Expect(plainGame.write && plainGame.stub.empty(), "standard writes hex for non-denied opcodes");

    const auto opener = Classify(Scope::Standard,
                                 MakeInfo(Direction::Outgoing, StreamKind::Game, true, { { 1, 0 } }));
    Expect(opener.write && opener.stub == "stub:t=0001", "standard stubs a type 0x0001 frame");

    const auto mixed = Classify(Scope::Standard,
                                MakeInfo(Direction::Outgoing, StreamKind::Game, true, { { 3, 0x00CA }, { 3, 0x0003 } }));
    Expect(mixed.write && mixed.stub == "stub:op=00CA;op=0003", "standard stubs the whole mixed frame");

    const auto typed = Classify(Scope::Standard,
                                MakeInfo(Direction::Incoming, StreamKind::Game, true, { { 3, 0x0123 }, { 0x0A1B, 0 } }));
    Expect(typed.write && typed.stub == "stub:op=0123;t=0A1B", "standard stubs a frame with a non-message subpacket");

    const auto outgoingMail = Classify(Scope::Standard,
                                       MakeInfo(Direction::Outgoing, StreamKind::Game, true, { { 3, 0x01D5 } }));
    Expect(outgoingMail.stub == "stub:op=01D5", "outgoing 0x01D5 is stubbed");
    const auto incomingMail = Classify(Scope::Standard,
                                       MakeInfo(Direction::Incoming, StreamKind::Game, true, { { 3, 0x01D5 } }));
    Expect(incomingMail.write && incomingMail.stub.empty(), "incoming 0x01D5 is not stubbed");
    const auto incomingChat = Classify(Scope::Standard,
                                       MakeInfo(Direction::Incoming, StreamKind::Game, true, { { 3, 0x00C9 } }));
    Expect(incomingChat.stub == "stub:op=00C9", "incoming 0x00C9 is stubbed");
    const auto searchComments = Classify(Scope::Standard,
                                         MakeInfo(Direction::Incoming, StreamKind::Game, true, { { 3, 0x01E0 } }));
    Expect(searchComments.stub == "stub:op=01E0", "incoming 0x01E0 is stubbed");
    const auto searchProfile = Classify(Scope::Standard,
                                        MakeInfo(Direction::Outgoing, StreamKind::Game, true, { { 3, 0x01DC } }));
    Expect(searchProfile.stub == "stub:op=01DC", "outgoing 0x01DC is stubbed");

    Expect(packet_capture::ParseScope("full") == Scope::Full, "scope full");
    Expect(packet_capture::ParseScope("Full") == Scope::Standard, "scope is case sensitive");
    Expect(packet_capture::ParseScope("") == Scope::Standard, "empty scope is standard");
    Expect(packet_capture::ParseScope("standard") == Scope::Standard, "scope standard");
}

} // namespace

int main()
{
    WSADATA winsock{};
    if (WSAStartup(MAKEWORD(2, 2), &winsock) != 0 || MH_Initialize() != MH_OK)
    {
        std::cerr << "Winsock or MinHook initialization failed\n";
        return 1;
    }
    SOCKET      listener = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    sockaddr_in address{};
    address.sin_family      = AF_INET;
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    address.sin_port        = 0;
    int addressLength       = sizeof(address);
    if (listener == INVALID_SOCKET ||
        bind(listener, reinterpret_cast<sockaddr*>(&address), addressLength) != 0 ||
        listen(listener, 4) != 0 ||
        getsockname(listener, reinterpret_cast<sockaddr*>(&address), &addressLength) != 0)
    {
        std::cerr << "loopback listener failed\n";
        return 1;
    }
    SOCKET sender = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (sender == INVALID_SOCKET ||
        connect(sender, reinterpret_cast<sockaddr*>(&address), addressLength) != 0)
    {
        std::cerr << "loopback connect failed\n";
        return 1;
    }
    SOCKET receiver = accept(listener, nullptr, nullptr);
    if (receiver == INVALID_SOCKET)
    {
        std::cerr << "loopback accept failed\n";
        return 1;
    }
    const DWORD timeout = 2000;
    setsockopt(receiver, SOL_SOCKET, SO_RCVTIMEO, reinterpret_cast<const char*>(&timeout), sizeof(timeout));

    packet_observer::PacketObserver           observer;
    std::vector<packet_observer::GameMessage> seen;
    std::vector<Record>                       frames;
    observer.SetSink([&](const packet_observer::GameMessage& message)
                     {
                         seen.push_back(message);
                     });
    observer.SetFrameSink([&](const packet_observer::FrameInfo& info)
                          {
                              Record record;
                              record.direction = info.direction;
                              record.stream    = info.stream;
                              record.parsed    = info.parsed;
                              record.bytes.assign(info.bytes.begin(), info.bytes.end());
                              record.subpackets.assign(info.subpackets.begin(), info.subpackets.end());
                              frames.push_back(std::move(record));
                          });
    if (!observer.InstallHook())
    {
        std::cerr << "packet observer hooks failed to install\n";
        return 1;
    }

    std::array<std::uint8_t, 51> frame{};
    frame[1] = 0;
    WriteU16(frame, 4, static_cast<std::uint16_t>(frame.size()));
    WriteU16(frame, 6, 1);
    WriteU16(frame, 16, 35);
    WriteU16(frame, 18, 3);
    WriteU32(frame, 20, 7);
    WriteU32(frame, 24, 8);
    WriteU16(frame, 34, 123);
    WriteU32(frame, 40, 1000);
    frame[48]                 = 0x00;
    frame[49]                 = 0xA5;
    frame[50]                 = 0xFF;
    const int            sent = send(sender, reinterpret_cast<const char*>(frame.data()), static_cast<int>(frame.size()), 0);
    std::array<char, 51> received{};
    const int            count = sent == static_cast<int>(frame.size())
                                     ? recv(receiver, received.data(), static_cast<int>(received.size()), MSG_WAITALL)
                                     : SOCKET_ERROR;
    bool                 valid = sent == static_cast<int>(frame.size()) &&
                                 count == static_cast<int>(frame.size()) &&
                                 seen.size() == 2 &&
                                 seen[0].direction == Direction::Outgoing &&
                                 seen[1].direction == Direction::Incoming &&
                                 seen[0].opcode == 123 && seen[1].opcode == 123 &&
                                 seen[0].sourceId == 7 && seen[1].targetId == 8 &&
                                 seen[0].timestamp == 1000 &&
                                 seen[0].payload == std::vector<std::uint8_t>{ 0x00, 0xA5, 0xFF };
    valid                      = valid && frames.size() == 2 &&
                                 frames[0].direction == Direction::Outgoing &&
                                 frames[1].direction == Direction::Incoming &&
                                 frames[0].parsed && frames[1].parsed &&
                                 frames[0].bytes == std::vector<std::uint8_t>(frame.begin(), frame.end());
    WSABUF    outbound{ static_cast<ULONG>(frame.size()),
                        reinterpret_cast<char*>(frame.data()) };
    WSABUF    inbound{ static_cast<ULONG>(received.size()), received.data() };
    DWORD     bytesSent     = 0;
    DWORD     bytesReceived = 0;
    DWORD     flags         = 0;
    const int sendResult    = WSASend(sender, &outbound, 1, &bytesSent, 0, nullptr, nullptr);
    const int recvResult    = sendResult == 0
                                  ? WSARecv(receiver, &inbound, 1, &bytesReceived, &flags, nullptr, nullptr)
                                  : SOCKET_ERROR;
    valid                   = valid && sendResult == 0 && recvResult == 0 &&
                              bytesSent == frame.size() && bytesReceived == frame.size() &&
                              seen.size() == 4 &&
                              seen[2].direction == Direction::Outgoing &&
                              seen[3].direction == Direction::Incoming &&
                              seen[2].opcode == 123 && seen[3].opcode == 123 &&
                              frames.size() == 4;
    WriteU16(frame, 18, 4);
    const int otherSent     = send(sender, reinterpret_cast<const char*>(frame.data()), static_cast<int>(frame.size()), 0);
    const int otherReceived = otherSent == static_cast<int>(frame.size())
                                  ? recv(receiver, received.data(), static_cast<int>(received.size()), MSG_WAITALL)
                                  : SOCKET_ERROR;
    valid                   = valid && otherReceived == static_cast<int>(frame.size()) &&
                              seen.size() == 4 && frames.size() == 6 &&
                              frames[4].bytes == std::vector<std::uint8_t>(frame.begin(), frame.end());
    Expect(valid, "loopback decode of both directions");
    // The first outgoing frame had only a type 0x0003 subpacket, so the socket is Other
    // while the receiver socket never sent and stays Unclassified.
    Expect(frames.size() == 6 &&
               frames[0].stream == StreamKind::Other && frames[2].stream == StreamKind::Other &&
               frames[1].stream == StreamKind::Unclassified && frames[3].stream == StreamKind::Unclassified,
           "type 0x0003 opener is Other; incoming before any outgoing is Unclassified");
    Expect(frames.size() == 6 && SubpacketsAre(frames[4], { { 4, 0 } }) && SubpacketsAre(frames[0], { { 3, 123 } }),
           "single-subpacket lists");

    // Game socket: the first outgoing frame opens with clear type 0x0001.
    SocketPair game = MakePair(listener, address);
    Expect(game.a != INVALID_SOCKET && game.b != INVALID_SOCKET, "game pair");
    frames.clear();
    const auto gameOpen = BuildFrame({ { 1, 0, 8 } });
    Expect(SendAll(game.a, gameOpen) && ReceiveAll(game.b, gameOpen.size()), "game open transfer");
    Expect(frames.size() == 2 &&
               frames[0].direction == Direction::Outgoing && frames[0].stream == StreamKind::Game &&
               frames[0].parsed && SubpacketsAre(frames[0], { { 1, 0 } }) &&
               frames[0].bytes == gameOpen,
           "outgoing type 0x0001 opener is Game");
    Expect(frames.size() == 2 && frames[1].direction == Direction::Incoming &&
               frames[1].stream == StreamKind::Unclassified && frames[1].parsed,
           "incoming frame on a fresh socket is Unclassified");

    frames.clear();
    const auto gameReply = BuildFrame({ { 3, 0x00CA, 4 }, { 3, 0x0003, 0 }, { 7, 0, 12 } });
    Expect(SendAll(game.b, gameReply) && ReceiveAll(game.a, gameReply.size()), "game reply transfer");
    Expect(frames.size() == 2 && frames[1].direction == Direction::Incoming && frames[1].stream == StreamKind::Game &&
               frames[1].parsed && SubpacketsAre(frames[1], { { 3, 0x00CA }, { 3, 0x0003 }, { 7, 0 } }),
           "incoming frame on a Game socket is Game with ordered subpackets");
    Expect(frames.size() == 2 && frames[0].stream == StreamKind::Other, "peer socket classifies independently");

    frames.clear();
    const auto gameLater = BuildFrame({ { 3, 0x0150, 2 } });
    Expect(SendAll(game.a, gameLater) && ReceiveAll(game.b, gameLater.size()), "game later transfer");
    Expect(frames.size() == 2 && frames[0].stream == StreamKind::Game && frames[0].direction == Direction::Outgoing &&
               SubpacketsAre(frames[0], { { 3, 0x0150 } }),
           "later outgoing frame stays Game");

    frames.clear();
    const auto packed = BuildFrame({ { 3, 0x00C8, 20 }, { 5, 0, 4 }, { 3, 0x0010, 0 } }, true);
    Expect(!packed.empty() && packed[1] == 1u && SendAll(game.b, packed) && ReceiveAll(game.a, packed.size()),
           "compressed transfer");
    Expect(frames.size() == 2 && frames[1].direction == Direction::Incoming && frames[1].stream == StreamKind::Game &&
               frames[1].parsed && frames[1].bytes == packed &&
               SubpacketsAre(frames[1], { { 3, 0x00C8 }, { 5, 0 }, { 3, 0x0010 } }),
           "compressed incoming frame lists inflated subpackets in order");
    Expect(frames.size() == 2 && frames[0].parsed && SubpacketsAre(frames[0], { { 3, 0x00C8 }, { 5, 0 }, { 3, 0x0010 } }),
           "compressed outgoing frame lists inflated subpackets in order");

    // Lobby socket: the first outgoing frame opens with clear type 0x0009.
    SocketPair lobby = MakePair(listener, address);
    Expect(lobby.a != INVALID_SOCKET && lobby.b != INVALID_SOCKET, "lobby pair");
    frames.clear();
    const auto lobbyOpen = BuildFrame({ { 9, 0, 24 } });
    Expect(SendAll(lobby.a, lobbyOpen) && ReceiveAll(lobby.b, lobbyOpen.size()), "lobby open transfer");
    Expect(frames.size() == 2 && frames[0].stream == StreamKind::Other && frames[0].parsed &&
               SubpacketsAre(frames[0], { { 9, 0 } }),
           "outgoing type 0x0009 opener is Other");
    frames.clear();
    const auto lobbyReply = BuildFrame({ { 3, 0x0002, 4 } });
    Expect(SendAll(lobby.b, lobbyReply) && ReceiveAll(lobby.a, lobbyReply.size()), "lobby reply transfer");
    Expect(frames.size() == 2 && frames[1].direction == Direction::Incoming && frames[1].stream == StreamKind::Other,
           "incoming frame on a lobby socket is Other");
    frames.clear();
    const auto lobbyLater = BuildFrame({ { 1, 0, 0 } });
    Expect(SendAll(lobby.a, lobbyLater) && ReceiveAll(lobby.b, lobbyLater.size()), "lobby later transfer");
    Expect(frames.size() == 2 && frames[0].stream == StreamKind::Other, "a lobby socket is never reclassified");

    // A frame carrying both type 0x0001 and 0x000A is not a game opener.
    SocketPair secure = MakePair(listener, address);
    Expect(secure.a != INVALID_SOCKET && secure.b != INVALID_SOCKET, "secure pair");
    frames.clear();
    const auto secureOpen = BuildFrame({ { 1, 0, 0 }, { 10, 0, 8 } });
    Expect(SendAll(secure.a, secureOpen) && ReceiveAll(secure.b, secureOpen.size()), "secure open transfer");
    Expect(frames.size() == 2 && frames[0].stream == StreamKind::Other, "type 0x000A with 0x0001 is Other");

    // Parse failure: one frame call with parsed=false, then the stream is disabled.
    SocketPair broken = MakePair(listener, address);
    Expect(broken.a != INVALID_SOCKET && broken.b != INVALID_SOCKET, "broken pair");
    frames.clear();
    auto badCount = BuildFrame({ { 1, 0, 8 } });
    PutU16(badCount, 6, 2);
    Expect(SendAll(broken.a, badCount) && ReceiveAll(broken.b, badCount.size()), "parse failure transfer");
    std::size_t outgoingFailures = 0;
    for (const Record& record : frames)
    {
        if (record.direction == Direction::Outgoing)
        {
            ++outgoingFailures;
            Expect(!record.parsed && record.subpackets.empty() && record.stream == StreamKind::Other &&
                       record.bytes == badCount,
                   "parse failure frame facts");
        }
    }
    Expect(outgoingFailures == 1, "parse failure dispatches once");
    const std::size_t afterFailure = frames.size();
    const auto        goodAfter    = BuildFrame({ { 1, 0, 8 } });
    Expect(SendAll(broken.a, goodAfter) && ReceiveAll(broken.b, goodAfter.size()), "post failure transfer");
    bool outgoingAfter = false;
    for (std::size_t index = afterFailure; index < frames.size(); ++index)
    {
        outgoingAfter = outgoingAfter || frames[index].direction == Direction::Outgoing;
    }
    Expect(!outgoingAfter, "a failed stream is disabled");

    // Inflate failure takes the same path.
    SocketPair garbled = MakePair(listener, address);
    Expect(garbled.a != INVALID_SOCKET && garbled.b != INVALID_SOCKET, "garbled pair");
    frames.clear();
    std::vector<std::uint8_t> badZlib(16u + 12u, 0x5A);
    badZlib[1] = 1;
    PutU16(badZlib, 4, static_cast<std::uint16_t>(badZlib.size()));
    PutU16(badZlib, 6, 1);
    Expect(SendAll(garbled.a, badZlib) && ReceiveAll(garbled.b, badZlib.size()), "inflate failure transfer");
    std::size_t garbledOutgoing = 0;
    for (const Record& record : frames)
    {
        if (record.direction == Direction::Outgoing)
        {
            ++garbledOutgoing;
            Expect(!record.parsed && record.subpackets.empty() && record.stream == StreamKind::Other,
                   "inflate failure frame facts");
        }
    }
    Expect(garbledOutgoing == 1, "inflate failure dispatches once");

    RunPolicyTests();

    const bool stopped = observer.Shutdown();
    ClosePair(garbled);
    ClosePair(broken);
    ClosePair(secure);
    ClosePair(lobby);
    ClosePair(game);
    closesocket(receiver);
    closesocket(sender);
    closesocket(listener);
    MH_Uninitialize();
    WSACleanup();
    Expect(stopped, "observer shutdown");
    if (gFailures != 0)
    {
        std::cerr << "packet observer tests failed: " << gFailures << '\n';
        return 1;
    }
    return 0;
}
