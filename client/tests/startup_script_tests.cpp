#include "startup_script.h"

#include <windows.h>

#include <filesystem>
#include <fstream>
#include <iostream>

namespace
{

bool ParseText(const std::filesystem::path& path, std::string_view text, StartupScriptPlan& plan, StartupScriptError& error)
{
    std::ofstream file(path, std::ios::binary | std::ios::trunc);
    file << text;
    file.close();
    return ParseStartupScript(path, { "distance", "fps" }, plan, error);
}

} // namespace

int wmain()
{
    const std::filesystem::path root = std::filesystem::temp_directory_path() / (L"bahamut-startup-script-test-" + std::to_wstring(GetCurrentProcessId()));
    std::error_code             errorCode;
    std::filesystem::create_directories(root, errorCode);
    const std::filesystem::path script = root / L"default.txt";
    StartupScriptPlan           plan;
    StartupScriptError          error;

    const bool valid = ParseText(script,
                                 "# session overrides\n/bind f11 /fillmode\n/bind f12 /fps\n"
                                 "/load screenshot\n/bind f7 /screenshot\n"
                                 "/addon unload fps\n/addon load distance\n/addon reload fps\n"
                                 "/unload screenshot\n",
                                 plan,
                                 error);
    if (!valid || plan.actions.size() != 8 || plan.actions[0].kind != StartupActionKind::FillModeBind || plan.actions[0].virtualKey != VK_F11 || plan.actions[1].kind != StartupActionKind::FpsBind || plan.actions[1].virtualKey != VK_F12 || plan.actions[2].kind != StartupActionKind::ScreenshotLoad || plan.actions[3].kind != StartupActionKind::ScreenshotBind || plan.actions[3].virtualKey != VK_F7 || plan.actions[3].hideOverlays || plan.actions[4].kind != StartupActionKind::AddonUnload || plan.actions[5].addonId != "distance" || plan.actions[6].kind != StartupActionKind::AddonReload || plan.actions[7].kind != StartupActionKind::ScreenshotUnload)
    {
        std::cerr << "valid commands did not preserve source order\n";
        return 1;
    }
    if (ParseText(script, "/bind f12 /fillmode\n", plan, error) || error.line != 1 || error.message.find("expected") == std::string::npos)
    {
        std::cerr << "built-in bindings did not remain fixed\n";
        return 1;
    }
    if (ParseText(script, "# comment\n\n/addon load missing\n", plan, error) || error.line != 3 || error.message.find("not installed") == std::string::npos)
    {
        std::cerr << "missing addon did not report its physical line\n";
        return 1;
    }
    if (ParseText(script, "/include launcher.txt\n", plan, error) || error.line != 1 || error.message.find("expected") == std::string::npos)
    {
        std::cerr << "generated launcher include remained accepted\n";
        return 1;
    }
    std::filesystem::remove(script, errorCode);
    if (!ParseStartupScript(script, { "fps" }, plan, error) || !plan.actions.empty())
    {
        std::cerr << "missing startup script did not preserve bootstrap defaults\n";
        return 1;
    }
    std::filesystem::remove_all(root, errorCode);
    std::cout << "startup-script-tests: PASS\n";
    return 0;
}
