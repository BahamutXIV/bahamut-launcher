#include "packet_observer.h"
#include "fault_guard.h"

#include <windows.h>
#include <winsock2.h>

#include <MinHook.h>
#include <miniz.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>
#include <memory>
#include <mutex>
#include <span>
#include <unordered_map>
#include <utility>
#include <vector>

namespace packet_observer
{

struct PacketObserver::Impl
{
    std::mutex sinkMutex;
    Sink       sink;
    FrameSink  frameSink;
    bool       installed = false;
};

struct ObserverImplementation
{
    static void Dispatch(PacketObserver*    observer,
                         const GameMessage& message) noexcept;
    static void DispatchFrame(PacketObserver* observer, Direction direction, std::span<const std::uint8_t> frame) noexcept;
    static bool Install(PacketObserver* observer) noexcept;
    static bool Shutdown(PacketObserver* observer) noexcept;
};

namespace
{

constexpr std::size_t kBaseHeaderSize        = 16u;
constexpr std::size_t kSubpacketHeaderSize   = 16u;
constexpr std::size_t kGameMessageHeaderSize = 16u;
constexpr std::size_t kGameMessageSubpacketPrefix =
    kSubpacketHeaderSize + kGameMessageHeaderSize;
constexpr std::size_t   kMaximumInflatedBytes        = 1024u * 1024u;
constexpr std::size_t   kMaximumObservedBytesPerCall = 1024u * 1024u;
constexpr std::size_t   kMaximumTrackedStreams       = 128u;
constexpr std::size_t   kMaximumWsabufs              = 64u;
constexpr std::size_t   kCopyChunkSize               = 4096u;
constexpr std::uint16_t kGameMessageSubpacketType    = 0x03u;

// Retail layout offsets from XIVLegacy/xivl-client-structs:
// manifests/structs.json BCS-S-0001, BCS-S-0002, and BCS-S-0003.
constexpr std::size_t kBaseCompressedOffset       = 1u;
constexpr std::size_t kBaseFrameSizeOffset        = 4u;
constexpr std::size_t kBaseSubpacketCountOffset   = 6u;
constexpr std::size_t kSubpacketTypeOffset        = 2u;
constexpr std::size_t kSubpacketSourceIdOffset    = 4u;
constexpr std::size_t kSubpacketTargetIdOffset    = 8u;
constexpr std::size_t kGameMessageOpcodeOffset    = 2u;
constexpr std::size_t kGameMessageTimestampOffset = 8u;

using RecvFunction        = int(WSAAPI*)(SOCKET, char*, int, int);
using SendFunction        = int(WSAAPI*)(SOCKET, const char*, int, int);
using WsaRecvFunction     = int(WSAAPI*)(SOCKET,
                                         LPWSABUF,
                                         DWORD,
                                         LPDWORD,
                                         LPDWORD,
                                         LPWSAOVERLAPPED,
                                         LPWSAOVERLAPPED_COMPLETION_ROUTINE);
using WsaSendFunction     = int(WSAAPI*)(SOCKET,
                                         LPWSABUF,
                                         DWORD,
                                         LPDWORD,
                                         DWORD,
                                         LPWSAOVERLAPPED,
                                         LPWSAOVERLAPPED_COMPLETION_ROUTINE);
using CloseSocketFunction = int(WSAAPI*)(SOCKET);

int WSAAPI HookedRecv(SOCKET socket, char* buffer, int length, int flags);
int WSAAPI HookedSend(SOCKET socket, const char* buffer, int length, int flags);
int WSAAPI HookedWsaRecv(SOCKET                             socket,
                         LPWSABUF                           buffers,
                         DWORD                              bufferCount,
                         LPDWORD                            bytesReceived,
                         LPDWORD                            flags,
                         LPWSAOVERLAPPED                    overlapped,
                         LPWSAOVERLAPPED_COMPLETION_ROUTINE completionRoutine);
int WSAAPI HookedWsaSend(SOCKET                             socket,
                         LPWSABUF                           buffers,
                         DWORD                              bufferCount,
                         LPDWORD                            bytesSent,
                         DWORD                              flags,
                         LPWSAOVERLAPPED                    overlapped,
                         LPWSAOVERLAPPED_COMPLETION_ROUTINE completionRoutine);
int WSAAPI HookedCloseSocket(SOCKET socket);

struct HookRecord
{
    const char* name    = nullptr;
    void*       detour  = nullptr;
    void*       target  = nullptr;
    bool        created = false;
    bool        enabled = false;
};

void ResetHook(HookRecord& hook) noexcept
{
    hook.target  = nullptr;
    hook.created = false;
    hook.enabled = false;
}

std::array<HookRecord, 5> gHooks = {
    HookRecord{ "recv", reinterpret_cast<void*>(&HookedRecv) },
    HookRecord{ "WSARecv", reinterpret_cast<void*>(&HookedWsaRecv) },
    HookRecord{ "closesocket", reinterpret_cast<void*>(&HookedCloseSocket) },
    HookRecord{ "send", reinterpret_cast<void*>(&HookedSend) },
    HookRecord{ "WSASend", reinterpret_cast<void*>(&HookedWsaSend) },
};

bool HasCreatedHooks() noexcept
{
    return std::any_of(gHooks.begin(), gHooks.end(), [](const HookRecord& hook)
                       {
                           return hook.created;
                       });
}

std::atomic<RecvFunction>        gOriginalRecv        = nullptr;
std::atomic<WsaRecvFunction>     gOriginalWsaRecv     = nullptr;
std::atomic<CloseSocketFunction> gOriginalCloseSocket = nullptr;
std::atomic<SendFunction>        gOriginalSend        = nullptr;
std::atomic<WsaSendFunction>     gOriginalWsaSend     = nullptr;

std::atomic<PacketObserver*> gObserver        = nullptr;
std::atomic<std::uint32_t>   gActiveHookCalls = 0;
std::mutex                   gLifecycleMutex;
thread_local std::uint32_t   gHookDepth = 0;

struct StreamState
{
    std::mutex                mutex;
    std::vector<std::uint8_t> pending;
    Direction                 direction         = Direction::Incoming;
    std::size_t               activeCalls       = 0;
    bool                      transportResolved = false;
    bool                      isTcp             = false;
    bool                      disabled          = false;
};

void DisableStream(StreamState& state) noexcept;

struct BufferRange
{
    const char* data   = nullptr;
    std::size_t length = 0;
};

struct StreamKey
{
    std::uintptr_t socket                             = 0;
    Direction      direction                          = Direction::Incoming;
    bool           operator==(const StreamKey&) const = default;
};

struct StreamKeyHash
{
    std::size_t operator()(const StreamKey& key) const noexcept
    {
        return std::hash<std::uintptr_t>{}(key.socket) ^
               (key.direction == Direction::Outgoing ? 0x9e3779b9u : 0u);
    }
};

std::mutex                                                                 gStreamsMutex;
std::unordered_map<StreamKey, std::shared_ptr<StreamState>, StreamKeyHash> gStreams;

class HookCallScope
{
public:
    HookCallScope() noexcept
    : outermost_(gHookDepth == 0)
    {
        ++gHookDepth;
        gActiveHookCalls.fetch_add(1u, std::memory_order_acq_rel);
    }

