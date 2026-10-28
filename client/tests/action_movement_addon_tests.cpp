#include "action_movement.h"
#include "addon_host.h"

#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <string_view>

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
    std::ofstream manifest(root / L"addon.toml", std::ios::binary);
    manifest << "id = \"" << id << "\"\nentry = \"fixture.lua\"\n";
    manifest.close();
    std::ofstream lua(root / L"fixture.lua", std::ios::binary);
    lua << script;
    lua.close();
    return root / L"addon.toml";
}

bool Arm(AddonHost& host, ActionMovementService& service, const std::filesystem::path& manifest, std::uint64_t session)
{
    if (!host.AdmitActionMovementConsumer(service, manifest, session))
    {
        return false;
    }
    const auto status = service.Status();
    return service.Configure(status.owner, session, 40, true);
}

struct UnloadCapture
{
    ActionMovementService* service;
    AddonHost*             host;
    std::filesystem::path  manifest;
    std::uint64_t          session;
    bool                   called     = false;
    bool                   revoked    = false;
    bool                   readmitted = false;
};

bool ObserveUnload(void* context, std::string_view)
{
    auto& capture      = *static_cast<UnloadCapture*>(context);
    capture.called     = true;
    capture.revoked    = !capture.service->Status().enabled && capture.service->Status().owner == 0;
    capture.readmitted = capture.host->AdmitActionMovementConsumer(*capture.service, capture.manifest, capture.session);
    return true;
}

bool CheckLifecycle(const std::filesystem::path& root)
{
    bool                  passed   = true;
    const auto            manifest = WriteAddon(root / L"consumer", "nolock", "function unload() bahamut.chat_print('closing') end\n");
    const auto            other    = WriteAddon(root / L"other", "other", "function update() error('ordinary fault') end\n");
    ActionMovementService service;
    AddonHost             host;
    host.LoadManifests({ manifest, other }, root / L"settings", root / L"logs");
    passed &= Check(!host.AdmitActionMovementConsumer(service, manifest, 1), "Unavailable service admitted a consumer");
    service.SetBoundaryState(ActionMovementBoundaryState::Ready);
    const auto session = service.BeginSession();
    passed &= Check(!host.AdmitActionMovementConsumer(service, other, session), "Other addon acquired NoLock");
    passed &= Check(!host.AdmitActionMovementConsumer(service, root / L"missing/addon.toml", session), "Wrong manifest acquired NoLock");
    passed &= Check(!host.AdmitActionMovementConsumer(service, manifest, session + 1), "Stale session was admitted");
    passed &= Check(Arm(host, service, manifest, session), "Healthy consumer was not admitted");
    const auto oldOwner = service.Status().owner;
    passed &= Check(host.AdmitActionMovementConsumer(service, manifest, session) && service.Status().owner == oldOwner && service.Status().enabled,
                    "Repeated admission replaced current policy");
    host.Update(0.01);
    passed &= Check(host.FaultedCount() == 1 && service.Status().enabled, "Unrelated fault revoked NoLock");
    UnloadCapture capture{ &service, &host, manifest, session };
    host.SetChatSink({ &capture, ObserveUnload });
    passed &= Check(host.Reload("nolock"), "Reload failed");
    passed &= Check(capture.called && capture.revoked && !capture.readmitted, "Reload unload ran before revocation or rearmed its closing instance");
    passed &= Check(service.Status().owner == 0 && !service.Configure(oldOwner, session, 0, true), "Reload retained a stale owner");
    passed &= Check(Arm(host, service, manifest, session) && service.Status().owner != oldOwner, "Replacement admission reused its owner");
    capture.called = false;
    WriteAddon(root / L"consumer", "nolock", "function unload() bahamut.chat_print('closing'); error('unload fault') end\n");
    passed &= Check(host.Reload("nolock"), "Unload-fault fixture reload failed");
    passed &= Check(!service.Status().enabled, "Reload rearmed policy without explicit admission");
    passed &= Check(Arm(host, service, manifest, session), "Unload-fault fixture admission failed");
    capture.called = false;
    passed &= Check(host.Disable("nolock"), "Disable failed");
    passed &= Check(capture.called && capture.revoked && !capture.readmitted && service.Status().owner == 0,
                    "Faulting unload retained or reacquired policy");
    WriteAddon(root / L"consumer", "nolock", "");
    passed &= Check(host.Enable(manifest) && Arm(host, service, manifest, session), "Re-enable did not admit fresh ownership");
    const auto lostOwner = service.Status().owner;
    passed &= Check(service.EndSession(session), "Session loss failed");
    passed &= Check(!host.AdmitActionMovementConsumer(service, manifest, session) && !service.Configure(lostOwner, session, 0, true),
                    "Lost session was readmitted");
    const auto freshSession = service.BeginSession();
    passed &= Check(Arm(host, service, manifest, freshSession), "Fresh session admission failed");
    const auto shutdownOwner = service.Status().owner;
    host.ShutdownActionMovement();
    passed &= Check(service.Status().owner == 0 && !service.Status().enabled && !service.Configure(shutdownOwner, freshSession, 0, true),
                    "Host shutdown retained policy");
    passed &= Check(!host.AdmitActionMovementConsumer(service, manifest, freshSession), "Stopped host readmitted a consumer");
    return passed;
}

