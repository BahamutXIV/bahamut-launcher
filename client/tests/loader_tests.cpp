#include <windows.h>

#include "../api/runtime_api.h"
#include "monitor_selection.h"
#include "process_affinity.h"

#include <algorithm>
#include <atomic>
#include <cstdint>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <thread>
#include <tuple>
#include <vector>

namespace
{

struct RunResult
{
    DWORD       exitCode = 0xffffffff;
    std::string output;
};

std::wstring Quote(const std::wstring& value)
{
    return L"\"" + value + L"\"";
}

std::string Narrow(const std::wstring& value)
{
    if (value.empty())
        return {};
    const int   length = WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
    std::string result(length, '\0');
    WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), result.data(), length, nullptr, nullptr);
    return result;
}

std::string ReadText(const std::wstring& path)
{
    std::ifstream file{ std::filesystem::path{ path } };
    return std::string((std::istreambuf_iterator<char>(file)), {});
}

bool ExpectSuccess(const RunResult& result);

bool HasFileSignature(const std::filesystem::path&      path,
                      const std::vector<unsigned char>& signature)
{
    std::ifstream              file(path, std::ios::binary);
    std::vector<unsigned char> actual(signature.size());
    file.read(reinterpret_cast<char*>(actual.data()),
              static_cast<std::streamsize>(actual.size()));
    return file.gcount() == static_cast<std::streamsize>(signature.size()) && actual == signature;
}

bool VerifyScreenshot(const char* name, const std::filesystem::path& directory, const std::filesystem::path& resultPath, const wchar_t* extension, const std::vector<unsigned char>& signature, bool hideOverlays, const RunResult& result)
{
    if (!ExpectSuccess(result))
    {
        std::cerr << name << " failed: exit=" << result.exitCode
                  << " output=" << result.output << "\n";
        return false;
    }
    std::vector<std::filesystem::path> captures;
    for (const auto& entry : std::filesystem::directory_iterator(directory))
    {
        if (entry.is_regular_file() && entry.path().extension() == extension)
        {
            captures.push_back(entry.path());
        }
    }
    const std::string captureResult = ReadText(resultPath.wstring());
    const std::string hideMarker    = std::string("hide_overlays=") + (hideOverlays ? "1" : "0");
    const std::string frameMarker   = hideOverlays
                                          ? "overlay_frames=119"
                                          : "overlay_frames=120";
    const bool        valid         = captures.size() == 1 && HasFileSignature(captures.front(), signature) && captureResult.find("status=captured") != std::string::npos && captureResult.find(hideMarker) != std::string::npos && result.output.find(frameMarker) != std::string::npos;
    if (!valid)
    {
        std::cerr << name << " failed: captures=" << captures.size()
                  << " result=" << captureResult << "\n";
    }
    return valid;
}

struct RunningHelper
{
    HANDLE      process  = nullptr;
    HANDLE      readPipe = nullptr;
    std::string output;
};

// Reads only buffered bytes: under Wine the pipe may not report EOF after a
// helper that launched a GUI client exits.
void DrainAvailable(RunningHelper& running)
{
    DWORD available = 0;
    while (PeekNamedPipe(running.readPipe, nullptr, 0, nullptr, &available, nullptr) && available != 0)
    {
        char  buffer[1024];
        DWORD count = 0;
        if (!ReadFile(running.readPipe, buffer, std::min<DWORD>(available, sizeof(buffer)), &count, nullptr) || count == 0)
        {
            return;
        }
        running.output.append(buffer, count);
    }
}

bool StartHelper(const std::wstring& helper, const std::wstring& args, RunningHelper& running)
{
    SECURITY_ATTRIBUTES security{};
    security.nLength        = sizeof(security);
    security.bInheritHandle = TRUE;
    HANDLE readPipe         = nullptr;
    HANDLE writePipe        = nullptr;
    if (!CreatePipe(&readPipe, &writePipe, &security, 0))
    {
        return {};
    }
    SetHandleInformation(readPipe, HANDLE_FLAG_INHERIT, 0);

    STARTUPINFOW startup{};
    startup.cb         = sizeof(startup);
    startup.dwFlags    = STARTF_USESTDHANDLES;
    startup.hStdOutput = writePipe;
    startup.hStdError  = writePipe;
    PROCESS_INFORMATION  processInfo{};
    std::wstring         command = Quote(helper) + L" " + args;
    std::vector<wchar_t> mutableCommand(command.begin(), command.end());
    mutableCommand.push_back(L'\0');
    const BOOL created = CreateProcessW(helper.c_str(), mutableCommand.data(), nullptr, nullptr, TRUE, CREATE_UNICODE_ENVIRONMENT, nullptr, nullptr, &startup, &processInfo);
    CloseHandle(writePipe);
    if (!created)
    {
        CloseHandle(readPipe);
        return false;
    }
    CloseHandle(processInfo.hThread);
    running.process  = processInfo.hProcess;
    running.readPipe = readPipe;
    return true;
}

RunResult FinishHelper(RunningHelper& running)
{
    const ULONGLONG deadline = GetTickCount64() + 20000;
    do
    {
        DrainAvailable(running);
    } while (WaitForSingleObject(running.process, 10) == WAIT_TIMEOUT && GetTickCount64() < deadline);
    DrainAvailable(running);

    RunResult result;
    GetExitCodeProcess(running.process, &result.exitCode);
    if (result.exitCode == STILL_ACTIVE)
    {
        TerminateProcess(running.process, 1);
    }
    result.output = std::move(running.output);
    CloseHandle(running.process);
    CloseHandle(running.readPipe);
    return result;
}

RunResult RunHelper(const std::wstring& helper, const std::wstring& args)
{
    RunningHelper running;
    if (!StartHelper(helper, args, running))
    {
        return {};
    }
    return FinishHelper(running);
}

bool Contains(const RunResult& result, std::string_view text)
{
    return result.output.find(text) != std::string::npos;
}

bool Expect(const char* name, const RunResult& result, DWORD exitCode, std::string_view output)
{
    if (result.exitCode == exitCode && Contains(result, output))
    {
        return true;
    }
    std::cerr << name << " failed: exit=" << result.exitCode << " output=" << result.output << "\n";
    return false;
}

bool ExpectExit(const char* name, const RunResult& result, DWORD exitCode)
{
    if (result.exitCode == exitCode)
    {
        return true;
    }
    std::cerr << name << " failed: exit=" << result.exitCode
              << " output=" << result.output << "\n";
    return false;
}

bool ExpectTerminated(const char* name, const RunResult& result, std::string_view code)
{
    const std::string prefix = "ERROR " + std::string(code) + " target_terminated pid=";
    const std::size_t start  = result.output.find(prefix);
    if (result.exitCode != 1 || start == std::string::npos)
    {
        std::cerr << name << " failed: exit=" << result.exitCode << " output=" << result.output << "\n";
        return false;
    }

    const std::size_t pidStart = start + prefix.size();
    std::size_t       pidEnd   = pidStart;
    while (pidEnd < result.output.size() && result.output[pidEnd] >= '0' && result.output[pidEnd] <= '9')
    {
        ++pidEnd;
    }
    if (pidEnd == pidStart)
    {
        std::cerr << name << " failed: missing pid output=" << result.output << "\n";
        return false;
    }

    const DWORD processId = static_cast<DWORD>(std::stoul(result.output.substr(pidStart, pidEnd - pidStart)));
    HANDLE      process   = OpenProcess(SYNCHRONIZE, FALSE, processId);
    const bool  signaled  = process != nullptr
                                ? WaitForSingleObject(process, 0) == WAIT_OBJECT_0
                                : GetLastError() == ERROR_INVALID_PARAMETER;
    if (process != nullptr)
        CloseHandle(process);
    if (!signaled)
    {
        std::cerr << name << " failed: target still live pid=" << processId << " output=" << result.output << "\n";
    }
    return signaled;
}