    ~HookCallScope()
    {
        --gHookDepth;
        gActiveHookCalls.fetch_sub(1u, std::memory_order_acq_rel);
    }

    [[nodiscard]] bool IsOutermost() const noexcept
    {
        return outermost_;
    }

private:
    bool outermost_ = false;
};

bool SafeCopyMemory(const void* source, void* destination, std::size_t length) noexcept
{
    if (length == 0)
    {
        return true;
    }
    if (source == nullptr || destination == nullptr)
    {
        return false;
    }
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(destination, source, length);
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

std::uint16_t ReadU16(const std::uint8_t* bytes) noexcept
{
    return static_cast<std::uint16_t>(
        static_cast<std::uint16_t>(bytes[0]) |
        static_cast<std::uint16_t>(static_cast<std::uint16_t>(bytes[1]) << 8u));
}

std::uint32_t ReadU32(const std::uint8_t* bytes) noexcept
{
    return static_cast<std::uint32_t>(bytes[0]) |
           (static_cast<std::uint32_t>(bytes[1]) << 8u) |
           (static_cast<std::uint32_t>(bytes[2]) << 16u) |
           (static_cast<std::uint32_t>(bytes[3]) << 24u);
}

bool QueryTcpSocket(SOCKET socket, bool& resolved) noexcept
{
    resolved       = false;
    int socketType = 0;
    int typeLength = sizeof(socketType);
    if (getsockopt(socket,
                   SOL_SOCKET,
                   SO_TYPE,
                   reinterpret_cast<char*>(&socketType),
                   &typeLength) == SOCKET_ERROR)
    {
        return false;
    }
    if (socketType != SOCK_STREAM)
    {
        resolved = true;
        return false;
    }

    WSAPROTOCOL_INFOW protocol{};
    int               protocolLength = sizeof(protocol);
    if (getsockopt(socket,
                   SOL_SOCKET,
                   SO_PROTOCOL_INFOW,
                   reinterpret_cast<char*>(&protocol),
                   &protocolLength) == SOCKET_ERROR)
    {
        return false;
    }
    resolved = true;
    return protocol.iProtocol == IPPROTO_TCP;
}

std::shared_ptr<StreamState> BeginStreamCall(SOCKET socket, Direction direction) noexcept
{
    try
    {
        std::shared_ptr<StreamState> state;
        {
            std::lock_guard lock(gStreamsMutex);
            const StreamKey key{ static_cast<std::uintptr_t>(socket), direction };
            const auto      found = gStreams.find(key);
            if (found != gStreams.end())
            {
                state = found->second;
            }
            else
            {
                if (gStreams.size() >= kMaximumTrackedStreams)
                {
                    return {};
                }
                state            = std::make_shared<StreamState>();
                state->direction = direction;
                gStreams.emplace(key, state);
            }
        }

        std::lock_guard lock(state->mutex);
        if (!state->transportResolved)
        {
            bool       resolved = false;
            const bool isTcp    = QueryTcpSocket(socket, resolved);
            if (resolved)
            {
                state->transportResolved = true;
                state->isTcp             = isTcp;
            }
        }
        if (!state->transportResolved || !state->isTcp)
        {
            return {};
        }
        if (state->activeCalls != 0)
        {
            state->disabled = true;
            state->pending.clear();
        }
        ++state->activeCalls;
        return state;
    }
    catch (...)
    {
        return {};
    }
}

void DisableSocketStream(SOCKET socket, Direction direction) noexcept
{
    try
    {
        std::shared_ptr<StreamState> state;
        {
            std::lock_guard lock(gStreamsMutex);
            const StreamKey key{ static_cast<std::uintptr_t>(socket), direction };
            const auto      found = gStreams.find(key);
            if (found != gStreams.end())
            {
                state = found->second;
            }
            else if (gStreams.size() < kMaximumTrackedStreams)
            {
                state            = std::make_shared<StreamState>();
                state->direction = direction;
                gStreams.emplace(key, state);
            }
        }
        if (state)
        {
            std::lock_guard lock(state->mutex);
            DisableStream(*state);
        }
    }
    catch (...)
    {
    }
}

struct GameMessageView
{
    std::uint16_t       opcode        = 0;
    std::uint32_t       sourceId      = 0;
    std::uint32_t       targetId      = 0;
    std::uint32_t       timestamp     = 0;
    const std::uint8_t* payload       = nullptr;
    std::size_t         payloadLength = 0;
};

bool InflateBounded(std::span<const std::uint8_t> input,
                    std::vector<std::uint8_t>&    output,
                    int                           windowBits)
{
    mz_stream stream{};
    if (mz_inflateInit2(&stream, windowBits) != MZ_OK)
    {
        return false;
    }

    stream.next_in  = const_cast<unsigned char*>(input.data());
    stream.avail_in = static_cast<mz_uint>(input.size());
    std::array<std::uint8_t, kCopyChunkSize> chunk{};
    bool                                     success = false;
    try
    {
        for (;;)
        {
            stream.next_out             = chunk.data();
            stream.avail_out            = static_cast<mz_uint>(chunk.size());
            const mz_ulong    beforeIn  = stream.total_in;
            const mz_ulong    beforeOut = stream.total_out;
            const int         status    = mz_inflate(&stream, MZ_NO_FLUSH);
            const std::size_t produced  = chunk.size() - stream.avail_out;
            if (produced > kMaximumInflatedBytes - output.size())
            {
                break;
            }
            output.insert(output.end(),
                          chunk.begin(),
                          chunk.begin() + static_cast<std::ptrdiff_t>(produced));

            if (status == MZ_STREAM_END)
            {
                success = stream.avail_in == 0;
                break;
            }
            if (status != MZ_OK ||
                (stream.total_in == beforeIn && stream.total_out == beforeOut))
            {
                break;
            }
        }
    }
    catch (...)
    {
        success = false;
    }
    const int endStatus = mz_inflateEnd(&stream);
    return success && endStatus == MZ_OK;
}

bool InflateZlibOrRaw(std::span<const std::uint8_t> input,
                      std::vector<std::uint8_t>&    output)
{
    output.clear();
    if (InflateBounded(input, output, MZ_DEFAULT_WINDOW_BITS))
    {
        return true;
    }
    output.clear();
    return InflateBounded(input, output, -MZ_DEFAULT_WINDOW_BITS);
}

bool ParseSubpackets(std::span<const std::uint8_t> body,
                     std::uint16_t                 subpacketCount,
                     std::vector<GameMessageView>& messages)
{
    if (subpacketCount > body.size() / kSubpacketHeaderSize)
    {
        return false;
    }
    messages.clear();
    messages.reserve(std::min<std::size_t>(
        subpacketCount, body.size() / kGameMessageSubpacketPrefix));

    std::size_t offset = 0;
    for (std::uint32_t index = 0; index < subpacketCount; ++index)
    {
        if (body.size() - offset < kSubpacketHeaderSize)
        {
            return false;
        }
        const std::uint8_t* const subpacket     = body.data() + offset;
        const std::size_t         subpacketSize = ReadU16(subpacket);
        const std::uint16_t       subpacketType =
            ReadU16(subpacket + kSubpacketTypeOffset);
        if (subpacketSize < kSubpacketHeaderSize ||
            subpacketSize > body.size() - offset)
        {
            return false;
        }

        if (subpacketType == kGameMessageSubpacketType)
        {
            if (subpacketSize < kGameMessageSubpacketPrefix)
            {
                return false;
            }
            const std::uint8_t* const gameMessage =
                subpacket + kSubpacketHeaderSize;
            messages.push_back(GameMessageView{
                ReadU16(gameMessage + kGameMessageOpcodeOffset),
                ReadU32(subpacket + kSubpacketSourceIdOffset),
                ReadU32(subpacket + kSubpacketTargetIdOffset),
                ReadU32(gameMessage + kGameMessageTimestampOffset),
                gameMessage + kGameMessageHeaderSize,
                subpacketSize - kGameMessageSubpacketPrefix,
            });
        }
        offset += subpacketSize;
    }
    if (offset == body.size())
    {
        return true;
    }
    const std::size_t trailingLength = body.size() - offset;
    if (trailingLength >= kSubpacketHeaderSize)
    {
        return false;
    }
    return std::all_of(body.begin() + static_cast<std::ptrdiff_t>(offset),
                       body.end(),
                       [](std::uint8_t value)
                       {
                           return value == 0u;
                       });
}

bool DecodeFrame(std::span<const std::uint8_t> frame,
                 PacketObserver*               observer,
                 Direction                     direction)
{
    if (frame.size() < kBaseHeaderSize ||
        frame[kBaseCompressedOffset] > 1u ||
        ReadU16(frame.data() + kBaseFrameSizeOffset) != frame.size())
    {
        return false;
    }

    ObserverImplementation::DispatchFrame(observer, direction, frame);

    std::vector<std::uint8_t>     inflated;
    std::span<const std::uint8_t> body = frame.subspan(kBaseHeaderSize);
    if (frame[kBaseCompressedOffset] != 0u)
    {
        if (!InflateZlibOrRaw(body, inflated))
        {
            return false;
        }
        body = inflated;
    }

    std::vector<GameMessageView> views;
    if (!ParseSubpackets(body,
                         ReadU16(frame.data() + kBaseSubpacketCountOffset),
                         views))
    {
        return false;
    }

    for (const GameMessageView& view : views)
    {
        GameMessage message;
        message.direction = direction;
        message.opcode    = view.opcode;
        message.sourceId  = view.sourceId;
        message.targetId  = view.targetId;
        message.timestamp = view.timestamp;
        if (view.payloadLength != 0)
        {
            message.payload.assign(view.payload,
                                   view.payload + view.payloadLength);
        }
        ObserverImplementation::Dispatch(observer, message);
    }
    return true;
}

void DisableStream(StreamState& state) noexcept
{
    state.disabled = true;
    state.pending.clear();
}

bool AppendObservedBytes(StreamState&        state,
                         PacketObserver*     observer,
                         const std::uint8_t* bytes,
                         std::size_t         length)
{
    std::size_t offset = 0;
    while (offset < length && !state.disabled)
    {
        if (state.pending.size() < 6u)
        {
            const std::size_t amount =
                std::min(length - offset, 6u - state.pending.size());
            state.pending.insert(state.pending.end(),
                                 bytes + offset,
                                 bytes + offset + amount);
            offset += amount;
            if (state.pending.size() < 6u)
            {
                continue;
            }

            const std::size_t frameSize =
                ReadU16(state.pending.data() + kBaseFrameSizeOffset);
            if (state.pending[kBaseCompressedOffset] > 1u ||
                frameSize < kBaseHeaderSize)
            {
                DisableStream(state);
                break;
            }
        }

        const std::size_t frameSize =
            ReadU16(state.pending.data() + kBaseFrameSizeOffset);
        if (state.pending.size() > frameSize)
        {
            DisableStream(state);
            break;
        }
        const std::size_t amount =
            std::min(length - offset, frameSize - state.pending.size());
        state.pending.insert(state.pending.end(),
                             bytes + offset,
                             bytes + offset + amount);
        offset += amount;
        if (state.pending.size() == frameSize)
        {
            if (!DecodeFrame(state.pending, observer, state.direction))
            {
                DisableStream(state);
                break;
            }
            state.pending.clear();
        }
    }
    return !state.disabled;
}

void FinishStreamCall(const std::shared_ptr<StreamState>& state,
                      PacketObserver*                     observer,
                      const BufferRange*                  ranges,
                      std::size_t                         rangeCount,
                      std::size_t                         transferred) noexcept
{
    if (!state)
    {
        return;
    }

    try
    {
        std::lock_guard lock(state->mutex);
        if (state->activeCalls == 0)
        {
            DisableStream(*state);
            return;
        }
        if (state->activeCalls != 1u)
        {
            DisableStream(*state);
        }

        if (transferred > kMaximumObservedBytesPerCall)
        {
            DisableStream(*state);
        }
        if (!state->disabled && transferred != 0)
        {
            std::size_t                              remaining = transferred;
            std::array<std::uint8_t, kCopyChunkSize> copied{};
            for (std::size_t index = 0;
                 index < rangeCount && remaining != 0;
                 ++index)
            {
                const BufferRange& range = ranges[index];
                const std::size_t  rangeLength =
                    std::min(remaining, range.length);
                const std::uintptr_t base =
                    reinterpret_cast<std::uintptr_t>(range.data);
                if (rangeLength != 0 && range.data == nullptr)
                {
                    DisableStream(*state);
                    break;
                }
                for (std::size_t offset = 0;
                     offset < rangeLength && !state->disabled;)
                {
                    const std::size_t amount =
                        std::min(rangeLength - offset, copied.size());
                    if (base > std::numeric_limits<std::uintptr_t>::max() - offset ||
                        !SafeCopyMemory(reinterpret_cast<const void*>(base + offset),
                                        copied.data(),
                                        amount))
                    {
                        DisableStream(*state);
                        break;
                    }
                    if (!AppendObservedBytes(*state,
                                             observer,
                                             copied.data(),
                                             amount))
                    {
                        break;
                    }
                    offset += amount;
                }
                remaining -= rangeLength;
            }
            if (remaining != 0)
            {
                DisableStream(*state);
            }
        }
        --state->activeCalls;
    }
    catch (...)
    {
        try
        {
            std::lock_guard lock(state->mutex);
            DisableStream(*state);
            if (state->activeCalls != 0)
            {
                --state->activeCalls;
            }
        }
        catch (...)
        {
        }
    }
}

void EraseSocketStreams(SOCKET socket) noexcept
{
    try
    {
        std::lock_guard lock(gStreamsMutex);
        gStreams.erase(StreamKey{ static_cast<std::uintptr_t>(socket), Direction::Incoming });
        gStreams.erase(StreamKey{ static_cast<std::uintptr_t>(socket), Direction::Outgoing });
    }
    catch (...)
    {
    }
}

bool CaptureWsabufs(LPWSABUF                                  buffers,
                    DWORD                                     bufferCount,
                    std::array<BufferRange, kMaximumWsabufs>& ranges) noexcept
{
    if (bufferCount > kMaximumWsabufs ||
        (bufferCount != 0u && buffers == nullptr))
    {
        return false;
    }
    std::array<WSABUF, kMaximumWsabufs> copied{};
    if (!SafeCopyMemory(buffers,
                        copied.data(),
                        static_cast<std::size_t>(bufferCount) * sizeof(WSABUF)))
    {
        return false;
    }
    for (std::size_t index = 0; index < bufferCount; ++index)
    {
        if (copied[index].len != 0u && copied[index].buf == nullptr)
        {
            return false;
        }
        ranges[index] = BufferRange{
            copied[index].buf,
            static_cast<std::size_t>(copied[index].len),
        };
    }
    return true;
}

bool SafeRead(const DWORD* source, DWORD& destination) noexcept
{
    return SafeCopyMemory(source, &destination, sizeof(DWORD));
}

struct LastErrorState
{
    DWORD win32   = GetLastError();
    int   winsock = WSAGetLastError();

    void Restore() const noexcept
    {
        WSASetLastError(winsock);
        SetLastError(win32);
    }
};

void StoreOriginal(std::size_t index, LPVOID original) noexcept
{
    switch (index)
    {
        case 0:
            gOriginalRecv.store(reinterpret_cast<RecvFunction>(original),
                                std::memory_order_release);
            break;
        case 1:
            gOriginalWsaRecv.store(reinterpret_cast<WsaRecvFunction>(original),
                                   std::memory_order_release);
            break;
        case 2:
            gOriginalCloseSocket.store(reinterpret_cast<CloseSocketFunction>(original),
                                       std::memory_order_release);
            break;
        case 3:
            gOriginalSend.store(reinterpret_cast<SendFunction>(original),
                                std::memory_order_release);
            break;
        case 4:
            gOriginalWsaSend.store(reinterpret_cast<WsaSendFunction>(original),
                                   std::memory_order_release);
            break;
        default:
            break;
    }
}

void ClearOriginals() noexcept
{
    gOriginalRecv.store(nullptr, std::memory_order_release);
    gOriginalWsaRecv.store(nullptr, std::memory_order_release);
    gOriginalCloseSocket.store(nullptr, std::memory_order_release);
    gOriginalSend.store(nullptr, std::memory_order_release);
    gOriginalWsaSend.store(nullptr, std::memory_order_release);
}

bool ShutdownHooksLocked(PacketObserver* observer, bool* installed)
{
    PacketObserver* const owner = gObserver.load(std::memory_order_acquire);
    if (owner != nullptr && owner != observer)
    {
        return false;
    }

    bool allDisabled = true;
    for (HookRecord& hook : gHooks)
    {
        if (!hook.created || !hook.enabled)
        {
            continue;
        }
        const MH_STATUS status = MH_DisableHook(hook.target);
        if (status == MH_OK || status == MH_ERROR_DISABLED)
        {
            hook.enabled = false;
        }
        else
        {
            allDisabled = false;
        }
    }
    if (!allDisabled)
    {
        return false;
    }

    while (gActiveHookCalls.load(std::memory_order_acquire) != 0u)
    {
        SwitchToThread();
    }

    bool allRemoved = true;
    for (HookRecord& hook : gHooks)
    {
        if (!hook.created)
        {
            continue;
        }
        const MH_STATUS status = MH_RemoveHook(hook.target);
        if (status == MH_OK || status == MH_ERROR_NOT_CREATED)
        {
            ResetHook(hook);
        }
        else
        {
            allRemoved = false;
        }
    }
    if (!allRemoved)
    {
        return false;
    }

    {
        std::lock_guard lock(gStreamsMutex);
        gStreams.clear();
    }
    ClearOriginals();
    gObserver.store(nullptr, std::memory_order_release);
    if (installed != nullptr)
    {
        *installed = false;
    }
    return true;
}

bool InstallHooks(PacketObserver* observer, bool& installed) noexcept
{
    try
    {
        std::lock_guard lock(gLifecycleMutex);
        if (observer == nullptr)
        {
            return false;
        }
        PacketObserver* const owner = gObserver.load(std::memory_order_acquire);
        if (owner == observer && installed)
        {
            return true;
        }
        if (owner != nullptr && owner != observer)
        {
            return false;
        }
        if (owner == observer || HasCreatedHooks())
        {
            if (!ShutdownHooksLocked(observer, &installed))
            {
                return false;
            }
        }

        const HMODULE winsock = GetModuleHandleW(L"ws2_32.dll");
        if (winsock == nullptr)
        {
            return false;
        }
        for (HookRecord& hook : gHooks)
        {
            const FARPROC procedure = GetProcAddress(winsock, hook.name);
            if (procedure == nullptr)
            {
                for (HookRecord& resetHook : gHooks)
                {
                    if (!resetHook.created)
                    {
                        ResetHook(resetHook);
                    }
                }
                return false;
            }
            hook.target = reinterpret_cast<void*>(procedure);
        }

        for (std::size_t index = 0; index < gHooks.size(); ++index)
        {
            HookRecord&     hook     = gHooks[index];
            LPVOID          original = nullptr;
            const MH_STATUS created =
                MH_CreateHook(hook.target, hook.detour, &original);
            if (created == MH_OK)
            {
                hook.created = true;
                StoreOriginal(index, original);
            }
            if (created != MH_OK || original == nullptr)
            {
                static_cast<void>(ShutdownHooksLocked(observer, &installed));
                return false;
            }
        }

        gObserver.store(observer, std::memory_order_release);
        for (HookRecord& hook : gHooks)
        {
            if (MH_EnableHook(hook.target) != MH_OK)
            {
                static_cast<void>(ShutdownHooksLocked(observer, &installed));
                return false;
            }
            hook.enabled = true;
        }
        installed = true;
        return true;
    }
    catch (...)
    {
        return false;
    }
}

std::shared_ptr<StreamState> StartObservedStreamCall(PacketObserver* observer,
                                                     bool            outermost,
                                                     SOCKET          socket,
                                                     Direction       direction = Direction::Incoming) noexcept
{
    if (!outermost || observer == nullptr)
    {
        return {};
    }
    return BeginStreamCall(socket, direction);
}

} // namespace

void ObserverImplementation::DispatchFrame(PacketObserver*               observer,
                                           Direction                     direction,
                                           std::span<const std::uint8_t> frame) noexcept
{
    if (observer == nullptr)
    {
        return;
    }
    try
    {
        PacketObserver::FrameSink sink;
        {
            std::lock_guard lock(observer->impl_->sinkMutex);
            sink = observer->impl_->frameSink;
        }
        if (sink)
        {
            sink(direction, frame);
        }
    }
    catch (...)
    {
    }
}

void ObserverImplementation::Dispatch(PacketObserver*    observer,
                                      const GameMessage& message) noexcept
{
    if (observer == nullptr)
    {
        return;
    }
    try
    {
        PacketObserver::Sink sink;
        {
            std::lock_guard lock(observer->impl_->sinkMutex);
            sink = observer->impl_->sink;
        }
        if (sink)
        {
            sink(message);
        }
    }
    catch (...)
    {
    }
}

bool ObserverImplementation::Install(PacketObserver* observer) noexcept
{
    return observer != nullptr && InstallHooks(observer, observer->impl_->installed);
}

bool ObserverImplementation::Shutdown(PacketObserver* observer) noexcept
{
    try
    {
        std::lock_guard       lock(gLifecycleMutex);
        const PacketObserver* owner = gObserver.load(std::memory_order_acquire);
        if (owner == nullptr)
        {
            return !HasCreatedHooks() ||
                   ShutdownHooksLocked(observer,
                                       observer != nullptr
                                           ? &observer->impl_->installed
                                           : nullptr);
        }
        if (owner != observer || observer == nullptr)
        {
            return false;
        }
        return ShutdownHooksLocked(observer, &observer->impl_->installed);
    }
    catch (...)
    {
        return false;
    }
}

PacketObserver::PacketObserver()
: impl_(std::make_unique<Impl>())
{
}

PacketObserver::~PacketObserver() = default;

void PacketObserver::SetFrameSink(FrameSink sink)
{
    std::lock_guard lock(impl_->sinkMutex);
    impl_->frameSink = std::move(sink);
}

void PacketObserver::SetSink(Sink sink)
{
    std::lock_guard lock(impl_->sinkMutex);
    impl_->sink = std::move(sink);
}

bool PacketObserver::InstallHook()
{
    return ObserverImplementation::Install(this);
}

bool PacketObserver::Shutdown()
{
    return ObserverImplementation::Shutdown(this);
}

namespace
{

int WSAAPI HookedRecv(SOCKET socket, char* buffer, int length, int flags)
{
    HookCallScope        scope;
    const LastErrorState beforeCall;
    const RecvFunction   original = gOriginalRecv.load(std::memory_order_acquire);
    if (original == nullptr)
    {
        WSASetLastError(WSAEFAULT);
        return SOCKET_ERROR;
    }

    PacketObserver* const        observer = gObserver.load(std::memory_order_acquire);
    const bool                   peek     = (flags & MSG_PEEK) != 0;
    std::shared_ptr<StreamState> state;
    if (!peek)
    {
        state = StartObservedStreamCall(observer, scope.IsOutermost(), socket);
    }
    beforeCall.Restore();
    const int            result = original(socket, buffer, length, flags);
    const LastErrorState afterCall;
    if (state)
    {
        BufferRange range{ buffer, length > 0 ? static_cast<std::size_t>(length) : 0u };
        FinishStreamCall(state,
                         observer,
                         &range,
                         1u,
                         result > 0 ? static_cast<std::size_t>(result) : 0u);
    }
    afterCall.Restore();
    return result;
}

int WSAAPI HookedSend(SOCKET socket, const char* buffer, int length, int flags)
{
    HookCallScope        scope;
    const LastErrorState beforeCall;
    const SendFunction   original = gOriginalSend.load(std::memory_order_acquire);
    if (original == nullptr)
    {
        WSASetLastError(WSAEFAULT);
        return SOCKET_ERROR;
    }
    PacketObserver* const observer = gObserver.load(std::memory_order_acquire);
    const auto            state    = StartObservedStreamCall(observer, scope.IsOutermost(), socket, Direction::Outgoing);
    beforeCall.Restore();
    const int            result = original(socket, buffer, length, flags);
    const LastErrorState afterCall;
    if (state)
    {
        BufferRange range{ buffer, length > 0 ? static_cast<std::size_t>(length) : 0u };
        FinishStreamCall(state, observer, &range, 1u, result > 0 ? static_cast<std::size_t>(result) : 0u);
    }
    afterCall.Restore();
    return result;
}

int WSAAPI HookedWsaRecv(SOCKET                             socket,
                         LPWSABUF                           buffers,
                         DWORD                              bufferCount,
                         LPDWORD                            bytesReceived,
                         LPDWORD                            flags,
                         LPWSAOVERLAPPED                    overlapped,
                         LPWSAOVERLAPPED_COMPLETION_ROUTINE completionRoutine)
{
    HookCallScope         scope;
    const LastErrorState  beforeCall;
    const WsaRecvFunction original =
        gOriginalWsaRecv.load(std::memory_order_acquire);
    if (original == nullptr)
    {
        WSASetLastError(WSAEFAULT);
        return SOCKET_ERROR;
    }

    PacketObserver* const                    observer       = gObserver.load(std::memory_order_acquire);
    DWORD                                    requestedFlags = 0;
    const bool                               flagsCaptured  = flags != nullptr && SafeRead(flags, requestedFlags);
    const bool                               peek           = flagsCaptured && (requestedFlags & MSG_PEEK) != 0u;
    std::array<BufferRange, kMaximumWsabufs> ranges{};
    const bool                               observing            = scope.IsOutermost() && observer != nullptr;
    const bool                               synchronousCandidate = overlapped == nullptr &&
                                                                    completionRoutine == nullptr &&
                                                                    bytesReceived != nullptr && flagsCaptured &&
                                                                    !peek;
    const bool                               rangesCaptured       = observing && synchronousCandidate &&
                                                                    CaptureWsabufs(buffers, bufferCount, ranges);
    std::shared_ptr<StreamState>             state;
    if (rangesCaptured)
    {
        state = StartObservedStreamCall(observer, true, socket);
    }
    beforeCall.Restore();
    const int            result = original(socket,
                                           buffers,
                                           bufferCount,
                                           bytesReceived,
                                           flags,
                                           overlapped,
                                           completionRoutine);
    const LastErrorState afterCall;
    if (state)
    {
        DWORD      transferred = 0;
        const bool wouldBlock  = result == SOCKET_ERROR &&
                                 afterCall.winsock == WSAEWOULDBLOCK;
        if (result != 0)
        {
            if (!wouldBlock)
            {
                DisableSocketStream(socket, Direction::Incoming);
            }
            FinishStreamCall(state,
                             observer,
                             ranges.data(),
                             bufferCount,
                             0u);
        }
        else if (!SafeRead(bytesReceived, transferred))
        {
            DisableSocketStream(socket, Direction::Incoming);
            FinishStreamCall(state,
                             observer,
                             ranges.data(),
                             bufferCount,
                             0u);
        }
        else
        {
            FinishStreamCall(state,
                             observer,
                             ranges.data(),
                             bufferCount,
                             transferred);
        }
    }
    else if (observing && !peek &&
             (!synchronousCandidate || !rangesCaptured))
    {
        DisableSocketStream(socket, Direction::Incoming);
    }
    afterCall.Restore();
    return result;
}

int WSAAPI HookedWsaSend(SOCKET                             socket,
                         LPWSABUF                           buffers,
                         DWORD                              bufferCount,
                         LPDWORD                            bytesSent,
                         DWORD                              flags,
                         LPWSAOVERLAPPED                    overlapped,
                         LPWSAOVERLAPPED_COMPLETION_ROUTINE completionRoutine)
{
    HookCallScope         scope;
    const LastErrorState  beforeCall;
    const WsaSendFunction original = gOriginalWsaSend.load(std::memory_order_acquire);
    if (original == nullptr)
    {
        WSASetLastError(WSAEFAULT);
        return SOCKET_ERROR;
    }
    PacketObserver* const                    observer = gObserver.load(std::memory_order_acquire);
    std::array<BufferRange, kMaximumWsabufs> ranges{};
    const bool                               observing            = scope.IsOutermost() && observer != nullptr;
    const bool                               synchronousCandidate = overlapped == nullptr &&
                                                                    completionRoutine == nullptr && bytesSent != nullptr;
    const bool                               rangesCaptured       = observing && synchronousCandidate &&
                                                                    CaptureWsabufs(buffers, bufferCount, ranges);
    const auto                               state                = rangesCaptured
                                                                        ? StartObservedStreamCall(observer, true, socket, Direction::Outgoing)
                                                                        : std::shared_ptr<StreamState>{};
    beforeCall.Restore();
    const int            result = original(socket, buffers, bufferCount, bytesSent, flags, overlapped, completionRoutine);
    const LastErrorState afterCall;
    if (state)
    {
        DWORD      transferred = 0;
        const bool wouldBlock  = result == SOCKET_ERROR &&
                                 afterCall.winsock == WSAEWOULDBLOCK;
        if (result != 0)
        {
            if (!wouldBlock)
            {
                DisableSocketStream(socket, Direction::Outgoing);
            }
            FinishStreamCall(state, observer, ranges.data(), bufferCount, 0u);
        }
        else if (!SafeRead(bytesSent, transferred))
        {
            DisableSocketStream(socket, Direction::Outgoing);
            FinishStreamCall(state, observer, ranges.data(), bufferCount, 0u);
        }
        else
        {
            FinishStreamCall(state, observer, ranges.data(), bufferCount, transferred);
        }
    }
    else if (observing && (!synchronousCandidate || !rangesCaptured))
    {
        DisableSocketStream(socket, Direction::Outgoing);
    }
    afterCall.Restore();
    return result;
}

int WSAAPI HookedCloseSocket(SOCKET socket)
{
    HookCallScope             scope;
    const LastErrorState      beforeCall;
    const CloseSocketFunction original =
        gOriginalCloseSocket.load(std::memory_order_acquire);
    if (original == nullptr)
    {
        WSASetLastError(WSAEFAULT);
        return SOCKET_ERROR;
    }

    beforeCall.Restore();
    const int            result = original(socket);
    const LastErrorState afterCall;
    if (scope.IsOutermost() && result == 0)
    {
        EraseSocketStreams(socket);
    }
    afterCall.Restore();
    return result;
}

} // namespace

} // namespace packet_observer
