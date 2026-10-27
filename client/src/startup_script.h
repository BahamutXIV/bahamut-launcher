#pragma once

#include <array>
#include <filesystem>
#include <string>
#include <string_view>
#include <vector>

// Native plugin names accepted by /load and /unload.
inline constexpr std::array<std::string_view, 2> kRuntimePluginNames = {
    "screenshot",
    "discord-rpc",
};

enum class StartupActionKind
{
    PluginLoad,
    PluginUnload,
    ScreenshotBind,
    FillModeBind,
    FpsBind,
    AddonLoad,
    AddonUnload,
    AddonReload,
};

struct StartupAction
{
    StartupActionKind kind;
    // Addon id for addon actions, plugin name for plugin actions.
    std::string   addonId;
    unsigned int  virtualKey   = 0;
    bool          hideOverlays = false;
    unsigned long line         = 0;
};

struct StartupScriptPlan
{
    std::vector<StartupAction> actions;
};

struct StartupScriptError
{
    unsigned long line = 0;
    std::string   message;
};

bool ParseStartupScript(const std::filesystem::path&    path,
                        const std::vector<std::string>& installedAddonIds,
                        StartupScriptPlan&              plan,
                        StartupScriptError&             error);
