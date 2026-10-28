#include "action_movement.h"
#include "addon_host.h"
#include "command_boundary.h"
#include "runtime_commands.h"

#include <algorithm>
#include <array>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <string_view>
#include <vector>

namespace
{

using bahamut_client::ActionMovementBoundaryState;
using bahamut_client::ActionMovementService;

bool Check(bool value, std::string_view message)
{
    if (!value)
    {
        std::cerr << message << '\n';
    }
    return value;
}

std::filesystem::path WriteAddon(const std::filesystem::path& root, std::string_view id, std::string_view script)
{
    std::filesystem::create_directories(root);
    std::ofstream(root / L"addon.toml", std::ios::binary) << "id = \"" << id << "\"\nentry = \"fixture.lua\"\n";
    std::ofstream(root / L"fixture.lua", std::ios::binary) << script;
    return root / L"addon.toml";
}

bool CaptureChat(void* context, std::string_view text)
{
    static_cast<std::vector<std::string>*>(context)->emplace_back(text);
    return true;
}

bool Contains(const std::vector<std::string>& lines, std::string_view text)
{
    for (const auto& line : lines)
    {
        if (line.find(text) != std::string::npos)
        {
            return true;
        }
    }
    return false;
}

bool CheckApi(const std::filesystem::path& root)
{
    constexpr std::string_view script   = R"lua(
local configure = bahamut.action_movement_configure
local status = bahamut.action_movement_status
function load()
    assert(type(configure) == 'function' and type(status) == 'function')
    assert(not status().available and status().effective_percent == 100)
    local ok, reason = configure(0, true)
    assert(not ok and reason == 'not_admitted')
end
function command(name)
    if name == '/api valid' then
        local ok, reason = configure(40, true)
        assert(ok and reason == 'ready')
    elseif name == '/api invalid' then
        local function bad(...)
            local ok, reason = configure(...)
            assert(not ok and reason == 'invalid_request')
        end
        bad(); bad(40); bad(40, true, 'extra'); bad('40', true)
        bad(40, 'true'); bad(40, 1); bad(nil, true); bad({}, true)
        bad(-1, true); bad(101, true); bad(40.5, true)
        bad(0/0, true); bad(math.huge, true); bad(-math.huge, true)
        local value, reason = status('extra')
        assert(value == nil and reason == 'invalid_request')
    elseif name == '/api copy' then
        local value = status()
        assert(value.available and value.enabled and value.requested_percent == 40 and value.effective_percent == 40)
        assert(value.owner == nil and value.session == nil)
        value.requested_percent = 0; value.enabled = false
        assert(status().requested_percent == 40 and status().enabled)
    elseif name == '/api stale' then
        assert(not configure(0, true))
        local value = status()
        assert(not value.available and not value.enabled and value.effective_percent == 100 and value.requested_percent == 0)
    elseif name == '/api off' then
        assert(configure(40, false))
        assert(not status().enabled and status().requested_percent == 40 and status().effective_percent == 100)
    elseif name:match('^/api reason ') then
        local expected = name:sub(13)
        local value = status()
        assert(not value.available and not value.enabled and value.reason == expected and value.effective_percent == 100)
        local ok, reason = configure(0, true)
        assert(not ok and reason == expected)
    elseif name == '/api fault' then
        assert(configure(0, true))
        error('fault after acquisition')
    else
        return false
    end
    return true
end
function unload()
    local ok, reason = configure(0, true)
    assert(not ok and reason == 'not_admitted' and not status().available)
    bahamut.chat_print('unload stayed revoked')
end
)lua";
    const auto                 manifest = WriteAddon(root / L"consumer", "nolock", script);
    const auto                 other    = WriteAddon(root / L"other", "other", "function load() assert(bahamut.action_movement_configure == nil and bahamut.action_movement_status == nil) end\n");
    ActionMovementService      service;
    service.SetBoundaryState(ActionMovementBoundaryState::Ready);
    auto                     session = service.BeginSession();
    AddonHost                first;
    AddonHost                second;
    std::vector<std::string> chat;
    first.SetChatSink({ &chat, CaptureChat });
    second.SetChatSink({ &chat, CaptureChat });
    first.LoadManifests({ manifest, other }, root / L"first-settings", root / L"first-logs");
    second.LoadManifests({ manifest }, root / L"second-settings", root / L"second-logs");
    bool passed = Check(first.LoadedCount() == 2 && second.LoadedCount() == 1, "API admission or isolation load assertions failed");
    passed &= Check(first.AdmitActionMovementConsumer(service, manifest, session) && first.DispatchCommand("/api valid"), "Typed configure failed");
    passed &= Check(first.DispatchCommand("/api invalid") && first.DispatchCommand("/api copy") && service.Status().effectivePercent == 40,
                    "Invalid requests or copied status changed policy");
    passed &= Check(first.DispatchCommand("/api off") && !service.Status().enabled, "Typed off failed");
    passed &= Check(second.AdmitActionMovementConsumer(service, manifest, session) && second.DispatchCommand("/api valid"), "Replacement admission failed");
    passed &= Check(first.DispatchCommand("/api stale") && service.Status().effectivePercent == 40, "Stale Lua instance affected replacement");
    first.ShutdownActionMovement();
    const std::array<std::pair<ActionMovementBoundaryState, std::string_view>, 4> failures = { {
        { ActionMovementBoundaryState::TargetMismatch, "target_mismatch" },
        { ActionMovementBoundaryState::InstallFailed, "install_failed" },
        { ActionMovementBoundaryState::CleanupFailed, "cleanup_failed" },
        { ActionMovementBoundaryState::Unavailable, "unavailable" },
    } };
    for (const auto& [boundary, reason] : failures)
    {
        service.SetBoundaryState(boundary);
        passed &= Check(second.DispatchCommand(std::string("/api reason ") + std::string(reason)), "Boundary reason was not exposed accurately");
    }
    service.SetBoundaryState(ActionMovementBoundaryState::Ready);
    session = service.BeginSession();
    passed &= Check(second.AdmitActionMovementConsumer(service, manifest, session) && second.DispatchCommand("/api valid"), "Fresh API admission failed");
    passed &= Check(service.EndSession(session) && second.DispatchCommand("/api reason session_unavailable"), "Lost session still accepted Lua requests");
    session = service.BeginSession();
    passed &= Check(second.AdmitActionMovementConsumer(service, manifest, session) && second.DispatchCommand("/api valid"), "Session replacement API failed");
    passed &= Check(!second.DispatchCommand("/api fault") && second.FaultedCount() == 1 && !service.Status().enabled && service.Status().owner == 0,
                    "Lua fault retained policy");
    passed &= Check(first.Disable("nolock") && Contains(chat, "unload stayed revoked"), "Lua unload regained policy or failed its assertion");
    second.ShutdownActionMovement();
    return passed;
}

