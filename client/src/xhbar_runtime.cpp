#include "xhbar_runtime.h"

#include "fault_guard.h"
#include "retail_identity.h"
#include "runtime_contract.h"
#include "xhbar_pad.h"
#include "xhbar_slots.h"

#include <MinHook.h>
#include <windows.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <cwchar>
#include <fstream>
#include <intrin.h>
#include <iterator>
#include <limits>
#include <string>

namespace bahamut_client::xhbar
{
namespace
{

// Retail 1.23b: MainModule input step is __thiscall, ret 0xC; routing finishes before return.
// Registry [0x01358728]; storyboard type-1 parameter is 0x78 bytes (0x005437B1-0x005437E9).
constexpr std::uintptr_t kInputStepVa      = 0x004d6570;
constexpr std::uintptr_t kCommandLookupVa  = 0x00928280;
constexpr std::uintptr_t kParameterBuildVa = 0x004ddcf0;
constexpr std::uintptr_t kStringReleaseVa  = 0x00446f50;
constexpr std::size_t    kParameterBytes   = 0x78;
constexpr std::size_t    kParameterString  = 4;

constexpr std::array<unsigned char, 6> kInputStepPrefix      = { 0x53, 0x55, 0x8b, 0x6c, 0x24, 0x14 };
constexpr std::array<unsigned char, 5> kCommandLookupPrefix  = { 0xa1, 0x28, 0x87, 0x35, 0x01 };
constexpr std::array<unsigned char, 7> kParameterBuildPrefix = { 0x6a, 0xff, 0x68, 0xab, 0x22, 0xe9, 0x00 };
constexpr std::array<unsigned char, 6> kStringReleasePrefix  = { 0x80, 0x79, 0x11, 0x00, 0x75, 0x11 };

constexpr char kShortCutAction[] = "UILuaCommands.ShortCutAction";

// Lookup compares only the character pointer and NUL-inclusive length (0x00445D20).
struct NativeName
{
    const char*   characters;
    std::uint32_t capacity;
    std::uint32_t length;
};

using InputStepFunction      = void(__thiscall*)(void*, void*, void*, void*);
using CommandLookupFunction  = void*(__cdecl*)(const NativeName*);
using ParameterBuildFunction = void*(__thiscall*)(void*, std::int32_t);
using StringReleaseFunction  = void(__thiscall*)(void*);
// Binding executor (0x00921200-0x00921233): vtable +4 CanExecute(V, T), +8 Execute(V, T, E).
using CanExecuteFunction = bool(__thiscall*)(void*, void*, void*);
using ExecuteFunction    = bool(__thiscall*)(void*, void*, void*, std::uint32_t);

InputStepFunction                                  gOriginalInputStep = nullptr;
std::atomic<bool>                                  gInstalled{ false };
std::atomic<int>                                   gSide{ static_cast<int>(Side::None) };
std::atomic<DWORD>                                 gPadThread{ 0 };
std::array<std::atomic<std::uint64_t>, kSlotEdges> gDispatchTicks{};
// Pack icon and state from one read to prevent mixed snapshots.
std::array<std::atomic<std::uint64_t>, kSlotEdges> gSlotViews{};
std::uint64_t                                      gSlotReadTick = 0;
thread_local int                                   gInputDepth   = 0;

// Pad-thread-only state, including input-step access.
InputPolicy             gPolicy;
Sample                  gSample;
std::array<SlotEdge, 8> gPending{};
std::size_t             gPendingCount = 0;

bool IsEnabled()
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(L"BAHAMUT_RUNTIME_XHBAR_ENABLED", value, ARRAYSIZE(value));
    return count == 4 && std::wcscmp(value, L"true") == 0;
}

bool IsTestStub()
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(L"BAHAMUT_RUNTIME_TEST_STUB", value, ARRAYSIZE(value));
    // The loader sets "0" for retail launches.
    return count != 0 && count < ARRAYSIZE(value) && value[0] != L'0';
}