bool ExpectSuccess(const RunResult& result)
{
    return Contains(result, "SUCCESS pid=") && Contains(result, "entry_reached") && Contains(result, "ready_signal") && Contains(result, "resume_observed") && Contains(result, "frame_boundary_observed") && Contains(result, "frames=") && Contains(result, "overlay_frames=") && Contains(result, "overlay_avg_us=") && Contains(result, "overlay_max_us=") && Contains(result, "resets=2") && Contains(result, "overlay_returned") && Contains(result, "captured=") && Contains(result, "forwarded=") && Contains(result, "toggles=0") && Contains(result, "input_routed") && Contains(result, "state_preserved") && Contains(result, "patches_verified");
}

bool TransferredProcessHandleIsExact(const RunResult& result)
{
    const std::string pidMarker    = "SUCCESS pid=";
    const std::size_t pidStart     = result.output.find(pidMarker);
    const std::string handleMarker = "process_handle=";
    const std::size_t handleStart  = result.output.find(handleMarker);
    if (result.exitCode != 0 || pidStart == std::string::npos || handleStart == std::string::npos)
    {
        std::cerr << "transferred_process_handle_is_exact failed: missing success fields output="
                  << result.output << "\n";
        return false;
    }
    try
    {
        std::size_t         pidEnd      = 0;
        const unsigned long expectedPid = std::stoul(
            result.output.substr(pidStart + pidMarker.size()), &pidEnd, 10);
        std::size_t              handleEnd = 0;
        const unsigned long long rawHandle = std::stoull(
            result.output.substr(handleStart + handleMarker.size()), &handleEnd, 10);
        if (expectedPid == 0 || rawHandle == 0)
        {
            throw std::invalid_argument("zero process identity");
        }
        HANDLE     process          = reinterpret_cast<HANDLE>(static_cast<std::uintptr_t>(rawHandle));
        DWORD      observedExitCode = STILL_ACTIVE;
        const bool exact            = GetProcessId(process) == static_cast<DWORD>(expectedPid) && WaitForSingleObject(process, 0) == WAIT_OBJECT_0 && GetExitCodeProcess(process, &observedExitCode) && observedExitCode == 0;
        CloseHandle(process);
        if (!exact)
        {
            std::cerr << "transferred_process_handle_is_exact failed: handle did not identify "
                      << "the exited target output=" << result.output << "\n";
        }
        return exact;
    }
    catch (const std::exception& error)
    {
        std::cerr << "transferred_process_handle_is_exact failed: " << error.what()
                  << " output=" << result.output << "\n";
        return false;
    }
}

std::wstring Prefix(unsigned int testNumber)
{
    return L"Local\\BahamutBootstrapTest_" + std::to_wstring(GetCurrentProcessId()) + L"_" + std::to_wstring(testNumber);
}

std::wstring CommonArgs(const std::wstring& stub, const std::wstring& runtime, unsigned int number)
{
    return L"--test-stub --client " + Quote(stub) + L" --module " + Quote(runtime) + L" --server-utc A1B2C3D4E5 --lobby-host 11223300 --event-prefix " + Quote(Prefix(number)) + L" --timeout-ms 10000";
}

void WriteAdversarialCounter(LARGE_INTEGER& target, std::uint64_t value)
{
    auto* words = reinterpret_cast<volatile LONG*>(&target);
    words[0]    = static_cast<LONG>(value & 0xffffffffULL);
    SwitchToThread();
    words[1] = static_cast<LONG>(value >> 32);
}

bool RunTelemetrySnapshotRace()
{
    BahamutRuntimeTelemetry   telemetry{};
    std::atomic<bool>         running        = true;
    std::atomic<bool>         failed         = false;
    std::atomic<unsigned int> validSnapshots = 0;
    std::thread               reader([&]()
                                     {
                           while (running.load(std::memory_order_acquire))
                           {
                               BahamutRuntimeTimingSnapshot snapshot{};
                               if (!BahamutReadTimingSnapshot(telemetry, snapshot))
                               {
                                   continue;
                               }
                               if (snapshot.frameCount == 0)
                               {
                                   continue;
                               }
                               const std::uint64_t first = static_cast<std::uint64_t>(
                                   snapshot.firstFrameCounter.QuadPart);
                               if (snapshot.overlayFrameCount != snapshot.frameCount || static_cast<std::uint64_t>(snapshot.lastFrameCounter.QuadPart) != first + 1 || static_cast<std::uint64_t>(snapshot.overlayTotalCounter.QuadPart) != first + 2 || static_cast<std::uint64_t>(snapshot.overlayMaxCounter.QuadPart) != first + 3)
                               {
                                   failed.store(true, std::memory_order_release);
                                   return;
                               }
                               validSnapshots.fetch_add(1, std::memory_order_relaxed);
                           }
                                     });
    std::thread               writer([&]()
                                     {
                           for (LONG generation = 1; generation <= 2000; ++generation)
                           {
                               const std::uint64_t first = 0x1000000000000000ULL + static_cast<std::uint64_t>(generation) * 0x1000ULL;
                               BahamutBeginTimingWrite(&telemetry);
                               telemetry.frameCount        = generation;
                               telemetry.overlayFrameCount = generation;
                               WriteAdversarialCounter(telemetry.firstFrameCounter, first);
                               WriteAdversarialCounter(telemetry.lastFrameCounter, first + 1);
                               WriteAdversarialCounter(telemetry.overlayTotalCounter, first + 2);
                               WriteAdversarialCounter(telemetry.overlayMaxCounter, first + 3);
                               BahamutEndTimingWrite(&telemetry);
                           }
                           running.store(false, std::memory_order_release);
                                     });
    writer.join();
    reader.join();
    return !failed.load(std::memory_order_acquire) && validSnapshots.load(std::memory_order_relaxed) != 0;
}

