#include "runtime_commands.h"

#include <filesystem>
#include <iostream>
#include <string>
#include <string_view>
#include <vector>

namespace
{

struct ChatCapture
{
    std::vector<std::string> lines;
    bool                     accept = true;
};

bool CaptureChat(void* context, std::string_view text)
{
    auto& capture = *static_cast<ChatCapture*>(context);
    if (!capture.accept)
    {
        return false;
    }
    capture.lines.emplace_back(text);
    return true;
}

struct PluginStub
{
    bool loaded   = false;
    bool failLoad = false;
    int  loads    = 0;
    int  unloads  = 0;
};

bool LoadStub(void* context, std::string& error)
{
    auto& plugin = *static_cast<PluginStub*>(context);
    ++plugin.loads;
    if (plugin.failLoad)
    {
        error = "plugin DLL could not be loaded";
        return false;
    }
    plugin.loaded = true;
    return true;
}

bool UnloadStub(void* context, std::string&)
{
    auto& plugin = *static_cast<PluginStub*>(context);
    ++plugin.unloads;
    plugin.loaded = false;
    return true;
}

struct OverlayStub
{
    std::string labels  = "xbox";
    bool        visible = true;
    bool        save    = true;
    int         sets    = 0;
};

bool SetStubLabels(void* context, std::string_view name)
{
    auto& overlay = *static_cast<OverlayStub*>(context);
    ++overlay.sets;
    overlay.labels = name;
    return overlay.save;
}

bool ToggleStub(void* context, bool& visible)
{
    auto& overlay   = *static_cast<OverlayStub*>(context);
    overlay.visible = !overlay.visible;
    visible         = overlay.visible;
    return overlay.save;
}

bool StubLoaded(void* context)
{
    return static_cast<PluginStub*>(context)->loaded;
}

bool Require(bool condition, const char* message)
{
    if (!condition)
    {
        std::cerr << message << '\n';
    }
    return condition;
}

bool ParsesAs(std::string_view text, RuntimeCommandKind kind, std::string_view name = {})
{
    const auto parsed = ParseRuntimeCommand(text);
    return parsed && parsed->kind == kind && parsed->name == name;
}

} // namespace