template <std::size_t Size>
bool HasCode(std::uintptr_t address, const std::array<unsigned char, Size>& expected)
{
    MEMORY_BASIC_INFORMATION memory{};
    if (VirtualQuery(reinterpret_cast<void*>(address), &memory, sizeof(memory)) == 0 || memory.State != MEM_COMMIT)
        return false;
    const DWORD protection = memory.Protect & 0xff;
    if (protection != PAGE_EXECUTE_READ && protection != PAGE_EXECUTE_READWRITE && protection != PAGE_EXECUTE_WRITECOPY)
        return false;
    return std::memcmp(reinterpret_cast<const void*>(address), expected.data(), Size) == 0;
}

void Dispatch(int number)
{
    const NativeName name{ kShortCutAction, sizeof(kShortCutAction), sizeof(kShortCutAction) };
    void*            command = reinterpret_cast<CommandLookupFunction>(kCommandLookupVa)(&name);
    if (command == nullptr)
        return;
    alignas(8) std::array<unsigned char, kParameterBytes> parameter{};
    reinterpret_cast<ParameterBuildFunction>(kParameterBuildVa)(parameter.data(), number);
    void** table = *static_cast<void***>(command);
    if (reinterpret_cast<CanExecuteFunction>(table[1])(command, parameter.data(), nullptr))
        reinterpret_cast<ExecuteFunction>(table[2])(command, parameter.data(), nullptr, 0);
    reinterpret_cast<StringReleaseFunction>(kStringReleaseVa)(parameter.data() + kParameterString);
}

bool CopyNativeMemory(void*, std::uint32_t address, void* output, std::size_t bytes) noexcept
{
    if (output == nullptr || address == 0 || bytes == 0 || bytes - 1 > std::numeric_limits<std::uint32_t>::max() - address)
        return false;
    volatile bool copied = false;
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(output, reinterpret_cast<const void*>(static_cast<std::uintptr_t>(address)), bytes);
        copied = true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        copied = false;
    }
    return copied;
}

bool WriteNativeByte(std::uint32_t address, std::int8_t value) noexcept
{
    volatile bool written = false;
    BAHAMUT_FAULT_TRY
    {
        *reinterpret_cast<volatile std::int8_t*>(static_cast<std::uintptr_t>(address)) = value;
        written                                                                        = true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        written = false;
    }
    return written;
}

SlotCommands ReadMenu(void* mainModule, bool labels, SlotStates* states = nullptr)
{
    return ReadSlotCommands(static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(mainModule)),
                            static_cast<std::uint32_t>(__readfsdword(0x2c)),
                            &CopyNativeMemory,
                            nullptr,
                            labels,
                            states);
}

// ShortCutAction reads work.slotPage inside Execute; restore before native input or other readers.
// Restore twice because a guard page faults once; nested input or failed restores disable swaps.
struct PageOverride
{
    std::uint32_t address    = 0;
    std::int8_t   page       = 0;
    std::int8_t   substitute = 0;
};

PageOverride      gPageOverride;
std::atomic<bool> gSwapBroken{ false };

void RestorePage()
{
    if (gPageOverride.address == 0)
        return;
    if (WriteNativeByte(gPageOverride.address, gPageOverride.page) || WriteNativeByte(gPageOverride.address, gPageOverride.page))
    {
        gPageOverride = {};
        return;
    }
    gSwapBroken.store(true, std::memory_order_release);
}

struct PageRestore
{
    PageRestore()                              = default;
    PageRestore(const PageRestore&)            = delete;
    PageRestore& operator=(const PageRestore&) = delete;

    ~PageRestore()
    {
        RestorePage();
    }
};

