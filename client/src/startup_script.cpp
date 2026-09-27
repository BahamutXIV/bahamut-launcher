#include "startup_script.h"

#include <windows.h>

#include <algorithm>
#include <cctype>
#include <fstream>
#include <sstream>

namespace
{

std::string Trim(std::string value)
{
    const auto notSpace = [](unsigned char character)
    {
        return !std::isspace(character);
    };
    value.erase(value.begin(), std::find_if(value.begin(), value.end(), notSpace));
    value.erase(std::find_if(value.rbegin(), value.rend(), notSpace).base(), value.end());
    return value;
}

bool Fail(StartupScriptError& error, unsigned long line, std::string message)
{
    error.line    = line;
    error.message = std::move(message);
    return false;
}

unsigned int ScreenshotKey(std::string_view token)
{
    std::string normalized(token);
    std::transform(normalized.begin(), normalized.end(), normalized.begin(), [](unsigned char character)
                   {
                       return static_cast<char>(std::toupper(character));
                   });
    if (normalized == "SYSRQ")
        return VK_SNAPSHOT;
    if (normalized == "INSERT")
        return VK_INSERT;
    if (normalized.size() == 2 && normalized[0] == 'F' && normalized[1] >= '1' && normalized[1] <= '9')
    {
        return VK_F1 + static_cast<unsigned int>(normalized[1] - '1');
    }
    return 0;
}

bool IsInstalled(std::string_view                id,
                 const std::vector<std::string>& installedAddonIds)
{
    return std::find(installedAddonIds.begin(), installedAddonIds.end(), id) != installedAddonIds.end();
}

} // namespace

bool ParseStartupScript(const std::filesystem::path&    path,
                        const std::vector<std::string>& installedAddonIds,
                        StartupScriptPlan&              plan,
                        StartupScriptError&             error)
{
    plan.actions.clear();
    error = {};
    std::error_code statusError;
    if (!std::filesystem::exists(path, statusError))
    {
        if (!statusError)
        {
            return true;
        }
        return Fail(error, 0, "could not inspect startup script");
    }

    std::ifstream file(path, std::ios::binary);
    if (!file)
    {
        return Fail(error, 0, "could not open startup script");
    }

    std::string   raw;
    unsigned long lineNumber = 0;
    while (std::getline(file, raw))
    {
        ++lineNumber;
        const std::string line = Trim(raw);
        if (line.empty() || line.front() == '#')
        {
            continue;
        }
        std::istringstream       words(line);
        std::vector<std::string> parts;
        std::string              part;
        while (words >> part)
        {
            parts.push_back(std::move(part));
        }

        if (parts.size() == 2 && parts[0] == "/load" && parts[1] == "screenshot")
        {
            StartupAction action{ StartupActionKind::ScreenshotLoad };
            action.line = lineNumber;
            plan.actions.push_back(std::move(action));
            continue;
        }
        if (parts.size() == 2 && parts[0] == "/unload" && parts[1] == "screenshot")
        {
            StartupAction action{ StartupActionKind::ScreenshotUnload };
            action.line = lineNumber;
            plan.actions.push_back(std::move(action));
            continue;
        }
        if (!parts.empty() && parts[0] == "/bind")
        {
            if (parts.size() == 3 && parts[1] == "f11" && parts[2] == "/fillmode")
            {
                StartupAction action{ StartupActionKind::FillModeBind };
                action.virtualKey = VK_F11;
                action.line       = lineNumber;
                plan.actions.push_back(std::move(action));
                continue;
            }
            if (parts.size() == 3 && parts[1] == "f12" && parts[2] == "/fps")
            {
                StartupAction action{ StartupActionKind::FpsBind };
                action.virtualKey = VK_F12;
                action.line       = lineNumber;
                plan.actions.push_back(std::move(action));
                continue;
            }
            if ((parts.size() != 3 && parts.size() != 4) || parts[2] != "/screenshot" || (parts.size() == 4 && parts[3] != "hide"))
            {
                return Fail(error, lineNumber, "expected /bind f11 /fillmode, /bind f12 /fps, or /bind <key> /screenshot [hide]");
            }
            const unsigned int key = ScreenshotKey(parts[1]);
            if (key == 0)
            {
                return Fail(error, lineNumber, "screenshot key must be SYSRQ, INSERT, or F1 through F9");
            }
            StartupAction action{ StartupActionKind::ScreenshotBind };
            action.virtualKey   = key;
            action.hideOverlays = parts.size() == 4;
            action.line         = lineNumber;
            plan.actions.push_back(std::move(action));
            continue;
        }
        if (parts.size() == 3 && parts[0] == "/addon")
        {
            if (!IsInstalled(parts[2], installedAddonIds))
            {
                return Fail(error, lineNumber, "addon package is not installed: " + parts[2]);
            }
            StartupActionKind kind;
            if (parts[1] == "load")
                kind = StartupActionKind::AddonLoad;
            else if (parts[1] == "unload")
                kind = StartupActionKind::AddonUnload;
            else if (parts[1] == "reload")
                kind = StartupActionKind::AddonReload;
            else
            {
                return Fail(error, lineNumber, "expected addon action load, unload, or reload");
            }
            StartupAction action{ kind };
            action.addonId = parts[2];
            action.line    = lineNumber;
            plan.actions.push_back(std::move(action));
            continue;
        }
        return Fail(error, lineNumber, "expected /load screenshot, /unload screenshot, /bind, or /addon");
    }
    if (!file.good() && !file.eof())
    {
        return Fail(error, lineNumber, "could not read startup script");
    }
    return true;
}