bool WriteMalformedExportImage(const std::wstring& path)
{
    std::vector<unsigned char> image(0x500, 0);
    auto*                      dos                                                = reinterpret_cast<IMAGE_DOS_HEADER*>(image.data());
    dos->e_magic                                                                  = IMAGE_DOS_SIGNATURE;
    dos->e_lfanew                                                                 = 0x40;
    auto* nt                                                                      = reinterpret_cast<IMAGE_NT_HEADERS32*>(image.data() + dos->e_lfanew);
    nt->Signature                                                                 = IMAGE_NT_SIGNATURE;
    nt->FileHeader.Machine                                                        = IMAGE_FILE_MACHINE_I386;
    nt->FileHeader.NumberOfSections                                               = 2;
    nt->FileHeader.SizeOfOptionalHeader                                           = sizeof(IMAGE_OPTIONAL_HEADER32);
    nt->OptionalHeader.Magic                                                      = IMAGE_NT_OPTIONAL_HDR32_MAGIC;
    nt->OptionalHeader.NumberOfRvaAndSizes                                        = IMAGE_NUMBEROF_DIRECTORY_ENTRIES;
    nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_EXPORT].VirtualAddress = 0x200;
    nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_EXPORT].Size           = 0x40;
    auto* section                                                                 = IMAGE_FIRST_SECTION(nt);
    section->VirtualAddress                                                       = 0x200;
    section->Misc.VirtualSize                                                     = 0x100;
    section->SizeOfRawData                                                        = 0x100;
    section->PointerToRawData                                                     = 0x200;
    auto* dataSection                                                             = section + 1;
    dataSection->VirtualAddress                                                   = 0x300;
    dataSection->Misc.VirtualSize                                                 = 0x100;
    dataSection->SizeOfRawData                                                    = 0x100;
    dataSection->PointerToRawData                                                 = 0x300;
    auto* exports                                                                 = reinterpret_cast<IMAGE_EXPORT_DIRECTORY*>(image.data() + 0x200);
    exports->NumberOfFunctions                                                    = 1;
    exports->NumberOfNames                                                        = 1;
    exports->AddressOfNames                                                       = 0x2ff;
    exports->AddressOfNameOrdinals                                                = 0x310;
    exports->AddressOfFunctions                                                   = 0x314;
    const DWORD nameRva                                                           = 0x320;
    std::memcpy(image.data() + 0x2ff, &nameRva, sizeof(nameRva));
    const WORD ordinal = 0;
    std::memcpy(image.data() + 0x310, &ordinal, sizeof(ordinal));
    const DWORD functionRva = 0x330;
    std::memcpy(image.data() + 0x314, &functionRva, sizeof(functionRva));
    std::memcpy(image.data() + 0x320, "BahamutRuntimeInitialize", 25);
    std::ofstream file{ std::filesystem::path{ path }, std::ios::binary | std::ios::trunc };
    file.write(reinterpret_cast<const char*>(image.data()), image.size());
    return file.good();
}

// RVA of the only occurrence of text in an x86 PE file.
std::optional<DWORD> UniqueTextRva(const std::wstring& path, std::string_view text)
{
    std::ifstream           file{ std::filesystem::path{ path }, std::ios::binary };
    const std::vector<char> bytes((std::istreambuf_iterator<char>(file)), {});
    const std::string_view  image(bytes.data(), bytes.size());
    const std::size_t       offset = image.find(text);
    if (offset == std::string_view::npos || image.find(text, offset + 1) != std::string_view::npos || bytes.size() < sizeof(IMAGE_DOS_HEADER))
    {
        return std::nullopt;
    }
    IMAGE_DOS_HEADER dos{};
    std::memcpy(&dos, bytes.data(), sizeof(dos));
    if (dos.e_magic != IMAGE_DOS_SIGNATURE || dos.e_lfanew < 0 || static_cast<std::size_t>(dos.e_lfanew) + sizeof(IMAGE_NT_HEADERS32) > bytes.size())
    {
        return std::nullopt;
    }
    IMAGE_NT_HEADERS32 nt{};
    std::memcpy(&nt, bytes.data() + dos.e_lfanew, sizeof(nt));
    const std::size_t sectionTable = static_cast<std::size_t>(dos.e_lfanew) + offsetof(IMAGE_NT_HEADERS32, OptionalHeader) + nt.FileHeader.SizeOfOptionalHeader;
    for (WORD index = 0; index < nt.FileHeader.NumberOfSections; ++index)
    {
        IMAGE_SECTION_HEADER section{};
        const std::size_t    at = sectionTable + index * sizeof(section);
        if (at + sizeof(section) > bytes.size())
        {
            return std::nullopt;
        }
        std::memcpy(&section, bytes.data() + at, sizeof(section));
        if (offset >= section.PointerToRawData && offset + text.size() <= static_cast<std::size_t>(section.PointerToRawData) + section.SizeOfRawData)
        {
            return section.VirtualAddress + static_cast<DWORD>(offset - section.PointerToRawData);
        }
    }
    return std::nullopt;
}

std::wstring PatchArgument(DWORD rva, std::string_view bytes)
{
    constexpr wchar_t digits[] = L"0123456789abcdef";
    std::wstring      value    = L" --patch ";
    for (int shift = 28; shift >= 0; shift -= 4)
    {
        value.push_back(digits[(rva >> shift) & 0x0f]);
    }
    value.push_back(L':');
    for (const unsigned char byte : bytes)
    {
        value.push_back(digits[byte >> 4]);
        value.push_back(digits[byte & 0x0f]);
    }
    return value;
}

bool WaitForClientReportsExit(const std::wstring& helper, const std::wstring& stub, const std::wstring& runtime)
{
    constexpr unsigned int number  = 36;
    HANDLE                 release = CreateEventW(nullptr, TRUE, FALSE, (Prefix(number) + L"_release").c_str());
    RunningHelper          running;
    if (release == nullptr || !StartHelper(helper, CommonArgs(stub, runtime, number) + L" --wait-for-client --fault hold-after-success", running))
    {
        std::cerr << "wait_for_client_reports_client_exit failed: could not start the helper\n";
        if (release != nullptr)
            CloseHandle(release);
        return false;
    }

    // The hold fault parks the helper after SUCCESS, so the whole line must be
    // readable while the helper is still running.
    bool            visibleWhileRunning = false;
    const ULONGLONG deadline            = GetTickCount64() + 20000;
    while (GetTickCount64() < deadline)
    {
        DrainAvailable(running);
        const std::size_t line = running.output.find("SUCCESS pid=");
        if (line != std::string::npos && running.output.find('\n', line) != std::string::npos)
        {
            visibleWhileRunning = WaitForSingleObject(running.process, 0) == WAIT_TIMEOUT;
            break;
        }
        if (WaitForSingleObject(running.process, 10) != WAIT_TIMEOUT)
        {
            break;
        }
    }
    SetEvent(release);
    CloseHandle(release);
    const RunResult result = FinishHelper(running);

    const std::string pidMarker = "SUCCESS pid=";
    const std::size_t successAt = result.output.find(pidMarker);
    std::string       pid;
    if (successAt != std::string::npos)
    {
        const std::size_t pidStart = successAt + pidMarker.size();
        pid                        = result.output.substr(pidStart, result.output.find_first_not_of("0123456789", pidStart) - pidStart);
    }
    const std::string exitLine = "EXIT pid=" + pid + " code=0";
    const std::size_t exitAt   = pid.empty() ? std::string::npos : result.output.find(exitLine);
    const std::size_t exitEnd  = exitAt == std::string::npos ? exitAt : exitAt + exitLine.size();
    const bool        exitReported =
        exitAt != std::string::npos && exitAt > successAt && (exitEnd == result.output.size() || result.output[exitEnd] == '\r' || result.output[exitEnd] == '\n');
    const bool handleWithheld = !Contains(result, "process_handle=");
    const bool passed         = visibleWhileRunning && result.exitCode == 0 && ExpectSuccess(result) && handleWithheld && exitReported;
    if (!passed)
    {
        std::cerr << "wait_for_client_reports_client_exit failed: visible_while_running=" << visibleWhileRunning
                  << " handle_withheld=" << handleWithheld << " exit_reported=" << exitReported
                  << " exit=" << result.exitCode << " output=" << result.output << "\n";
    }
    return passed;
}

} // namespace