bool DispatchSlot(void* mainModule, ShortcutTarget target)
{
    if (gPageOverride.address != 0)
        return false;
    const SlotCommands menu = ReadMenu(mainModule, false);
    if (!menu.valid || menu.macroType != 0 || menu.slotMoving)
        return false;
    if (menu.page == target.page)
    {
        Dispatch(target.button);
        return true;
    }
    if (gSwapBroken.load(std::memory_order_acquire) || menu.pageAddress == 0)
        return false;
    std::int8_t current = 0;
    if (!CopyNativeMemory(nullptr, menu.pageAddress, &current, sizeof(current)) || current != menu.page)
        return false;
    if (!WriteNativeByte(menu.pageAddress, static_cast<std::int8_t>(target.page)))
        return false;
    gPageOverride = { menu.pageAddress, current, static_cast<std::int8_t>(target.page) };
    // Do not fault-guard native Execute: catching its structured exception would resume the game mid-command.
    PageRestore restore;
    Dispatch(target.button);
    return true;
}

// Retry only at the live page address while it holds the substitute; never write a reused buffer.
void RetryRestore(void* mainModule)
{
    const SlotCommands menu  = ReadMenu(mainModule, false);
    std::int8_t        value = 0;
    if (menu.pageAddress == gPageOverride.address && CopyNativeMemory(nullptr, gPageOverride.address, &value, sizeof(value)) &&
        value == gPageOverride.substitute)
    {
        RestorePage();
        return;
    }
    gPageOverride = {};
}

// Edges expire each update; fixed generation guards only the policy's disable/release state.
void DispatchPending(void* mainModule)
{
    for (std::size_t index = 0; index < gPendingCount; ++index)
    {
        const std::size_t    slot   = gPending[index].slot;
        const ShortcutTarget target = Shortcut(slot);
        if (target.page != 0 && gPolicy.IsCurrent(gPending[index], gSample) && DispatchSlot(mainModule, target))
            gDispatchTicks[slot].store(GetTickCount64(), std::memory_order_release);
    }
    gPendingCount = 0;
}

// Setters: __thiscall(value, policy, notify), 4-byte slots, ret 0xC, AL = admitted.
// Visibility 0x0093C2A0: +0x158 (0 Collapsed, 1 Hidden, 2 Visible); IsHitTestVisible 0x0093C300: +0x15A bit 0x20.
// VisualOpacity 0x009402C0: float +0x48.
constexpr std::uintptr_t kSetVisibilityVa = 0x0093c2a0;
constexpr std::uintptr_t kSetHitTestVa    = 0x0093c300;
constexpr std::uintptr_t kSetOpacityVa    = 0x009402c0;

constexpr std::array<unsigned char, 16> kSetVisibilityPrefix = { 0x56, 0x8b, 0x74, 0x24, 0x10, 0x56, 0x8b, 0x74, 0x24, 0x10, 0x56, 0x68, 0xbc, 0x96, 0x35, 0x01 };
constexpr std::array<unsigned char, 16> kSetHitTestPrefix    = { 0x56, 0x8b, 0x74, 0x24, 0x10, 0x56, 0x8b, 0x74, 0x24, 0x10, 0x56, 0x68, 0x54, 0x93, 0x35, 0x01 };
constexpr std::array<unsigned char, 16> kSetOpacityPrefix    = { 0x8b, 0x44, 0x24, 0x0c, 0xd9, 0x44, 0x24, 0x04, 0x8b, 0x54, 0x24, 0x08, 0x50, 0x52, 0x68, 0x8c };

using SetEnumFunction  = bool(__thiscall*)(void*, std::uint32_t, std::uint32_t, std::uint32_t);
using SetFloatFunction = bool(__thiscall*)(void*, float, std::uint32_t, std::uint32_t);

constexpr std::uint32_t kGridTable    = 0x01074554;
constexpr std::uint8_t  kHiddenEnum   = 1;
constexpr std::uint8_t  kVisibleEnum  = 2;
constexpr std::uint8_t  kHitTestBit   = 0x20;
constexpr std::uint64_t kHideGrace    = 2000;
constexpr std::uint32_t kRestoreTries = 60;

