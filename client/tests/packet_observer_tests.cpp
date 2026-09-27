#include "packet_observer.h"

#include <MinHook.h>
#include <winsock2.h>
#include <ws2tcpip.h>

#include <array>
#include <cstdint>
#include <iostream>
#include <vector>

namespace
{

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
        listen(listener, 1) != 0 ||
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
    std::vector<packet_observer::Direction>   frameDirections;
    std::vector<std::vector<std::uint8_t>>    frames;
    observer.SetSink([&](const packet_observer::GameMessage& message)
                     {
                         seen.push_back(message);
                     });
    observer.SetFrameSink([&](packet_observer::Direction    direction,
                              std::span<const std::uint8_t> frame)
                          {
                              frameDirections.push_back(direction);
                              frames.emplace_back(frame.begin(), frame.end());
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
                                 seen[0].direction == packet_observer::Direction::Outgoing &&
                                 seen[1].direction == packet_observer::Direction::Incoming &&
                                 seen[0].opcode == 123 && seen[1].opcode == 123 &&
                                 seen[0].sourceId == 7 && seen[1].targetId == 8 &&
                                 seen[0].timestamp == 1000 &&
                                 seen[0].payload == std::vector<std::uint8_t>{ 0x00, 0xA5, 0xFF };
    valid                      = valid && frames.size() == 2 &&
                                 frameDirections[0] == packet_observer::Direction::Outgoing &&
                                 frameDirections[1] == packet_observer::Direction::Incoming &&
                                 frames[0] == std::vector<std::uint8_t>(frame.begin(), frame.end());
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
                              seen[2].direction == packet_observer::Direction::Outgoing &&
                              seen[3].direction == packet_observer::Direction::Incoming &&
                              seen[2].opcode == 123 && seen[3].opcode == 123 &&
                              frames.size() == 4;
    WriteU16(frame, 18, 4);
    const int otherSent     = send(sender, reinterpret_cast<const char*>(frame.data()), static_cast<int>(frame.size()), 0);
    const int otherReceived = otherSent == static_cast<int>(frame.size())
                                  ? recv(receiver, received.data(), static_cast<int>(received.size()), MSG_WAITALL)
                                  : SOCKET_ERROR;
    valid                   = valid && otherReceived == static_cast<int>(frame.size()) &&
                              seen.size() == 4 && frames.size() == 6 &&
                              frames[4] == std::vector<std::uint8_t>(frame.begin(), frame.end());
    const bool stopped      = observer.Shutdown();
    closesocket(receiver);
    closesocket(sender);
    closesocket(listener);
    MH_Uninitialize();
    WSACleanup();
    if (!valid || !stopped)
    {
        std::cerr << "packet observer did not decode both loopback directions\n";
        return 1;
    }
    return 0;
}