struct Token
{
    const char*                                           text = nullptr;
    std::array<unsigned char, 0x54 - sizeof(const char*)> rest{};
};

static_assert(sizeof(Token) == 0x54);

int OriginalLookup(void*, const void*, void*)
{
    return 42;
}

int Lookup(CommandBoundary& boundary, std::initializer_list<const char*> text)
{
    std::array<Token, 4> tokens{};
    std::size_t          count = 0;
    for (const char* value : text)
    {
        tokens[count++].text = value;
    }
    const auto* end = reinterpret_cast<const unsigned char*>(tokens.data()) + count * sizeof(Token);
    return boundary.HandleLookupForTest(OriginalLookup, nullptr, tokens.data(), nullptr, true, tokens.data(), end);
}

bool CheckLoadAppliesFiftyPercent(const std::filesystem::path& root, const std::filesystem::path& manifest)
{
    std::filesystem::remove_all(root);
    const auto               settings = root / L"settings";
    ActionMovementService    service;
    AddonHost                host;
    std::vector<std::string> chat;
    host.SetChatSink({ &chat, CaptureChat });
    // The service is not ready: load must not fault and admission must be refused.
    host.LoadManifests({ manifest }, settings, root / L"logs");
    bool passed = Check(host.IsLoaded("nolock") && host.FaultedCount() == 0, "NoLock faulted while the service was unavailable");
    passed &= Check(!host.AdmitActionMovementConsumer(service, manifest, 1) && service.Status().owner == 0 && !service.Status().enabled, "Unavailable service was admitted");
    host.Update(0.01);
    passed &= Check(host.FaultedCount() == 0 && !service.Status().enabled, "Update faulted or enabled the effect without the service");

    service.SetBoundaryState(ActionMovementBoundaryState::Ready);
    auto session = service.BeginSession();
    passed &= Check(host.AdmitActionMovementConsumer(service, manifest, session) && !service.Status().enabled, "Ready admission failed or applied before an update");
    host.Update(0.01);
    passed &= Check(service.Status().enabled && service.Status().requestedPercent == 50 && service.Status().effectivePercent == 50, "Update did not apply 50% once the service was available");

    // Session loss disables the effect; the next admitted session is configured again.
    passed &= Check(service.EndSession(session), "Session end failed");
    host.Update(0.01);
    passed &= Check(host.FaultedCount() == 0 && !service.Status().enabled && service.Status().owner == 0, "Update rearmed a lost session");
    session = service.BeginSession();
    passed &= Check(host.AdmitActionMovementConsumer(service, manifest, session), "Fresh session admission failed");
    host.Update(0.01);
    passed &= Check(service.Status().enabled && service.Status().effectivePercent == 50, "Fresh session was not configured");

    host.ShutdownActionMovement();
    host.Update(0.01);
    passed &= Check(!service.Status().enabled && !host.AdmitActionMovementConsumer(service, manifest, session), "Stopped host Lua update rearmed policy");
    passed &= Check(chat.empty() && !std::filesystem::exists(settings / L"nolock/settings.ini"), "NoLock printed chat or saved a preference");
    return passed;
}