int wmain(int argc, wchar_t** argv)
{
    if (argc != 8)
    {
        std::cerr << "usage: bootstrap-tests helper stub runtime screenshot-plugin fps-manifest addon-settings chat-logs\n";
        return 2;
    }
    const std::wstring          helper           = argv[1];
    const std::wstring          stub             = argv[2];
    const std::wstring          runtime          = argv[3];
    const std::wstring          screenshotPlugin = argv[4];
    const std::wstring          fpsManifest      = argv[5];
    const std::filesystem::path addonSettings    = argv[6];
    const std::filesystem::path chatLogs         = argv[7];

    bool success = true;
    success      = bahamut_loader::LimitGameProcessorMask(0x0fffu) == 0x0fffu &&
                   bahamut_loader::LimitGameProcessorMask(0xffffu) == 0x7fffu &&
                   bahamut_loader::LimitGameProcessorMask(0xaaaau) == 0xaaaau &&
                   bahamut_loader::LimitGameProcessorMask(0xaaaaaaaau) == 0x2aaaaaaau && success;
    if (!success)
    {
        std::cerr << "game affinity mask cap failed\n";
    }
    if (!RunTelemetrySnapshotRace())
    {
        std::cerr << "telemetry_snapshot_never_observes_torn_timing failed\n";
        success = false;
    }
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_READY_EVENT", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_FRAME_EVENT", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_TELEMETRY_MAPPING", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_PLUGIN",
                            screenshotPlugin.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_STUB_DIRECT_LAUNCH", L"1");
    success = ExpectExit("direct launch", RunHelper(stub, L""), 0) && success;
    SetEnvironmentVariableW(L"BAHAMUT_STUB_DIRECT_LAUNCH", nullptr);

    const std::wstring overlayTargetPath =
        (std::filesystem::temp_directory_path() / L"bahamut-overlay-target-test.txt").wstring();
    DeleteFileW(overlayTargetPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_TARGET_RESULT_FILE",
                            overlayTargetPath.c_str());
    std::error_code storageError;
    std::filesystem::remove(addonSettings / L"fps" / L"settings.ini", storageError);
    const std::filesystem::path startupScript = std::filesystem::temp_directory_path() / (L"bahamut-default-test-" + std::to_wstring(GetCurrentProcessId()) + L".txt");
    {
        std::ofstream script(startupScript, std::ios::trunc);
        script << "/bind f11 /fillmode\n"
               << "/bind f12 /fps\n"
               << "/addon load fps\n";
    }
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_ADDON_CATALOG", fpsManifest.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_ADDON_SETTINGS", addonSettings.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_CHAT_LOGS", chatLogs.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_STARTUP_SCRIPT", startupScript.c_str());
    const RunResult successResult = RunHelper(helper, CommonArgs(stub, runtime, 1));
    SetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_TARGET_RESULT_FILE", nullptr);
    if (!ExpectSuccess(successResult) || !TransferredProcessHandleIsExact(successResult) || ReadText(overlayTargetPath).find("overlay_target=backbuffer") == std::string::npos || ReadText((addonSettings / L"fps" / L"settings.ini").wstring()).find("visible=true") == std::string::npos)
    {
        std::cerr << "success failed: exit=" << successResult.exitCode
                  << " output=" << successResult.output << "\n";
        success = false;
    }

    const std::wstring fillModeResultPath =
        (std::filesystem::temp_directory_path() / L"bahamut-fill-mode-test.txt").wstring();
    const std::wstring overlayFillModeResultPath =
        (std::filesystem::temp_directory_path() / L"bahamut-overlay-fill-mode-test.txt").wstring();
    DeleteFileW(fillModeResultPath.c_str());
    DeleteFileW(overlayFillModeResultPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_FILL_MODE_BOUNDARY", L"1");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_FILL_MODE_RESULT_FILE",
                            fillModeResultPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_FILL_MODE_RESULT_FILE",
                            overlayFillModeResultPath.c_str());
    const RunResult   fillMode              = RunHelper(helper, CommonArgs(stub, runtime, 30));
    const std::string fillModeResult        = ReadText(fillModeResultPath);
    const std::string overlayFillModeResult = ReadText(overlayFillModeResultPath);
    if (!ExpectSuccess(fillMode) || fillModeResult.find("f11_repeat_stable=1") == std::string::npos || fillModeResult.find("solid_request_ignored=1") == std::string::npos || fillModeResult.find("disabled_restores_solid=1") == std::string::npos || fillModeResult.find("reset_preserves_wireframe=1") == std::string::npos || fillModeResult.find("inactive_ignored=1") == std::string::npos || fillModeResult.find("secondary_remains_solid=1") == std::string::npos || fillModeResult.find("overlay_restores_wireframe=1") == std::string::npos || fillModeResult.find("depth_enabled_wireframe=1") == std::string::npos || fillModeResult.find("depth_disabled_solid=1") == std::string::npos || fillModeResult.find("depth_reenabled_wireframe=1") == std::string::npos || fillModeResult.find("f11_inactive_forward_pair=1") == std::string::npos || fillModeResult.find("retail_f11_suppressed=1") == std::string::npos || overlayFillModeResult.find("overlay_fill_mode=solid") == std::string::npos)
    {
        std::cerr << "fill mode boundary failed: result=" << fillModeResult
                  << " overlay=" << overlayFillModeResult
                  << " output=" << fillMode.output << "\n";
        success = false;
    }
    SetEnvironmentVariableW(L"BAHAMUT_TEST_FILL_MODE_BOUNDARY", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_FILL_MODE_RESULT_FILE", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_FILL_MODE_RESULT_FILE", nullptr);
    DeleteFileW(fillModeResultPath.c_str());
    DeleteFileW(overlayFillModeResultPath.c_str());

    std::filesystem::remove(addonSettings / L"fps" / L"settings.ini", storageError);
    const std::wstring fpsHotkeyResultPath =
        (std::filesystem::temp_directory_path() / L"bahamut-fps-hotkey-test.txt").wstring();
    DeleteFileW(fpsHotkeyResultPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_FPS_HOTKEY", L"1");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_FPS_HOTKEY_RESULT_FILE",
                            fpsHotkeyResultPath.c_str());
    const RunResult fpsHotkey = RunHelper(
        helper, CommonArgs(stub, runtime, 31));
    const std::string fpsHotkeyResult = ReadText(fpsHotkeyResultPath);
    const std::string fpsSettings     = ReadText(
        (addonSettings / L"fps" / L"settings.ini").wstring());
    if (!ExpectSuccess(fpsHotkey) || fpsSettings.find("visible=false") == std::string::npos || fpsHotkeyResult.find("f12_inactive_forward_pair=1") == std::string::npos || fpsHotkeyResult.find("retail_f12_suppressed=1") == std::string::npos)
    {
        std::cerr << "fps hotkey boundary failed: result="
                  << fpsHotkeyResult << " settings=" << fpsSettings
                  << " output=" << fpsHotkey.output << "\n";
        success = false;
    }
    SetEnvironmentVariableW(L"BAHAMUT_TEST_FPS_HOTKEY", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_FPS_HOTKEY_RESULT_FILE", nullptr);
    DeleteFileW(fpsHotkeyResultPath.c_str());

    const auto activeMonitors     = EnumerateBorderlessMonitors();
    auto       selectedMonitor    = std::find_if(activeMonitors.begin(), activeMonitors.end(), [](const auto& monitor)
                                                 {
                                            return !monitor.id.empty() && !monitor.primary;
                                                 });
    const bool selectedNonPrimary = selectedMonitor != activeMonitors.end();
    if (selectedMonitor == activeMonitors.end())
    {
        selectedMonitor = std::find_if(activeMonitors.begin(), activeMonitors.end(), [](const auto& monitor)
                                       {
                                           return !monitor.id.empty();
                                       });
    }
    const std::wstring requestedMonitorId = selectedMonitor == activeMonitors.end()
                                                ? std::wstring{}
                                                : selectedMonitor->id;
    std::cout << "borderless monitor fixture: active=" << activeMonitors.size()
              << " selected="
              << (requestedMonitorId.empty()
                      ? "nearest-window (no device identity available)"
                      : (selectedNonPrimary ? "non-primary" : "primary"));
    if (activeMonitors.size() <= 1)
    {
        std::cout << " (non-primary monitor selection skipped: one or fewer active monitors)";
    }
    std::cout << "\n";
    const std::wstring borderlessResultPath =
        (std::filesystem::temp_directory_path() / L"bahamut-borderless-test.txt").wstring();
    auto runBorderlessCase = [&](const wchar_t* monitorId, int sequence, const char* label)
    {
        DeleteFileW(borderlessResultPath.c_str());
        SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_BORDERLESS", L"true");
        SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_BORDERLESS_MONITOR", monitorId);
        SetEnvironmentVariableW(L"BAHAMUT_TEST_BORDERLESS_RESULT_FILE",
                                borderlessResultPath.c_str());
        const RunResult   result = RunHelper(helper, CommonArgs(stub, runtime, sequence));
        const std::string text   = ReadText(borderlessResultPath);
        const bool        passed = ExpectSuccess(result) &&
                                   text.find("applied=1") != std::string::npos &&
                                   text.find("preserved_after_reset=1") != std::string::npos;
        if (!passed)
        {
            std::cerr << label << " borderless boundary failed: result=" << text
                      << " output=" << result.output << "\n";
        }
        SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_BORDERLESS", nullptr);
        SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_BORDERLESS_MONITOR", nullptr);
        SetEnvironmentVariableW(L"BAHAMUT_TEST_BORDERLESS_RESULT_FILE", nullptr);
        DeleteFileW(borderlessResultPath.c_str());
        return passed;
    };
    success &= runBorderlessCase(nullptr, 32, "default nearest-window");
    if (!requestedMonitorId.empty())
    {
        success &= runBorderlessCase(requestedMonitorId.c_str(), 34, "selected monitor");
        success &= runBorderlessCase(L"missing-monitor-interface-id", 35, "missing monitor fallback");
    }

    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_BORDERLESS", L"false");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_BORDERLESS_RESULT_FILE",
                            borderlessResultPath.c_str());
    const RunResult   windowed       = RunHelper(helper, CommonArgs(stub, runtime, 33));
    const std::string windowedResult = ReadText(borderlessResultPath);
    if (!ExpectSuccess(windowed) || windowedResult.find("windowed_style_preserved=1") == std::string::npos)
    {
        std::cerr << "windowed boundary failed: result=" << windowedResult
                  << " output=" << windowed.output << "\n";
        success = false;
    }
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_BORDERLESS", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_BORDERLESS_RESULT_FILE", nullptr);
    DeleteFileW(borderlessResultPath.c_str());

    std::filesystem::remove(addonSettings / L"fps" / L"settings.ini", storageError);
    {
        std::ofstream script(startupScript, std::ios::trunc);
        script << "/bind f12 /fps\n"
               << "/addon unload fps\n";
    }
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_ADDON_MANIFESTS", fpsManifest.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_FPS_HOTKEY", L"1");
    const RunResult unloadScript = RunHelper(helper, CommonArgs(stub, runtime, 24));
    SetEnvironmentVariableW(L"BAHAMUT_TEST_FPS_HOTKEY", nullptr);
    const std::string unloadedFpsSettings = ReadText(
        (addonSettings / L"fps" / L"settings.ini").wstring());
    if (!ExpectSuccess(unloadScript) || unloadedFpsSettings.find("visible=true") == std::string::npos || unloadedFpsSettings.find("visible=false") != std::string::npos)
    {
        std::cerr << "startup unload did not override auto-load: output="
                  << unloadScript.output << " settings=" << unloadedFpsSettings
                  << "\n";
        success = false;
    }

    {
        std::ofstream script(startupScript, std::ios::trunc);
        script << "/include launcher.txt\n";
    }
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_ADDON_MANIFESTS", nullptr);
    const RunResult invalidScript = RunHelper(helper, CommonArgs(stub, runtime, 23));
    success                       = ExpectTerminated("client_runtime_rejects_invalid_startup_script",
                                                     invalidScript,
                                                     "RuntimeStartupScriptFailed") &&
                                    success;
    success                       = Expect("startup_script_failure_reports_physical_line",
                                           invalidScript,
                                           1,
                                           "line=1") &&
                                    success;
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_ADDON_CATALOG", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_ADDON_SETTINGS", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_CHAT_LOGS", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_STARTUP_SCRIPT", nullptr);
    std::filesystem::remove(startupScript, storageError);

    const std::wstring missingScreenshot =
        (std::filesystem::temp_directory_path() / L"bahamut-missing-screenshot.dll").wstring();
    DeleteFileW(missingScreenshot.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_PLUGIN",
                            missingScreenshot.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_ENABLED", L"false");
    success = ExpectSuccess(RunHelper(helper, CommonArgs(stub, runtime, 25))) && success;
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_ENABLED", L"true");
    const RunResult missingPlugin = RunHelper(
        helper, CommonArgs(stub, runtime, 26));
    success = ExpectTerminated("missing Screenshot plugin is isolated",
                               missingPlugin,
                               "RuntimeNativePluginFailed") &&
              success;
    success = Expect("missing Screenshot plugin names package",
                     missingPlugin,
                     1,
                     "plugin=\"screenshot\"") &&
              success;

    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_PLUGIN",
                            screenshotPlugin.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_SCREENSHOT_PLUGIN_ABI_MISMATCH", L"1");
    const RunResult incompatiblePlugin = RunHelper(
        helper, CommonArgs(stub, runtime, 27));
    success = ExpectTerminated("incompatible Screenshot ABI is isolated",
                               incompatiblePlugin,
                               "RuntimeNativePluginFailed") &&
              success;
    SetEnvironmentVariableW(L"BAHAMUT_TEST_SCREENSHOT_PLUGIN_ABI_MISMATCH", nullptr);

    SetEnvironmentVariableW(
        L"BAHAMUT_TEST_SCREENSHOT_PLUGIN_ID_UNTERMINATED", L"1");
    const RunResult invalidPluginId = RunHelper(
        helper, CommonArgs(stub, runtime, 29));
    success = ExpectTerminated("unterminated Screenshot identity is isolated",
                               invalidPluginId,
                               "RuntimeNativePluginFailed") &&
              success;
    SetEnvironmentVariableW(
        L"BAHAMUT_TEST_SCREENSHOT_PLUGIN_ID_UNTERMINATED", nullptr);

    const std::wstring callbackFaultHotkeyResult =
        (std::filesystem::temp_directory_path() / L"bahamut-callback-fault-hotkey-test.txt").wstring();
    DeleteFileW(callbackFaultHotkeyResult.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_PRINT_SCREEN_OWNERSHIP", L"1");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_PRINT_SCREEN_RESULT_FILE",
                            callbackFaultHotkeyResult.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_SCREENSHOT_PLUGIN_CALLBACK_FAULT", L"1");
    const RunResult callbackFault = RunHelper(
        helper, CommonArgs(stub, runtime, 28));
    const std::string callbackFaultHotkey = ReadText(callbackFaultHotkeyResult);
    success                               = ExpectSuccess(callbackFault) && callbackFault.output.find("overlay_frames=120") != std::string::npos && callbackFaultHotkey.find("owned_while_active=0 released_while_inactive=1") != std::string::npos && success;
    SetEnvironmentVariableW(L"BAHAMUT_TEST_SCREENSHOT_PLUGIN_CALLBACK_FAULT", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_PRINT_SCREEN_OWNERSHIP", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_PRINT_SCREEN_RESULT_FILE", nullptr);
    DeleteFileW(callbackFaultHotkeyResult.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_ENABLED", nullptr);

    const std::filesystem::path screenshotRoot =
        std::filesystem::temp_directory_path() / (L"bahamut-screenshot-test-" + std::to_wstring(GetCurrentProcessId()));
    std::filesystem::remove_all(screenshotRoot, storageError);
    std::filesystem::create_directories(screenshotRoot);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_SCREENSHOT_REQUEST", L"1");
    for (const auto& test : {
             std::tuple<const char*, const wchar_t*, const wchar_t*, const wchar_t*, std::vector<unsigned char>, bool, unsigned int>{
                 "PNG screenshot after device reset", L"png", L".png", L"print_screen", { 0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a }, true, 19 },
             std::tuple<const char*, const wchar_t*, const wchar_t*, const wchar_t*, std::vector<unsigned char>, bool, unsigned int>{
                 "BMP screenshot after device reset", L"bmp", L".bmp", L"insert", { 0x42, 0x4d }, false, 20 } })
    {
        const auto& [name, format, extension, hotkey, signature, hideOverlays, number] = test;
        const std::filesystem::path directory                                          = screenshotRoot / format;
        const std::filesystem::path resultPath                                         = screenshotRoot / (std::wstring(format) + L"-result.txt");
        std::filesystem::create_directories(directory);
        SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOTS",
                                directory.c_str());
        SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_FORMAT", format);
        SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_HIDE_OVERLAYS",
                                hideOverlays ? L"true" : L"false");
        SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_HOTKEY", hotkey);
        SetEnvironmentVariableW(L"BAHAMUT_TEST_SCREENSHOT_RESULT_FILE",
                                resultPath.c_str());
        success = VerifyScreenshot(name, directory, resultPath, extension, signature, hideOverlays, RunHelper(helper, CommonArgs(stub, runtime, number))) && success;
    }

    const std::filesystem::path blockedDirectory = screenshotRoot / L"blocked";
    const std::filesystem::path failureResult    = screenshotRoot / L"failure-result.txt";
    {
        std::ofstream blocked(blockedDirectory, std::ios::trunc);
        blocked << "not a directory\n";
    }
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOTS", blockedDirectory.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_FORMAT", L"png");
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_HIDE_OVERLAYS", L"true");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_SCREENSHOT_RESULT_FILE",
                            failureResult.c_str());
    const RunResult screenshotWriteFailure = RunHelper(
        helper, CommonArgs(stub, runtime, 21));
    const std::string failureText          = ReadText(failureResult.wstring());
    const bool        writeFailureSurvived = ExpectSuccess(screenshotWriteFailure) && failureText.find("status=failed") != std::string::npos && screenshotWriteFailure.output.find("overlay_frames=119") != std::string::npos;
    if (!writeFailureSurvived)
    {
        std::cerr << "screenshot write failure remains non-fatal failed: result="
                  << failureText << " output=" << screenshotWriteFailure.output << "\n";
        success = false;
    }
    SetEnvironmentVariableW(L"BAHAMUT_TEST_SCREENSHOT_REQUEST", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOTS", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_FORMAT", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_HIDE_OVERLAYS", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_HOTKEY", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_SCREENSHOT_RESULT_FILE", nullptr);
    std::filesystem::remove_all(screenshotRoot, storageError);

    const std::wstring printScreenResultPath =
        (std::filesystem::temp_directory_path() / L"bahamut-print-screen-ownership-test.txt").wstring();
    const std::wstring printScreenRegistrationPath =
        (std::filesystem::temp_directory_path() / L"bahamut-print-screen-registration-test.txt").wstring();
    DeleteFileW(printScreenResultPath.c_str());
    DeleteFileW(printScreenRegistrationPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_HOTKEY", L"print_screen");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_PRINT_SCREEN_OWNERSHIP", L"1");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_PRINT_SCREEN_RESULT_FILE",
                            printScreenResultPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_INPUT_RESULT_FILE",
                            printScreenRegistrationPath.c_str());
    const RunResult printScreenOwnership = RunHelper(
        helper, CommonArgs(stub, runtime, 22));
    const std::string printScreenResult       = ReadText(printScreenResultPath);
    const std::string printScreenRegistration = ReadText(printScreenRegistrationPath);
    if (!ExpectSuccess(printScreenOwnership) || printScreenRegistration.find("print_screen_registration=ok") == std::string::npos || printScreenResult.find("owned_while_active=1 released_while_inactive=1") == std::string::npos)
    {
        std::cerr << "Print Screen ownership lifecycle failed: result="
                  << printScreenResult << " registration=" << printScreenRegistration
                  << " output=" << printScreenOwnership.output << "\n";
        success = false;
    }

    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_HOTKEY", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_PRINT_SCREEN_OWNERSHIP", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_PRINT_SCREEN_RESULT_FILE", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_INPUT_RESULT_FILE", nullptr);
    DeleteFileW(printScreenResultPath.c_str());
    DeleteFileW(printScreenRegistrationPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_SCREENSHOT_PLUGIN", nullptr);

    const std::wstring malformed =
        (std::filesystem::temp_directory_path() / L"bahamut-runtime-malformed.dll").wstring();
    DeleteFileW(malformed.c_str());
    if (!WriteMalformedExportImage(malformed))
    {
        std::cerr << "malformed_export_is_rejected failed: could not write fixture\n";
        success = false;
    }
    success = Expect("malformed_export_is_rejected", RunHelper(helper, CommonArgs(stub, malformed, 2) + L" --fault malformed-export"), 0, "SUCCESS malformed_export_rejected") && success;
    DeleteFileW(malformed.c_str());

    success = ExpectTerminated("post_create_exception_cleans_target", RunHelper(helper, CommonArgs(stub, runtime, 3) + L" --fault post-create-exception"), "BootstrapException") && success;

    const std::wstring telemetryPath =
        (std::filesystem::temp_directory_path() / L"bahamut-loader-terminal.toml").wstring();
    DeleteFileW(telemetryPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_TELEMETRY_FILE", telemetryPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_TIMING_SEQUENCE_ODD", L"1");
    success = ExpectTerminated("early_exit_writes_telemetry", RunHelper(helper, CommonArgs(stub, runtime, 4) + L" --no-signal --timeout-ms 250"), "RuntimeReadyTimeout") && success;
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_TELEMETRY_FILE", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_TIMING_SEQUENCE_ODD", nullptr);
    {
        std::ifstream     telemetry{ std::filesystem::path{ telemetryPath } };
        const std::string text((std::istreambuf_iterator<char>(telemetry)), {});
        const bool        recorded = text.find("terminal = true") != std::string::npos && text.find("terminal_code = RuntimeReadyTimeout") != std::string::npos && text.find("timing_snapshot_valid = false") != std::string::npos;
        if (!recorded)
        {
            std::cerr << "early_exit_writes_telemetry failed: output=" << text << "\n";
        }
        success = recorded && success;
    }

    success = ExpectExit("patch_payload_size_rejected", RunHelper(helper, L"--test-stub --client " + Quote(stub) + L" --module " + Quote(runtime) + L" --server-utc A1 --lobby-host 11223300"), 2) && success;

    success = Expect("wrong identity", RunHelper(helper, L"--test-stub --client " + Quote(helper) + L" --module " + Quote(runtime) + L" --server-utc A1B2C3D4E5 --lobby-host 11223300 --event-prefix " + Quote(Prefix(5))), 1, "ERROR WrongClientIdentity no_target_created") && success;

    const std::wstring faultPrefix = L" --fault ";
    const RunResult    loadTimeout = RunHelper(helper,
                                               CommonArgs(stub, runtime, 6) + faultPrefix + L"remote-load-timeout");
    success                        = ExpectTerminated("remote_load_timeout_retains_buffer", loadTimeout, "RuntimeLoadFailed") && success;
    success                        = Expect("remote_load_timeout_retains_buffer resources", loadTimeout, 1, "DIAG remote_resources_retained threads=1 buffers=1") && success;

    success = ExpectTerminated("helper_patch_restore_failure", RunHelper(helper, CommonArgs(stub, runtime, 7) + faultPrefix + L"patch-restore-failure"), "PatchApplyFailed") && success;
    success = ExpectTerminated("remote_runtime_identity_mismatch", RunHelper(helper, CommonArgs(stub, runtime, 8) + faultPrefix + L"runtime-identity-mismatch"), "RuntimeInitializeAddressFailed") && success;
    success = ExpectTerminated("runtime_api_version_mismatch", RunHelper(helper, CommonArgs(stub, runtime, 9) + faultPrefix + L"runtime-api-version-mismatch"), "RuntimeApiVersionMismatch") && success;
    success = Expect("remote_module_lookup_has_no_fixed_sleep", RunHelper(helper, CommonArgs(stub, runtime, 10) + faultPrefix + L"remote-module-lookup"), 0, "SUCCESS remote_module_lookup") && success;

    const std::wstring ownerTelemetryPath =
        (std::filesystem::temp_directory_path() / L"bahamut-render-owner-terminal.toml").wstring();
    DeleteFileW(ownerTelemetryPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_TELEMETRY_FILE", ownerTelemetryPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_RENDER_BOUNDARY_OWNER_FAILURE", L"1");
    const RunResult ownerFailure = RunHelper(helper, CommonArgs(stub, runtime, 18));
    success                      = ExpectTerminated("render_owner_failure_reports_target_termination",
                                                    ownerFailure,
                                                    "RenderBoundaryFailed") &&
                                   success;
    success                      = Expect("render_owner_failure_reports_stage", ownerFailure, 1, "DIAG render_boundary_stage=8") && success;
    success                      = Expect("render_owner_failure_reports_path", ownerFailure, 1, "stage=8 owner_path=\"") && success;
    {
        const std::string text          = ReadText(ownerTelemetryPath);
        const std::string runtimeName   = Narrow(std::filesystem::path(runtime).filename().wstring());
        const bool        ownerRecorded = text.find("terminal_code = RenderBoundaryFailed") != std::string::npos && text.find("render_boundary_stage = 8") != std::string::npos && text.find("render_boundary_owner_path = \"") != std::string::npos && text.find(runtimeName) != std::string::npos && ownerFailure.output.find(runtimeName) != std::string::npos;
        if (!ownerRecorded)
        {
            std::cerr << "render_owner_failure_reports_path failed: output="
                      << ownerFailure.output << " telemetry=" << text << "\n";
            success = false;
        }
    }
    SetEnvironmentVariableW(L"BAHAMUT_TEST_RENDER_BOUNDARY_OWNER_FAILURE", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_RUNTIME_TELEMETRY_FILE", nullptr);
    DeleteFileW(ownerTelemetryPath.c_str());

    const std::wstring renderResultPath =
        (std::filesystem::temp_directory_path() / L"bahamut-render-boundary-test.txt").wstring();
    DeleteFileW(renderResultPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_RENDER_BOUNDARY_FAILURE", L"1");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_RENDER_RESULT_FILE", renderResultPath.c_str());
    const RunResult renderFailure = RunHelper(helper, CommonArgs(stub, runtime, 11));
    success                       = ExpectTerminated("render_boundary_failure_rolls_back", renderFailure, "RenderBoundaryFailed") && success;
    success                       = Expect("render_boundary_failure_rolls_back rollback", renderFailure, 1, "DIAG render_boundary_stage=10") && success;
    success                       = ReadText(renderResultPath).find("render_boundary_rollback=ok") != std::string::npos && success;
    SetEnvironmentVariableW(L"BAHAMUT_TEST_RENDER_BOUNDARY_FAILURE", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_RENDER_RESULT_FILE", nullptr);

    DeleteFileW(renderResultPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_RENDER_DEVICE_HOOK_FAILURE", L"1");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_RENDER_RESULT_FILE", renderResultPath.c_str());
    const RunResult renderDeviceFailure = RunHelper(helper, CommonArgs(stub, runtime, 15));
    success                             = ExpectTerminated("render_boundary_failure_rolls_back reset/present",
                                                           renderDeviceFailure,
                                                           "TargetEntryTimeout") &&
                                          success;
    success                             = ReadText(renderResultPath).find("render_device_rollback=ok") != std::string::npos && success;
    SetEnvironmentVariableW(L"BAHAMUT_TEST_RENDER_DEVICE_HOOK_FAILURE", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_RENDER_RESULT_FILE", nullptr);

    const std::wstring overlayResultPath =
        (std::filesystem::temp_directory_path() / L"bahamut-overlay-restore-test.txt").wstring();
    const std::wstring stubResultPath =
        (std::filesystem::temp_directory_path() / L"bahamut-stub-state-test.txt").wstring();
    DeleteFileW(overlayResultPath.c_str());
    DeleteFileW(stubResultPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_RESTORE_FAILURE", L"1");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_RESULT_FILE", overlayResultPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_STUB_STATE_CORRUPTION", L"1");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_STUB_RESULT_FILE", stubResultPath.c_str());
    const RunResult overlayFailure  = RunHelper(helper, CommonArgs(stub, runtime, 12));
    success                         = ExpectTerminated("overlay_restore_failure_disables_draw", overlayFailure, "StubValidationFailed") && success;
    success                         = Expect("overlay_restore_failure_disables_draw result", overlayFailure, 1, "ERROR StubValidationFailed") && success;
    const std::string overlayResult = ReadText(overlayResultPath);
    success                         = overlayResult.find("overlay_restore=failed") != std::string::npos && overlayResult.find("visible=0") != std::string::npos && success;
    success                         = ReadText(stubResultPath).find("state_failure=59") != std::string::npos && success;
    SetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_RESTORE_FAILURE", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_RESULT_FILE", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_STUB_STATE_CORRUPTION", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_STUB_RESULT_FILE", nullptr);

    const std::wstring inputResultPath =
        (std::filesystem::temp_directory_path() / L"bahamut-input-boundary-test.txt").wstring();
    DeleteFileW(inputResultPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_INPUT_BOUNDARY_FAILURE", L"1");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_INPUT_RESULT_FILE", inputResultPath.c_str());
    const RunResult inputFailure = RunHelper(helper, CommonArgs(stub, runtime, 13));
    success                      = ExpectTerminated("input_boundary_failure_rolls_back", inputFailure, "TargetEntryTimeout") && success;
    success                      = ReadText(inputResultPath).find("input_boundary_rollback=ok") != std::string::npos && success;
    SetEnvironmentVariableW(L"BAHAMUT_TEST_INPUT_BOUNDARY_FAILURE", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_INPUT_RESULT_FILE", nullptr);

    const std::wstring directInputResultPath =
        (std::filesystem::temp_directory_path() / L"bahamut-directinput-boundary-test.txt").wstring();
    DeleteFileW(directInputResultPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_DIRECTINPUT_BOUNDARY", L"1");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_DIRECTINPUT_RESULT_FILE",
                            directInputResultPath.c_str());
    const RunResult directInputBoundary = RunHelper(
        helper, CommonArgs(stub, runtime, 16));
    success                             = ExpectExit("DirectInput boundary", directInputBoundary, 0) && success;
    const std::string directInputResult = ReadText(directInputResultPath);
    success                             = directInputResult.find(
                                              "directinput_b_forward=1 directinput_chord_forward=1 "
                                              "directinput_a_forward=1 "
                                              "directinput_disconnect=1 directinput_reconnect=1") != std::string::npos &&
                                          success;
    success                             = directInputResult.find("original_calls=120") != std::string::npos && success;
    SetEnvironmentVariableW(L"BAHAMUT_TEST_DIRECTINPUT_BOUNDARY", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_DIRECTINPUT_RESULT_FILE", nullptr);

    const std::wstring xInputResultPath =
        (std::filesystem::temp_directory_path() / L"bahamut-xinput-boundary-test.txt").wstring();
    DeleteFileW(xInputResultPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_XINPUT_BOUNDARY", L"1");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_XINPUT_RESULT_FILE",
                            xInputResultPath.c_str());
    const RunResult xInputBoundary = RunHelper(
        helper, CommonArgs(stub, runtime, 17));
    success                        = ExpectExit("XInput boundary", xInputBoundary, 0) && success;
    const std::string xInputResult = ReadText(xInputResultPath);
    success                        = xInputResult.find(
                                         "xinput_a_forward=1 xinput_b_forward=1 "
                                         "xinput_chord_forward=1") != std::string::npos &&
                                     success;
    SetEnvironmentVariableW(L"BAHAMUT_TEST_XINPUT_BOUNDARY", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_XINPUT_RESULT_FILE", nullptr);

    const std::wstring backendResultPath =
        (std::filesystem::temp_directory_path() / L"bahamut-overlay-backend-test.txt").wstring();
    DeleteFileW(backendResultPath.c_str());
    SetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_BACKEND_RETRY", L"1");
    SetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_RESULT_FILE", backendResultPath.c_str());
    const RunResult backendRetry    = RunHelper(helper, CommonArgs(stub, runtime, 14));
    success                         = Expect("overlay_backend_recreation_retries", backendRetry, 1, "ERROR OverlayValidationFailed") && success;
    const std::string backendResult = ReadText(backendResultPath);
    if (backendResult.find("backend_retry=ok") == std::string::npos)
    {
        std::cerr << "overlay_backend_recreation_retries failed: result="
                  << backendResult << "\n";
        success = false;
    }
    SetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_BACKEND_RETRY", nullptr);
    SetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_RESULT_FILE", nullptr);

    success = WaitForClientReportsExit(helper, stub, runtime) && success;

    // Outside --test-stub, reaching the identity check proves the parser accepted the arguments.
    const std::wstring productionArgs = L"--client " + Quote(helper) + L" --module " + Quote(runtime) + L" --server-utc A1B2C3D4E5 --lobby-host 11223300";
    success                           = Expect("wait_for_client_accepts_timeout", RunHelper(helper, productionArgs + L" --wait-for-client --timeout-ms 20000"), 1, "ERROR WrongClientIdentity no_target_created") && success;
    success                           = Expect("production_timeout_requires_wait_for_client", RunHelper(helper, productionArgs + L" --timeout-ms 20000"), 2, "ERROR InvalidArguments no_target_created") && success;

    std::wstring eightPatches;
    for (int count = 0; count < 8; ++count)
    {
        eightPatches += L" --patch 1000:90";
    }
    success = Expect("patch_limit_accepts_eight", RunHelper(helper, productionArgs + eightPatches), 1, "ERROR WrongClientIdentity no_target_created") && success;
    success = Expect("patch_limit_rejects_nine", RunHelper(helper, productionArgs + eightPatches + L" --patch 1000:90"), 2, "ERROR InvalidArguments no_target_created") && success;
    success = Expect("patch_accepts_64_bytes", RunHelper(helper, productionArgs + L" --patch 1000:" + std::wstring(128, L'9')), 1, "ERROR WrongClientIdentity no_target_created") && success;
    for (const std::wstring& malformedPatch : std::vector<std::wstring>{ L"1000", L":90", L"123456789:90", L"10g0:90", L"1000:", L"1000:9", L"1000:zz", L"1000:" + std::wstring(130, L'9') })
    {
        if (!Expect("malformed_patch_is_rejected", RunHelper(helper, productionArgs + L" --patch " + malformedPatch), 2, "ERROR InvalidArguments no_target_created"))
        {
            std::cerr << "  rejected value: " << Narrow(malformedPatch) << "\n";
            success = false;
        }
    }
    success = Expect("patch_outside_client_image_is_rejected", RunHelper(helper, CommonArgs(stub, runtime, 38) + L" --patch ffffff00:90"), 1, "ERROR PatchApplyFailed no_target_created") && success;

    // The stub copies this literal into its DirectInput result, so the result
    // shows whether the client ran with the patched bytes.
    const auto originalCallsRva = UniqueTextRva(stub, " original_calls=");
    if (!originalCallsRva)
    {
        std::cerr << "patch_is_visible_to_client failed: stub literal is missing or not unique\n";
        success = false;
    }
    else
    {
        const std::wstring patchResultPath =
            (std::filesystem::temp_directory_path() / L"bahamut-patch-boundary-test.txt").wstring();
        DeleteFileW(patchResultPath.c_str());
        SetEnvironmentVariableW(L"BAHAMUT_TEST_DIRECTINPUT_BOUNDARY", L"1");
        SetEnvironmentVariableW(L"BAHAMUT_TEST_DIRECTINPUT_RESULT_FILE", patchResultPath.c_str());
        const RunResult   patched       = RunHelper(helper, CommonArgs(stub, runtime, 37) + PatchArgument(*originalCallsRva, " ORIGINAL_CALLS="));
        const std::string patchedResult = ReadText(patchResultPath);
        if (patchedResult.find(" ORIGINAL_CALLS=") == std::string::npos || patchedResult.find(" original_calls=") != std::string::npos)
        {
            std::cerr << "patch_is_visible_to_client failed: result=" << patchedResult
                      << " exit=" << patched.exitCode << " output=" << patched.output << "\n";
            success = false;
        }
        SetEnvironmentVariableW(L"BAHAMUT_TEST_DIRECTINPUT_BOUNDARY", nullptr);
        SetEnvironmentVariableW(L"BAHAMUT_TEST_DIRECTINPUT_RESULT_FILE", nullptr);
        DeleteFileW(patchResultPath.c_str());
    }

    if (success)
    {
        std::cout << "bootstrap tests passed: direct launch, overlay, N02, and N06 guards\n";
        return 0;
    }
    return 1;
}
