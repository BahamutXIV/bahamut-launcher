#include "seasonal_weather.h"
#include "fault_guard.h"
#include "runtime_contract.h"
#include "seasonal_bindings.h"

#include <MinHook.h>
#include <bcrypt.h>
#include <windows.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cstdio>
#include <cstring>
#include <limits>
#include <optional>
#include <string_view>

#if defined(_MSC_VER)
#include <intrin.h>
#define SEASONAL_CALLER reinterpret_cast<std::uintptr_t>(_ReturnAddress())
#else
#define SEASONAL_CALLER reinterpret_cast<std::uintptr_t>(__builtin_return_address(0))
#endif

// The inline condition adapter preserves the original integer and FP state.
extern "C"
{
    void*        gSeasonalConditionOriginal = nullptr;
    void __cdecl AdjustSeasonalCondition(void* savedRegisters) noexcept;
}

namespace
{

using namespace bahamut_client::seasonal;
constexpr std::uintptr_t kImageBase             = bahamut_runtime_contract::kRetailImageBase;
constexpr std::uintptr_t kMapActorGlobal        = 0x0133DEF4;
constexpr std::uint32_t  kMapActorTable         = 0x00FBAE3C;
constexpr std::uint32_t  kInstanceTable         = 0x00FF279C;
constexpr std::uint32_t  kTimeLineBaseTable     = 0x0106207C;
constexpr std::uint32_t  kUnitTreeBaseTable     = 0x010621F4;
constexpr std::uint32_t  kUnitInfoTable         = 0x00FF298C;
constexpr std::uint32_t  kTimeLinePropertyTable = 0x00FF2D2C;

using SceneOperation  = void(__thiscall*)(void*, std::uint32_t, std::uint32_t);
using MapUpdate       = void(__thiscall*)(void*);
using WeatherKey      = std::uint32_t(__thiscall*)(void*, std::uint32_t);
using DrawEnvLookup   = std::uint32_t(__thiscall*)(void*, const void*);
using MapDestroy      = void(__thiscall*)(void*);
using WeatherClear    = void(__thiscall*)(void*);
using RegionLookup    = void*(__thiscall*)(void*, std::uint32_t);
using OwnerReady      = bool(__thiscall*)(void*);
using StringConstruct = void*(__thiscall*)(void*, const char*, std::uint32_t);
using StringDestroy   = void(__thiscall*)(void*);

SceneOperation              gOriginalOperation  = nullptr;
MapUpdate                   gOriginalMapUpdate  = nullptr;
WeatherKey                  gOriginalWeatherKey = nullptr;
DrawEnvLookup               gOriginalDrawEnv    = nullptr;
MapDestroy                  gOriginalDestroy    = nullptr;
std::atomic<bool>           gEnabled{ false };
thread_local std::uintptr_t gUpdateActor = 0;
SRWLOCK                     gPolicyLock  = SRWLOCK_INIT;
SeasonalPolicy              gPolicy;
SRWLOCK                     gLogLock  = SRWLOCK_INIT;
HANDLE                      gLogFile  = INVALID_HANDLE_VALUE;
std::uint32_t               gLogLines = 0;
std::atomic<std::uint32_t>  gConditionRoles{ 0 };
std::atomic<std::uint32_t>  gRejectedConditions{ 0 };
std::atomic<std::uint64_t>  gLastKey{ 0 };

#ifdef BAHAMUT_SEASONAL_WEATHER_TEST
ConditionFixture gConditionFixture = nullptr;
#endif

std::uintptr_t Entry(NativeEntry entry)
{
    return kImageBase + kNativeEntries[static_cast<std::size_t>(entry)].rva;
}

bool GuardedRead(void*, std::uintptr_t address, void* output, std::size_t bytes)
{
    if (address == 0 || output == nullptr || bytes > 1024 * 1024 || address > std::numeric_limits<std::uintptr_t>::max() - bytes)
    {
        return false;
    }
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(output, reinterpret_cast<const void*>(address), bytes);
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

template <typename T>
bool Read(std::uintptr_t address, T& output)
{
    return GuardedRead(nullptr, address, &output, sizeof(output));
}

std::uint32_t Word(std::uintptr_t address)
{
    std::uint32_t value = 0;
    static_cast<void>(Read(address, value));
    return value;
}

std::uintptr_t CurrentActor()
{
    const std::uintptr_t actor = Word(kMapActorGlobal);
    return actor != 0 && Word(actor) == kMapActorTable ? actor : 0;
}

void Log(const char* text)
{
    AcquireSRWLockExclusive(&gLogLock);
    if (gLogLines < 4096)
    {
        ++gLogLines;
        std::array<char, 512> line{};
        const int             count  = std::snprintf(line.data(), line.size(), "[stock-seasons pid=%lu] %s\r\n", GetCurrentProcessId(), text);
        const DWORD           length = count > 0 ? static_cast<DWORD>(std::min<std::size_t>(static_cast<std::size_t>(count), line.size() - 1)) : 0;
        OutputDebugStringA(line.data());
        if (gLogFile != INVALID_HANDLE_VALUE && length != 0)
        {
            DWORD written = 0;
            static_cast<void>(WriteFile(gLogFile, line.data(), length, &written, nullptr));
        }
    }
    ReleaseSRWLockExclusive(&gLogLock);
}

Season Mode(std::uintptr_t actor)
{
    AcquireSRWLockShared(&gPolicyLock);
    const Season mode = gPolicy.Mode(actor);
    ReleaseSRWLockShared(&gPolicyLock);
    return mode;
}

bool NativeAdmission(void* actor, std::uint32_t root, bool& ownerReady)
{
    BAHAMUT_FAULT_TRY
    {
        const auto address = reinterpret_cast<std::uintptr_t>(actor);
        void*      table   = reinterpret_cast<void*>(Word(address + 0x154));
        void*      region  = reinterpret_cast<RegionLookup>(Entry(NativeEntry::RegionLookup))(table, root);
        if (region == nullptr || Word(reinterpret_cast<std::uintptr_t>(region) + 0xB0) != root)
        {
            return false;
        }
        void* owner = reinterpret_cast<void*>(Word(address + 0x17C));
        ownerReady  = owner == nullptr || reinterpret_cast<OwnerReady>(Entry(NativeEntry::OwnerReady))(owner);
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

Admission Snapshot(void* actor)
{
    Admission  state{};
    const auto address = reinterpret_cast<std::uintptr_t>(actor);
    if (address == 0 || CurrentActor() != address)
    {
        return state;
    }
    state.actor             = address;
    std::uint8_t  enabled   = 0;
    std::uint32_t stateCode = 0, pendingRegion = 0, pendingWeather = 0;
    if (!Read(address + 0x190, state.region) || !Read(address + 0x19C, state.currentId) || !Read(address + 0x1A4, state.currentKey) ||
        !Read(address + 0x1B8, enabled) || !Read(address + 0x1F0, stateCode) || !Read(address + 0x188, pendingRegion) || !Read(address + 0x18C, pendingWeather))
    {
        return state;
    }
    state.weatherEnabled = enabled != 0;
    state.stateThree     = stateCode == 3;
    state.pendingRegion  = pendingRegion != 0;
    state.pendingWeather = pendingWeather != 0;
    // Only the original map-update callback may admit a deferred request.
    if (gUpdateActor == address && state.region != 0)
    {
        state.regionValid = NativeAdmission(actor, state.region, state.ownerReady);
    }
    return state;
}

void Dispatch(void* actor, const Decision& decision)
{
    if (!decision.forward)
    {
        Log("weather request deferred until native region/weather admission");
        return;
    }
    std::array<char, 160> line{};
    std::snprintf(line.data(), line.size(), "dispatch selector=%u value=%u region=%u mode=%u clear=%u", decision.selector, decision.value, Word(reinterpret_cast<std::uintptr_t>(actor) + 0x190), static_cast<unsigned>(Mode(reinterpret_cast<std::uintptr_t>(actor))), decision.clearWeather ? 1u : 0u);
    Log(line.data());
    if (decision.clearWeather)
    {
        reinterpret_cast<WeatherClear>(Entry(NativeEntry::WeatherClear))(actor);
    }
    gOriginalOperation(actor, decision.selector, decision.value);
}

void __fastcall HookedOperation(void* actor, void*, std::uint32_t selector, std::uint32_t value)
{
    if (!gEnabled.load(std::memory_order_acquire) || (selector != 0x0C && selector != 0x0D) || CurrentActor() != reinterpret_cast<std::uintptr_t>(actor))
    {
        gOriginalOperation(actor, selector, value);
        return;
    }
    const Admission state = Snapshot(actor);
    AcquireSRWLockExclusive(&gPolicyLock);
    const Decision decision = gPolicy.Request(selector, value, state);
    ReleaseSRWLockExclusive(&gPolicyLock);
    Dispatch(actor, decision);
}

void __fastcall HookedMapUpdate(void* actor, void*)
{
    const auto previous = gUpdateActor;
    gUpdateActor        = reinterpret_cast<std::uintptr_t>(actor);
    gOriginalMapUpdate(actor);
    if (gEnabled.load(std::memory_order_acquire) && CurrentActor() == gUpdateActor)
    {
        const Admission state = Snapshot(actor);
        AcquireSRWLockExclusive(&gPolicyLock);
        const auto pending = gPolicy.Replay(state);
        ReleaseSRWLockExclusive(&gPolicyLock);
        if (pending)
        {
            Dispatch(actor, *pending);
        }
    }
    gUpdateActor = previous;
}

std::uint32_t __fastcall HookedWeatherKey(void* region, void*, std::uint32_t id)
{
    const auto          caller   = SEASONAL_CALLER;
    const std::uint32_t original = gOriginalWeatherKey(region, id);
    const auto          actor    = CurrentActor();
    if (!gEnabled.load(std::memory_order_acquire) || actor == 0 || gUpdateActor != actor || id != 8027 ||
        (caller != 0x006241F7 && caller != 0x007E8DD9 && caller != 0x007E8E5D))
    {
        return original;
    }
    const std::uint32_t root     = Word(reinterpret_cast<std::uintptr_t>(region) + 0xB0);
    const auto          selected = SelectedWeatherKey(Mode(actor), root);
    if (!selected)
    {
        return original;
    }
    const auto key = (static_cast<std::uint64_t>(root) << 32) | *selected;
    if (gLastKey.exchange(key) != key)
    {
        std::array<char, 160> line{};
        std::snprintf(line.data(), line.size(), "weather key region=%u id=8027 stock=%08X selected=%08X caller=%08X", root, original, *selected, static_cast<unsigned>(caller));
        Log(line.data());
    }
    return *selected;
}

std::uint32_t LookupDefaultDrawEnv(void* owner)
{
    // The native string has a 0x12-byte header and 0x40 inline bytes.
    alignas(16) std::array<std::uint8_t, (0x12 + 0x40 + 15) & ~15> name{};
    reinterpret_cast<StringConstruct>(Entry(NativeEntry::StringConstruct))(name.data(), "dwev_00", 7);
    const auto result = gOriginalDrawEnv(owner, name.data());
    reinterpret_cast<StringDestroy>(Entry(NativeEntry::StringDestroy))(name.data());
    return result;
}

std::uint32_t __fastcall HookedDrawEnv(void* owner, void*, const void* name)
{
    const auto          caller   = SEASONAL_CALLER;
    const std::uint32_t original = gOriginalDrawEnv(owner, name);
    const auto          actor    = CurrentActor();
    if (!gEnabled.load(std::memory_order_acquire) || original != 0 || caller != 0x007DCEFA || actor == 0 || Mode(actor) != Season::Halloween ||
        Word(actor + 0x158) != reinterpret_cast<std::uintptr_t>(owner) || CityRoot(Word(reinterpret_cast<std::uintptr_t>(owner) + 0x64)) != 103 || Word(actor + 0x1A4) != 0x29D9001F)
    {
        return original;
    }
    std::array<char, 8> requested{};
    if (!GuardedRead(nullptr, Word(reinterpret_cast<std::uintptr_t>(name)), requested.data(), requested.size()) || std::memcmp(requested.data(), "dwev_01", requested.size()) != 0)
    {
        return original;
    }
    const auto result = LookupDefaultDrawEnv(owner);
    if (result != 0)
    {
        Log("Gridania Halloween DrawEnv alias dwev_01 -> dwev_00");
    }
    return result;
}

void __fastcall HookedDestroy(void* actor, void*)
{
    AcquireSRWLockExclusive(&gPolicyLock);
    gPolicy.Reset(reinterpret_cast<std::uintptr_t>(actor));
    ReleaseSRWLockExclusive(&gPolicyLock);
    gConditionRoles.store(0);
    gRejectedConditions.store(0);
    gOriginalDestroy(actor);
}

struct SavedRegisters
{
    std::uint32_t edi, esi, ebp, esp, ebx, edx, ecx, eax, flags;
};

bool TimeLineRecords(std::uintptr_t property, std::uintptr_t& record, std::uintptr_t& owner)
{
    if (Word(property + 4) != kTimeLinePropertyTable)
    {
        return false;
    }
    std::uint8_t admitted = 0;
    if (!Read(property + 0x44, admitted) || admitted == 0)
    {
        return false;
    }
    const auto instance = Word(property + 0x0C);
    if (instance == 0 || Word(instance) != kInstanceTable)
    {
        return false;
    }
    const auto base    = Word(instance + 0x10);
    const auto context = Word(instance + 0x1C);
    if (base == 0 || Word(base) != kTimeLineBaseTable || context == 0 || Word(context) != kUnitInfoTable)
    {
        return false;
    }
    const auto parent = Word(context + 0x80);
    if (parent == 0 || Word(parent) != kInstanceTable)
    {
        return false;
    }
    const auto parentBase = Word(parent + 0x10);
    if (parentBase == 0 || Word(parentBase) != kUnitTreeBaseTable)
    {
        return false;
    }
    record = Word(base + 4);
    owner  = Word(parentBase + 4);
    return record != 0 && owner != 0;
}

void AdjustCondition(SavedRegisters& registers) noexcept
{
#ifdef BAHAMUT_SEASONAL_WEATHER_TEST
    if (gConditionFixture != nullptr)
    {
        const auto value = gConditionFixture(registers.esi, registers.edi, static_cast<std::uint8_t>(registers.ebx));
        registers.ebx    = (registers.ebx & 0xFFFFFF00u) | value;
        return;
    }
#endif
    if (!gEnabled.load(std::memory_order_acquire))
    {
        return;
    }
    const auto    actor   = CurrentActor();
    const Season  season  = Mode(actor);
    std::uint16_t entryId = 0;
    if (season == Season::Ordinary || !Read(registers.edi, entryId) || entryId != 8027)
    {
        return;
    }
    std::uintptr_t record = 0;
    std::uintptr_t owner  = 0;
    if (!TimeLineRecords(registers.esi, record, owner))
    {
        return;
    }
    const auto role = ClassifyTimeline(&GuardedRead, nullptr, record, owner, Word(actor + 0x190));
    if (role == TimelineRole::None)
    {
        if (gRejectedConditions.fetch_add(1) < 16)
        {
            std::array<char, 120> line{};
            std::snprintf(line.data(), line.size(), "condition retained: record=%08X owner=%08X did not match the stock catalog", static_cast<unsigned>(record), static_cast<unsigned>(owner));
            Log(line.data());
        }
        return;
    }
    const auto value = bahamut_client::seasonal::SeasonalFlag(role, season, static_cast<std::uint8_t>(registers.ebx));
    registers.ebx    = (registers.ebx & 0xFFFFFF00u) | value;
    const auto bit   = 1u << (static_cast<unsigned>(season) * 8 + static_cast<unsigned>(role));
    if ((gConditionRoles.fetch_or(bit) & bit) == 0)
    {
        std::array<char, 100> line{};
        std::snprintf(line.data(), line.size(), "condition override mode=%u role=%u selected=%u", static_cast<unsigned>(season), static_cast<unsigned>(role), value);
        Log(line.data());
    }
}

} // namespace

extern "C" void __cdecl AdjustSeasonalCondition(void* savedRegisters) noexcept
{
    AdjustCondition(*static_cast<SavedRegisters*>(savedRegisters));
}

#if defined(_MSC_VER)
__declspec(naked) void SeasonalConditionDetour()
{
    __asm
    {
        pushfd
        pushad
        cld
        mov eax, esp
        sub esp, 528
        and esp, -16
        fxsave [esp]
        mov [esp + 512], eax
        sub esp, 12
        push eax
        call AdjustSeasonalCondition
        add esp, 16
        fxrstor [esp]
        mov esp, [esp + 512]
        popad
        popfd
        jmp dword ptr [gSeasonalConditionOriginal]
    }
}
#else
__attribute__((naked)) void SeasonalConditionDetour()
{
    __asm__("pushfl\n\t"
            "pushal\n\t"
            "cld\n\t"
            "movl %esp, %eax\n\t"
            "subl $528, %esp\n\t"
            "andl $-16, %esp\n\t"
            "fxsave (%esp)\n\t"
            "movl %eax, 512(%esp)\n\t"
            "subl $12, %esp\n\t"
            "pushl %eax\n\t"
            "call _AdjustSeasonalCondition\n\t"
            "addl $16, %esp\n\t"
            "fxrstor (%esp)\n\t"
            "movl 512(%esp), %esp\n\t"
            "popal\n\t"
            "popfl\n\t"
            "jmp *_gSeasonalConditionOriginal");
}
#endif

namespace
{

struct Hook
{
    NativeEntry entry;
    void*       detour;
    void**      original;
    bool        created = false;
};

std::array<Hook, 6> gHooks = { {
    { NativeEntry::SceneOperation, reinterpret_cast<void*>(&HookedOperation), reinterpret_cast<void**>(&gOriginalOperation) },
    { NativeEntry::MapUpdate, reinterpret_cast<void*>(&HookedMapUpdate), reinterpret_cast<void**>(&gOriginalMapUpdate) },
    { NativeEntry::WeatherKey, reinterpret_cast<void*>(&HookedWeatherKey), reinterpret_cast<void**>(&gOriginalWeatherKey) },
    { NativeEntry::DrawEnvLookup, reinterpret_cast<void*>(&HookedDrawEnv), reinterpret_cast<void**>(&gOriginalDrawEnv) },
    { NativeEntry::ConditionCompare, reinterpret_cast<void*>(&SeasonalConditionDetour), &gSeasonalConditionOriginal },
    { NativeEntry::MapDestroy, reinterpret_cast<void*>(&HookedDestroy), reinterpret_cast<void**>(&gOriginalDestroy) },
} };

bool DisableHooks()
{
    gEnabled.store(false, std::memory_order_release);
    bool ok = true;
    for (auto& hook : gHooks)
    {
        if (hook.created)
        {
            // Cancel queued enables even when creation failed before publication.
            const auto queued = MH_QueueDisableHook(reinterpret_cast<void*>(Entry(hook.entry)));
            ok                = queued == MH_OK && ok;
            const auto status = MH_DisableHook(reinterpret_cast<void*>(Entry(hook.entry)));
            ok                = (status == MH_OK || status == MH_ERROR_DISABLED) && ok;
        }
    }
    // Retain published trampolines until process exit, like the DAT hook.
    return ok;
}

bool HashMatches(const std::filesystem::path& path, std::uint64_t expectedBytes, std::string_view expectedHash)
{
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
    {
        return false;
    }
    LARGE_INTEGER                       length{};
    BCRYPT_ALG_HANDLE                   algorithm = nullptr;
    BCRYPT_HASH_HANDLE                  hash      = nullptr;
    std::array<std::uint8_t, 32>        digest{};
    bool                                ok = GetFileSizeEx(file, &length) && length.QuadPart >= 0 && static_cast<std::uint64_t>(length.QuadPart) == expectedBytes &&
                                             BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0) == 0 &&
                                             BCryptCreateHash(algorithm, &hash, nullptr, 0, nullptr, 0, 0) == 0;
    std::array<std::uint8_t, 64 * 1024> buffer{};
    while (ok)
    {
        DWORD read = 0;
        if (!ReadFile(file, buffer.data(), static_cast<DWORD>(buffer.size()), &read, nullptr))
        {
            ok = false;
            break;
        }
        if (read == 0)
        {
            break;
        }
        ok = BCryptHashData(hash, buffer.data(), read, 0) == 0;
    }
    ok = ok && BCryptFinishHash(hash, digest.data(), static_cast<ULONG>(digest.size()), 0) == 0;
    if (hash != nullptr)
    {
        BCryptDestroyHash(hash);
    }
    if (algorithm != nullptr)
    {
        BCryptCloseAlgorithmProvider(algorithm, 0);
    }
    CloseHandle(file);
    std::array<char, 65> encoded{};
    constexpr char       hex[] = "0123456789abcdef";
    for (std::size_t index = 0; index < digest.size(); ++index)
    {
        encoded[index * 2]     = hex[digest[index] >> 4];
        encoded[index * 2 + 1] = hex[digest[index] & 15];
    }
    return ok && std::string_view(encoded.data(), 64) == expectedHash;
}

bool CheckStockInputs(std::string& error)
{
    std::error_code ec;
    const auto      root = std::filesystem::current_path(ec);
    if (ec)
    {
        error = "cannot read game working directory";
        return false;
    }
    const auto check = [&](std::string_view path, std::uint64_t bytes, std::string_view digest)
    {
        if (HashMatches(root / std::filesystem::path(path), bytes, digest))
        {
            return true;
        }
        error = "stock DAT identity mismatch: " + std::string(path);
        return false;
    };
    if (!check("data/03/C0/00/00.DAT", 52336, "c04b0d998aea4c1b13ed322292a5aa5af45485c698da2315171c3c024bcb9a74") ||
        !check("data/29/D9/00/1F.DAT", 120912, "13e4c6b12da2b0cd34063e13e8910bb3473c185a5843f02fa5fb9954f83e0add") ||
        !check("data/29/B0/00/20.DAT", 118768, "44cff7e75f20f8c0735f4ecd91bc1e7c2b9c28551806d5ee3fa3e5b1b1a122f1") ||
        !check("data/61/5A/00/23.DAT", 123232, "aae0a9f6d5506c6468192c90b8fb1c0b6c8d4dbf4ab628cc13594a4cba759ef9"))
    {
        return false;
    }
    for (const auto& input : LayoutInputs())
    {
        if (!check(input.path, input.bytes, input.sha256))
        {
            return false;
        }
    }
    return true;
}

void CloseLog()
{
    AcquireSRWLockExclusive(&gLogLock);
    if (gLogFile != INVALID_HANDLE_VALUE)
    {
        CloseHandle(gLogFile);
        gLogFile = INVALID_HANDLE_VALUE;
    }
    ReleaseSRWLockExclusive(&gLogLock);
}

} // namespace

namespace bahamut_client::seasonal
{

bool VerifySeasonalBindings(ReadMemory read, void* context, std::uintptr_t imageBase) noexcept
{
    if (read == nullptr || imageBase != kImageBase)
    {
        return false;
    }
    for (const auto& binding : kNativeEntries)
    {
        std::array<std::uint8_t, 16> actual{};
        if (!read(context, imageBase + binding.rva, actual.data(), binding.length) || !std::equal(actual.begin(), actual.begin() + binding.length, binding.signature.begin()))
        {
            return false;
        }
    }
    for (const auto& site : kNativeCallSites)
    {
        std::array<std::uint8_t, 5> code{};
        std::int32_t                displacement = 0;
        if (!read(context, imageBase + site.rva, code.data(), code.size()) || code[0] != 0xE8)
        {
            return false;
        }
        std::memcpy(&displacement, code.data() + 1, sizeof(displacement));
        const auto target = static_cast<std::int64_t>(site.rva) + 5 + displacement;
        if (target != site.targetRva)
        {
            return false;
        }
    }
    for (const auto& cell : kNativeCells)
    {
        std::uint32_t actual = 0;
        if (!read(context, imageBase + cell.rva, &actual, sizeof(actual)) || actual != cell.value)
        {
            return false;
        }
    }
    return true;
}

std::uint8_t SeasonalFlag(TimelineRole role, Season season, std::uint8_t original) noexcept
{
    if (role == TimelineRole::None || season == Season::Ordinary)
    {
        return original;
    }
    const bool halloween = season == Season::Halloween;
    switch (role)
    {
        case TimelineRole::HalloweenShow:
        case TimelineRole::WinterHide:
            return halloween ? 1 : 0;
        case TimelineRole::HalloweenHide:
        case TimelineRole::WinterShow:
            return halloween ? 0 : 1;
        default:
            return original;
    }
}

#ifdef BAHAMUT_SEASONAL_WEATHER_TEST
bool InstallConditionFixture(void* target, ConditionFixture adjuster)
{
    const auto initialized = MH_Initialize();
    if (initialized != MH_OK && initialized != MH_ERROR_ALREADY_INITIALIZED)
    {
        return false;
    }
    gConditionFixture = adjuster;
    if (MH_CreateHook(target, reinterpret_cast<void*>(&SeasonalConditionDetour), &gSeasonalConditionOriginal) != MH_OK)
    {
        return false;
    }
    return MH_EnableHook(target) == MH_OK;
}

bool RemoveConditionFixture(void* target)
{
    const bool disabled        = MH_DisableHook(target) == MH_OK;
    const bool removed         = MH_RemoveHook(target) == MH_OK;
    gConditionFixture          = nullptr;
    gSeasonalConditionOriginal = nullptr;
    return disabled && removed;
}
#endif

} // namespace bahamut_client::seasonal

namespace bahamut_client
{

bool SeasonalWeatherService::Install(bool exactBuild, const std::filesystem::path& logFile)
{
    if (installed_)
    {
        return true;
    }
    error_.clear();
    if (!logFile.empty())
    {
        if (!logFile.is_absolute())
        {
            error_ = "stock-season diagnostic log path must be absolute";
            return false;
        }
        gLogFile = CreateFileW(logFile.c_str(), FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
        if (gLogFile == INVALID_HANDLE_VALUE)
        {
            error_ = "cannot open stock-season diagnostic log";
            return false;
        }
    }
    const auto fail = [this](const char* reason)
    {
        error_ = reason;
        Log(reason);
        static_cast<void>(DisableHooks());
        CloseLog();
        return false;
    };
    Log("initializing opt-in stock-DAT seasonal mode");
    if (!exactBuild || reinterpret_cast<std::uintptr_t>(GetModuleHandleW(nullptr)) != kImageBase || !VerifySeasonalBindings(&GuardedRead, nullptr, kImageBase))
    {
        return fail("unsupported executable or changed seasonal native bindings");
    }
    std::string stockError;
    if (!CheckStockInputs(stockError))
    {
        return fail(stockError.c_str());
    }
    for (auto& hook : gHooks)
    {
        if (hook.created)
        {
            return fail("cannot create seasonal native hook; restart with the opt-in disabled");
        }
        const auto status = MH_CreateHook(reinterpret_cast<void*>(Entry(hook.entry)), hook.detour, hook.original);
        hook.created      = status == MH_OK;
        if (!hook.created || *hook.original == nullptr)
        {
            return fail("cannot create seasonal native hook; restart with the opt-in disabled");
        }
    }
    for (const auto& hook : gHooks)
    {
        if (MH_QueueEnableHook(reinterpret_cast<void*>(Entry(hook.entry))) != MH_OK)
        {
            return fail("cannot queue seasonal native hooks");
        }
    }
    if (MH_ApplyQueued() != MH_OK)
    {
        return fail("cannot enable seasonal native hooks");
    }
    gEnabled.store(true, std::memory_order_release);
    installed_ = true;
    Log("stock-DAT seasonal hooks enabled; runtime DAT redirection must be disabled");
    return true;
}

bool SeasonalWeatherService::Shutdown()
{
    if (!installed_)
    {
        return true;
    }
    const bool stopped = DisableHooks();
    if (stopped)
    {
        installed_ = false;
        AcquireSRWLockExclusive(&gPolicyLock);
        gPolicy.Reset();
        ReleaseSRWLockExclusive(&gPolicyLock);
        Log("stock-DAT seasonal hooks disabled");
        CloseLog();
    }
    return stopped;
}

const std::string& SeasonalWeatherService::LastError() const
{
    return error_;
}

} // namespace bahamut_client