bool CheckNoCommands(const std::filesystem::path& root, const std::filesystem::path& manifest, const std::filesystem::path& fps)
{
    std::filesystem::remove_all(root);
    AddonHost                host;
    std::vector<std::string> chat;
    host.SetChatSink({ &chat, CaptureChat });
    host.LoadManifests({ manifest }, root / L"settings", root / L"logs");
    RuntimeCommands commands;
    commands.Configure(&host, { { "nolock", manifest }, { "fps", fps } }, {}, { &chat, CaptureChat });
    const auto&     words  = commands.CommandWords();
    bool            passed = Check(host.IsLoaded("nolock") && std::find(words.begin(), words.end(), "/nolock") == words.end() &&
                                       std::find(words.begin(), words.end(), "/fps") != words.end(),
                                   "NoLock registered a command word");
    CommandBoundary boundary(&commands, &RuntimeCommands::Dispatch, words);
    passed &= Check(!host.DispatchCommand("/nolock") && !host.DispatchCommand("/nolock on") && !host.DispatchCommand("/nolock status"), "NoLock handled a command");
    passed &= Check(Lookup(boundary, { "/nolock" }) == 42 && Lookup(boundary, { "/nolock", "on" }) == 42 && Lookup(boundary, { "/nolock", "help" }) == 42,
                    "NoLock commands were not forwarded to retail");
    // With another command addon loaded, /nolock still passes through while /fps is handled.
    passed &= Check(host.Enable(fps) && host.IsLoaded("fps"), "Command addon did not enable");
    passed &= Check(Lookup(boundary, { "/nolock" }) == 42 && Lookup(boundary, { "/nolock", "off" }) == 42 && Lookup(boundary, { "/fps", "help" }) == -2,
                    "Command boundary intercepted /nolock or dropped /fps");
    passed &= Check(!Contains(chat, "NoLock"), "NoLock printed chat");
    return passed;
}

bool CheckRegisteredAdmission(const std::filesystem::path& root, const std::filesystem::path& manifest)
{
    std::filesystem::remove_all(root);
    ActionMovementService service;
    service.SetBoundaryState(ActionMovementBoundaryState::Ready);
    const auto session = service.BeginSession();
    AddonHost  host;
    bool       passed = Check(!host.RegisterActionMovementConsumer(service, "relative/addon.toml", session) && !host.RegisterActionMovementConsumer(service, manifest, 0),
                              "Invalid registration accepted");
    passed &= Check(host.RegisterActionMovementConsumer(service, manifest, session) && service.Status().owner == 0, "Registration without a loaded addon bound an owner");
    // A late load is admitted before its load callback requests 50%.
    host.LoadManifests({ manifest }, root / L"settings", root / L"logs");
    const auto first = service.Status().owner;
    passed &= Check(first != 0 && service.Status().enabled && service.Status().effectivePercent == 50, "Late load was not admitted before its load callback");
    passed &= Check(service.Configure(first, session, 50, false) && !service.Status().enabled, "Service could not revoke the effect");
    host.Update(0.01);
    passed &= Check(service.Status().enabled && service.Status().requestedPercent == 50 && service.Status().effectivePercent == 50, "Update did not restore a disabled effect");
    passed &= Check(service.Configure(first, session, 40, true), "Service could not change the percentage");
    host.Update(0.01);
    passed &= Check(service.Status().enabled && service.Status().requestedPercent == 50 && service.Status().effectivePercent == 50, "Update did not restore 50%");
    // Reload must revoke the old owner and request 50% before an update.
    passed &= Check(host.Reload("nolock"), "Reload failed");
    const auto second = service.Status().owner;
    passed &= Check(second != 0 && second != first && !service.Configure(first, session, 50, true), "Reload did not replace the owner");
    passed &= Check(service.Status().enabled && service.Status().requestedPercent == 50 && service.Status().effectivePercent == 50, "Reload did not request 50%");
    passed &= Check(host.Disable("nolock") && service.Status().owner == 0 && !service.Status().enabled, "Disable kept the owner");
    passed &= Check(host.Enable(manifest) && service.Status().owner != 0 && service.Status().enabled, "Late enable was not admitted");
    host.ShutdownActionMovement();
    passed &= Check(service.Status().owner == 0 && !service.Status().enabled, "Shutdown kept the owner");
    passed &= Check(host.Reload("nolock") && service.Status().owner == 0 && !host.RegisterActionMovementConsumer(service, manifest, session), "Admitted after shutdown");
    passed &= Check(host.Disable("nolock") && host.Enable(manifest) && service.Status().owner == 0, "Late enable admitted after shutdown");
    return passed;
}