// GetTickCount64 timestamps; 0 means unavailable.
std::atomic<std::uint64_t> gAvailableTick{ 0 };
std::atomic<std::uint64_t> gDrawnTick{ 0 };
// Failed Desktop reads leave the overlay available.
std::atomic<bool> gWithheld{ false };
bool              gSettersPresent = false;
bool              gHideBroken     = false; // failed restores forbid new hides this session

std::atomic<LabelStyle> gLabels{ LabelStyle::Xbox };
std::atomic<bool>       gVisible{ true };
std::filesystem::path   gSettingsFile;

bool WriteSettingsFile()
{
    if (gSettingsFile.empty())
        return false;
    std::error_code error;
    std::filesystem::create_directories(gSettingsFile.parent_path(), error);
    std::ofstream output(gSettingsFile, std::ios::binary | std::ios::trunc);
    output << WriteSettings({ gLabels.load(std::memory_order_acquire), gVisible.load(std::memory_order_acquire) });
    // Close before checking failure so buffered write errors are reported.
    output.close();
    return !output.fail();
}

HANDLE        gLog      = INVALID_HANDLE_VALUE;
std::uint64_t gLogStart = 0;

void Log(const char* format, ...)
{
    if (gLog == INVALID_HANDLE_VALUE)
        return;
    char    line[512];
    va_list arguments;
    va_start(arguments, format);
    const int length = std::vsnprintf(line, sizeof(line) - 1, format, arguments);
    va_end(arguments);
    if (length <= 0)
        return;
    const DWORD bytes = static_cast<DWORD>(length < static_cast<int>(sizeof(line) - 1) ? length : sizeof(line) - 2);
    line[bytes]       = '\n';
    DWORD written     = 0;
    WriteFile(gLog, line, bytes + 1, &written, nullptr);
}

std::uint64_t LogTime()
{
    return GetTickCount64() - gLogStart;
}

