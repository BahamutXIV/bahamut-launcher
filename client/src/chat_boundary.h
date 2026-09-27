#pragma once

#include <string>
#include <string_view>

enum class ChatBoundaryInstallResult
{
    Disabled,
    Installed,
    RollbackFailed,
};

struct ChatCaptureSink
{
    void* context                                                                     = nullptr;
    void (*enqueue)(void* context, std::string_view source, std::string_view message) = nullptr;
};

ChatBoundaryInstallResult InstallChatBoundary(const ChatCaptureSink& sink);
bool                      ShutdownChatBoundary();
bool                      CopyRetailChatText(const void* value, std::string& copy);

// Emits addon text through the retail chat insertion path captured by the hook.
// The context parameter is reserved for AddonChatSink compatibility.
bool PrintAddonChatText(void* context, std::string_view text);