bool CheckFaults(const std::filesystem::path& root)
{
    bool passed = true;
    for (const auto callback : { "update", "draw", "command", "chat" })
    {
        const auto            directory = root / callback;
        const auto            manifest  = WriteAddon(directory, "nolock", std::string("function ") + callback + "() error('consumer fault') end\nfunction unload() error('must be skipped') end\n");
        ActionMovementService service;
        service.SetBoundaryState(ActionMovementBoundaryState::Ready);
        const auto session = service.BeginSession();
        AddonHost  host;
        host.LoadManifests({ manifest }, directory / L"settings", directory / L"logs");
        passed &= Check(Arm(host, service, manifest, session), "Fault fixture admission failed");
        const auto owner = service.Status().owner;
        if (std::string_view(callback) == "draw")
        {
            host.Draw({});
        }
        else if (std::string_view(callback) == "command")
        {
            passed &= Check(!host.DispatchCommand("fixture"), "Faulted command was handled");
        }
        else
        {
            if (std::string_view(callback) == "chat")
            {
                host.QueueChat("fixture", "message");
            }
            host.Update(0.01);
        }
        passed &= Check(host.FaultedCount() == 1 && service.Status().owner == 0 && !service.Status().enabled,
                        std::string(callback) + " fault did not revoke native policy");
        passed &= Check(!host.AdmitActionMovementConsumer(service, manifest, session) && !service.Configure(owner, session, 0, true),
                        "Faulted consumer or old owner was readmitted");
        passed &= Check(host.Disable("nolock"), "Faulted instance could not close");
        host.ShutdownActionMovement();
    }
    for (const auto script : { "function load() error('load fault') end\n", "error('top-level fault')\n", "function broken(\n" })
    {
        const auto            directory = root / (std::string_view(script).starts_with("function load") ? L"load" : std::string_view(script).starts_with("error") ? L"top-level"
                                                                                                                                                                  : L"syntax");
        const auto            manifest  = WriteAddon(directory, "nolock", "");
        ActionMovementService service;
        service.SetBoundaryState(ActionMovementBoundaryState::Ready);
        const auto session = service.BeginSession();
        AddonHost  host;
        host.LoadManifests({ manifest }, directory / L"settings", directory / L"logs");
        passed &= Check(Arm(host, service, manifest, session), "Reload-fault fixture admission failed");
        const auto oldOwner = service.Status().owner;
        WriteAddon(directory, "nolock", script);
        passed &= Check(host.Reload("nolock") && service.Status().owner == 0 && !service.Status().enabled &&
                            !service.Configure(oldOwner, session, 0, true),
                        "Failed reload retained old policy");
        passed &= Check(host.FaultedCount() == 1 && !host.AdmitActionMovementConsumer(service, manifest, session) && service.Status().owner == 0,
                        "Failed load was admitted");
        host.Disable("nolock");
        host.ShutdownActionMovement();
    }
    return passed;
}

bool CheckReplacement(const std::filesystem::path& root)
{
    const auto            manifest = WriteAddon(root, "nolock", "");
    ActionMovementService service;
    service.SetBoundaryState(ActionMovementBoundaryState::Ready);
    const auto session = service.BeginSession();
    AddonHost  first;
    AddonHost  second;
    first.LoadManifests({ manifest }, root / L"settings-one", root / L"logs-one");
    second.LoadManifests({ manifest }, root / L"settings-two", root / L"logs-two");
    bool       passed     = Check(Arm(first, service, manifest, session), "First host admission failed");
    const auto firstOwner = service.Status().owner;
    passed &= Check(Arm(second, service, manifest, session) && service.Status().owner != firstOwner, "Replacement host reused ownership");
    const auto secondOwner = service.Status().owner;
    first.ShutdownActionMovement();
    passed &= Check(service.Status().owner == secondOwner && service.Status().enabled, "Stale host shutdown revoked replacement");
    second.ShutdownActionMovement();
    passed &= Check(service.Status().owner == 0 && !service.Status().enabled, "Replacement host shutdown retained policy");
    return passed;
}

} // namespace

int wmain(int argc, wchar_t** argv)
{
    if (argc != 2 || !std::filesystem::path(argv[1]).is_absolute())
    {
        return 2;
    }
    const std::filesystem::path root(argv[1]);
    const bool                  lifecycle   = CheckLifecycle(root / L"lifecycle");
    const bool                  faults      = CheckFaults(root / L"faults");
    const bool                  replacement = CheckReplacement(root / L"replacement");
    return lifecycle && faults && replacement ? 0 : 1;
}