void OpenLog()
{
    wchar_t     path[1024]{};
    const DWORD count = GetEnvironmentVariableW(L"BAHAMUT_TEST_XHBAR_LOG_FILE", path, ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
        return;
    gLog      = CreateFileW(path, GENERIC_WRITE, FILE_SHARE_READ, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr);
    gLogStart = GetTickCount64();
    Log("start,setters=%d", gSettersPresent ? 1 : 0);
}

template <typename T>
bool ReadNative(std::uint32_t address, T& value)
{
    return CopyNativeMemory(nullptr, address, &value, sizeof(value));
}

// Pad-thread-only override; saved values belong to window.
struct HideState
{
    std::uint64_t allowedTick     = 0; // latest allowed read; 0 forbids hiding
    std::uint32_t window          = 0;
    std::uint32_t grid            = 0;
    std::uint8_t  gridVisibility  = kVisibleEnum;
    bool          hitTest         = true;
    float         opacity         = 1.0f;
    std::uint32_t restoreAttempts = 0;
    std::uint8_t  rootVisibility  = 0xff; // last logged
    std::uint64_t withheldTick    = 0;    // latest withheld step
};

HideState gHide;

struct Seen
{
    bool          menu    = false;
    std::uint32_t window  = 0;
    int           macro   = -1;
    bool          states  = false;
    std::uint32_t shared  = 0;
    bool          desktop = false;
    bool          locked  = false;
    int           mode    = -1;
    bool          map     = false;
};

Seen gSeen;

// Read on the input step's thread because Sqwt is thread-local.
void RefreshSlots(void* mainModule)
{
    constexpr std::uint64_t kSlotReadInterval = 250;
    const std::uint64_t     now               = GetTickCount64();
    if (now - gSlotReadTick < kSlotReadInterval)
        return;
    gSlotReadTick = now;
    SlotStates         states;
    const SlotCommands current = ReadMenu(mainModule, true, &states);
    const bool         action  = current.valid && current.macroType == 0;
    for (std::size_t slot = 0; slot < gSlotViews.size(); ++slot)
    {
        const ShortcutTarget target       = Shortcut(slot);
        std::uint32_t        icon         = 0;
        std::uint32_t        remaining    = 0;
        bool                 incompatible = false;
        if (target.page != 0)
        {
            const auto position = static_cast<std::size_t>((target.page - 1) * 10 + target.button - 1);
            if (action && current.commands[position] > 0)
            {
                icon = current.icons[position];
                if (states.valid)
                {
                    remaining    = static_cast<std::uint32_t>(std::min(states.remaining[position], 0xffff));
                    incompatible = !states.compatible[position];
                }
            }
        }
        const std::uint64_t value = (icon & 0x00ffffffu) | (static_cast<std::uint64_t>(target.page) << 24) | (static_cast<std::uint64_t>(remaining) << 32) |
                                    (static_cast<std::uint64_t>(incompatible ? 1 : 0) << 48);
        const std::uint64_t old   = gSlotViews[slot].exchange(value, std::memory_order_acq_rel);
        // Log recast transitions, not each countdown tick.
        const bool started = ((old >> 32) & 0xffff) != 0;
        if (target.page != 0 && ((old & 0x00ffffffu) != icon || ((old >> 48) & 1) != (incompatible ? 1u : 0u) || started != (remaining != 0)))
            Log("slot,t=%llu,position=%d,page=%d,icon=%u,remaining=%u,incompatible=%d,now=%d",
                LogTime(),
                (target.page - 1) * 10 + target.button,
                current.page,
                icon,
                remaining,
                incompatible ? 1 : 0,
                states.now);
    }
    if (current.valid)
        gAvailableTick.store(now, std::memory_order_release);
    const DesktopState desktop =
        ReadDesktop(static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(mainModule)), &CopyNativeMemory, nullptr);
    gWithheld.store(desktop.valid && (desktop.locked || desktop.map), std::memory_order_release);

    if (current.valid && current.macroType != 0)
        gHide.allowedTick = 0;
    else if (action && states.valid)
        gHide.allowedTick = now;

    if (current.valid != gSeen.menu || current.window != gSeen.window || (current.valid ? current.macroType : -1) != gSeen.macro)
        Log("menu,t=%llu,valid=%d,window=%08x,macro=%d,page=%d", LogTime(), current.valid ? 1 : 0, current.window, current.macroType, current.page);
    if (states.valid != gSeen.states || states.shared != gSeen.shared)
        Log("player,t=%llu,valid=%d,shared=%08x,now=%d", LogTime(), states.valid ? 1 : 0, states.shared, states.now);
    if (desktop.valid != gSeen.desktop || desktop.locked != gSeen.locked || desktop.mode != gSeen.mode || desktop.map != gSeen.map)
        Log("desktop,t=%llu,valid=%d,mode=%d,locked=%d,map=%d", LogTime(), desktop.valid ? 1 : 0, desktop.mode, desktop.locked ? 1 : 0, desktop.map ? 1 : 0);
    gSeen = { current.valid, current.window, current.valid ? current.macroType : -1, states.valid, states.shared, desktop.valid, desktop.locked, desktop.mode, desktop.map };
}

// Grid_Content: template [[W+0x318]+0xB0] (0x00936DF0), logical parent +0x240 must be W; unreadable differs from absent.
std::uint32_t TemplateGrid(std::uint32_t window, bool* readable = nullptr)
{
    std::uint32_t templ = 0, root = 0, table = 0, parent = 0;
    const bool    read = ReadNative(window + 0x318, templ) && (templ == 0 || (ReadNative(templ + 0xb0, root) && (root == 0 || (ReadNative(root, table) && ReadNative(root + 0x240, parent)))));
    if (readable != nullptr)
        *readable = read;
    return read && root != 0 && table == kGridTable && parent == window ? root : 0;
}

void* Object(std::uint32_t address)
{
    return reinterpret_cast<void*>(static_cast<std::uintptr_t>(address));
}

