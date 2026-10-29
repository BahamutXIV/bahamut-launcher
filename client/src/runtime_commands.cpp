#include "runtime_commands.h"

#include <algorithm>
#include <utility>

namespace
{

// Retail local-log lines accept 512 bytes; list output wraps well below that.
constexpr std::size_t kMaximumListLineBytes = 400;

std::vector<std::string_view> SplitWords(std::string_view command)
{
    std::vector<std::string_view> words;
    std::size_t                   cursor = 0;
    while (cursor < command.size())
    {
        const std::size_t begin = command.find_first_not_of(' ', cursor);
        if (begin == std::string_view::npos)
        {
            break;
        }
        const std::size_t end = std::min(command.find(' ', begin), command.size());
        words.push_back(command.substr(begin, end - begin));
        cursor = end;
    }
    return words;
}

bool IsRuntimeWord(std::string_view word)
{
    return std::find(kRuntimeCommandWords.begin(), kRuntimeCommandWords.end(), word) != kRuntimeCommandWords.end();
}

std::vector<std::string> ListLines(std::string_view prefix, const std::vector<std::string>& entries)
{
    std::vector<std::string> lines{ std::string(prefix) };
    bool                     first = true;
    for (const auto& entry : entries)
    {
        if (!first && lines.back().size() + 2 + entry.size() > kMaximumListLineBytes)
        {
            lines.emplace_back("  ");
            first = true;
        }
        if (!first)
        {
            lines.back().append(", ");
        }
        lines.back().append(entry);
        first = false;
    }
    return lines;
}

} // namespace

std::optional<RuntimeCommand> ParseRuntimeCommand(std::string_view command)
{
    const auto words = SplitWords(command);
    if (words.empty() || !IsRuntimeWord(words[0]))
    {
        return std::nullopt;
    }
    if (words[0] == "/addon")
    {
        if (words.size() == 2 && words[1] == "list")
        {
            return RuntimeCommand{ RuntimeCommandKind::AddonList, {} };
        }
        if (words.size() == 3)
        {
            const std::string name(words[2]);
            if (words[1] == "load")
                return RuntimeCommand{ RuntimeCommandKind::AddonLoad, name };
            if (words[1] == "unload")
                return RuntimeCommand{ RuntimeCommandKind::AddonUnload, name };
            if (words[1] == "reload")
                return RuntimeCommand{ RuntimeCommandKind::AddonReload, name };
        }
        return RuntimeCommand{ RuntimeCommandKind::AddonUsage, {} };
    }
    if (words[0] == "/plugins")
    {
        return RuntimeCommand{ words.size() == 1 ? RuntimeCommandKind::PluginList : RuntimeCommandKind::PluginUsage, {} };
    }
    if (words[0] == "/xhbar")
    {
        if (words.size() == 2 && (words[1] == "xbox" || words[1] == "playstation"))
            return RuntimeCommand{ RuntimeCommandKind::XHBarLabels, std::string(words[1]) };
        if (words.size() == 2 && words[1] == "toggle")
            return RuntimeCommand{ RuntimeCommandKind::XHBarToggle, {} };
        if (words.size() == 2 && words[1] == "help")
            return RuntimeCommand{ RuntimeCommandKind::XHBarHelp, {} };
        return std::nullopt;
    }
    if (words.size() != 2)
    {
        return RuntimeCommand{ RuntimeCommandKind::PluginUsage, {} };
    }
    return RuntimeCommand{ words[0] == "/load" ? RuntimeCommandKind::PluginLoad : RuntimeCommandKind::PluginUnload, std::string(words[1]) };
}

void RuntimeCommands::Configure(AddonHost*                                             addons,
                                std::unordered_map<std::string, std::filesystem::path> catalog,
                                std::vector<RuntimePlugin>                             plugins,
                                const AddonChatSink&                                   chat,
                                const RuntimeXHBar&                                    xhbar)
{
    addons_  = addons;
    catalog_ = std::move(catalog);
    plugins_ = std::move(plugins);
    chat_    = chat;
    xhbar_   = xhbar;
    words_.assign(kRuntimeCommandWords.begin(), kRuntimeCommandWords.end());
    // Sort ids so the claimed word order does not depend on map iteration.
    std::vector<std::string> ids;
    for (const auto& [id, manifest] : catalog_)
    {
        ids.push_back(id);
    }
    std::sort(ids.begin(), ids.end());
    for (const auto& id : ids)
    {
        for (auto& word : ReadAddonManifestCommandWords(catalog_.at(id)))
        {
            if (std::find(words_.begin(), words_.end(), word) == words_.end())
            {
                words_.push_back(std::move(word));
            }
        }
    }
}

bool RuntimeCommands::IsInstalled(std::string_view addonId) const
{
    return catalog_.find(std::string(addonId)) != catalog_.end();
}

const std::vector<std::string>& RuntimeCommands::CommandWords() const
{
    return words_;
}

std::vector<std::string> RuntimeCommands::Execute(const RuntimeCommand& command)
{
    switch (command.kind)
    {
        case RuntimeCommandKind::AddonLoad:
        case RuntimeCommandKind::AddonUnload:
        case RuntimeCommandKind::AddonReload:
        case RuntimeCommandKind::AddonList:
            return ExecuteAddon(command);
        case RuntimeCommandKind::AddonUsage:
            return { "Usage: /addon load|unload|reload <addon>, /addon list" };
        case RuntimeCommandKind::PluginLoad:
        case RuntimeCommandKind::PluginUnload:
        case RuntimeCommandKind::PluginList:
            return ExecutePlugin(command);
        case RuntimeCommandKind::PluginUsage:
            return { "Usage: /load <plugin>, /unload <plugin>, /plugins" };
        case RuntimeCommandKind::XHBarLabels:
        case RuntimeCommandKind::XHBarToggle:
            return ExecuteXHBar(command);
        case RuntimeCommandKind::XHBarHelp:
            return { "/xhbar xbox", "/xhbar playstation", "/xhbar toggle" };
    }
    return {};
}

