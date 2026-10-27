#include "packet_capture.h"

#include <windows.h>

#include <algorithm>
#include <array>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <regex>
#include <string>
#include <string_view>
#include <thread>
#include <vector>

namespace
{

using packet_capture::EndReason;
using packet_capture::PacketCaptureIdentity;
using packet_capture::PacketCaptureLimits;
using packet_capture::PacketCaptureWriter;
using packet_capture::SanitizeField;
using packet_capture::Scope;
using packet_observer::Direction;
using namespace std::chrono_literals;

constexpr std::string_view kHeader   = "capture_unix_ms_utc,direction,size,frame_hex";
constexpr auto             kWaitIdle = 10s;

int g_failures = 0;

bool Expect(bool condition, const char* message)
{
    if (!condition)
    {
        std::cerr << "FAIL: " << message << '\n';
        ++g_failures;
    }
    return condition;
}

class TempDirectory
{
public:
    TempDirectory()
    : path_(std::filesystem::temp_directory_path() /
            ("bahamut-packet-capture-tests-" + std::to_string(GetCurrentProcessId())))
    {
        std::error_code error;
        std::filesystem::remove_all(path_, error);
        std::filesystem::create_directories(path_);
    }

    ~TempDirectory()
    {
        std::error_code error;
        std::filesystem::remove_all(path_, error);
    }

    TempDirectory(const TempDirectory&)            = delete;
    TempDirectory& operator=(const TempDirectory&) = delete;