// Save all originals before writing; a replaced Grid on the same window replaces only its saved value.
void ApplyHide(std::uint32_t window, std::uint32_t grid, bool newWindow)
{
    std::uint8_t hit            = 0;
    float        opacity        = 0.0f;
    std::uint8_t gridVisibility = 0;
    if ((newWindow && (!ReadNative(window + 0x15a, hit) || !ReadNative(window + 0x48, opacity))) || !ReadNative(grid + 0x158, gridVisibility))
        return;
    if (newWindow)
    {
        gHide.window          = window;
        gHide.hitTest         = (hit & kHitTestBit) != 0;
        gHide.opacity         = opacity;
        gHide.restoreAttempts = 0;
    }
    gHide.grid           = grid;
    gHide.gridVisibility = gridVisibility;
    const bool hidden    = reinterpret_cast<SetEnumFunction>(kSetVisibilityVa)(Object(grid), kHiddenEnum, 0, 1);
    const bool blocked   = reinterpret_cast<SetEnumFunction>(kSetHitTestVa)(Object(window), 0, 0, 1);
    const bool clear     = reinterpret_cast<SetFloatFunction>(kSetOpacityVa)(Object(window), 0.0f, 0, 1);
    Log("hide,t=%llu,event=apply,window=%08x,grid=%08x,new=%d,saved=%u/%d/%.3f,admitted=%d%d%d",
        LogTime(),
        window,
        grid,
        newWindow ? 1 : 0,
        gHide.gridVisibility,
        gHide.hitTest ? 1 : 0,
        gHide.opacity,
        hidden ? 1 : 0,
        blocked ? 1 : 0,
        clear ? 1 : 0);
}

// Write only the current template Grid; shown roots need opacity 1 to avoid restoring a faded-out bar.
// Retain rejected restores for retry; kRestoreTries forbids new hides from saving partially restored values.
void RestoreHide()
{
    bool                visibility = true;
    bool                readable   = true;
    const std::uint32_t current    = gHide.grid == 0 ? 0 : TemplateGrid(gHide.window, &readable);
    if (gHide.grid != 0 && !readable)
        visibility = false;
    else if (gHide.grid != 0 && current == gHide.grid)
        visibility = reinterpret_cast<SetEnumFunction>(kSetVisibilityVa)(Object(gHide.grid), gHide.gridVisibility, 0, 1);
    else
        gHide.grid = 0;
    // Without the root's Visibility, defer opacity restoration.
    std::uint8_t root     = 0;
    const bool   rootRead = ReadNative(gHide.window + 0x158, root);
    const float  opacity  = root == kVisibleEnum ? 1.0f : gHide.opacity;
    const bool   hitTest  = reinterpret_cast<SetEnumFunction>(kSetHitTestVa)(Object(gHide.window), gHide.hitTest ? 1u : 0u, 0, 1);
    const bool   alpha    = rootRead && reinterpret_cast<SetFloatFunction>(kSetOpacityVa)(Object(gHide.window), opacity, 0, 1);
    ++gHide.restoreAttempts;
    const bool done = visibility && hitTest && alpha;
    if (gHide.restoreAttempts <= kRestoreTries)
        Log("hide,t=%llu,event=restore,window=%08x,attempt=%u,root=%u,alpha=%.3f,admitted=%d%d%d",
            LogTime(),
            gHide.window,
            gHide.restoreAttempts,
            root,
            opacity,
            visibility ? 1 : 0,
            hitTest ? 1 : 0,
            alpha ? 1 : 0);
    if (!done)
    {
        if (gHide.restoreAttempts == kRestoreTries)
        {
            gHideBroken = true;
            Log("hide,t=%llu,event=restore-failing,window=%08x", LogTime(), gHide.window);
        }
        return;
    }
    gHide.window          = 0;
    gHide.grid            = 0;
    gHide.restoreAttempts = 0;
}

