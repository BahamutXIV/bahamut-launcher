#include "seasonal_bindings.h"
#include "seasonal_weather.h"

#include <windows.h>
#include <xmmintrin.h>

#include <algorithm>
#include <array>
#include <cstdio>
#include <cstring>
#include <vector>

namespace
{

using namespace bahamut_client::seasonal;
constexpr std::uintptr_t kBase             = 0x00400000;
int                      gFailures         = 0;
std::uintptr_t           gExpectedProperty = 0;
std::uintptr_t           gExpectedEntry    = 0;
std::uint8_t             gSelected         = 0;

void Check(bool condition, const char* message)
{
    if (!condition)
    {
        std::fprintf(stderr, "FAIL: %s\n", message);
        ++gFailures;
    }
}

bool ReadImage(void* context, std::uintptr_t address, void* output, std::size_t count)
{
    const auto& image = *static_cast<std::vector<std::uint8_t>*>(context);
    if (address < kBase || address - kBase > image.size() || count > image.size() - (address - kBase))
    {
        return false;
    }
    std::memcpy(output, image.data() + address - kBase, count);
    return true;
}

void BindingChecks()
{
    std::vector<std::uint8_t> image(0x00D00000);
    for (const auto& binding : kNativeEntries)
    {
        std::copy_n(binding.signature.begin(), binding.length, image.begin() + binding.rva);
    }
    for (const auto& site : kNativeCallSites)
    {
        image[site.rva]         = 0xE8;
        const auto displacement = static_cast<std::int32_t>(site.targetRva - site.rva - 5);
        std::memcpy(image.data() + site.rva + 1, &displacement, 4);
    }
    for (const auto& cell : kNativeCells)
    {
        std::memcpy(image.data() + cell.rva, &cell.value, 4);
    }
    Check(VerifySeasonalBindings(ReadImage, &image, kBase), "all native entry/caller/table bindings admitted");
    Check(!VerifySeasonalBindings(ReadImage, &image, kBase + 0x1000), "relocated image refused");
    for (const auto& binding : kNativeEntries)
    {
        image[binding.rva] ^= 1;
        Check(!VerifySeasonalBindings(ReadImage, &image, kBase), "changed native entry refused");
        image[binding.rva] ^= 1;
    }
    for (const auto& site : kNativeCallSites)
    {
        image[site.rva + 1] ^= 1;
        Check(!VerifySeasonalBindings(ReadImage, &image, kBase), "different call route refused");
        image[site.rva + 1] ^= 1;
    }
    for (const auto& cell : kNativeCells)
    {
        image[cell.rva] ^= 1;
        Check(!VerifySeasonalBindings(ReadImage, &image, kBase), "different method cell refused");
        image[cell.rva] ^= 1;
    }
    image.resize(100);
    Check(!VerifySeasonalBindings(ReadImage, &image, kBase), "missing image ranges refused");
}

std::uint8_t AdjustFixture(std::uintptr_t property, std::uintptr_t entry, std::uint8_t original)
{
    Check(property == gExpectedProperty && entry == gExpectedEntry, "inline adapter supplies original ESI/EDI");
    Check(original <= 1, "inline adapter supplies normalized original BL");
    _mm_setcsr(_mm_getcsr() ^ 0x6000u);
    return gSelected;
}

void InlineConditionCheck()
{
    // Synthetic x86 function: save ABI registers, load three arguments,
    // execute the retail-shaped comparison/branch, then publish EBX and return.
    const std::array<std::uint8_t, 45> code = {
        0x53, 0x56, 0x57, 0x8B, 0x74, 0x24, 0x10, 0x8B, 0x5C, 0x24, 0x14, 0x8B, 0x7C, 0x24, 0x18, 0x38, 0x5E, 0x46, 0x74, 0x0B, 0xB8, 0x11, 0x11, 0x11, 0x11, 0xEB, 0x09, 0x90, 0x90, 0x90, 0x90, 0xB8, 0x22, 0x22, 0x22, 0x22, 0x89, 0x5E, 0x40, 0x5F, 0x5E, 0x5B, 0xC2, 0x0C, 0x00
    };
    void* allocation = VirtualAlloc(nullptr, code.size(), MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    Check(allocation != nullptr, "headless condition fixture allocated");
    if (allocation == nullptr)
    {
        return;
    }
    std::memcpy(allocation, code.data(), code.size());
    DWORD previousProtection = 0;
    Check(VirtualProtect(allocation, code.size(), PAGE_EXECUTE_READ, &previousProtection) != FALSE, "condition fixture executable");
    static_cast<void>(FlushInstructionCache(GetCurrentProcess(), allocation, code.size()));
    const auto                     function = reinterpret_cast<std::uint32_t(__stdcall*)(void*, std::uint32_t, void*)>(allocation);
    void*                          target   = static_cast<std::uint8_t*>(allocation) + 15;
    std::array<std::uint8_t, 0x50> property{};
    std::uint16_t                  entry = 8027;
    gExpectedProperty                    = reinterpret_cast<std::uintptr_t>(property.data());
    gExpectedEntry                       = reinterpret_cast<std::uintptr_t>(&entry);
    property[0x46]                       = 1;
    Check(function(property.data(), 0xA5A50000, &entry) == 0x11111111, "original compare unequal arm");
    const bool installed = InstallConditionFixture(target, AdjustFixture);
    Check(installed, "MinHook admits the five-byte conditional site");
    if (installed)
    {
        for (const auto selected : { 0u, 1u })
        {
            gSelected         = static_cast<std::uint8_t>(selected);
            const auto mxcsr  = _mm_getcsr();
            const auto result = function(property.data(), 0xA5A50000, &entry);
            Check(result == (selected == 1 ? 0x22222222u : 0x11111111u), "original relocated branch uses overridden flag");
            Check(_mm_getcsr() == mxcsr, "inline adapter restores floating-point state");
            std::uint32_t saved = 0;
            std::memcpy(&saved, property.data() + 0x40, 4);
            Check(saved == (0xA5A50000u | selected), "only the original BL byte changes");
        }
        Check(RemoveConditionFixture(target), "condition fixture hook removed after synchronous calls");
        Check(function(property.data(), 0xA5A50000, &entry) == 0x11111111, "original branch restored");
    }
    static_cast<void>(VirtualFree(allocation, 0, MEM_RELEASE));
}

void RoleChecks()
{
    Check(SeasonalFlag(TimelineRole::HalloweenShow, Season::Halloween, 0) == 1, "Halloween show");
    Check(SeasonalFlag(TimelineRole::HalloweenHide, Season::Halloween, 1) == 0, "Halloween hide");
    Check(SeasonalFlag(TimelineRole::WinterShow, Season::Halloween, 1) == 0, "winter suppressed at Halloween");
    Check(SeasonalFlag(TimelineRole::WinterHide, Season::Halloween, 0) == 1, "winter hide at Halloween");
    Check(SeasonalFlag(TimelineRole::HalloweenShow, Season::Christmas, 1) == 0, "Halloween suppressed at Christmas");
    Check(SeasonalFlag(TimelineRole::HalloweenHide, Season::Christmas, 0) == 1, "Halloween hide at Christmas");
    Check(SeasonalFlag(TimelineRole::WinterShow, Season::Christmas, 0) == 1, "winter show");
    Check(SeasonalFlag(TimelineRole::WinterHide, Season::Christmas, 1) == 0, "winter hide");
    Check(SeasonalFlag(TimelineRole::None, Season::Christmas, 7) == 7, "unclassified original byte preserved");
    Check(SeasonalFlag(TimelineRole::WinterShow, Season::Ordinary, 7) == 7, "ordinary original byte preserved");
}

} // namespace

int main()
{
    BindingChecks();
    RoleChecks();
    InlineConditionCheck();
    bahamut_client::SeasonalWeatherService service;
    Check(!service.Install(false), "unverified executable cannot enable stock mode");
    Check(!service.LastError().empty(), "unsupported executable has a diagnostic");
    Check(service.Shutdown(), "failed install leaves shutdown safe");
    if (gFailures != 0)
    {
        return 1;
    }
    std::puts("PASS seasonal native bindings, role policy, and headless inline-hook ABI");
    return 0;
}