std::vector<std::string> RuntimeCommands::ExecuteXHBar(const RuntimeCommand& command)
{
    if (xhbar_.setLabels == nullptr || xhbar_.toggle == nullptr)
    {
        return { "XHBar is unavailable" };
    }
    if (command.kind == RuntimeCommandKind::XHBarToggle)
    {
        bool       visible = false;
        const bool saved   = xhbar_.toggle(xhbar_.context, visible);
        return { std::string(visible ? "XHBar shown" : "XHBar hidden") + (saved ? "" : " (not saved)") };
    }
    const bool saved = xhbar_.setLabels(xhbar_.context, command.name);
    return { "XHBar labels: " + command.name + (saved ? "" : " (not saved)") };
}

std::vector<std::string> RuntimeCommands::ExecuteAddon(const RuntimeCommand& command)
{
    if (addons_ == nullptr)
    {
        return { "Addons are unavailable" };
    }
    const std::string& name = command.name;
    if (command.kind == RuntimeCommandKind::AddonList)
    {
        std::vector<std::string> entries;
        for (const auto& id : addons_->LoadedIds())
        {
            entries.push_back(addons_->IsFaulted(id) ? id + " (faulted)" : id);
        }
        if (entries.empty())
        {
            return { "Addons: none loaded" };
        }
        return ListLines("Addons: ", entries);
    }

    const bool loaded = addons_->IsLoaded(name);
    const auto found  = catalog_.find(name);
    if (!loaded && found == catalog_.end())
    {
        if (FindPlugin(name) != nullptr)
        {
            return { name + " is a plugin; use /load " + name };
        }
        return { "Addon is not installed: " + name };
    }
    if (command.kind == RuntimeCommandKind::AddonUnload)
    {
        if (!loaded)
        {
            return { "Addon is not loaded: " + name };
        }
        return { addons_->Disable(name) ? "Unloaded addon: " + name : "Addon failed to unload: " + name };
    }
    if (command.kind == RuntimeCommandKind::AddonLoad && loaded && !addons_->IsFaulted(name))
    {
        return { "Addon is already loaded: " + name };
    }

    const bool applied   = loaded ? addons_->Reload(name) : addons_->Enable(found->second);
    const bool succeeded = applied && addons_->IsLoaded(name) && !addons_->IsFaulted(name);
    if (!succeeded)
    {
        return { "Addon failed to load: " + name };
    }
    return { (command.kind == RuntimeCommandKind::AddonReload && loaded ? "Reloaded addon: " : "Loaded addon: ") + name };
}

std::vector<std::string> RuntimeCommands::ExecutePlugin(const RuntimeCommand& command)
{
    if (command.kind == RuntimeCommandKind::PluginList)
    {
        std::vector<std::string> entries;
        for (const auto& plugin : plugins_)
        {
            const bool loaded = plugin.loaded != nullptr && plugin.loaded(plugin.context);
            entries.push_back(plugin.name + (loaded ? " (loaded)" : " (not loaded)"));
        }
        if (entries.empty())
        {
            return { "Plugins: none installed" };
        }
        return ListLines("Plugins: ", entries);
    }

    const std::string&   name   = command.name;
    const RuntimePlugin* plugin = FindPlugin(name);
    if (plugin == nullptr)
    {
        if (IsInstalled(name) || (addons_ != nullptr && addons_->IsLoaded(name)))
        {
            return { name + " is an addon; use /addon " + (command.kind == RuntimeCommandKind::PluginLoad ? "load " : "unload ") + name };
        }
        return { "Plugin is not installed: " + name };
    }
    const bool  loaded = plugin->loaded != nullptr && plugin->loaded(plugin->context);
    std::string error;
    if (command.kind == RuntimeCommandKind::PluginLoad)
    {
        if (loaded)
        {
            return { "Plugin is already loaded: " + name };
        }
        if (plugin->load == nullptr || !plugin->load(plugin->context, error))
        {
            return { "Plugin failed to load: " + name + (error.empty() ? "" : " (" + error + ")") };
        }
        return { "Loaded plugin: " + name };
    }
    if (!loaded)
    {
        return { "Plugin is not loaded: " + name };
    }
    if (plugin->unload == nullptr || !plugin->unload(plugin->context, error))
    {
        return { "Plugin failed to unload: " + name + (error.empty() ? "" : " (" + error + ")") };
    }
    return { "Unloaded plugin: " + name };
}

const RuntimePlugin* RuntimeCommands::FindPlugin(std::string_view name) const
{
    const auto found = std::find_if(plugins_.begin(), plugins_.end(), [&](const RuntimePlugin& plugin)
                                    {
                                        return plugin.name == name;
                                    });
    return found == plugins_.end() ? nullptr : &*found;
}

bool RuntimeCommands::Dispatch(void* context, std::string_view command)
{
    auto* commands = static_cast<RuntimeCommands*>(context);
    if (commands == nullptr)
    {
        return false;
    }
    const auto parsed = ParseRuntimeCommand(command);
    if (!parsed)
    {
        // Keep unclaimed reserved words out of addon callbacks, which see every command.
        const auto words = SplitWords(command);
        if (!words.empty() && IsRuntimeWord(words[0]))
        {
            return false;
        }
        return commands->addons_ != nullptr && commands->addons_->DispatchCommand(command);
    }
    for (const auto& line : commands->Execute(*parsed))
    {
        if (commands->chat_.print != nullptr)
        {
            static_cast<void>(commands->chat_.print(commands->chat_.context, line));
        }
    }
    return true;
}