// Shown/Hiding storyboards overwrite opacity every frame; reapply overrides after native input.
void UpdateHide(void* mainModule)
{
    if (!gSettersPresent)
        return;
    // Load drawn before now: the render thread may advance it; withholds give the overlay a full grace period to return.
    const std::uint64_t drawn    = gDrawnTick.load(std::memory_order_acquire);
    const std::uint64_t now      = GetTickCount64();
    const bool          withheld = gWithheld.load(std::memory_order_acquire);
    if (withheld)
        gHide.withheldTick = now;
    const bool shown  = withheld || (drawn != 0 && now - drawn < kHideGrace) || (gHide.withheldTick != 0 && now - gHide.withheldTick < kHideGrace);
    const bool wanted = !gHideBroken && gVisible.load(std::memory_order_acquire) && gHide.allowedTick != 0 && now - gHide.allowedTick < kHideGrace && shown;
    if (!wanted && gHide.window == 0)
        return;
    // Write only to the window uniquely found this step; failed searches retain saved state without writing.
    const std::uint32_t found = ReadMenu(mainModule, false).window;
    if (found == 0)
        return;
    if (gHide.window != 0 && found != gHide.window)
    {
        // Never write the replaced window again.
        Log("hide,t=%llu,event=receiver-replaced,window=%08x,found=%08x", LogTime(), gHide.window, found);
        gHide.window          = 0;
        gHide.grid            = 0;
        gHide.restoreAttempts = 0;
    }
    if (!wanted)
    {
        if (gHide.window != 0)
            RestoreHide();
        return;
    }
    std::uint8_t root = 0xff;
    ReadNative(found + 0x158, root);
    if (root != gHide.rootVisibility)
    {
        Log("hide,t=%llu,event=root,window=%08x,visibility=%u", LogTime(), found, root);
        gHide.rootVisibility = root;
    }
    const std::uint32_t grid = TemplateGrid(found);
    if (grid == 0)
        return;
    if (gHide.window == 0 || grid != gHide.grid)
    {
        ApplyHide(found, grid, gHide.window == 0);
        return;
    }
    std::uint8_t visibility = 0;
    std::uint8_t hit        = 0;
    float        opacity    = 0.0f;
    if (ReadNative(grid + 0x158, visibility) && visibility != kHiddenEnum)
        reinterpret_cast<SetEnumFunction>(kSetVisibilityVa)(Object(grid), kHiddenEnum, 0, 1);
    if (ReadNative(gHide.window + 0x15a, hit) && (hit & kHitTestBit) != 0)
        reinterpret_cast<SetEnumFunction>(kSetHitTestVa)(Object(gHide.window), 0, 0, 1);
    if (ReadNative(gHide.window + 0x48, opacity) && opacity != 0.0f)
        reinterpret_cast<SetFloatFunction>(kSetOpacityVa)(Object(gHide.window), 0.0f, 0, 1);
}

void __fastcall HookedInputStep(void* mainModule, void*, void* pad, void* mouse, void* keyboard)
{
    ++gInputDepth;
    // Restore before nested native input; nested input disables further swaps.
    if (gInputDepth > 1 && gPageOverride.address != 0)
    {
        gSwapBroken.store(true, std::memory_order_release);
        RestorePage();
    }
    else if (gInputDepth == 1 && gPageOverride.address != 0)
    {
        RetryRestore(mainModule);
    }
    gOriginalInputStep(mainModule, pad, mouse, keyboard);
    if (gInputDepth == 1 && GetCurrentThreadId() == gPadThread.load(std::memory_order_acquire))
    {
        DispatchPending(mainModule);
        RefreshSlots(mainModule);
        UpdateHide(mainModule);
    }
    --gInputDepth;
}

} // namespace

