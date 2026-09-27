#pragma once

#include <filesystem>
#include <string>
#include <vector>

enum class StartupActionKind
{
    ScreenshotLoad,
    ScreenshotUnload,
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
    std::string       addonId;
    unsigned int      virtualKey   = 0;
    bool              hideOverlays = false;
    unsigned long     line         = 0;
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
