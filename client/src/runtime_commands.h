#pragma once

#include "addon_host.h"

#include <array>
#include <filesystem>
#include <optional>
#include <string>
#include <string_view>
#include <unordered_map>
#include <vector>

enum class RuntimeCommandKind
{
    AddonLoad,
    AddonUnload,
    AddonReload,
    AddonList,
    AddonUsage,
    PluginLoad,
    PluginUnload,
    PluginList,
    PluginUsage,
};

struct RuntimeCommand
{
    RuntimeCommandKind kind;
    std::string        name;
};

// Words owned by the runtime. Addon manifests cannot claim them.
inline constexpr std::array<std::string_view, 4> kRuntimeCommandWords = {
    "/addon",
    "/load",
    "/unload",
    "/plugins",
};

// Parses a space-separated command. Returns nullopt when its first word is
// not a runtime command word; malformed runtime forms parse as usage.
std::optional<RuntimeCommand> ParseRuntimeCommand(std::string_view command);

struct RuntimePlugin
{
    std::string name;
    void*       context                               = nullptr;
    bool (*load)(void* context, std::string& error)   = nullptr;
    bool (*unload)(void* context, std::string& error) = nullptr;
    bool (*loaded)(void* context)                     = nullptr;
};

// Runs on the retail command thread, where chat feedback can print; host locks
// serialize it with render and window callbacks.
class RuntimeCommands
{
public:
    // Call before the command boundary installs. The catalog maps installed
    // addon ids to manifests; catalog and plugins are fixed afterward.
    void Configure(AddonHost*                                             addons,
                   std::unordered_map<std::string, std::filesystem::path> catalog,
                   std::vector<RuntimePlugin>                             plugins,
                   const AddonChatSink&                                   chat);

    bool IsInstalled(std::string_view addonId) const;
    // Runtime words, then each catalog command word once.
    const std::vector<std::string>& CommandWords() const;

    std::vector<std::string> Execute(const RuntimeCommand& command);

    // CommandDispatchFunction for the command boundary: runtime commands
    // first, then loaded addons.
    static bool Dispatch(void* context, std::string_view command);

private:
    std::vector<std::string> ExecuteAddon(const RuntimeCommand& command);
    std::vector<std::string> ExecutePlugin(const RuntimeCommand& command);
    const RuntimePlugin*     FindPlugin(std::string_view name) const;

    AddonHost*                                             addons_ = nullptr;
    std::unordered_map<std::string, std::filesystem::path> catalog_;
    std::vector<RuntimePlugin>                             plugins_;
    std::vector<std::string>                               words_;
    AddonChatSink                                          chat_;
};