    std::filesystem::path Sub(const char* name) const
    {
        const auto directory = path_ / name;
        std::filesystem::create_directories(directory);
        return directory;
    }

private:
    std::filesystem::path path_;
};

std::vector<std::filesystem::path> CsvFiles(const std::filesystem::path& directory)
{
    std::vector<std::filesystem::path> files;
    for (const auto& entry : std::filesystem::directory_iterator(directory))
    {
        files.push_back(entry.path());
    }
    std::sort(files.begin(), files.end());
    return files;
}

std::string ReadAll(const std::filesystem::path& path)
{
    std::ifstream file(path, std::ios::binary);
    return std::string(std::istreambuf_iterator<char>(file), {});
}

std::vector<std::string> Lines(const std::string& text)
{
    std::vector<std::string> lines;
    std::size_t              start = 0;
    while (start < text.size())
    {
        const std::size_t end = text.find('\n', start);
        if (end == std::string::npos)
        {
            lines.push_back(text.substr(start) + "<unterminated>");
            break;
        }
        lines.push_back(text.substr(start, end - start));
        start = end + 1;
    }
    return lines;
}

struct Parsed
{
    std::vector<std::string> all;
    std::vector<std::string> data; // incoming, outgoing and stub rows
    std::vector<std::string> meta;
    std::vector<std::string> zone;
    std::vector<std::string> end;
};

std::string Kind(const std::string& row)
{
    const std::size_t first = row.find(',');
    if (first == std::string::npos)
    {
        return {};
    }
    const std::size_t second = row.find(',', first + 1);
    return row.substr(first + 1, second == std::string::npos ? std::string::npos : second - first - 1);
}

Parsed Parse(const std::string& text)
{
    Parsed parsed;
    parsed.all = Lines(text);
    for (std::size_t index = 1; index < parsed.all.size(); ++index)
    {
        const std::string& row  = parsed.all[index];
        const std::string  kind = Kind(row);
        if (kind == "meta")
        {
            parsed.meta.push_back(row);
        }
        else if (kind == "zone")
        {
            parsed.zone.push_back(row);
        }
        else if (kind == "end")
        {
            parsed.end.push_back(row);
        }
        else
        {
            parsed.data.push_back(row);
        }
    }
    return parsed;
}

// Value of a meta row, or "<missing>".
std::string MetaValue(const Parsed& parsed, const std::string& key)
{
    const std::string needle = ",meta,0," + key + "=";
    for (const auto& row : parsed.meta)
    {
        const std::size_t at = row.find(needle);
        if (at != std::string::npos)
        {
            return row.substr(at + needle.size());
        }
    }
    return "<missing>";
}

std::string MetaKeys(const Parsed& parsed)
{
    std::string keys;
    for (const auto& row : parsed.meta)
    {
        const std::size_t start = row.find(",meta,0,") + 8;
        keys += row.substr(start, row.find('=', start) - start) + " ";
    }
    return keys;
}

std::chrono::system_clock::time_point TimeMs(std::int64_t milliseconds)
{
    return std::chrono::system_clock::time_point(std::chrono::milliseconds(milliseconds));
}

constexpr std::int64_t kBaseMs = 1700000000000;

std::string Hex(const std::vector<std::uint8_t>& bytes)
{
    static constexpr char digits[] = "0123456789ABCDEF";
    std::string           text;
    for (const std::uint8_t byte : bytes)
    {
        text.push_back(digits[byte >> 4u]);
        text.push_back(digits[byte & 0x0fu]);
    }
    return text;
}

std::vector<std::uint8_t> NumberedFrame(int index)
{
    return { static_cast<std::uint8_t>(index & 0xff), static_cast<std::uint8_t>((index >> 8) & 0xff), 0xAB };
}

std::string ExpectedRow(int index)
{
    return std::to_string(kBaseMs + index) + (index % 2 == 0 ? ",incoming,3," : ",outgoing,3,") + Hex(NumberedFrame(index));
}

bool EnqueueNumbered(PacketCaptureWriter& writer, int index)
{
    const auto frame = NumberedFrame(index);
    return writer.Enqueue(index % 2 == 0 ? Direction::Incoming : Direction::Outgoing, TimeMs(kBaseMs + index), frame);
}

// The header and meta rows of a fresh part, measured from a real session.
std::uint64_t PrologueBytes(const TempDirectory& root)
{
    static std::uint64_t cached = 0;
    if (cached == 0)
    {
        const auto          directory = root.Sub("prologue-probe");
        PacketCaptureWriter writer(directory);
        if (writer.Start())
        {
            cached = std::filesystem::file_size(directory / writer.Status().file);
        }
        writer.Stop();
    }
    return cached;
}

// The writer keeps this much of a part free for the end row.
constexpr std::uint64_t kEndRowReserve = 256;
constexpr std::uint64_t kDataRowBytes  = 32; // A 3-byte numbered frame row.

// A part cap that holds exactly `rows` numbered frame rows.
std::uint64_t PartLimit(const TempDirectory& root, int rows)
{
    return PrologueBytes(root) + kEndRowReserve + kDataRowBytes * static_cast<std::uint64_t>(rows);
}

void ExpectMeta(const Parsed& parsed, const std::string& session, unsigned int part, const char* scope, const char* what)
{
    Expect(!parsed.all.empty() && parsed.all[0] == kHeader, what);
    Expect(MetaKeys(parsed) == "format session part launcher runtime_api client os wine scope ", what);
    Expect(MetaValue(parsed, "format") == "2" && MetaValue(parsed, "session") == session &&
               MetaValue(parsed, "part") == std::to_string(part) && MetaValue(parsed, "client") == "2012.09.19.0001" &&
               MetaValue(parsed, "scope") == scope,
           what);
    Expect(std::regex_match(session, std::regex("[0-9a-f]{16}")), "session id is not 16 lowercase hex");
    Expect(std::regex_match(MetaValue(parsed, "os"), std::regex(R"(windows-\d+\.\d+\.\d+)")), "os field");
    Expect(MetaValue(parsed, "wine") != "<missing>" && MetaValue(parsed, "wine").find(',') == std::string::npos, "wine field");
}

void TestOrderingAndSharing(const TempDirectory& root)
{
    const auto          directory = root.Sub("ordering");
    PacketCaptureWriter writer(directory);
    if (!Expect(writer.Start(), "start failed"))
    {
        return;
    }
    const std::regex grammar(R"(capture-\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}\.\d{3}Z(-\d{1,3})?\.csv)");
    const auto       files = CsvFiles(directory);
    Expect(files.size() == 1 && std::regex_match(files[0].filename().string(), grammar),
           "session file name does not match the capture grammar");
    Expect(writer.Status().recording && writer.Status().part == 1 &&
               writer.Status().file == files[0].filename().string(),
           "status after start");
    constexpr int count = 2000;
    for (int index = 0; index < count; ++index)
    {
        Expect(EnqueueNumbered(writer, index), "enqueue rejected a frame within the bounds");
    }
    Expect(writer.WaitIdle(kWaitIdle), "queue did not drain");
    // The writer holds the file open; a reader must still be able to open it.
    const auto parsed = Parse(ReadAll(files[0]));
    ExpectMeta(parsed, writer.Status().session, 1, "standard", "meta rows after the header");
    Expect(MetaValue(parsed, "launcher") == "unknown" && MetaValue(parsed, "runtime_api") == "0", "default identity");
    if (Expect(parsed.data.size() == static_cast<std::size_t>(count), "row count"))
    {
        bool exact = true;
        for (int index = 0; index < count; ++index)
        {
            exact = exact && parsed.data[static_cast<std::size_t>(index)] == ExpectedRow(index);
        }
        Expect(exact, "rows are not exact and in order");
    }
    Expect(parsed.end.empty() && parsed.all.size() == parsed.data.size() + parsed.meta.size() + 1,
           "a recording part must not have an end row yet");
    const auto status = writer.Status();
    Expect(status.captured == count && status.written == count && status.dropped == 0 &&
               status.queued == 0 && status.queue_high_water >= 1 && status.error.empty(),
           "counters after drain");
    Expect(!writer.TakeNotice().has_value(), "unexpected notice");
    Expect(!writer.Enqueue(Direction::Incoming, TimeMs(kBaseMs), std::span<const std::uint8_t>()), "empty frame accepted");
    Expect(writer.Start() && CsvFiles(directory).size() == 1, "start while recording must not create a session");
    Expect(writer.Stop(), "stop of an active session");
    {
        const auto stopped = Parse(ReadAll(files[0]));
        Expect(stopped.end.size() == 1 &&
                   Kind(stopped.end[0]) == "end" &&
                   stopped.end[0].find(",end,0,reason=stop;captured=2000;written=2000;skipped=0;dropped=0;queue_high_water=") !=
                       std::string::npos &&
                   stopped.all.back() == stopped.end[0],
               "stop end row");
    }
    Expect(!writer.Stop(), "stop of an inactive writer");
    Expect(!writer.Status().recording && writer.Status().file == files[0].filename().string(), "status after stop");
    Expect(!EnqueueNumbered(writer, 0), "frame accepted after stop");
    Expect(writer.Status().captured == count, "rejected frame was counted as captured");
}

void TestStopDrainsIntoOldSession(const TempDirectory& root)
{
    const auto          directory = root.Sub("stop-drain");
    PacketCaptureWriter writer(directory);
    if (!Expect(writer.Start(), "start failed"))
    {
        return;
    }
    const std::string firstName = writer.Status().file;
    writer.SetPausedForTest(true);
    for (int index = 0; index < 10; ++index)
    {
        Expect(EnqueueNumbered(writer, index), "enqueue before stop");
    }
    Expect(writer.Stop(0ms), "async stop");
    Expect(!EnqueueNumbered(writer, 99), "frame accepted after async stop");
    Expect(writer.Start(0ms), "async start");
    for (int index = 100; index < 105; ++index)
    {
        Expect(EnqueueNumbered(writer, index), "enqueue after start");
    }
    writer.SetPausedForTest(false);
    Expect(writer.WaitIdle(kWaitIdle), "queue did not drain");
    const auto files      = CsvFiles(directory);
    const auto secondName = writer.Status().file;
    if (Expect(files.size() == 2 && firstName != secondName, "a stop followed by a start must create a second file"))
    {
        const auto first  = Parse(ReadAll(directory / firstName));
        const auto second = Parse(ReadAll(directory / secondName));
        bool       exact  = first.data.size() == 10 && second.data.size() == 5;
        for (int index = 0; exact && index < 10; ++index)
        {
            exact = first.data[static_cast<std::size_t>(index)] == ExpectedRow(index);
        }
        for (int index = 0; exact && index < 5; ++index)
        {
            exact = second.data[static_cast<std::size_t>(index)] == ExpectedRow(100 + index);
        }
        Expect(exact, "queued frames must stay in the session they were queued in");
        Expect(first.end.size() == 1 && first.end[0].find("reason=stop;captured=15;written=10;") != std::string::npos &&
                   MetaValue(first, "session") != MetaValue(second, "session") && second.end.empty(),
               "a restart must start a new session and only the stopped part has an end row");
    }
    const auto status = writer.Status();
    Expect(status.recording && status.captured == 15 && status.written == 15 && status.file == secondName,
           "status after restart");
}

void TestRotation(const TempDirectory& root)
{
    const auto          directory = root.Sub("rotation");
    PacketCaptureLimits limits;
    limits.maxPartBytes = PartLimit(root, 8);
    PacketCaptureWriter writer(directory, limits);
    if (!Expect(writer.Start(), "start failed"))
    {
        return;
    }
    const std::string firstName = writer.Status().file;
    const std::string session   = writer.Status().session;
    constexpr int     count     = 40;
    for (int index = 0; index < count; ++index)
    {
        Expect(EnqueueNumbered(writer, index), "enqueue in rotation test");
    }
    // One row larger than a whole part is still written, alone, never split.
    const std::vector<std::uint8_t> large(400, 0x5A);
    Expect(writer.Enqueue(Direction::Incoming, TimeMs(kBaseMs + 1000), large), "large frame");
    Expect(writer.Enqueue(Direction::Incoming, TimeMs(kBaseMs + 1001), large), "second large frame");
    Expect(writer.WaitIdle(kWaitIdle), "queue did not drain");
    const auto        status   = writer.Status();
    const std::string baseName = std::filesystem::path(firstName).stem().string();
    Expect(status.part >= 5 && CsvFiles(directory).size() == status.part, "rotation produced the wrong number of parts");
    std::vector<std::filesystem::path> files;
    for (unsigned int part = 1; part <= status.part; ++part)
    {
        char suffix[32]{};
        std::snprintf(suffix, sizeof(suffix), "-part%04u", part);
        files.push_back(directory / (baseName + (part == 1 ? "" : suffix) + ".csv"));
    }
    Expect(writer.Stop(), "stop after rotation");
    std::vector<std::string> rows;
    bool                     partsOk = true;
    for (std::size_t number = 0; number < files.size(); ++number)
    {
        const std::string text   = ReadAll(files[number]);
        const auto        parsed = Parse(text);
        partsOk                  = partsOk && !parsed.all.empty() && parsed.all[0] == kHeader && !text.empty() && text.back() == '\n';
        // A frame larger than a whole part is written alone.
        const bool wholePart = parsed.data.size() == 1 && parsed.data[0].find(",400,") != std::string::npos;
        partsOk              = partsOk && (text.size() <= limits.maxPartBytes || wholePart);
        // The header and meta rows are part of every part and count toward its size.
        partsOk         = partsOk && MetaKeys(parsed) == "format session part launcher runtime_api client os wine scope " &&
                          MetaValue(parsed, "part") == std::to_string(number + 1) && MetaValue(parsed, "session") == session;
        const bool last = number + 1 == files.size();
        partsOk         = partsOk && parsed.end.size() == 1 && parsed.all.back() == parsed.end[0] &&
                          parsed.end[0].find(last ? ",end,0,reason=stop;" : ",end,0,reason=rotate;") != std::string::npos;
        for (std::size_t index = 1; index < parsed.all.size(); ++index)
        {
            partsOk = partsOk && std::count(parsed.all[index].begin(), parsed.all[index].end(), ',') == 3;
        }
        rows.insert(rows.end(), parsed.data.begin(), parsed.data.end());
    }
    Expect(partsOk, "a part lacks its header, meta rows or end row, exceeds its bound, or holds a split row");
    bool exact = rows.size() == count + 2;
    for (int index = 0; exact && index < count; ++index)
    {
        exact = rows[static_cast<std::size_t>(index)] == ExpectedRow(index);
    }
    Expect(exact, "rows across parts are not exact and in order");
    Expect(status.file == files.back().filename().string() && status.written == count + 2,
           "status after rotation");
}

void TestQueueBounds(const TempDirectory& root)
{
    {
        const auto          directory = root.Sub("bounds-frames");
        PacketCaptureLimits limits;
        limits.maxQueuedFrames = 8;
        PacketCaptureWriter writer(directory, limits);
        if (!Expect(writer.Start(), "start failed"))
        {
            return;
        }
        writer.SetPausedForTest(true);
        int accepted = 0;
        for (int index = 0; index < 12; ++index)
        {
            accepted += EnqueueNumbered(writer, index) ? 1 : 0;
        }
        auto status = writer.Status();
        Expect(accepted == 8 && status.captured == 12 && status.dropped == 4 &&
                   status.queued == 8 && status.queue_high_water == 8,
               "frame-count bound");
        Expect(!writer.WaitIdle(20ms), "a paused writer must not report idle");
        writer.SetPausedForTest(false);
        Expect(writer.WaitIdle(kWaitIdle), "queue did not drain");
        status = writer.Status();
        Expect(status.written == 8 && status.dropped == 4 && status.queued == 0 && status.queue_high_water == 8,
               "counters after the bound test");
        Expect(EnqueueNumbered(writer, 50), "queue must accept frames again after draining");
    }
    {
        const auto          directory = root.Sub("bounds-bytes");
        PacketCaptureLimits limits;
        limits.maxQueuedBytes = 8;
        PacketCaptureWriter writer(directory, limits);
        if (!Expect(writer.Start(), "start failed"))
        {
            return;
        }
        writer.SetPausedForTest(true);
        const bool first  = EnqueueNumbered(writer, 0);
        const bool second = EnqueueNumbered(writer, 1);
        const bool third  = EnqueueNumbered(writer, 2);
        Expect(first && second && !third && writer.Status().dropped == 1, "payload byte bound");
        writer.SetPausedForTest(false);
        Expect(writer.WaitIdle(kWaitIdle) && writer.Status().written == 2, "byte bound drain");
    }
}

void TestWriteFailures(const TempDirectory& root)
{
    const auto          directory = root.Sub("failures");
    PacketCaptureWriter writer(directory);
    if (!Expect(writer.Start(), "start failed"))
    {
        return;
    }
    const std::string firstFile = writer.Status().file;

    writer.FailNextWritesForTest(1);
    Expect(EnqueueNumbered(writer, 0), "enqueue before first failure");
    Expect(writer.WaitIdle(kWaitIdle), "queue did not drain after first failure");
    auto status = writer.Status();
    Expect(status.recording && status.file != firstFile && status.part == 1 && status.error.empty() &&
               status.dropped == 1 && status.written == 0,
           "first failure must start a fresh session");
    const auto notice = writer.TakeNotice();
    Expect(notice && *notice == "Packet capture: recording again after a save error", "restart notice");
    Expect(!writer.TakeNotice().has_value(), "restart notice was repeated");
    Expect(EnqueueNumbered(writer, 1), "enqueue after restart");
    Expect(writer.WaitIdle(kWaitIdle) && writer.Status().written == 1, "frame after restart was not written");
    Expect(CsvFiles(directory).size() == 1 && Parse(ReadAll(directory / writer.Status().file)).data.size() == 1,
           "restart session file");

    writer.FailNextWritesForTest(1);
    Expect(EnqueueNumbered(writer, 2), "enqueue before second failure");
    Expect(writer.WaitIdle(kWaitIdle), "queue did not drain after second failure");
    status = writer.Status();
    Expect(!status.recording && !status.error.empty() && status.dropped == 2 && status.written == 1,
           "second failure must stop recording");
    const auto stopNotice = writer.TakeNotice();
    Expect(stopNotice && *stopNotice == "Packet capture: stopped after a save error", "stop notice");
    Expect(!EnqueueNumbered(writer, 3), "frame accepted after a write-error stop");
    Expect(!writer.Stop(), "stop after a write-error stop");

    Expect(writer.Start() && writer.Status().recording && writer.Status().error.empty(),
           "an explicit start must recover from a write-error stop");
}

void TestStartAfterFailureKeepsAcceptance(const TempDirectory& root)
{
    const auto          directory = root.Sub("failure-then-start");
    PacketCaptureWriter writer(directory);
    if (!Expect(writer.Start(), "start failed"))
    {
        return;
    }
    writer.SetPausedForTest(true);
    writer.FailNextWritesForTest(2);
    const std::vector<std::uint8_t> large(65536, 0x11);
    Expect(writer.Enqueue(Direction::Incoming, TimeMs(kBaseMs), large), "first large frame");
    Expect(writer.Enqueue(Direction::Incoming, TimeMs(kBaseMs + 1), large), "second large frame");
    Expect(writer.Start(0ms), "queued start");
    writer.SetPausedForTest(false);
    Expect(writer.WaitIdle(kWaitIdle), "queue did not drain");
    const auto status = writer.Status();
    Expect(status.recording && status.error.empty() && status.dropped == 2,
           "a later start must recover from the earlier failure");
    Expect(EnqueueNumbered(writer, 3), "a recording writer must accept frames");
    Expect(writer.WaitIdle(kWaitIdle) && writer.Status().written == 1, "frame after recovery");
}

void TestStopFlushFailure(const TempDirectory& root)
{
    const auto          directory = root.Sub("stop-flush-failure");
    PacketCaptureWriter writer(directory);
    if (!Expect(writer.Start(), "start failed"))
    {
        return;
    }
    writer.SetPausedForTest(true);
    Expect(EnqueueNumbered(writer, 0), "enqueue before stop");
    Expect(writer.Stop(0ms), "async stop");
    writer.FailNextWritesForTest(1);
    writer.SetPausedForTest(false);
    Expect(writer.WaitIdle(kWaitIdle), "queue did not drain");
    const auto status = writer.Status();
    Expect(!status.recording && !status.error.empty() && status.written == 0 && status.dropped == 1,
           "a failed flush at stop must record the error");
    const auto notice = writer.TakeNotice();
    Expect(notice && *notice == "Packet capture: stopped after a save error", "stop flush notice");
    Expect(!EnqueueNumbered(writer, 1), "frame accepted after stop");
}

// Pre-existing sessions use 2020 stamps, so they are always older than a new one.
std::string OldName(int second, unsigned int suffix = 0, unsigned int part = 1)
{
    char name[96]{};
    std::snprintf(name, sizeof(name), "capture-2020-01-01T00-00-%02d.000Z", second);
    std::string result = name;
    if (suffix != 0)
    {
        result += "-" + std::to_string(suffix);
    }
    if (part != 1)
    {
        std::snprintf(name, sizeof(name), "-part%04u", part);
        result += name;
    }
    return result + ".csv";
}

std::filesystem::path MakeCapture(const std::filesystem::path& directory, const std::string& name, bool dataRows, int rows = 1)
{
    std::string text = std::string(kHeader) + "\n";
    for (int index = 0; dataRows && index < rows; ++index)
    {
        text += "1700000000000,incoming,3,00A5FF\n";
    }
    const auto path = directory / name;
    std::ofstream(path, std::ios::binary) << text;
    return path;
}

bool Has(const std::filesystem::path& directory, const std::string& name)
{
    return std::filesystem::exists(directory / name);
}

std::uint64_t DirectoryBytes(const std::filesystem::path& directory)
{
    std::uint64_t total = 0;
    for (const auto& entry : std::filesystem::directory_iterator(directory))
    {
        if (entry.is_regular_file())
        {
            total += std::filesystem::file_size(entry.path());
        }
    }
    return total;
}

void TestRetentionSessionCount(const TempDirectory& root)
{
    const auto directory = root.Sub("retention-count");
    for (int second = 1; second <= 5; ++second)
    {
        MakeCapture(directory, OldName(second), true, 3);
    }
    PacketCaptureLimits limits;
    limits.maxSessions = 3;
    PacketCaptureWriter writer(directory, limits);
    Expect(writer.Start(), "start failed");
    Expect(!Has(directory, OldName(1)) && !Has(directory, OldName(2)) && !Has(directory, OldName(3)) &&
               Has(directory, OldName(4)) && Has(directory, OldName(5)) && Has(directory, writer.Status().file),
           "session count cap keeps the newest sessions and the active one");
    Expect(CsvFiles(directory).size() == 3, "session count cap");
}

void TestRetentionBytes(const TempDirectory& root)
{
    const auto directory = root.Sub("retention-bytes");
    for (int second = 1; second <= 4; ++second)
    {
        MakeCapture(directory, OldName(second), true, 60);
    }
    PacketCaptureLimits limits;
    limits.maxTotalBytes = 5000;
    PacketCaptureWriter writer(directory, limits);
    Expect(writer.Start(), "start failed");
    Expect(!Has(directory, OldName(1)) && !Has(directory, OldName(2)) && Has(directory, OldName(3)) &&
               Has(directory, OldName(4)),
           "byte cap removes the oldest whole sessions first");
    Expect(DirectoryBytes(directory) <= limits.maxTotalBytes, "byte cap respected");
    Expect(writer.Status().bytes_on_disk == DirectoryBytes(directory), "bytes_on_disk after the scan");
    Expect(EnqueueNumbered(writer, 1) && writer.WaitIdle(kWaitIdle), "enqueue");
    Expect(writer.Status().bytes_on_disk == DirectoryBytes(directory), "bytes_on_disk after a write");
}

void TestRetentionActivePart(const TempDirectory& root)
{
    const auto directory = root.Sub("retention-active");
    // Each part holds its header, meta rows and four 32-byte rows.
    PacketCaptureLimits limits;
    limits.maxPartBytes  = PartLimit(root, 4);
    limits.maxTotalBytes = 100;
    PacketCaptureWriter writer(directory, limits);
    Expect(writer.Start(), "start failed");
    for (int index = 0; index < 20; ++index)
    {
        Expect(EnqueueNumbered(writer, index), "enqueue");
    }
    Expect(writer.WaitIdle(kWaitIdle), "queue did not drain");
    const auto status = writer.Status();
    const auto files  = CsvFiles(directory);
    Expect(status.part >= 5 && files.size() == 1 && files[0].filename().string() == status.file &&
               std::filesystem::file_size(files[0]) > limits.maxTotalBytes,
           "the active part survives a cap it alone exceeds and older parts are pruned");
}

void TestRetentionOldestPartsFirst(const TempDirectory& root)
{
    const auto          directory = root.Sub("retention-parts");
    PacketCaptureLimits limits;
    limits.maxPartBytes  = PartLimit(root, 4);
    limits.maxTotalBytes = 3 * limits.maxPartBytes;
    PacketCaptureWriter writer(directory, limits);
    Expect(writer.Start(), "start failed");
    const std::string base = std::filesystem::path(writer.Status().file).stem().string();
    for (int index = 0; index < 22; ++index)
    {
        Expect(EnqueueNumbered(writer, index), "enqueue");
    }
    Expect(writer.WaitIdle(kWaitIdle), "queue did not drain");
    const auto part = [&](unsigned int number)
    {
        char suffix[32]{};
        std::snprintf(suffix, sizeof(suffix), "-part%04u", number);
        return base + (number == 1 ? "" : suffix) + ".csv";
    };
    Expect(writer.Status().part == 6 && !Has(directory, part(1)) && !Has(directory, part(2)) &&
               Has(directory, part(3)) && Has(directory, part(4)) && Has(directory, part(5)) && Has(directory, part(6)),
           "the active session's oldest parts are pruned first");
}

void TestRetentionRotationPrunes(const TempDirectory& root)
{
    const auto          directory = root.Sub("retention-rotation");
    PacketCaptureLimits limits;
    limits.maxPartBytes = PartLimit(root, 4);
    limits.maxSessions  = 2;
    PacketCaptureWriter writer(directory, limits);
    Expect(writer.Start(), "start failed");
    for (int second = 1; second <= 3; ++second)
    {
        MakeCapture(directory, OldName(second), true, 2);
    }
    for (int index = 0; index < 6; ++index)
    {
        Expect(EnqueueNumbered(writer, index), "enqueue");
    }
    Expect(writer.WaitIdle(kWaitIdle), "queue did not drain");
    Expect(writer.Status().part == 2 && !Has(directory, OldName(1)) && !Has(directory, OldName(2)) &&
               Has(directory, OldName(3)),
           "a rotation runs retention");
}

void TestRetentionEmptySessions(const TempDirectory& root)
{
    const auto directory = root.Sub("retention-empty");
    MakeCapture(directory, OldName(1), false);
    MakeCapture(directory, OldName(2), true);
    MakeCapture(directory, OldName(3), false);
    // A multi-part session is never an empty single-part session.
    MakeCapture(directory, OldName(4), false);
    MakeCapture(directory, OldName(4, 0, 2), false);
    PacketCaptureWriter writer(directory);
    Expect(writer.Start(), "start failed");
    Expect(!Has(directory, OldName(1)) && Has(directory, OldName(2)) && !Has(directory, OldName(3)) &&
               Has(directory, OldName(4)) && Has(directory, OldName(4, 0, 2)),
           "empty single-part sessions are pruned and others are kept");
    const std::string active = writer.Status().file;
    Expect(Has(directory, active), "active session exists");
    Expect(writer.Stop() && !Has(directory, active), "a session that wrote no rows deletes itself at stop");
    Expect(writer.Start(), "second start");
    const std::string second = writer.Status().file;
    Expect(EnqueueNumbered(writer, 0) && writer.WaitIdle(kWaitIdle) && writer.Stop() && Has(directory, second),
           "a session with rows survives stop");
}

void TestRetentionIgnoresForeignFiles(const TempDirectory& root)
{
    const auto                     directory = root.Sub("retention-foreign");
    const std::vector<std::string> foreign   = {
        "capture-bad.csv",
        "notes.txt",
        "capture-2020-01-01T00-00-01.000Z-part0001.csv",
        "capture-2020-01-01T00-00-01.000Z-0.csv",
        "capture-2020-01-01T00-00-01.000Z-01.csv",
        "capture-2020-01-01T00-00-01.000Z-1000.csv",
        "capture-2020-01-01T00-00-01.000Z-part00023.csv",
        "capture-2020-01-01T00-00-01.000Z.CSV",
        "capture-2020-01-01T00-00-01.000Z.csv.bak",
        "Capture-2020-01-01T00-00-01.000Z.csv",
    };
    for (const auto& name : foreign)
    {
        MakeCapture(directory, name, false);
    }
    const auto folder = directory / OldName(2);
    std::filesystem::create_directories(folder);
    MakeCapture(folder, OldName(3), false);
    MakeCapture(directory, OldName(4), true, 3);
    PacketCaptureLimits limits;
    limits.maxSessions   = 1;
    limits.maxTotalBytes = 1;
    PacketCaptureWriter writer(directory, limits);
    Expect(writer.Start(), "start failed");
    bool intact = !Has(directory, OldName(4)) && std::filesystem::is_directory(folder) && Has(folder, OldName(3));
    for (const auto& name : foreign)
    {
        intact = intact && Has(directory, name);
    }
    Expect(intact, "retention deleted something outside the capture grammar");
    Expect(Has(directory, writer.Status().file), "the active part was deleted");
}

bool MakeSymlink(const std::filesystem::path& link, const std::filesystem::path& target, bool directory)
{
    constexpr DWORD kUnprivileged = 0x2;
    return CreateSymbolicLinkW(link.c_str(), target.c_str(), (directory ? SYMBOLIC_LINK_FLAG_DIRECTORY : 0) | kUnprivileged) != 0;
}

void TestRetentionSkipsReparsePoints(const TempDirectory& root)
{
    const auto directory = root.Sub("retention-links");
    const auto outside   = root.Sub("retention-links-outside");
    const auto target    = MakeCapture(outside, OldName(9), true, 3);
    const auto linkedDir = root.Sub("retention-links-dir-target");
    MakeCapture(linkedDir, OldName(8), true, 3);
    const bool         fileLink     = MakeSymlink(directory / OldName(1), target, false);
    const bool         dirLink      = MakeSymlink(directory / OldName(2), linkedDir, true);
    const auto         junction     = directory / OldName(3);
    const std::wstring command      = L"cmd /c mklink /J \"" + junction.wstring() + L"\" \"" + linkedDir.wstring() + L"\" >nul 2>&1";
    const bool         junctionMade = _wsystem(command.c_str()) == 0 && std::filesystem::exists(junction);
    if (!fileLink && !dirLink && !junctionMade)
    {
        std::cerr << "skipped: links cannot be created here\n";
        return;
    }
    MakeCapture(directory, OldName(5), true, 3);
    PacketCaptureLimits limits;
    limits.maxSessions   = 1;
    limits.maxTotalBytes = 1;
    PacketCaptureWriter writer(directory, limits);
    Expect(writer.Start(), "start failed");
    Expect(!Has(directory, OldName(5)), "the ordinary old session is pruned");
    Expect(!fileLink || (std::filesystem::is_symlink(directory / OldName(1)) && Has(outside, OldName(9))), "file link or its target was deleted");
    Expect(!dirLink || (std::filesystem::is_symlink(directory / OldName(2)) && Has(linkedDir, OldName(8))), "directory link or its target was touched");
    Expect(!junctionMade || (Has(directory, OldName(3)) && Has(linkedDir, OldName(8))), "junction or its target was touched");
    writer.Stop();

    // A capture directory that is itself a reparse point is never pruned or
    // deleted from, including by the self-delete paths.
    const auto checkLinkedRoot = [&](const char* name, bool junctionRoot)
    {
        const auto realDirectory = root.Sub((std::string("retention-linked-target-") + name).c_str());
        MakeCapture(realDirectory, OldName(1), true, 3);
        MakeCapture(realDirectory, OldName(2), true, 3);
        const auto linkedRoot = root.Sub((std::string("retention-linked-parent-") + name).c_str()) / "packets";
        bool       made       = false;
        if (junctionRoot)
        {
            const std::wstring junctionCommand = L"cmd /c mklink /J \"" + linkedRoot.wstring() + L"\" \"" + realDirectory.wstring() + L"\" >nul 2>&1";
            made                               = _wsystem(junctionCommand.c_str()) == 0 && std::filesystem::exists(linkedRoot);
        }
        else
        {
            made = MakeSymlink(linkedRoot, realDirectory, true);
        }
        if (!made)
        {
            std::cerr << "skipped: " << name << " capture directory cannot be created here\n";
            return;
        }
        std::string sessionFile;
        {
            PacketCaptureWriter linked(linkedRoot, limits);
            Expect(linked.Start(), "start through a linked directory");
            sessionFile = linked.Status().file;
            Expect(Has(realDirectory, OldName(1)) && Has(realDirectory, OldName(2)), "a linked capture directory was pruned");
            Expect(linked.Stop(), "stop through a linked directory");
            Expect(Has(realDirectory, OldName(1)) && Has(realDirectory, OldName(2)) && Has(realDirectory, sessionFile),
                   "stopping deleted through a linked capture directory");
        }
        PacketCaptureWriter failing(linkedRoot, limits);
        failing.FailNextWritesForTest(1);
        Expect(!failing.Start(), "a failed header write must fail the start");
        Expect(Has(realDirectory, OldName(1)) && Has(realDirectory, OldName(2)),
               "failed-header cleanup deleted through a linked capture directory");
    };
    checkLinkedRoot("symlink", false);
    checkLinkedRoot("junction", true);
}

std::wstring Wide(const std::string& text)
{
    return std::wstring(text.begin(), text.end());
}

void TestHandleRelativeHelpers(const TempDirectory& root)
{
    using packet_capture::detail::DeleteCaptureFileForTest;
    using packet_capture::detail::HasDataRowForTest;
    const auto directory = root.Sub("helpers");
    const auto outside   = root.Sub("helpers-outside");
    const auto target    = MakeCapture(outside, OldName(9), false);

    MakeCapture(directory, OldName(1), true);
    MakeCapture(directory, OldName(2), false);
    // A data row beyond the 64 KiB probe bound is not seen.
    {
        std::ofstream big(directory / OldName(3), std::ios::binary);
        big << std::string(kHeader) << "\n"
            << std::string(70 * 1024, 'x') << "\n1700000000000,incoming,3,00A5FF\n";
    }
    Expect(HasDataRowForTest(directory, Wide(OldName(1))) && !HasDataRowForTest(directory, Wide(OldName(2))) &&
               !HasDataRowForTest(directory, Wide(OldName(3))),
           "data row probe");
    Expect(HasDataRowForTest(directory, Wide(OldName(7))), "an unreadable file must count as data");
    Expect(!DeleteCaptureFileForTest(directory, L"..\\helpers-outside\\" + Wide(OldName(9))) && Has(outside, OldName(9)),
           "a name with a separator must be refused");
    Expect(DeleteCaptureFileForTest(directory, Wide(OldName(2))) && !Has(directory, OldName(2)),
           "a regular file is deleted");

    std::filesystem::create_directories(directory / OldName(4));
    Expect(!DeleteCaptureFileForTest(directory, Wide(OldName(4))) && std::filesystem::is_directory(directory / OldName(4)),
           "a directory must be refused");
    const auto         junction = directory / OldName(5);
    const std::wstring command  = L"cmd /c mklink /J \"" + junction.wstring() + L"\" \"" + outside.wstring() + L"\" >nul 2>&1";
    if (_wsystem(command.c_str()) == 0 && std::filesystem::exists(junction))
    {
        Expect(!DeleteCaptureFileForTest(directory, Wide(OldName(5))) && Has(outside, OldName(9)),
               "a junction must be refused");
    }
    if (MakeSymlink(directory / OldName(6), target, false))
    {
        Expect(!DeleteCaptureFileForTest(directory, Wide(OldName(6))) && std::filesystem::is_symlink(directory / OldName(6)) &&
                   Has(outside, OldName(9)),
               "a file symlink must be refused");
        Expect(HasDataRowForTest(directory, Wide(OldName(6))), "the probe must not read through a file symlink");
    }
    else
    {
        std::cerr << "skipped: file symlinks cannot be created here\n";
    }
}

void TestRetentionKeepsOtherWritersParts(const TempDirectory& root)
{
    const auto          directory = root.Sub("retention-two-writers");
    PacketCaptureWriter first(directory);
    if (!Expect(first.Start(), "start failed"))
    {
        return;
    }
    const std::string firstFile = first.Status().file;
    // The first writer is retiring: a frame is queued and a stop follows.
    first.SetPausedForTest(true);
    Expect(EnqueueNumbered(first, 4), "enqueue");
    Expect(first.Stop(0ms), "async stop");
    PacketCaptureLimits limits;
    limits.maxSessions   = 1;
    limits.maxTotalBytes = 1;
    PacketCaptureWriter second(directory, limits);
    Expect(second.Start(), "second start");
    Expect(Has(directory, firstFile), "retention deleted another writer's open part");
    first.SetPausedForTest(false);
    Expect(first.WaitIdle(kWaitIdle), "first writer did not drain");
    Expect(ReadAll(directory / firstFile).find(ExpectedRow(4)) != std::string::npos, "the retiring writer lost its frame");
    Expect(second.Stop(), "second stop");
}

void TestRetentionParsedOrder(const TempDirectory& root)
{
    const auto directory = root.Sub("retention-order");
    // Raw filename order puts "-10", "-2" and "-partNNNN" before the base name.
    MakeCapture(directory, OldName(5), true);
    MakeCapture(directory, OldName(5, 0, 2), true);
    MakeCapture(directory, OldName(5, 2), true);
    MakeCapture(directory, OldName(5, 10), true);
    PacketCaptureLimits limits;
    limits.maxSessions = 2;
    PacketCaptureWriter writer(directory, limits);
    Expect(writer.Start(), "start failed");
    Expect(!Has(directory, OldName(5)) && !Has(directory, OldName(5, 0, 2)) && !Has(directory, OldName(5, 2)) &&
               Has(directory, OldName(5, 10)),
           "sessions are ordered by parsed stamp and suffix");
}

void TestRetentionPruneFailure(const TempDirectory& root)
{
    const auto directory = root.Sub("retention-failure");
    const auto locked    = MakeCapture(directory, OldName(1), true, 3);
    // No FILE_SHARE_DELETE, so deleting the file fails.
    HANDLE hold = CreateFileW(locked.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (hold == INVALID_HANDLE_VALUE)
    {
        std::cerr << "skipped: could not hold the file open\n";
        return;
    }
    PacketCaptureLimits limits;
    limits.maxSessions = 1;
    PacketCaptureWriter writer(directory, limits);
    const bool          started = writer.Start();
    const auto          status  = writer.Status();
    CloseHandle(hold);
    Expect(started && status.recording && status.prune_failures >= 1 && Has(directory, OldName(1)),
           "a failed delete is counted and capture continues");
}

void TestDestroyedWriterDrains(const TempDirectory& root)
{
    const auto directory = root.Sub("destroyed");
    {
        PacketCaptureWriter writer(directory);
        if (!Expect(writer.Start(), "start failed"))
        {
            return;
        }
        writer.SetPausedForTest(true);
        Expect(EnqueueNumbered(writer, 7), "enqueue before destruction");
    }
    // The detached thread finishes the queue after the owner is gone.
    const auto deadline = std::chrono::steady_clock::now() + 10s;
    bool       written  = false;
    while (!written && std::chrono::steady_clock::now() < deadline)
    {
        const auto files = CsvFiles(directory);
        written          = files.size() == 1 && ReadAll(files[0]).find(ExpectedRow(7)) != std::string::npos;
        if (!written)
        {
            std::this_thread::sleep_for(5ms);
        }
    }
    Expect(written, "queued frames were lost when the writer was destroyed");
}

} // namespace

void TestStubRows(const TempDirectory& root)
{
    const auto          directory = root.Sub("stubs");
    PacketCaptureLimits limits;
    limits.maxQueuedBytes = 40;
    PacketCaptureWriter writer(directory, limits);
    if (!Expect(writer.Start(), "start failed"))
    {
        return;
    }
    Expect(writer.GetScope() == Scope::Standard, "default scope");
    writer.SetScope(Scope::Full);
    Expect(writer.GetScope() == Scope::Full, "scope was not stored");
    writer.SetPausedForTest(true);
    const std::string stub = "stub:op=0003;t=0001"; // 19 bytes
    Expect(writer.EnqueueStub(Direction::Outgoing, TimeMs(kBaseMs + 1), 77, stub), "stub rejected");
    Expect(writer.EnqueueStub(Direction::Incoming, TimeMs(kBaseMs + 2), 900000, "stub:op=00C8"), "second stub rejected");
    // 19 + 12 queued bytes: the frame size column does not count toward the budget.
    Expect(!writer.EnqueueStub(Direction::Incoming, TimeMs(kBaseMs + 3), 1, stub), "stub over the byte budget accepted");
    Expect(!writer.EnqueueStub(Direction::Incoming, TimeMs(kBaseMs + 4), 1, std::string_view()), "empty stub accepted");
    writer.CountSkipped();
    writer.CountSkipped();
    writer.SetPausedForTest(false);
    Expect(writer.WaitIdle(kWaitIdle), "queue did not drain");
    const auto status = writer.Status();
    Expect(status.captured == 2 + 1 && status.written == 2 && status.dropped == 1 && status.skipped == 2,
           "stub counters");
    const auto files = CsvFiles(directory);
    if (Expect(files.size() == 1, "stub session file"))
    {
        const auto parsed = Parse(ReadAll(files[0]));
        if (Expect(parsed.data.size() == 2, "stub row count"))
        {
            Expect(parsed.data[0] == "1700000000001,outgoing,77,stub:op=0003;t=0001", "stub row text");
            Expect(parsed.data[1] == "1700000000002,incoming,900000,stub:op=00C8", "second stub row text");
        }
        Expect(MetaValue(parsed, "scope") == "standard", "scope set after the part opened must not rewrite it");
    }
    Expect(writer.Stop(), "stop failed");
    writer.CountSkipped();
    Expect(writer.Status().skipped == 2, "skipped counted while not recording");
}

void TestMetaIdentityAndScope(const TempDirectory& root)
{
    const auto            directory = root.Sub("identity");
    PacketCaptureIdentity identity;
    identity.launcher   = "v1.2.3-4-gabc, bad\x01name";
    identity.runtimeApi = 7;
    PacketCaptureWriter writer(directory, {}, identity);
    writer.SetScope(Scope::Full);
    if (!Expect(writer.Start(), "start failed"))
    {
        return;
    }
    Expect(EnqueueNumbered(writer, 0) && writer.WaitIdle(kWaitIdle), "frame");
    const auto parsed = Parse(ReadAll(directory / writer.Status().file));
    ExpectMeta(parsed, writer.Status().session, 1, "full", "meta rows with a custom identity");
    Expect(MetaValue(parsed, "launcher") == "v1.2.3-4-gabc  bad name" && MetaValue(parsed, "runtime_api") == "7",
           "launcher is sanitized and the runtime api is passed through");
    Expect(writer.Stop(), "stop");
    // Each session has its own id.
    Expect(writer.Start(), "second start");
    Expect(EnqueueNumbered(writer, 1) && writer.WaitIdle(kWaitIdle), "second frame");
    const std::string second = writer.Status().session;
    Expect(second.size() == 16 && second != parsed.meta[1].substr(parsed.meta[1].find('=') + 1), "session ids repeat");

    PacketCaptureIdentity blank;
    blank.launcher = " , ";
    PacketCaptureWriter blankWriter(root.Sub("identity-blank"), {}, blank);
    Expect(blankWriter.Start(), "blank start");
    Expect(MetaValue(Parse(ReadAll(root.Sub("identity-blank") / blankWriter.Status().file)), "launcher") == "unknown",
           "an empty launcher must read unknown");
    Expect(SanitizeField(std::string(300, 'a')).size() == 128 && SanitizeField("  a,b  ") == "a b" &&
               SanitizeField(std::string_view("a\0b", 3)) == "a b",
           "field sanitizer");
}

void TestZoneRows(const TempDirectory& root)
{
    const auto          directory = root.Sub("zones");
    PacketCaptureWriter writer(directory);
    Expect(!writer.EnqueueZone(TimeMs(kBaseMs), 128, "Lower La Noscea"), "zone accepted before start");
    if (!Expect(writer.Start(), "start failed"))
    {
        return;
    }
    writer.SetPausedForTest(true);
    Expect(EnqueueNumbered(writer, 0), "frame before zone");
    Expect(writer.EnqueueZone(TimeMs(kBaseMs + 10), 148, "Mor Dhona,\tCity\x7f"), "zone rejected");
    Expect(writer.EnqueueZone(TimeMs(kBaseMs + 11), 129, std::string_view()), "unnamed zone rejected");
    Expect(EnqueueNumbered(writer, 1), "frame after zone");
    writer.SetPausedForTest(false);
    Expect(writer.WaitIdle(kWaitIdle), "queue did not drain");
    const auto status = writer.Status();
    Expect(status.captured == 2 && status.written == 2 && status.queue_high_water == 2,
           "zone rows are not frames and must not be counted");
    const auto parsed = Parse(ReadAll(directory / status.file));
    if (Expect(parsed.zone.size() == 2, "zone row count"))
    {
        Expect(parsed.zone[0] == "1700000000010,zone,0,id=148;name=Mor Dhona  City", "zone row text");
        Expect(parsed.zone[1] == "1700000000011,zone,0,id=129;name=", "unnamed zone row text");
    }
    // Zone rows keep their place between the frames around them.
    const std::size_t zoneAt = std::find(parsed.all.begin(), parsed.all.end(), parsed.zone[0]) - parsed.all.begin();
    Expect(zoneAt >= 2 && parsed.all[zoneAt - 1] == ExpectedRow(0) && parsed.all[zoneAt + 2] == ExpectedRow(1),
           "zone row order");
    Expect(writer.Stop(), "stop");
    Expect(!writer.EnqueueZone(TimeMs(kBaseMs), 128, "Lower La Noscea"), "zone accepted after stop");

    // A session holding only zone rows wrote no frames and deletes itself.
    const auto          emptyDirectory = root.Sub("zones-only");
    PacketCaptureWriter only(emptyDirectory);
    Expect(only.Start() && only.EnqueueZone(TimeMs(kBaseMs), 128, "Lower La Noscea") && only.WaitIdle(kWaitIdle), "zone only");
    Expect(only.Stop() && CsvFiles(emptyDirectory).empty(), "a zone-only session must delete itself");

    // The queue bound applies to zone rows too.
    PacketCaptureLimits limits;
    limits.maxQueuedFrames = 4;
    PacketCaptureWriter bounded(root.Sub("zones-bound"), limits);
    Expect(bounded.Start(), "bound start");
    bounded.SetPausedForTest(true);
    int accepted = 0;
    for (int index = 0; index < 20; ++index)
    {
        accepted += bounded.EnqueueZone(TimeMs(kBaseMs + index), 128, "Lower La Noscea") ? 1 : 0;
    }
    Expect(accepted < 20, "unbounded zone queue");
    bounded.SetPausedForTest(false);
    Expect(bounded.WaitIdle(kWaitIdle), "bounded drain");
}

void TestEndRows(const TempDirectory& root)
{
    const auto          directory = root.Sub("end-rows");
    PacketCaptureWriter writer(directory);
    if (!Expect(writer.Start(), "start failed"))
    {
        return;
    }
    writer.SetPausedForTest(true);
    Expect(EnqueueNumbered(writer, 0) && EnqueueNumbered(writer, 1), "frames");
    writer.CountSkipped();
    writer.SetPausedForTest(false);
    Expect(writer.Stop(EndReason::Disable), "disable stop");
    auto first = Parse(ReadAll(directory / writer.Status().file));
    Expect(first.end.size() == 1 &&
               first.end[0].find(",end,0,reason=disable;captured=2;written=2;skipped=1;dropped=0;queue_high_water=2") != std::string::npos &&
               first.end[0].find("reason=disable") != std::string::npos,
           "disable end row");

    Expect(writer.Start() && EnqueueNumbered(writer, 2), "second session");
    Expect(writer.Stop(EndReason::Split), "split stop");
    first = Parse(ReadAll(directory / writer.Status().file));
    Expect(first.end.size() == 1 && first.end[0].find("reason=split;captured=3;written=3;skipped=1;dropped=0;") != std::string::npos,
           "split end row");

    // A write failure closes the part with a fault row and continues in a new session.
    Expect(writer.Start() && EnqueueNumbered(writer, 3) && writer.WaitIdle(kWaitIdle), "third session");
    const std::string faultFile = writer.Status().file;
    writer.FailNextWritesForTest(1);
    Expect(EnqueueNumbered(writer, 4) && writer.WaitIdle(kWaitIdle), "failing frame");
    Expect(writer.Status().file != faultFile && writer.Status().recording, "the writer must restart after a fault");
    const auto faulted = Parse(ReadAll(directory / faultFile));
    Expect(faulted.data.size() == 1 && faulted.end.size() == 1 &&
               faulted.end[0].find("reason=fault;captured=5;written=4;skipped=1;dropped=1;") != std::string::npos,
           "fault end row");
    Expect(writer.Stop(), "final stop");
}

void TestZoneRowsRotate(const TempDirectory& root)
{
    PacketCaptureLimits limits;
    limits.maxPartBytes = PartLimit(root, 4);
    {
        // Zone rows fill a part like frames: they must not slip past the cap.
        const auto          directory = root.Sub("zones-rotate");
        PacketCaptureWriter writer(directory, limits);
        if (!Expect(writer.Start(), "start failed"))
        {
            return;
        }
        for (int index = 0; index < 60; ++index)
        {
            Expect(writer.EnqueueZone(TimeMs(kBaseMs + index), 128, "Lower La Noscea"), "zone");
        }
        Expect(writer.WaitIdle(kWaitIdle), "zones did not drain");
        Expect(EnqueueNumbered(writer, 0), "frame after the zones");
        Expect(writer.WaitIdle(kWaitIdle) && writer.Stop(), "stop");
        auto files = CsvFiles(directory);
        bool ok    = files.size() > 1;
        // Directory order puts "-part0002" before ".csv"; order the parts by number.
        std::sort(files.begin(), files.end(), [](const auto& left, const auto& right)
                  {
                      const auto key = [](const std::filesystem::path& path)
                      {
                          const std::string name = path.filename().string();
                          return name.find("-part") == std::string::npos ? std::string(name.substr(0, name.size() - 4)) + "-part0001"
                                                                         : name.substr(0, name.size() - 4);
                      };
                      return key(left) < key(right);
                  });
        std::size_t zones = 0;
        std::size_t data  = 0;
        for (std::size_t number = 0; number < files.size(); ++number)
        {
            const auto parsed = Parse(ReadAll(files[number]));
            ok                = ok && std::filesystem::file_size(files[number]) <= limits.maxPartBytes &&
                                MetaValue(parsed, "part") == std::to_string(number + 1) && parsed.end.size() == 1;
            zones += parsed.zone.size();
            data += parsed.data.size();
        }
        Expect(ok, "zone rows must rotate parts and keep every part within the cap");
        Expect(zones == 60 && data == 1, "zone rotation lost rows");
    }
    {
        // Emptiness counts frames only: a multi-part zone-only session deletes itself.
        const auto          directory = root.Sub("zones-rotate-empty");
        PacketCaptureWriter writer(directory, limits);
        Expect(writer.Start(), "start failed");
        for (int index = 0; index < 60; ++index)
        {
            Expect(writer.EnqueueZone(TimeMs(kBaseMs + index), 128, "Lower La Noscea"), "zone");
        }
        Expect(writer.WaitIdle(kWaitIdle), "zones did not drain");
        Expect(writer.Status().part > 1, "the zones did not rotate");
        Expect(writer.Stop() && CsvFiles(directory).empty(), "a zone-only session must delete every part");
    }
}

void TestRotationEndRowFailure(const TempDirectory& root)
{
    PacketCaptureLimits limits;
    limits.maxPartBytes           = PartLimit(root, 4);
    const auto          directory = root.Sub("end-row-failure");
    PacketCaptureWriter writer(directory, limits);
    if (!Expect(writer.Start(), "start failed"))
    {
        return;
    }
    // Fill a part exactly and flush it, so the next write is the rotation end row.
    for (int index = 0; index < 4; ++index)
    {
        Expect(EnqueueNumbered(writer, index), "fill");
    }
    Expect(writer.WaitIdle(kWaitIdle) && writer.Status().part == 1, "the part did not fill");
    const std::string firstFile    = writer.Status().file;
    const std::string firstSession = writer.Status().session;
    writer.FailNextWritesForTest(1);
    Expect(EnqueueNumbered(writer, 4) && writer.WaitIdle(kWaitIdle), "rotation frame");
    auto status = writer.Status();
    Expect(status.recording && status.error.empty() && status.file != firstFile && status.session != firstSession &&
               status.part == 1,
           "a failed rotation end row must restart capture once in a new session");
    const auto notice = writer.TakeNotice();
    Expect(notice && *notice == "Packet capture: recording again after a save error", "restart notice");
    const auto closed = Parse(ReadAll(directory / firstFile));
    Expect(closed.data.size() == 4 && closed.end.size() == 1 && closed.end[0].find("reason=fault;") != std::string::npos,
           "the old part must end with a fault row");
    Expect(Parse(ReadAll(directory / status.file)).data.size() == 1, "the rotation frame belongs to the new session");

    // A second end-row failure stops capture.
    for (int index = 5; index < 8; ++index)
    {
        Expect(EnqueueNumbered(writer, index), "fill again");
    }
    Expect(writer.WaitIdle(kWaitIdle) && writer.Status().part == 1, "the new part did not fill");
    writer.FailNextWritesForTest(1);
    Expect(EnqueueNumbered(writer, 8) && writer.WaitIdle(kWaitIdle), "second rotation frame");
    status = writer.Status();
    Expect(!status.recording && status.error == "write error" && status.dropped == 1,
           "a second end-row failure must stop capture");
    const auto stopped = writer.TakeNotice();
    Expect(stopped && *stopped == "Packet capture: stopped after a save error", "stop notice");
    Expect(!EnqueueNumbered(writer, 9), "frame accepted after the stop");
}

void TestStopEndRowFailure(const TempDirectory& root)
{
    const auto          directory = root.Sub("stop-end-row-failure");
    PacketCaptureWriter writer(directory);
    if (!Expect(writer.Start(), "start failed"))
    {
        return;
    }
    Expect(EnqueueNumbered(writer, 0) && writer.WaitIdle(kWaitIdle), "frame");
    const std::string file = writer.Status().file;
    writer.FailNextWritesForTest(1);
    Expect(writer.Stop(EndReason::Disable), "stop");
    const auto status = writer.Status();
    Expect(!status.recording && status.error == "write error", "a failed disable end row must report a write error");
    const auto notice = writer.TakeNotice();
    Expect(notice && *notice == "Packet capture: stopped after a save error", "stop notice");
    const auto parsed = Parse(ReadAll(directory / file));
    Expect(parsed.end.size() == 1 && parsed.end[0].find("reason=fault;") != std::string::npos,
           "the part must end with a best-effort fault row");
}

void TestSplit(const TempDirectory& root)
{
    const auto          directory = root.Sub("split");
    PacketCaptureWriter writer(directory);
    writer.SetScope(Scope::Full);
    Expect(!writer.Split(), "split accepted while not recording");
    if (!Expect(writer.Start(), "start failed"))
    {
        return;
    }
    const std::string firstFile = writer.Status().file;
    const std::string session   = writer.Status().session;
    writer.SetPausedForTest(true);
    Expect(EnqueueNumbered(writer, 0), "frame before the split");
    Expect(writer.Split(0ms), "queued split");
    Expect(EnqueueNumbered(writer, 1), "frame after the split");
    writer.SetPausedForTest(false);
    Expect(writer.WaitIdle(kWaitIdle), "queue did not drain");
    auto status = writer.Status();
    Expect(status.recording && status.part == 2 && status.session == session && status.file != firstFile,
           "a split must continue in the next part of the same session");
    Expect(writer.Split(), "second split");
    Expect(writer.Status().part == 3, "part number after the second split");
    Expect(writer.Stop(), "stop");
    const auto first  = Parse(ReadAll(directory / firstFile));
    const auto second = Parse(ReadAll(directory / (std::filesystem::path(firstFile).stem().string() + "-part0002.csv")));
    const auto third  = Parse(ReadAll(directory / (std::filesystem::path(firstFile).stem().string() + "-part0003.csv")));
    ExpectMeta(first, session, 1, "full", "meta rows of the split part");
    ExpectMeta(second, session, 2, "full", "meta rows after a split");
    ExpectMeta(third, session, 3, "full", "meta rows after a second split");
    Expect(first.data.size() == 1 && first.data[0] == ExpectedRow(0) && second.data.size() == 1 && second.data[0] == ExpectedRow(1),
           "frames must stay on their side of the split");
    Expect(first.end.size() == 1 && first.end[0].find(",end,0,reason=split;captured=2;written=") != std::string::npos &&
               second.end.size() == 1 && second.end[0].find("reason=split;") != std::string::npos &&
               third.end.size() == 1 && third.end[0].find("reason=stop;") != std::string::npos && third.data.empty(),
           "end rows after splits");

    // A failing split end row follows the write-error policy: restart once, then stop.
    const auto          failing = root.Sub("split-failure");
    PacketCaptureWriter other(failing);
    Expect(other.Start() && EnqueueNumbered(other, 0) && other.WaitIdle(kWaitIdle), "failure setup");
    const std::string failedSession = other.Status().session;
    other.FailNextWritesForTest(1);
    Expect(!other.Split(), "a failed split must report failure");
    status = other.Status();
    Expect(status.recording && status.session != failedSession && status.part == 1 && status.error.empty(),
           "the first failed split must restart capture in a new session");
    Expect(EnqueueNumbered(other, 1) && other.WaitIdle(kWaitIdle), "frame after the restart");
    other.FailNextWritesForTest(1);
    Expect(!other.Split() && !other.Status().recording && other.Status().error == "write error",
           "the second failed split must stop capture");
}

void TestDestructorWritesNothing(const TempDirectory& root)
{
    const auto            directory = root.Sub("destructor");
    std::filesystem::path file;
    {
        PacketCaptureWriter writer(directory);
        if (!Expect(writer.Start(), "start failed"))
        {
            return;
        }
        Expect(EnqueueNumbered(writer, 0) && writer.WaitIdle(kWaitIdle), "frame");
        file = directory / writer.Status().file;
    }
    std::this_thread::sleep_for(600ms);
    const auto parsed = Parse(ReadAll(file));
    Expect(parsed.data.size() == 1 && parsed.end.empty(), "the destructor must not write an end row");
}

int main()
{
    const TempDirectory root;
    TestOrderingAndSharing(root);
    TestStopDrainsIntoOldSession(root);
    TestRotation(root);
    TestQueueBounds(root);
    TestStubRows(root);
    TestMetaIdentityAndScope(root);
    TestZoneRows(root);
    TestEndRows(root);
    TestZoneRowsRotate(root);
    TestRotationEndRowFailure(root);
    TestStopEndRowFailure(root);
    TestSplit(root);
    TestDestructorWritesNothing(root);
    TestWriteFailures(root);
    TestStartAfterFailureKeepsAcceptance(root);
    TestStopFlushFailure(root);
    TestRetentionSessionCount(root);
    TestRetentionBytes(root);
    TestRetentionActivePart(root);
    TestRetentionOldestPartsFirst(root);
    TestRetentionRotationPrunes(root);
    TestRetentionEmptySessions(root);
    TestRetentionIgnoresForeignFiles(root);
    TestRetentionSkipsReparsePoints(root);
    TestHandleRelativeHelpers(root);
    TestRetentionKeepsOtherWritersParts(root);
    TestRetentionParsedOrder(root);
    TestRetentionPruneFailure(root);
    TestDestroyedWriterDrains(root);
    if (g_failures != 0)
    {
        std::cerr << g_failures << " packet capture check(s) failed\n";
        return 1;
    }
    return 0;
}
