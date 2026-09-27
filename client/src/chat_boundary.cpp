#include "chat_boundary.h"
#include "fault_guard.h"

#include "runtime_contract.h"

#include <MinHook.h>

#include <windows.h>

#include <cstdint>
#include <cstring>
#include <string>
#include <string_view>

namespace
{

constexpr std::uintptr_t kChatInsertionRva         = 0x000CE760u;
constexpr std::uintptr_t kChatInsertionVa          = 0x004CE760u;
constexpr std::uintptr_t kUtf8StringConstructorRva = 0x00045CF0u;
constexpr std::uintptr_t kUtf8StringAssignRva      = 0x000489C0u;
constexpr std::uintptr_t kUtf8StringDestructorRva  = 0x00046F50u;
constexpr std::uintptr_t kChatSourceRva            = 0x00E66B10u;
constexpr std::size_t    kUtf8StringSize           = 0x54u;
constexpr std::size_t    kUtf8StringDataOffset     = 0x00u;
// Retail assign/reserve stores the NUL-inclusive byte size at +0x08; see
// xivl-decomp 000489c0_FUN_004489c0.s and 00047010_FUN_00447010.s.
constexpr std::size_t kUtf8StringSizeOffset     = 0x08u;
constexpr std::size_t kMaximumAddonChatBytes    = 512u;
constexpr std::size_t kMaximumCapturedChatBytes = 8192u;

constexpr std::uint16_t kChatCategory = 0x20u;

constexpr unsigned char kChatInsertionPrologue[] = {
    0x6A,
    0xFF,
    0x68,
    0xDE,
    0xC1,
    0xE5,
    0x00,
    0x64,
    0xA1,
    0x00,
    0x00,
    0x00,
    0x00,
    0x50,
    0x81,
    0xEC,
    0x64,
    0x01,
    0x00,
    0x00,
};

using ChatInsertionFunction         = void(__thiscall*)(void*         receiver,
                                                        std::uint16_t category,
                                                        void*         source,
                                                        void*         message);
using Utf8StringConstructorFunction = void*(__thiscall*)(void* string);
using Utf8StringAssignFunction      = void*(__thiscall*)(void*       string,
                                                         const char* text);
using Utf8StringDestructorFunction  = void(__thiscall*)(void* string);

ChatInsertionFunction gOriginalChatInsertion = nullptr;
void*                 gChatInsertionTarget   = nullptr;
void* volatile gCapturedReceiver             = nullptr;
volatile LONG   gCapturedThread              = 0;
bool            gInstalled                   = false;
ChatCaptureSink gCaptureSink;

static_assert(kChatInsertionVa == bahamut_runtime_contract::kRetailImageBase + kChatInsertionRva,
              "chat insertion VA and RVA must identify the same retail address");

bool MatchesChatInsertionPrologue(const void* address)
{
    unsigned char bytes[sizeof(kChatInsertionPrologue)]{};
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(bytes, address, sizeof(bytes));
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
    return std::memcmp(bytes, kChatInsertionPrologue, sizeof(bytes)) == 0;
}

bool IsValidUtf8(std::string_view text)
{
    return text.empty() || (text.find('\0') == std::string_view::npos && MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text.data(), static_cast<int>(text.size()), nullptr, 0) != 0);
}

void __fastcall HookedChatInsertion(void* receiver, void*, std::uint16_t category, void* source, void* message)
{
    if (receiver != nullptr)
    {
        // Every entry is the same retail log insertion boundary. Keep the
        // receiver and thread current in case the client replaces its UI state.
        InterlockedExchangePointer(&gCapturedReceiver, receiver);
        InterlockedExchange(&gCapturedThread,
                            static_cast<LONG>(GetCurrentThreadId()));
    }

    const ChatCaptureSink sink = gCaptureSink;
    if (sink.enqueue != nullptr)
    {
        std::string sourceText;
        std::string messageText;
        if (CopyRetailChatText(source, sourceText) && CopyRetailChatText(message, messageText) && !messageText.empty())
        {
            sink.enqueue(sink.context, sourceText, messageText);
        }
    }

    const ChatInsertionFunction original = gOriginalChatInsertion;
    if (original != nullptr)
    {
        original(receiver, category, source, message);
    }
}

