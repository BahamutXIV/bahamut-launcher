#pragma once

#include "packet_capture_policy.h"
#include "packet_observer.h"

#include <chrono>
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <memory>
#include <optional>
#include <span>
#include <string>
#include <string_view>

namespace packet_capture
{

namespace detail
{

struct WriterState;

// Test entry points for the handle-relative delete and probe helpers.
bool DeleteCaptureFileForTest(const std::filesystem::path& directory, std::wstring_view name);
bool HasDataRowForTest(const std::filesystem::path& directory, std::wstring_view name);

} // namespace detail

// Why a part ended; written to its end row.
enum class EndReason : std::uint8_t
{
    Stop,
    Disable,
    Rotate,
    Split,
    Fault,
};

// Replaces commas and every byte outside printable ASCII with a space, trims
// the ends, and truncates to `maxLength`, so the value fits one CSV column.
std::string SanitizeField(std::string_view value, std::size_t maxLength = 128);

// Session metadata written at the top of every part.
struct PacketCaptureIdentity
{
    std::string   launcher   = "unknown"; // Sanitized; empty becomes "unknown".
    std::uint32_t runtimeApi = 0;
};

struct PacketCaptureLimits
{
    std::uint64_t maxPartBytes    = 8u * 1024u * 1024u;
    std::uint64_t maxTotalBytes   = 256u * 1024u * 1024u;
    std::size_t   maxSessions     = 10;
    std::size_t   maxQueuedFrames = 4096;
    std::size_t   maxQueuedBytes  = 4u * 1024u * 1024u;
};

struct PacketCaptureStatus
{
    bool          recording = false;
    std::string   file;    // Current part filename only; kept after a stop.
    std::string   session; // 16 lowercase hex characters; kept after a stop.
    unsigned int  part             = 0;
    std::uint64_t captured         = 0;
    std::uint64_t written          = 0;
    std::uint64_t dropped          = 0;
    std::uint64_t skipped          = 0; // Frames the capture scope excluded.
    std::size_t   queued           = 0;
    std::size_t   queue_high_water = 0;
    std::uint64_t bytes_on_disk    = 0; // Capture files after the last scan plus bytes written since.
    std::uint64_t prune_failures   = 0; // Retention deletions that failed and were skipped.
    std::string   error;                // Empty unless capture stopped after a write error.
};

// Frames and commands share one ordered queue drained by a detached thread
// that holds its own reference to the shared state.
class PacketCaptureWriter
{
public:
    explicit PacketCaptureWriter(std::filesystem::path directory,
                                 PacketCaptureLimits   limits   = {},
                                 PacketCaptureIdentity identity = {});
    ~PacketCaptureWriter();

    PacketCaptureWriter(const PacketCaptureWriter&)            = delete;
    PacketCaptureWriter& operator=(const PacketCaptureWriter&) = delete;

    // Start and Stop wait up to `wait` for the writer thread and return its
    // result. A zero wait only enqueues the command and returns true.
    bool Start(std::chrono::milliseconds wait = std::chrono::milliseconds(2000));
    bool Stop(std::chrono::milliseconds wait = std::chrono::milliseconds(2000));
    bool Stop(EndReason reason, std::chrono::milliseconds wait = std::chrono::milliseconds(2000));

    // Closes the current part with a `split` end row and continues in the next
    // part of the same session. False when not recording or when the split
    // failed (a failure follows the write-error policy).
    bool Split(std::chrono::milliseconds wait = std::chrono::milliseconds(2000));

    // Hook thread. False when not recording or when the frame was dropped.
    bool Enqueue(packet_observer::Direction            direction,
                 std::chrono::system_clock::time_point capturedAt,
                 std::span<const std::uint8_t>         frame);

    // Hook thread. Writes a stub row: `payload` replaces the hex column and
    // `frameSize` is the original frame size. Budgeted by payload length.
    bool EnqueueStub(packet_observer::Direction            direction,
                     std::chrono::system_clock::time_point capturedAt,
                     std::size_t                           frameSize,
                     std::string_view                      payload);

    // Hook thread. Writes a zone row with a sanitized name. False when not
    // recording or when the queue is far over its bound; never blocks on I/O.
    bool EnqueueZone(std::chrono::system_clock::time_point capturedAt,
                     std::uint32_t                         zoneId,
                     std::string_view                      name);

    // Hook thread. Counts a frame the capture scope excluded.
    void CountSkipped();

    // The scope is stored for the session metadata rows.
    void  SetScope(Scope scope);
    Scope GetScope() const;

    // True once the queue is drained and written.
    bool WaitIdle(std::chrono::milliseconds timeout);

    PacketCaptureStatus        Status() const;
    std::optional<std::string> TakeNotice();

    // Test hooks.
    void SetPausedForTest(bool paused);
    void FailNextWritesForTest(int count);

private:
    std::shared_ptr<detail::WriterState> state_;
};

} // namespace packet_capture