bool CheckFaultedAdmission(const std::filesystem::path& root)
{
    std::filesystem::remove_all(root);
    constexpr std::string_view good   = "function load() assert(bahamut.action_movement_configure(40, true)) end\n";
    constexpr std::string_view broken = "error('top level')\n";
    constexpr std::string_view late   = "function load() assert(bahamut.action_movement_configure(40, true)); error('load') end\n";
    const auto                 path   = WriteAddon(root / L"consumer", "nolock", good);
    const auto                 script = root / L"consumer/fixture.lua";
    ActionMovementService      service;
    service.SetBoundaryState(ActionMovementBoundaryState::Ready);
    const auto session = service.BeginSession();
    AddonHost  host;
    bool       passed = Check(host.RegisterActionMovementConsumer(service, path, session), "Fixture registration failed");
    host.LoadManifests({ path }, root / L"settings", root / L"logs");
    passed &= Check(service.Status().owner != 0 && service.Status().effectivePercent == 40 && host.FaultedCount() == 0, "Healthy fixture was not admitted");
    std::ofstream(script, std::ios::binary | std::ios::trunc) << broken;
    passed &= Check(host.Reload("nolock") && host.FaultedCount() == 1 && service.Status().owner == 0 && !service.Status().enabled, "Instance that faulted at load was admitted");
    std::ofstream(script, std::ios::binary | std::ios::trunc) << late;
    passed &= Check(host.Reload("nolock") && host.FaultedCount() == 1 && service.Status().owner == 0 && !service.Status().enabled, "Instance that faulted in load kept its owner");
    std::ofstream(script, std::ios::binary | std::ios::trunc) << good;
    passed &= Check(host.Reload("nolock") && host.FaultedCount() == 0 && service.Status().owner != 0 && service.Status().effectivePercent == 40, "Healthy reload after faults was not admitted");
    host.ShutdownActionMovement();
    return passed;
}

bool CheckOlderHost(const std::filesystem::path& root, const std::filesystem::path& manifest)
{
    std::ifstream      input(manifest.parent_path() / L"nolock.lua", std::ios::binary);
    std::ostringstream source;
    source << input.rdbuf();
    bool         passed = true;
    unsigned int index  = 0;
    for (const auto prefix : { "bahamut.action_movement_configure = nil\nbahamut.action_movement_status = nil\n",
                               "bahamut.action_movement_status = nil\n",
                               "bahamut.action_movement_configure = nil\n" })
    {
        const auto            directory   = root / std::to_string(index++);
        const auto            oldManifest = WriteAddon(directory / L"consumer", "nolock", std::string(prefix) + source.str());
        ActionMovementService service;
        service.SetBoundaryState(ActionMovementBoundaryState::Ready);
        const auto               session = service.BeginSession();
        AddonHost                host;
        std::vector<std::string> chat;
        host.SetChatSink({ &chat, CaptureChat });
        host.LoadManifests({ oldManifest }, directory / L"settings", directory / L"logs");
        passed &= Check(host.AdmitActionMovementConsumer(service, oldManifest, session), "Older-host fixture native admission failed");
        host.Update(0.01);
        passed &= Check(host.IsLoaded("nolock") && host.FaultedCount() == 0 && !service.Status().enabled && chat.empty(),
                        "Missing or partial older-host APIs faulted or enabled an effect");
        host.ShutdownActionMovement();
    }
    return passed;
}

} // namespace

int wmain(int argc, wchar_t** argv)
{
    if (argc != 4 || !std::filesystem::path(argv[1]).is_absolute())
    {
        return 2;
    }
    const std::filesystem::path root(argv[1]);
    const bool                  api       = CheckApi(root / L"api");
    const bool                  load      = CheckLoadAppliesFiftyPercent(root / L"load", argv[2]);
    const bool                  commands  = CheckNoCommands(root / L"commands", argv[2], argv[3]);
    const bool                  admission = CheckRegisteredAdmission(root / L"admission", argv[2]);
    const bool                  faulted   = CheckFaultedAdmission(root / L"faulted");
    const bool                  olderHost = CheckOlderHost(root / L"older-host", argv[2]);
    return api && load && commands && admission && faulted && olderHost ? 0 : 1;
}