bool InstallRuntime()
{
    if (gInstalled.load(std::memory_order_acquire))
        return true;
    if (!IsEnabled() || IsTestStub() || !VerifyRuntimeHostIdentity())
        return false;
    HMODULE image = GetModuleHandleW(nullptr);
    if (image == nullptr || reinterpret_cast<std::uintptr_t>(image) != bahamut_runtime_contract::kRetailImageBase)
        return false;
    if (!HasCode(kInputStepVa, kInputStepPrefix) || !HasCode(kCommandLookupVa, kCommandLookupPrefix) ||
        !HasCode(kParameterBuildVa, kParameterBuildPrefix) || !HasCode(kStringReleaseVa, kStringReleasePrefix))
        return false;
    gSettersPresent = HasCode(kSetVisibilityVa, kSetVisibilityPrefix) && HasCode(kSetHitTestVa, kSetHitTestPrefix) && HasCode(kSetOpacityVa, kSetOpacityPrefix);
    void* target    = reinterpret_cast<void*>(kInputStepVa);
    if (MH_CreateHook(target, reinterpret_cast<void*>(&HookedInputStep), reinterpret_cast<void**>(&gOriginalInputStep)) != MH_OK)
        return false;
    if (MH_EnableHook(target) != MH_OK)
    {
        MH_RemoveHook(target);
        gOriginalInputStep = nullptr;
        return false;
    }
    OpenLog();
    gInstalled.store(true, std::memory_order_release);
    return true;
}

void OnPadUpdate(ClientPadState* state, bool connected, bool foreground)
{
    if (!gInstalled.load(std::memory_order_acquire))
        return;
    gPadThread.store(GetCurrentThreadId(), std::memory_order_release);
    gSample.connected               = connected && state != nullptr;
    gSample.held                    = gSample.connected ? DecodeRetailPadControls(*state) : 0;
    gSample.playable                = gSample.connected && foreground;
    gSample.selectionChordAvailable = false;
    gSample.generation              = { 1, 1, 1 };
    const Routing routing           = gPolicy.Update(gSample);
    gSide.store(static_cast<int>(routing.side), std::memory_order_release);
    gPending      = routing.edges;
    gPendingCount = routing.edgeCount;
    if (gSample.connected && routing.consume != 0)
        *state = FilterRetailPadControls(*state, routing.consume);
}

Side ActiveSide()
{
    return static_cast<Side>(gSide.load(std::memory_order_acquire));
}

std::uint64_t LastDispatchTick(std::size_t slot)
{
    return slot < gDispatchTicks.size() ? gDispatchTicks[slot].load(std::memory_order_acquire) : 0;
}

bool BarAvailable()
{
    const std::uint64_t tick = gAvailableTick.load(std::memory_order_acquire);
    return gInstalled.load(std::memory_order_acquire) && gVisible.load(std::memory_order_acquire) && !gWithheld.load(std::memory_order_acquire) && tick != 0 &&
           GetTickCount64() - tick < kHideGrace;
}

void MarkBarDrawn()
{
    gDrawnTick.store(GetTickCount64(), std::memory_order_release);
}

SlotView SlotDisplay(std::size_t slot)
{
    if (slot >= gSlotViews.size())
        return {};
    const std::uint64_t value = gSlotViews[slot].load(std::memory_order_acquire);
    return { static_cast<std::uint32_t>(value & 0x00ffffffu), static_cast<int>((value >> 24) & 0xff), static_cast<int>((value >> 32) & 0xffff), ((value >> 48) & 1) != 0 };
}

void ConfigureSettings(const std::filesystem::path& file)
{
    gSettingsFile = file;
    std::ifstream     input(file, std::ios::binary);
    const std::string text{ std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>() };
    const Settings    settings = ReadSettings(text);
    gLabels.store(settings.labels, std::memory_order_release);
    gVisible.store(settings.visible, std::memory_order_release);
}

LabelStyle Labels()
{
    return gLabels.load(std::memory_order_acquire);
}

bool SaveLabels(LabelStyle style)
{
    gLabels.store(style, std::memory_order_release);
    return WriteSettingsFile();
}

bool ToggleVisible(bool& visible)
{
    visible = !gVisible.load(std::memory_order_acquire);
    gVisible.store(visible, std::memory_order_release);
    return WriteSettingsFile();
}

} // namespace bahamut_client::xhbar