int wmain(int argc, wchar_t** argv)
{
    if (argc != 8)
    {
        std::cerr << "usage: runtime-commands-tests <fps> <pos> <command-words> <load-fault> <command-table> <settings> <chat-logs>\n";
        return 2;
    }
    const std::filesystem::path fpsManifest(argv[1]);
    const std::filesystem::path posManifest(argv[2]);
    const std::filesystem::path wordsManifest(argv[3]);
    const std::filesystem::path faultManifest(argv[4]);
    const std::filesystem::path tableManifest(argv[5]);
    const std::filesystem::path settings(argv[6]);
    const std::filesystem::path chatLogs(argv[7]);

    if (!Require(ParsesAs("/addon load fps", RuntimeCommandKind::AddonLoad, "fps") &&
                     ParsesAs("/addon unload fps", RuntimeCommandKind::AddonUnload, "fps") &&
                     ParsesAs("/addon reload fps", RuntimeCommandKind::AddonReload, "fps") &&
                     ParsesAs("/addon list", RuntimeCommandKind::AddonList) &&
                     ParsesAs("/addon", RuntimeCommandKind::AddonUsage) &&
                     ParsesAs("/addon load", RuntimeCommandKind::AddonUsage) &&
                     ParsesAs("/addon load fps pos", RuntimeCommandKind::AddonUsage) &&
                     ParsesAs("/addon enable fps", RuntimeCommandKind::AddonUsage) &&
                     ParsesAs("/load screenshot", RuntimeCommandKind::PluginLoad, "screenshot") &&
                     ParsesAs("/unload discord-rpc", RuntimeCommandKind::PluginUnload, "discord-rpc") &&
                     ParsesAs("/load", RuntimeCommandKind::PluginUsage) &&
                     ParsesAs("/unload a b", RuntimeCommandKind::PluginUsage) &&
                     ParsesAs("/plugins", RuntimeCommandKind::PluginList) &&
                     ParsesAs("/plugins all", RuntimeCommandKind::PluginUsage) &&
                     ParsesAs("/xhbar xbox", RuntimeCommandKind::XHBarLabels, "xbox") &&
                     ParsesAs("/xhbar playstation", RuntimeCommandKind::XHBarLabels, "playstation") &&
                     ParsesAs("/xhbar toggle", RuntimeCommandKind::XHBarToggle) &&
                     ParsesAs("/xhbar help", RuntimeCommandKind::XHBarHelp),
                 "runtime command grammar was not parsed") ||
        !Require(!ParseRuntimeCommand("/fps") && !ParseRuntimeCommand("/addons list") && !ParseRuntimeCommand("/loader x") && !ParseRuntimeCommand("/xhbars") &&
                     !ParseRuntimeCommand("/xhbar") && !ParseRuntimeCommand("/xhbar ps") && !ParseRuntimeCommand("/xhbar xbox now") &&
                     !ParseRuntimeCommand("/xhbar help me") && !ParseRuntimeCommand(""),
                 "a non-runtime command was claimed"))
    {
        return 1;
    }

    std::error_code error;
    std::filesystem::remove_all(settings, error);
    std::filesystem::remove_all(chatLogs, error);
    AddonHost addons;
    addons.LoadManifests({}, settings, chatLogs);
    ChatCapture chat;
    PluginStub  screenshot;
    PluginStub  discord;
    discord.loaded = true;
    RuntimeCommands commands;
    commands.Configure(&addons,
                       {
                           { "fps", fpsManifest },
                           { "pos", posManifest },
                           { "command-words-test", wordsManifest },
                           { "load-fault-test", faultManifest },
                           { "command-table-test", tableManifest },
                       },
                       {
                           { "screenshot", &screenshot, &LoadStub, &UnloadStub, &StubLoaded },
                           { "discord-rpc", &discord, &LoadStub, &UnloadStub, &StubLoaded },
                       },
                       { &chat, &CaptureChat });

    // Reserved, malformed and repeated words are not claimed again.
    const std::vector<std::string> expectedWords = { "/addon", "/load", "/unload", "/plugins", "/xhbar", "/table", "/second", "/fps", "/words", "/later", "/single", "/pos" };
    if (!Require(commands.CommandWords() == expectedWords, "claimed command words were incorrect") ||
        !Require(commands.IsInstalled("pos") && !commands.IsInstalled("screenshot"), "catalog membership was incorrect"))
    {
        return 1;
    }

    const auto run = [&](std::string_view command)
    {
        chat.lines.clear();
        if (!RuntimeCommands::Dispatch(&commands, command))
        {
            return std::vector<std::string>{ "<not handled>" };
        }
        return chat.lines;
    };
    using Lines = std::vector<std::string>;

    if (!Require(!RuntimeCommands::Dispatch(&commands, "/fps lock"), "an unloaded addon command was handled"))
    {
        return 1;
    }
    if (!Require(run("/addon load fps") == Lines{ "Loaded addon: fps" } && addons.IsLoaded("fps"), "addon load was incorrect") ||
        !Require(RuntimeCommands::Dispatch(&commands, "/fps lock"), "loaded addon command was not dispatched") ||
        !Require(run("/addon load fps") == Lines{ "Addon is already loaded: fps" }, "duplicate load was not reported") ||
        !Require(run("/addon list") == Lines{ "Addons: fps" }, "addon list was incorrect") ||
        !Require(run("/addon reload fps") == Lines{ "Reloaded addon: fps" } && addons.IsLoaded("fps"), "addon reload was incorrect") ||
        !Require(run("/addon unload fps") == Lines{ "Unloaded addon: fps" } && !addons.IsLoaded("fps"), "addon unload was incorrect") ||
        !Require(!RuntimeCommands::Dispatch(&commands, "/fps lock"), "unloaded addon still handled its command") ||
        !Require(run("/addon unload fps") == Lines{ "Addon is not loaded: fps" }, "repeated unload was not reported") ||
        !Require(run("/addon load missing") == Lines{ "Addon is not installed: missing" }, "unknown addon was not reported") ||
        !Require(run("/addon load screenshot") == Lines{ "screenshot is a plugin; use /load screenshot" }, "plugin name was not redirected") ||
        !Require(run("/addon reload pos") == Lines{ "Loaded addon: pos" } && addons.IsLoaded("pos"), "reload of an unloaded addon did not load it") ||
        !Require(run("/addon unload pos") == Lines{ "Unloaded addon: pos" }, "pos unload was incorrect") ||
        !Require(run("/addon enable fps") == Lines{ "Usage: /addon load|unload|reload <addon>, /addon list" }, "addon usage was not printed"))
    {
        return 1;
    }

    if (!Require(run("/addon load load-fault-test") == Lines{ "Addon failed to load: load-fault-test" }, "load fault was not reported") ||
        !Require(run("/addon list") == Lines{ "Addons: load-fault-test (faulted)" }, "faulted addon was not listed") ||
        !Require(run("/addon unload load-fault-test") == Lines{ "Unloaded addon: load-fault-test" }, "faulted addon did not unload") ||
        !Require(run("/addon list") == Lines{ "Addons: none loaded" }, "empty addon list was incorrect"))
    {
        return 1;
    }

    if (!Require(run("/addon load command-words-test") == Lines{ "Loaded addon: command-words-test" }, "fixture addon did not load") ||
        !Require(RuntimeCommands::Dispatch(&commands, "/words go") && !RuntimeCommands::Dispatch(&commands, "/words stop"),
                 "manifest-declared command was not dispatched to its addon") ||
        !Require(run("/xhbar foo") == Lines{ "<not handled>" }, "an unclaimed reserved form reached an addon") ||
        !Require(run("/addon unload command-words-test") == Lines{ "Unloaded addon: command-words-test" }, "fixture addon did not unload"))
    {
        return 1;
    }

    if (!Require(run("/addon load command-table-test") == Lines{ "Loaded addon: command-table-test" }, "table fixture addon did not load") ||
        !Require(RuntimeCommands::Dispatch(&commands, "/table go"), "[[commands]] command was not dispatched to its addon") ||
        !Require(run("/addon unload command-table-test") == Lines{ "Unloaded addon: command-table-test" }, "table fixture addon did not unload"))
    {
        return 1;
    }

    if (!Require(run("/plugins") == Lines{ "Plugins: screenshot (not loaded), discord-rpc (loaded)" }, "plugin list was incorrect") ||
        !Require(run("/load screenshot") == Lines{ "Loaded plugin: screenshot" } && screenshot.loaded, "plugin load was incorrect") ||
        !Require(run("/load screenshot") == Lines{ "Plugin is already loaded: screenshot" } && screenshot.loads == 1, "duplicate plugin load reached the plugin") ||
        !Require(run("/unload discord-rpc") == Lines{ "Unloaded plugin: discord-rpc" } && !discord.loaded, "plugin unload was incorrect") ||
        !Require(run("/unload discord-rpc") == Lines{ "Plugin is not loaded: discord-rpc" } && discord.unloads == 1, "repeated plugin unload reached the plugin") ||
        !Require(run("/load fps") == Lines{ "fps is an addon; use /addon load fps" }, "addon name was not redirected") ||
        !Require(run("/unload nothing") == Lines{ "Plugin is not installed: nothing" }, "unknown plugin was not reported") ||
        !Require(run("/load") == Lines{ "Usage: /load <plugin>, /unload <plugin>, /plugins" }, "plugin usage was not printed"))
    {
        return 1;
    }
    discord.failLoad = true;
    if (!Require(run("/load discord-rpc") == Lines{ "Plugin failed to load: discord-rpc (plugin DLL could not be loaded)" }, "plugin load failure was not reported"))
    {
        return 1;
    }

    if (!Require(run("/xhbar help") == Lines{ "/xhbar xbox", "/xhbar playstation", "/xhbar toggle" }, "XHBar help was not printed") ||
        !Require(run("/xhbar toggle") == Lines{ "XHBar is unavailable" }, "missing XHBar was not reported"))
    {
        return 1;
    }
    {
        OverlayStub     overlay;
        ChatCapture     overlayChat;
        RuntimeCommands overlayCommands;
        overlayCommands.Configure(nullptr, {}, {}, { &overlayChat, &CaptureChat }, { &overlay, &SetStubLabels, &ToggleStub });
        const auto runOverlay = [&](std::string_view command)
        {
            overlayChat.lines.clear();
            return RuntimeCommands::Dispatch(&overlayCommands, command) ? overlayChat.lines : Lines{ "<not handled>" };
        };
        if (!Require(runOverlay("/xhbar playstation") == Lines{ "XHBar labels: playstation" } && overlay.labels == "playstation", "labels were not applied") ||
            !Require(runOverlay("/xhbar ps") == Lines{ "<not handled>" } && overlay.sets == 1, "an unknown style was claimed") ||
            !Require(runOverlay("/xhbar toggle") == Lines{ "XHBar hidden" } && !overlay.visible, "toggle did not hide") ||
            !Require(runOverlay("/xhbar toggle") == Lines{ "XHBar shown" } && overlay.visible, "toggle did not show"))
        {
            return 1;
        }
        overlay.save = false;
        if (!Require(runOverlay("/xhbar xbox") == Lines{ "XHBar labels: xbox (not saved)" } && overlay.labels == "xbox", "an unsaved label change was not marked") ||
            !Require(runOverlay("/xhbar toggle") == Lines{ "XHBar hidden (not saved)" }, "an unsaved toggle was not marked"))
        {
            return 1;
        }
    }

    // A command still applies when chat rejects its feedback.
    chat.accept = false;
    if (!Require(run("/addon load pos").empty() && addons.IsLoaded("pos"), "command did not apply while chat was unavailable"))
    {
        return 1;
    }

    std::filesystem::remove_all(settings, error);
    std::filesystem::remove_all(chatLogs, error);
    std::cout << "runtime-commands-tests: PASS\n";
    return 0;
}