bool DestroyUtf8StringSafely(Utf8StringDestructorFunction destructor,
                             void*                        string)
{
    BAHAMUT_FAULT_TRY
    {
        destructor(string);
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

bool PrintAddonChatTextCore(std::string_view text)
{
    if (text.empty() || text.size() > kMaximumAddonChatBytes)
    {
        return false;
    }
    if (!IsValidUtf8(text))
    {
        return false;
    }

    void* receiver = InterlockedCompareExchangePointer(
        &gCapturedReceiver, nullptr, nullptr);
    const LONG capturedThread = InterlockedCompareExchange(
        &gCapturedThread, 0, 0);
    if (receiver == nullptr || capturedThread == 0 || capturedThread != static_cast<LONG>(GetCurrentThreadId()) || gOriginalChatInsertion == nullptr)
    {
        return false;
    }

    char message[kMaximumAddonChatBytes + 1u]{};
    std::memcpy(message, text.data(), text.size());
    message[text.size()] = '\0';

    alignas(std::uint32_t) unsigned char stringStorage[kUtf8StringSize]{};
    volatile bool                        constructed = false;
    volatile bool                        success     = false;
    const auto                           imageBase   = bahamut_runtime_contract::kRetailImageBase;
    const auto                           constructor = reinterpret_cast<Utf8StringConstructorFunction>(
        imageBase + kUtf8StringConstructorRva);
    const auto assign = reinterpret_cast<Utf8StringAssignFunction>(
        imageBase + kUtf8StringAssignRva);
    const auto destructor = reinterpret_cast<Utf8StringDestructorFunction>(
        imageBase + kUtf8StringDestructorRva);
    const auto source = reinterpret_cast<void*>(imageBase + kChatSourceRva);

    BAHAMUT_FAULT_TRY
    {
        constructor(stringStorage);
        constructed = true;
        assign(stringStorage, message);
        gOriginalChatInsertion(receiver, kChatCategory, source, stringStorage);
        success = true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        success = false;
    }
    if (constructed)
    {
        success = DestroyUtf8StringSafely(destructor, stringStorage) && success;
    }
    return success;
}

} // namespace

bool CopyRetailChatText(const void* value, std::string& copy)
{
    if (value == nullptr)
    {
        copy.clear();
        return true;
    }
    BAHAMUT_FAULT_TRY
    {
        const auto* bytes = static_cast<const unsigned char*>(value);
        const auto* data  = *reinterpret_cast<const char* const*>(
            bytes + kUtf8StringDataOffset);
        const auto size = *reinterpret_cast<const std::uint32_t*>(
            bytes + kUtf8StringSizeOffset);
        if (data == nullptr || size == 0 || size > kMaximumCapturedChatBytes + 1u || data[size - 1u] != '\0')
        {
            return false;
        }
        copy.assign(data, size - 1u);
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
    return IsValidUtf8(copy);
}

ChatBoundaryInstallResult InstallChatBoundary(const ChatCaptureSink& sink)
{
    if (gInstalled)
    {
        return gChatInsertionTarget != nullptr
                   ? ChatBoundaryInstallResult::Installed
                   : ChatBoundaryInstallResult::RollbackFailed;
    }

    HMODULE image = GetModuleHandleW(nullptr);
    if (image == nullptr || reinterpret_cast<std::uintptr_t>(image) != bahamut_runtime_contract::kRetailImageBase)
    {
        return ChatBoundaryInstallResult::Disabled;
    }
    void* target = reinterpret_cast<void*>(
        bahamut_runtime_contract::kRetailImageBase + kChatInsertionRva);
    if (!MatchesChatInsertionPrologue(target))
    {
        return ChatBoundaryInstallResult::Disabled;
    }

    LPVOID original = nullptr;
    if (MH_CreateHook(target, reinterpret_cast<LPVOID>(&HookedChatInsertion), &original) != MH_OK)
    {
        return ChatBoundaryInstallResult::Disabled;
    }
    gOriginalChatInsertion = reinterpret_cast<ChatInsertionFunction>(original);
    gChatInsertionTarget   = target;
    gCaptureSink           = sink;
    InterlockedExchangePointer(&gCapturedReceiver, nullptr);
    InterlockedExchange(&gCapturedThread, 0);
    if (MH_EnableHook(target) != MH_OK)
    {
        return ShutdownChatBoundary()
                   ? ChatBoundaryInstallResult::Disabled
                   : ChatBoundaryInstallResult::RollbackFailed;
    }
    gInstalled = true;
    return ChatBoundaryInstallResult::Installed;
}

bool ShutdownChatBoundary()
{
    if (!gInstalled && gChatInsertionTarget == nullptr)
    {
        return true;
    }
    const MH_STATUS disabled   = MH_DisableHook(gChatInsertionTarget);
    const bool      disabledOk = disabled == MH_OK || disabled == MH_ERROR_DISABLED;
    const MH_STATUS removed    = MH_RemoveHook(gChatInsertionTarget);
    const bool      removedOk  = removed == MH_OK || removed == MH_ERROR_NOT_CREATED;
    if (!disabledOk || !removedOk)
    {
        return false;
    }
    gOriginalChatInsertion = nullptr;
    gChatInsertionTarget   = nullptr;
    gCaptureSink           = {};
    InterlockedExchangePointer(&gCapturedReceiver, nullptr);
    InterlockedExchange(&gCapturedThread, 0);
    gInstalled = false;
    return true;
}

bool PrintAddonChatText(void* context, std::string_view text)
{
    (void)context;
    bool result = false;
    BAHAMUT_FAULT_TRY
    {
        result = PrintAddonChatTextCore(text);
    }
    BAHAMUT_FAULT_EXCEPT
    {
        result = false;
    }
    return result;
}
