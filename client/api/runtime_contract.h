#pragma once

#include <cstddef>
#include <cstdint>

namespace bahamut_runtime_contract
{

inline constexpr char          kClientSha256[]   = "9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9";
inline constexpr std::uint64_t kClientByteLength = 15996808ULL;
inline constexpr char          kGameVersion[]    = "2012.09.19.0001";
inline constexpr char          kStubMarker[]     = "BAHAMUT_STUB_CLIENT_RUNTIME_V1";

inline constexpr std::uintptr_t kRetailImageBase    = 0x00400000u;
inline constexpr std::uint32_t  kRetailServerUtcRva = 0x009A15E3u;
inline constexpr std::uint32_t  kRetailLobbyHostRva = 0x00B90110u;
inline constexpr std::size_t    kServerUtcPatchSize = 5u;
inline constexpr std::size_t    kLobbyHostPatchSize = 0x14u;

inline constexpr std::uint32_t kRuntimeApiVersion = 5u;

// Launcher-provided ordered roots for enabled DAT overlay packages. The
// value is newline-delimited and is consumed only by the x86 runtime.
inline constexpr char kDatPackageRootsEnvironment[] =
    "BAHAMUT_RUNTIME_DAT_PACKAGE_ROOTS";

} // namespace bahamut_runtime_contract
