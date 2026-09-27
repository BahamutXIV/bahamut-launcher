#pragma once

#include <windows.h>

#include <cstddef>
#include <cstdint>

inline constexpr std::uint32_t kBahamutNativePluginAbiVersion = 3u;
inline constexpr int           kBahamutScreenshotHotkeyId     = 0xB140;

enum BahamutNativePluginFormat : std::uint32_t
{
    BahamutNativePluginFormatPng = 1u,
    BahamutNativePluginFormatBmp = 2u,
};

enum BahamutNativePluginEventFlags : std::uint32_t
{
    BahamutNativePluginEventNone            = 0u,
    BahamutNativePluginEventConsumed        = 1u << 0,
    BahamutNativePluginEventSuppressOverlay = 1u << 1,
};

struct BahamutNativePluginHostV3
{
    std::uint32_t abiVersion;
    std::uint32_t structSize;
};

struct BahamutNativePluginConfigV1
{
    std::uint32_t  structSize;
    const wchar_t* storagePath;
    std::uint32_t  format;
    std::uint32_t  hotkey;
    BOOL           enabled;
    BOOL           hideOverlays;
};

struct BahamutNativePluginInputV1
{
    std::uint32_t structSize;
    HWND          window;
    UINT          message;
    WPARAM        wParam;
    LPARAM        lParam;
};

inline constexpr std::size_t kBahamutNativePluginTextBytes = 64u;

enum BahamutNativePluginPlayerStateFlags : std::uint32_t
{
    BahamutNativePluginPlayerStateNone         = 0u,
    BahamutNativePluginPlayerStateHasCharacter = 1u << 0,
    BahamutNativePluginPlayerStateHasArea      = 1u << 1,
    BahamutNativePluginPlayerStateHasClassJob  = 1u << 2,
};

struct BahamutNativePluginPlayerStateV1
{
    std::uint32_t structSize;
    std::uint32_t actorId;
    std::uint32_t zoneId;
    std::uint16_t baseClassId;
    std::uint16_t jobId;
    std::uint16_t level;
    std::uint16_t reserved;
    std::uint32_t flags;
    char          displayName[kBahamutNativePluginTextBytes];
    char          areaName[kBahamutNativePluginTextBytes];
};

using BahamutNativePluginCreateV1              = void*(WINAPI*)(const BahamutNativePluginConfigV1* config);
using BahamutNativePluginConfigureV1           = BOOL(WINAPI*)(void*                              instance,
                                                               const BahamutNativePluginConfigV1* config);
using BahamutNativePluginSetEnabledV1          = BOOL(WINAPI*)(void* instance,
                                                               BOOL  enabled);
using BahamutNativePluginInputCallbackV1       = BOOL(WINAPI*)(void*                             instance,
                                                               const BahamutNativePluginInputV1* input,
                                                               std::uint32_t*                    flags);
using BahamutNativePluginDeviceCallbackV1      = BOOL(WINAPI*)(void*          instance,
                                                               void*          device,
                                                               std::uint32_t* flags);
using BahamutNativePluginPlayerStateCallbackV1 = BOOL(WINAPI*)(
    void* instance, const BahamutNativePluginPlayerStateV1* state);
using BahamutNativePluginShutdownV1 = void(WINAPI*)(void* instance);
using BahamutNativePluginDestroyV1  = void(WINAPI*)(void* instance);

struct BahamutNativePluginApiV3
{
    std::uint32_t                            abiVersion;
    std::uint32_t                            structSize;
    const char*                              id;
    BahamutNativePluginCreateV1              create;
    BahamutNativePluginConfigureV1           configure;
    BahamutNativePluginSetEnabledV1          setEnabled;
    BahamutNativePluginInputCallbackV1       onInput;
    BahamutNativePluginDeviceCallbackV1      onPresentBegin;
    BahamutNativePluginDeviceCallbackV1      onPresentEnd;
    BahamutNativePluginPlayerStateCallbackV1 onPlayerState;
    BahamutNativePluginShutdownV1            shutdown;
    BahamutNativePluginDestroyV1             destroy;
};

using BahamutNativePluginGetApiFunction = BOOL(WINAPI*)(std::uint32_t                    requestedVersion,
                                                        const BahamutNativePluginHostV3* host,
                                                        BahamutNativePluginApiV3*        api);

inline constexpr char kBahamutNativePluginGetApiExport[] =
    "BahamutNativePluginGetApi";
