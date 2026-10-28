#include "addon_host.h"
#include "equipment_memory_fixture.h"
#include "equipment_state.h"
#include "gearsets_debug.h"
#include "gearsets_debug_service.h"
#include "item_names.h"
#include "player_state.h"

#include <algorithm>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <optional>
#include <sstream>
#include <string>
#include <string_view>
#include <vector>

bool RunGearsetsTransactionTests();

namespace
{

namespace fs = std::filesystem;
using namespace bahamut_client;

bool Expect(bool condition, const char* message)
{
    if (!condition)
    {
        std::cerr << message << '\n';
        return false;
    }
    return true;
}

std::string ReadFile(const fs::path& path)
{
    std::ifstream      file(path, std::ios::binary);
    std::ostringstream contents;
    contents << file.rdbuf();
    return contents.str();
}

void WriteFile(const fs::path& path, const std::string& body)
{
    fs::create_directories(path.parent_path());
    std::ofstream file(path, std::ios::binary | std::ios::trunc);
    file << body;
}

std::string Result(const fs::path& settings)
{
    std::istringstream lines(ReadFile(settings / "gearsets" / "settings.ini"));
    std::string        line;
    while (std::getline(lines, line))
    {
        if (!line.empty() && line.back() == '\r')
        {
            line.pop_back();
        }
        if (line.starts_with("result="))
        {
            return line.substr(7);
        }
    }
    return "<no result>";
}

// The real addon script followed by a test driver, loaded under the id
// `gearsets` so the host binds the gearsets services.
fs::path WriteAddon(const fs::path& directory, const fs::path& addonScript, const fs::path& driver)
{
    fs::create_directories(directory);
    WriteFile(directory / "addon.toml", "id = \"gearsets\"\nentry = \"gearsets-test.lua\"\n");
    WriteFile(directory / "gearsets-test.lua", ReadFile(addonScript) + "\n" + ReadFile(driver));
    return directory / "addon.toml";
}

EquipmentSnapshot Equipment(std::uint64_t slot21Uid, bool unreadableFeet)
{
    EquipmentSnapshot snapshot;
    snapshot.complete = !unreadableFeet;
    snapshot.classId  = 3;
    for (std::size_t index = 0; index < kEquipmentSlotCount; ++index)
    {
        EquipmentSlot& slot = snapshot.slots[index];
        slot.slot           = kEquipmentSlots[index];
        slot.state          = EquipmentSlotState::Empty;
        const auto item     = [&](std::uint64_t uid, std::uint32_t catalog)
        {
            slot.state     = EquipmentSlotState::Item;
            slot.uid       = uid;
            slot.catalogId = catalog;
        };
        switch (slot.slot)
        {
            case 0:
                item(0x07AF7850u, 4030013u);
                break;
            case 10:
                item(0xFEDCBA9876543210ull, 8030423u);
                break;
            case 21:
                item(slot21Uid, 9050052u);
                break;
            case 22:
                item(0x06D4CCD2u, 9050052u);
                break;
            case 14:
                if (unreadableFeet)
                {
                    slot.state = EquipmentSlotState::Unknown;
                }
                break;
            default:
                break;
        }
    }
    return snapshot;
}

// The worn items again as bag entries, plus one WEATHERED_CANE (5030101) and two
// WEATHERED_HORA (4020001) that are not worn.
EquipmentInventorySnapshot Inventory()
{
    EquipmentInventorySnapshot inventory;
    inventory.equipment = Equipment(0x06D4CCD1u, false);
    for (const auto& slot : inventory.equipment.slots)
    {
        inventory.bag.push_back({ slot.state, slot.uid, slot.catalogId });
    }
    inventory.bag.push_back({ EquipmentSlotState::Item, 0x1111u, 5030101u });
    inventory.bag.push_back({ EquipmentSlotState::Item, 0x2001u, 4020001u });
    inventory.bag.push_back({ EquipmentSlotState::Item, 0x2002u, 4020001u });
    return inventory;
}

std::vector<std::string> RecordFiles(const fs::path& directory)
{
    std::vector<std::string> names;
    if (fs::exists(directory))
    {
        for (const auto& entry : fs::directory_iterator(directory))
        {
            names.push_back(entry.path().filename().string());
        }
    }
    std::sort(names.begin(), names.end());
    return names;
}

bool TestItemNames()
{
    bool ok = Expect(ItemIdByName("WEATHERED_HORA") == 4020001u && ItemIdByName("WEATHERED_CANE") == 5030101u, "a known item name gives its item type");
    ok      = Expect(ItemNameById(4020001u) == "WEATHERED_HORA" && ItemNameById(5030101u) == "WEATHERED_CANE", "a known item type gives its name") && ok;
    for (const std::uint32_t id : { 3910001u, 4020001u, 4030013u, 5030101u, 8030423u, 9050052u })
    {
        const auto name = ItemNameById(id);
        ok              = Expect(!name.empty() && ItemIdByName(name) == id, "an item name should round trip to its item type") && ok;
    }
    ok = Expect(!ItemIdByName("NO_SUCH_ITEM") && !ItemIdByName("") && !ItemIdByName("WEATHERED_CANE "), "an unknown name gives no item type") && ok;
    ok = Expect(ItemNameById(0).empty() && ItemNameById(1).empty() && ItemNameById(4020000u).empty() && ItemNameById(0xFFFFFFFFu).empty(), "an unknown item type gives no name") && ok;
    return ok;
}

bool TestRecordModel(const fs::path& addonScript, const fs::path& fixtures, const fs::path& scratch)
{
    AddonHost host;
    host.LoadManifests({ WriteAddon(scratch / "model" / "addon", addonScript, fixtures / "model_driver.lua") }, scratch / "model" / "state", scratch / "model" / "chat");
    const std::string result = Result(scratch / "model" / "state");
    if (!Expect(host.LoadedCount() == 1 && host.FaultedCount() == 0, "the model driver should load without a Lua error"))
    {
        return false;
    }
    if (result != "ok")
    {
        std::cerr << "model driver: " << result << '\n';
        return false;
    }
    return true;
}

// Item lookup through the real bindings: names, and the single copy of an
// item type across the worn items and the accessible bag.
bool TestItemLookup(const fs::path& addonScript, const fs::path& scratch)
{
    const fs::path root   = scratch / "lookup";
    const fs::path driver = root / "driver.lua";
    WriteFile(driver, R"lua(
local real_load = load
function load()
    real_load()
    local failures = {}
    local function check(condition, message)
        if not condition then failures[#failures + 1] = message end
    end

    check(bahamut.item_id('WEATHERED_CANE') == 5030101 and bahamut.item_id('WEATHERED_HORA') == 4020001, 'item_id should name item types')
    check(bahamut.item_id('NO_SUCH_ITEM') == nil and bahamut.item_id(5030101) == nil and bahamut.item_id() == nil, 'item_id should refuse unknown names')
    check(bahamut.item_name(5030101) == 'WEATHERED_CANE' and bahamut.item_name(8030423) == 'COTTON_DALMATICA', 'item_name should name item types')
    check(bahamut.item_name(1) == nil and bahamut.item_name(0) == nil and bahamut.item_name(-5) == nil and bahamut.item_name('WEATHERED_CANE') == nil and bahamut.item_name() == nil, 'item_name should refuse unknown types')

    local function copy(catalog)
        local state, uid = bahamut.find_item_copy(catalog)
        return state .. ':' .. tostring(uid)
    end

    check(copy(5030101) == 'available:0000000000001111', 'a single carried copy should be found, got ' .. copy(5030101))
    check(copy(4030013) == 'available:0000000007af7850', 'a worn item and its bag entry count once, got ' .. copy(4030013))
    check(copy(4020001) == 'ambiguous:nil', 'two carried copies are ambiguous, got ' .. copy(4020001))
    check(copy(9050052) == 'ambiguous:nil', 'two worn copies are ambiguous, got ' .. copy(9050052))
    check(copy(4020002) == 'missing:nil', 'an item that is not carried is missing, got ' .. copy(4020002))
    check(copy(0) == 'unavailable:nil' and copy('x') == 'unavailable:nil' and copy(nil) == 'unavailable:nil', 'a bad item type is unavailable')

    local snapshot = bahamut.equipment_snapshot()
    local set =
    {
        name = 'Mix',
        class = '',
        slots =
        {
            [0]  = { state = 'item', catalog = 5030101 },
            [1]  = { state = 'empty' },
            [10] = { state = 'item', catalog = 8030423 },
            [21] = { state = 'item', catalog = 9050052 },
            [22] = { state = 'item', catalog = 9050052 },
        },
    }
    local intent, reason = gearsets.intent(set, snapshot, bahamut.find_item_copy)
    check(intent ~= nil, 'the set should resolve: ' .. tostring(reason))
    if intent ~= nil then
        check(intent.slots[0].uid == '0000000000001111', 'an unworn item resolves to the one carried copy')
        check(intent.slots[10].uid == 'fedcba9876543210', 'a worn item keeps its copy')
        check(intent.slots[21].uid == '0000000006d4ccd1' and intent.slots[22].uid == '0000000006d4ccd2', 'each ring slot keeps its own worn copy although the type is ambiguous')
        check(intent.slots[1].state == 'empty' and intent.slots[8].state == 'empty', 'empty and omitted empty slots stay empty')
    end

    local function refusal(slot, catalog)
        local refused, why = gearsets.intent({ name = 'R', class = '', slots = { [slot] = { state = 'item', catalog = catalog } } }, snapshot, bahamut.find_item_copy)
        return refused == nil and why or 'resolved'
    end

    check(refusal(1, 4020001) == 'off WEATHERED_HORA: ambiguous', 'an ambiguous item is refused, got ' .. refusal(1, 4020001))
    check(refusal(1, 4020002) == 'off DATED_BONE_HORA: missing', 'a missing item is refused, got ' .. refusal(1, 4020002))
    check(refusal(8, 9050052) == 'head BONE_RING: ambiguous', 'a worn copy elsewhere does not settle an ambiguous type, got ' .. refusal(8, 9050052))
    local keep = gearsets.intent({ name = 'Keep', class = '', slots = { [0] = { state = 'item', catalog = 4030013 } } }, snapshot, bahamut.find_item_copy)
    check(keep ~= nil and keep.slots[0].uid == '0000000007af7850' and keep.slots[13].state == 'empty', 'a set naming only the worn weapon keeps it')

    bahamut.settings_set('result', #failures == 0 and 'ok' or 'fail: ' .. string.sub(table.concat(failures, ' | '), 1, 900))
end
)lua");
    EquipmentStateService equipment;
    equipment.PublishInventory(Inventory());
    AddonHost host;
    host.SetEquipmentStateService(&equipment);
    host.LoadManifests({ WriteAddon(root / "addon", addonScript, driver) }, root / "state", root / "chat");
    const std::string result = Result(root / "state");
    if (result != "ok")
    {
        std::cerr << "lookup driver: " << result << '\n';
    }
    return Expect(host.LoadedCount() == 1 && host.FaultedCount() == 0 && result == "ok", "item lookup through the host bindings");
}

bool TestHostBindingsAndPersistence(const fs::path& addonScript, const fs::path& fixtures, const fs::path& scratch)
{
    const fs::path root     = scratch / "host";
    const fs::path settings = root / "state";
    const fs::path sets     = settings / "gearsets" / "sets";
    const fs::path manifest = WriteAddon(root / "addon", addonScript, fixtures / "host_driver.lua");

    std::optional<std::string> profile = "profile-a";
    PlayerStateService         playerState;
    PlayerStateSnapshot        player;
    player.actorId     = 0x029B2941u;
    player.zoneId      = 230u;
    player.baseClassId = 3u;
    playerState.Publish(player);
    EquipmentStateService equipment;
    equipment.Publish(Equipment(0x06D4CCD1u, false));

    const std::vector<std::string> oneFile = { "sets.lua" };
    std::string                    finalSets;
    {
        AddonHost host(&playerState);
        host.SetEquipmentStateService(&equipment);
        host.SetProfileKeyProvider([&profile]()
                                   {
                                       return profile;
                                   });
        host.LoadManifests({ manifest }, settings, root / "chat");
        if (!Expect(host.LoadedCount() == 1 && host.FaultedCount() == 0, "the host driver should load"))
        {
            return false;
        }

        host.Update(0.01); // 1: save two sets
        const std::string setsInA = ReadFile(sets / "profile-a" / "sets.lua");
        if (!Expect(RecordFiles(sets / "profile-a") == oneFile, "one sets file should be on disk for profile-a") || !Expect(setsInA.find("-- Gear Sets for one character") == 0 && setsInA.find("\nreturn {\n") != std::string::npos && setsInA.find("        name = 'Gladiator - Tank',\n        class = 'GLD',\n") != std::string::npos && setsInA.find("        main = { 'BLUNT_GOBLIN_GLADIUS', copy = '0000000007af7850' },\n") != std::string::npos && setsInA.find("        body = { 'COTTON_DALMATICA', copy = 'fedcba9876543210' },\n") != std::string::npos && setsInA.find("        off = 'empty',\n") != std::string::npos && setsInA.find("        name = 'Set2',\n") != std::string::npos, "the sets file should hold the saved equipment as Lua data"))
        {
            return false;
        }
        host.Update(0.01); // 2: compare

        profile = "profile-b";
        host.Update(0.01); // 3: another profile
        if (!Expect(RecordFiles(sets / "profile-b") == oneFile, "profile-b should have its own sets file") || !Expect(ReadFile(sets / "profile-a" / "sets.lua") == setsInA, "profile-b must not touch profile-a"))
        {
            return false;
        }

        profile.reset();
        equipment.Clear();
        equipment.Publish(Equipment(0x06D4CCD1u, false));
        host.Update(0.01); // 4: session only
        if (!Expect(RecordFiles(sets / "profile-a") == oneFile && RecordFiles(sets / "profile-b") == oneFile && RecordFiles(sets).size() == 2 && ReadFile(sets / "profile-a" / "sets.lua") == setsInA, "session-only sets must write nothing"))
        {
            return false;
        }

        profile = "profile-a";
        host.Update(0.01); // 5: profile-a again
        equipment.Publish(Equipment(0x06D4CCD9u, false));
        host.Update(0.01); // 6: another copy of an item type
        equipment.Publish(Equipment(0x06D4CCD1u, true));
        host.Update(0.01); // 7: unreadable slot
        equipment.Publish(Equipment(0x06D4CCD1u, false));
        host.Update(0.01); // 8: save a third set
        profile.reset();
        host.Update(0.01); // 9: key missing within the session
        profile = "profile-a";
        host.Update(0.01); // 10: the same key returns

        if (!Expect(host.FaultedCount() == 0, "the host driver should not fault"))
        {
            return false;
        }
        if (const std::string result = Result(settings); result != "ok")
        {
            std::cerr << "host driver: " << result << '\n';
            return false;
        }
        finalSets = ReadFile(sets / "profile-a" / "sets.lua");
        if (!Expect(RecordFiles(sets / "profile-a") == oneFile && finalSets.find("        name = 'After',\n") != std::string::npos && finalSets.find("name = 'Gap'") == std::string::npos && finalSets.find("name = 'Unreadable'") == std::string::npos, "the file should hold the three saved sets and no refused one"))
        {
            return false;
        }
    }

    // A new host over the same state root, as after a restart.
    WriteFile(settings / "gearsets" / "settings.ini", "phase=verify\n");
    AddonHost restarted(&playerState);
    restarted.SetEquipmentStateService(&equipment);
    restarted.SetProfileKeyProvider([&profile]()
                                    {
                                        return profile;
                                    });
    restarted.LoadManifests({ manifest }, settings, root / "chat");
    if (!Expect(restarted.LoadedCount() == 1 && restarted.FaultedCount() == 0, "the restarted driver should load"))
    {
        return false;
    }
    if (const std::string result = Result(settings); result != "ok")
    {
        std::cerr << "restarted driver: " << result << '\n';
        return false;
    }
    return Expect(ReadFile(sets / "profile-a" / "sets.lua") == finalSets, "a restart must not rewrite the sets file");
}

bool TestWithoutServicesIsSessionOnly(const fs::path& addonScript, const fs::path& scratch)
{
    // No provider and no equipment service: the addon loads, reports
    // session-only mode, and the bindings refuse instead of inventing data.
    const fs::path driver = scratch / "bare" / "driver.lua";
    WriteFile(driver, R"lua(
local real_load = load
function load()
    real_load()
    local ok = gearsets.status().persistent == false
        and bahamut.profile_key() == nil
        and bahamut.equipment_snapshot() == nil
        and bahamut.record_save('sets', 'x') == 'unavailable'
        and select(2, bahamut.gearsets_equip({})) == 'inactive'
        and bahamut.find_item_copy(5030101) == 'unavailable'
        and bahamut.item_name(5030101) == 'WEATHERED_CANE'
    bahamut.settings_set('result', ok and 'ok' or 'fail')
end
)lua");
    AddonHost host;
    host.LoadManifests({ WriteAddon(scratch / "bare" / "addon", addonScript, driver) }, scratch / "bare" / "state", scratch / "bare" / "chat");
    return Expect(host.LoadedCount() == 1 && host.FaultedCount() == 0 && Result(scratch / "bare" / "state") == "ok", "an addon without services should run session-only") && Expect(!fs::exists(scratch / "bare" / "state" / "gearsets" / "sets"), "session-only mode must create no sets files");
}

class FakeDebugPort final : public GearsetsDebugPort
{
public:
    bool                               equip     = false;
    int                                submits   = 0;
    int                                frames    = 0;
    std::uint16_t                      bagSlot   = 0;
    std::uint8_t                       equipSlot = 0;
    std::optional<std::uint64_t>       uid;
    std::optional<GearsetsEquipStatus> status;
    std::vector<bool>                  transitions;

    bool EquipEnabled() const override
    {
        return equip;
    }

    void OnAddonStateChanged(bool active) override
    {
        transitions.push_back(active);
    }

    void OnRenderFrame() override
    {
        ++frames;
    }

    GearsetsSubmitResult SubmitDebugEquip(std::uint16_t bag, std::uint8_t slot, std::optional<std::uint64_t> requested) override
    {
        ++submits;
        bagSlot   = bag;
        equipSlot = slot;
        uid       = requested;
        return { true, GearsetsEquipReason::None, 5u };
    }

    std::optional<GearsetsEquipStatus> DebugEquipStatus() const override
    {
        return status;
    }
};

bool CaptureChat(void* context, std::string_view text)
{
    static_cast<std::vector<std::string>*>(context)->emplace_back(text);
    return true;
}

bool TestDebugEquipGates(const fs::path& addonScript, const fs::path& scratch)
{
    // The driver records which debug bindings exist and how invalid
    // arguments are refused; valid requests come through the command.
    // '/gearsets crash' faults the addon from inside a command.
    const fs::path driver = scratch / "debug" / "driver.lua";
    WriteFile(driver, R"lua(
local real_load = load
function load()
    real_load()
    local present = bahamut.debug_equip ~= nil and bahamut.debug_equip_status ~= nil
    local absent  = bahamut.debug_equip == nil and bahamut.debug_equip_status == nil
    local result  = absent and 'absent' or 'partial'
    if present then
        local ok_a, reason_a = bahamut.debug_equip(1.5, 11)
        local ok_b, reason_b = bahamut.debug_equip(1, 11, 'xyz')
        local ok_c, reason_c = bahamut.debug_equip(1, 11, '00000000000000001')
        result = (not ok_a and reason_a == 'invalid-slot' and not ok_b and reason_b == 'invalid-uid' and not ok_c and reason_c == 'invalid-uid' and bahamut.debug_equip_status() == nil) and 'present' or 'bad-arguments'
    end
    bahamut.settings_set('result', result)
end
local real_command = command
function command(name)
    if name == '/gearsets crash' then error('crash') end
    return real_command(name)
end
)lua");
    const fs::path manifest = WriteAddon(scratch / "debug" / "addon", addonScript, driver);
    bool           ok       = true;
    {
        // The default product path: no port is installed.
        std::vector<std::string> chat;
        AddonHost                host;
        host.SetChatSink({ &chat, &CaptureChat });
        host.LoadManifests({ manifest }, scratch / "debug" / "none", scratch / "debug" / "chat");
        ok = Expect(Result(scratch / "debug" / "none") == "absent", "without a port the debug bindings must not exist") && ok;
        ok = Expect(!host.DispatchCommand("/gearsets debug-equip 1 11") && !host.DispatchCommand("/gearsets debug-status") && chat.empty() && host.FaultedCount() == 0, "without a port the debug commands pass through to the game") && ok;
    }
    {
        FakeDebugPort            port;
        std::vector<std::string> chat;
        AddonHost                host;
        host.SetGearsetsDebugPort(&port);
        host.SetChatSink({ &chat, &CaptureChat });
        host.LoadManifests({ manifest }, scratch / "debug" / "off", scratch / "debug" / "chat");
        // Another command addon may keep the command hook installed, so the
        // addon itself must decline the commands.
        const bool equipHandled  = host.DispatchCommand("/gearsets debug-equip 1 11");
        const bool statusHandled = host.DispatchCommand("/gearsets debug-status");
        host.Update(0.01);
        ok = Expect(Result(scratch / "debug" / "off") == "absent", "without the equip switch the debug bindings must not exist") && ok;
        ok = Expect(!equipHandled && !statusHandled && port.submits == 0 && chat.empty() && host.FaultedCount() == 0, "without the equip switch the debug commands pass through to the game") && ok;
        ok = Expect(port.frames == 1 && port.transitions == std::vector<bool>({ true }), "the host reports the load and each update to the port") && ok;
    }
    {
        FakeDebugPort port;
        port.equip = true;
        AddonHost host;
        host.SetGearsetsDebugPort(&port);
        host.Update(0.01);
        ok = Expect(port.frames == 1 && port.transitions.empty(), "without the addon the port hears no transition") && ok;
    }
    {
        FakeDebugPort port;
        port.equip = true;
        std::vector<std::string> chat;
        AddonHost                host;
        host.SetGearsetsDebugPort(&port);
        host.SetChatSink({ &chat, &CaptureChat });
        host.LoadManifests({ manifest }, scratch / "debug" / "on", scratch / "debug" / "chat");
        ok = Expect(Result(scratch / "debug" / "on") == "present", "with the equip switch the bindings exist and validate arguments") && ok;
        ok = Expect(host.DispatchCommand("/gearsets debug-equip 1 11 0000001200000002") && port.submits == 1 && port.bagSlot == 1 && port.equipSlot == 11 && port.uid == 0x1200000002ull, "the command submits the bag slot, equip slot, and uid") && ok;
        ok = Expect(host.DispatchCommand("/gearsets debug-equip 7 1") && port.submits == 2 && port.bagSlot == 7 && port.equipSlot == 1 && !port.uid, "the uid is optional") && ok;

        GearsetsEquipStatus status;
        status.id          = 5;
        status.phase       = GearsetsEquipPhase::Refused;
        status.reason      = GearsetsEquipReason::InvokeUnavailable;
        status.bagSlot     = 1;
        status.equipSlot   = 11;
        status.expectedUid = 0x1200000002ull;
        port.status        = status;
        host.Update(0.01);
        const std::string expected = "gearsets: debug equip 5 refused (invoke-unavailable) bag=1 slot=11 invocations=0 uid=0000001200000002";
        ok                         = Expect(chat.size() == 2 && chat[0] == "gearsets: debug equip 5 queued" && chat[1] == chat[0], "update prints nothing; each command prints its own result") && ok;
        ok                         = Expect(host.DispatchCommand("/gearsets debug-status") && chat.size() == 3 && chat[2] == expected, "debug-status prints the last request") && ok;
        ok                         = Expect(host.FaultedCount() == 0, "the debug commands must not fault the addon") && ok;

        // Every lifecycle change reaches the port directly, not by polling:
        // a reload with no update between its halves is still two transitions.
        ok = Expect(host.Reload("gearsets") && port.transitions == std::vector<bool>({ true, false, true }), "a reload reports the unload and the load") && ok;
        static_cast<void>(host.DispatchCommand("/gearsets crash"));
        ok = Expect(host.FaultedCount() == 1 && port.transitions == std::vector<bool>({ true, false, true, false }), "a fault inside a command is reported at once") && ok;
        ok = Expect(host.Reload("gearsets") && host.Disable("gearsets") && port.transitions == std::vector<bool>({ true, false, true, false, false, true, false }), "reloading a faulted addon and unloading it are reported") && ok;
    }
    return ok;
}

struct Controls
{
    AddonGearsetsWindow    last;
    AddonGearsetsAction    next;
    EquipmentStateService* endSession = nullptr;
    int                    calls      = 0;

    static AddonGearsetsAction Draw(void* context, const AddonGearsetsWindow& window)
    {
        auto& self  = *static_cast<Controls*>(context);
        self.last   = window;
        auto action = self.next;
        self.next   = {};
        ++self.calls;
        if (self.endSession != nullptr)
        {
            self.endSession->Clear();
            self.endSession = nullptr;
        }
        return action;
    }
};

// Draws the addon's window. An action is returned by one draw and applied
// in it; the next draw shows the result.
struct Window
{
    Controls        controls;
    AddonWindowSink sink;
    AddonHost&      host;

    explicit Window(AddonHost& target)
    : host(target)
    {
        sink.context  = &controls;
        sink.gearsets = &Controls::Draw;
    }

    Window(const Window&)            = delete;
    Window& operator=(const Window&) = delete;

    void Draw()
    {
        host.Draw(sink);
    }

    const AddonGearsetsWindow& Act(AddonGearsetsAction action)
    {
        controls.next = std::move(action);
        Draw();
        Draw();
        return controls.last;
    }
};

std::vector<std::string> Names(const AddonGearsetsWindow& window)
{
    std::vector<std::string> names;
    for (const auto& row : window.rows)
    {
        names.push_back(row.name);
    }
    return names;
}

bool TestWindow(const fs::path& addonScript, const fs::path& scratch)
{
    const auto root   = scratch / "window";
    const auto file   = root / "state" / "gearsets" / "sets" / "window-profile" / "sets.lua";
    const auto driver = root / "driver.lua";
    WriteFile(driver, "-- no driver\n");
    const std::string handWritten = "-- hand written\nreturn {\n    { name = 'Hand Edited', class = 'cnj', main = 'WEATHERED_CANE', body = 8030423 },\n}\n";
    WriteFile(file, handWritten);

    EquipmentStateService equipment;
    equipment.PublishInventory(Inventory());
    AddonHost                host;
    std::vector<std::string> chat;
    host.SetChatSink({ &chat, &CaptureChat });
    host.SetEquipmentStateService(&equipment);
    host.SetProfileKeyProvider([]
                               {
                                   return std::optional<std::string>("window-profile");
                               });
    host.LoadManifests({ WriteAddon(root / "addon", addonScript, driver) }, root / "state", root / "chat");
    Window window(host);
    auto&  view = window.controls.last;

    window.Draw();
    bool ok = Expect(window.controls.calls == 0, "the window stays hidden until /gs");
    ok      = Expect(host.DispatchCommand("/gs"), "/gs toggles the window") && ok;
    window.Draw();
    ok           = Expect(window.controls.calls == 1 && Names(view) == std::vector<std::string>({ "Hand Edited" }) && view.rows[0].abbreviation == "CNJ" && !view.rows[0].current, "the window lists the hand written set with its class abbreviation") && ok;
    ok           = Expect(view.editable && view.status.empty() && view.selected == 0 && view.maximum == 100 && !view.equipping, "a readable file gives an editable window") && ok;
    ok           = Expect(view.iconDirectory.filename() == "icons" && view.iconDirectory.parent_path().filename() == "addon", "icons come from the addon folder") && ok;
    auto context = view.context;

    // Save: exact copies, default name, selection follows the new row.
    window.Act({ "save", 0, "Fixture" });
    ok               = Expect(Names(view) == std::vector<std::string>({ "Hand Edited", "Fixture" }) && view.rows[1].abbreviation == "GLD" && view.rows[1].current && view.selected == 2 && view.context != context, "save adds the worn equipment as the selected row and changes the context") && ok;
    const auto saved = ReadFile(file);
    ok               = Expect(saved.find("-- hand written") == std::string::npos && saved.find("-- Gear Sets for one character") == 0 && saved.find("name = 'Hand Edited'") != std::string::npos && saved.find("main = { 'BLUNT_GOBLIN_GLADIUS', copy = '0000000007af7850' },") != std::string::npos && saved.find("body = { 'COTTON_DALMATICA', copy = 'fedcba9876543210' },") != std::string::npos, "save rewrites the file with exact copies and keeps the other sets") && ok;
    window.Act({ "save", 0, {} });
    ok = Expect(Names(view) == std::vector<std::string>({ "Hand Edited", "Fixture", "Set3" }) && view.selected == 3, "save without a name uses Set and the set number") && ok;

    // Rename, move, select.
    window.Act({ "rename", 3, "Renamed" });
    ok = Expect(Names(view)[2] == "Renamed" && view.status.empty(), "rename changes the name") && ok;
    window.Act({ "rename", 3, std::string(65, 'x') });
    ok = Expect(Names(view)[2] == "Renamed" && view.status == "rename failed: bad-name", "a bad name is reported and changes nothing") && ok;
    window.Act({ "move", 3, "up" });
    ok = Expect(Names(view) == std::vector<std::string>({ "Hand Edited", "Renamed", "Fixture" }) && view.selected == 2 && view.status.empty(), "move up swaps with the row above and follows it") && ok;
    window.Act({ "move", 2, "down" });
    ok = Expect(Names(view) == std::vector<std::string>({ "Hand Edited", "Fixture", "Renamed" }) && view.selected == 3, "move down swaps with the row below") && ok;
    window.Act({ "move", 1, "up" });
    ok                  = Expect(Names(view)[0] == "Hand Edited" && view.status == "move failed: bad-position", "the top row cannot move up") && ok;
    const auto arranged = ReadFile(file);
    window.Act({ "select", 2, {} });
    ok = Expect(view.selected == 2 && ReadFile(file) == arranged, "select neither equips nor rewrites the file") && ok;

    // Equip: the item-type-only cane resolves to the one carried copy, so the
    // request reaches the runtime, which has no executor here.
    window.Act({ "equip", 1, {} });
    ok = Expect(view.equipping, "an accepted equip request shows as in progress") && ok;
    host.Update(0.01);
    window.Draw();
    ok = Expect(view.status.find("executor-unavailable") != std::string::npos && ReadFile(file) == arranged && !view.equipping, ("equip reports an unavailable executor and keeps the file, status: " + view.status).c_str()) && ok;

    // The file changed on disk: refuse the action and reload.
    context                   = view.context;
    const std::string outside = "return {\n    { name = 'Outside A' },\n    { name = 'Outside B', main = 'WEATHERED_CANE' },\n}\n";
    WriteFile(file, outside);
    window.Act({ "rename", 1, "Nope" });
    ok = Expect(ReadFile(file) == outside && Names(view) == std::vector<std::string>({ "Outside A", "Outside B" }) && view.status == "rename failed: sets.lua changed and was reloaded" && view.context != context, "an action after a hand edit is refused and the list reloads") && ok;
    window.Act({ "rename", 1, "Nope" });
    ok = Expect(Names(view)[0] == "Nope" && ReadFile(file).find("name = 'Outside B'") != std::string::npos && view.status.empty(), "the same action works on the reloaded list") && ok;
    window.Act({ "delete", 2, {} });
    ok = Expect(Names(view) == std::vector<std::string>({ "Nope" }) && view.selected == 0, "delete removes the row and clears the selection") && ok;

    // A file that cannot be read is never overwritten.
    const std::string broken = "return {\n    { name = os.time() },\n}\n";
    WriteFile(file, broken);
    ok = Expect(host.DispatchCommand("/gs") && host.DispatchCommand("/gs"), "the window closes and opens again") && ok;
    window.Draw();
    ok = Expect(view.rows.empty() && !view.editable && view.status == "sets.lua line 2: expected a value", "an unparseable file gives a read-only window with the problem") && ok;
    window.Act({ "save", 0, "Refused" });
    ok = Expect(ReadFile(file) == broken && view.rows.empty() && !view.editable, "a window save leaves an unparseable file alone") && ok;
    chat.clear();
    ok                       = Expect(host.DispatchCommand("/gearsets save Refused") && ReadFile(file) == broken && chat == std::vector<std::string>({ "gearsets: sets.lua line 2: expected a value" }), "a save command leaves an unparseable file alone and says why") && ok;
    const std::string sparse = "return {\n    [3] = { name = 'Keep me' },\n}\n";
    WriteFile(file, sparse);
    chat.clear();
    ok = Expect(host.DispatchCommand("/gearsets reload") && chat == std::vector<std::string>({ "gearsets: sets.lua sets must be numbered without gaps" }), "a sparse sets table is refused") && ok;
    window.Act({ "save", 0, "Refused" });
    ok = Expect(ReadFile(file) == sparse && view.rows.empty() && !view.editable && view.status == "sets.lua sets must be numbered without gaps", "a sparse sets file is read-only and stays untouched") && ok;

    // Commands by number and by name.
    WriteFile(file, "return {\n    { name = 'Dup', class = 'GLD', main = { 'BLUNT_GOBLIN_GLADIUS', copy = '0000000007af7850' } },\n    { name = 'dup' },\n    { name = 'Solo' },\n}\n");
    chat.clear();
    ok = Expect(host.DispatchCommand("/gearsets reload") && chat == std::vector<std::string>({ "gearsets: 3 sets" }), "reload reads the hand edit") && ok;
    chat.clear();
    ok = Expect(host.DispatchCommand("/gs list") && chat == std::vector<std::string>({ "gearsets: sets", "1: GLD Dup", "2: dup", "3: Solo" }), "/gs is the short form of /gearsets and lists the sets") && ok;
    chat.clear();
    const auto listed = ReadFile(file);
    ok                = Expect(host.DispatchCommand("/gs delete DUP") && chat == std::vector<std::string>({ "gearsets: ambiguous-name" }) && ReadFile(file) == listed, "a repeated name is refused without changing the file") && ok;
    chat.clear();
    ok = Expect(host.DispatchCommand("/gs rename 3 Lone") && host.DispatchCommand("/gs move 3 1") && chat == std::vector<std::string>({ "gearsets: rename 3: ok", "gearsets: move 3: ok" }), "rename and move work by number") && ok;
    chat.clear();
    ok = Expect(host.DispatchCommand("/gs delete lone") && chat == std::vector<std::string>({ "gearsets: delete 1: ok" }), "delete works by name without case") && ok;
    chat.clear();
    ok                 = Expect(host.DispatchCommand("/gs update 1") && host.DispatchCommand("/gs delete 9") && chat == std::vector<std::string>({ "gearsets: update 1: ok", "gearsets: no set 9" }), "update works by number and an unknown set is reported") && ok;
    const auto updated = ReadFile(file);
    ok                 = Expect(updated.find("name = 'Dup'") != std::string::npos && updated.find("name = 'dup'") != std::string::npos && updated.find("name = 'Lone'") == std::string::npos && updated.find("COTTON_DALMATICA") != std::string::npos, "the file holds the edits") && ok;
    ok                 = Expect(!host.DispatchCommand("/gsx list") && !host.DispatchCommand("/gs bogus"), "other commands pass through") && ok;
    window.Draw();
    ok = Expect(Names(view) == std::vector<std::string>({ "Dup", "dup" }) && view.rows[0].current && view.editable, "the open window shows the edited sets") && ok;

    // An action drawn before the session ended cannot act on its successor.
    window.controls.endSession = &equipment;
    window.Act({ "delete", 1, {} });
    ok = Expect(ReadFile(file) == updated && view.status == "delete failed: session-changed" && Names(view).size() == 2, "a stale window action cannot act on a replacement session") && ok;
    equipment.PublishInventory(Inventory());

    // Without a verified profile the sets live in memory for the session.
    context = view.context;
    host.SetProfileKeyProvider({});
    equipment.Clear();
    equipment.PublishInventory(Inventory());
    window.Draw();
    ok = Expect(view.rows.empty() && view.editable && view.context != context, "losing the profile starts an empty session-only list") && ok;
    window.Act({ "save", 0, "Session" });
    ok      = Expect(Names(view) == std::vector<std::string>({ "Session" }) && ReadFile(file) == updated, "session-only sets work without touching any file") && ok;
    context = view.context;
    equipment.Clear();
    equipment.PublishInventory(Inventory());
    window.Draw();
    ok = Expect(view.rows.empty() && !context.empty() && view.context != context, "a replacement session clears session-only sets and changes the context before drawing") && ok;

    // The close button hides the window.
    window.Act({ "close", 0, {} });
    const int calls = window.controls.calls;
    window.Draw();
    return Expect(window.controls.calls == calls && host.FaultedCount() == 0 && host.LoadedCount() == 1, "the close button hides the window and the addon stays healthy") && ok;
}

bool TestInterruptedLoadRetries(const fs::path& addonScript, const fs::path& scratch)
{
    // The key is missing for exactly the record_load call of the second
    // host's initial load; the next update must load the profile again.
    const fs::path root   = scratch / "interrupted";
    const fs::path driver = root / "driver.lua";
    WriteFile(driver, R"lua(
local real_load   = load
local real_update = update
function load()
    real_load()
    if bahamut.settings_get('phase', 'save') == 'save' then
        local index = gearsets.manager:save_current('Kept', bahamut.equipment_snapshot())
        bahamut.settings_set('result', index ~= nil and 'saved' or 'fail')
    else
        bahamut.settings_set('result', tostring(gearsets.manager.problem))
    end
end
function update(delta_seconds)
    real_update(delta_seconds)
    if bahamut.settings_get('phase', 'save') == 'verify' then
        local manager = gearsets.manager
        local ok      = manager.problem == nil and #manager.sets == 1 and manager.sets[1].name == 'Kept'
        bahamut.settings_set('result', bahamut.settings_get('result', '') .. (ok and ':reloaded' or ':missing'))
    end
end
)lua");
    const fs::path        manifest = WriteAddon(root / "addon", addonScript, driver);
    EquipmentStateService equipment;
    equipment.Publish(Equipment(0x06D4CCD1u, false));
    {
        AddonHost host;
        host.SetEquipmentStateService(&equipment);
        host.SetProfileKeyProvider([]
                                   {
                                       return std::optional<std::string>("profile-r");
                                   });
        host.LoadManifests({ manifest }, root / "state", root / "chat");
        if (!Expect(Result(root / "state") == "saved", "the first host should save a set"))
        {
            return false;
        }
    }
    WriteFile(root / "state" / "gearsets" / "settings.ini", "phase=verify\n");
    int       calls = 0;
    AddonHost host;
    host.SetEquipmentStateService(&equipment);
    host.SetProfileKeyProvider([&calls]
                               {
                                   return ++calls == 2 ? std::nullopt : std::optional<std::string>("profile-r");
                               });
    host.LoadManifests({ manifest }, root / "state", root / "chat");
    host.Update(0.01);
    return Expect(Result(root / "state") == "sets.lua: unavailable:reloaded", "a load interrupted by a missing key should reload when the same key returns");
}

bool TestInventoryReadRecoveryKeepsSessionSets(const fs::path& addonScript, const fs::path& scratch)
{
    using namespace equipment_memory_fixture;
    Fixture fixture;
    fixture.Bag(1, 0xFEDCBA9876543210ull, 8030423u);
    fixture.Link(10, 0, 1);
    const auto player = reinterpret_cast<const void*>(static_cast<std::uintptr_t>(fixture.player));
    const auto root   = scratch / "read-recovery";
    WriteFile(root / "addon.toml", "id = \"gearsets\"\nentry = \"gearsets.lua\"\n");
    WriteFile(root / "gearsets.lua", ReadFile(addonScript));
    EquipmentStateService equipment;
    GearsetsDebugService  service(&equipment);
    service.Configure({ {}, true });
    AddonHost host;
    host.SetEquipmentStateService(&equipment);
    host.SetGearsetsDebugPort(&service);
    host.LoadManifests({ root / "addon.toml" }, root / "state", root / "chat");
    service.OnGameFrame(player, 0);
    const auto session    = equipment.SessionGeneration();
    const auto generation = equipment.Snapshot()->generation;
    Window     window(host);
    auto&      view = window.controls.last;
    host.DispatchCommand("/gs");
    window.Act({ "save", 0, "Keep through read failure" });
    bool ok = Expect(view.rows.size() == 1 && view.rows[0].current, "read recovery starts with a current session-only set");

    Memory::Put32(fixture.player, kPlayerStoreOffset, 0);
    service.OnGameFrame(player, 100);
    window.Draw();
    ok = Expect(!equipment.Snapshot() && !equipment.Inventory(), "a failed inventory read invalidates both copied snapshots") && ok;
    ok = Expect(equipment.SessionGeneration() == session && view.rows.size() == 1 && !view.rows[0].current, "a failed read for the same player preserves session-only sets") && ok;
    window.Act({ "save", 0, "Unavailable" });
    service.OnGameFrame(player, 200);
    window.Draw();
    ok = Expect(view.rows.size() == 1 && view.status == "save failed: incomplete-snapshot", "repeated read failure preserves sets and cannot save stale equipment") && ok;
    Memory::Put32(fixture.player, kPlayerStoreOffset, fixture.store);
    service.OnGameFrame(player, 300);
    window.Draw();
    ok = Expect(equipment.SessionGeneration() == session && equipment.Snapshot() && equipment.Snapshot()->generation > generation, "read recovery publishes fresh equipment in the same session") && ok;
    ok = Expect(Names(view) == std::vector<std::string>({ "Keep through read failure" }) && view.rows[0].current, "read recovery restores the saved set and its current mark") && ok;

    Memory::Put32(fixture.player, kPlayerStoreOffset, 0);
    service.OnGameFrame(player, 400);
    Fixture    replacement;
    const auto replacementPlayer = reinterpret_cast<const void*>(static_cast<std::uintptr_t>(replacement.player));
    service.OnGameFrame(replacementPlayer, 410);
    window.Draw();
    ok = Expect(equipment.SessionGeneration() != session && view.rows.empty(), "player replacement after read failure clears the old session-only sets") && ok;
    window.Act({ "save", 0, "Replacement session" });
    const auto replacementSession = equipment.SessionGeneration();
    Memory::Put32(replacement.player, kPlayerStoreOffset, 0);
    service.OnGameFrame(replacementPlayer, 510);
    window.Draw();
    ok = Expect(view.rows.size() == 1, "the replacement session also retains sets during a read failure") && ok;
    service.OnGameFrame(nullptr, 520);
    window.Draw();
    ok = Expect(equipment.SessionGeneration() != replacementSession && view.rows.empty(), "player loss after read failure clears the session-only sets") && ok;
    return Expect(host.FaultedCount() == 0 && !fs::exists(root / "state" / "gearsets" / "sets"), "read recovery stays healthy and writes no files") && ok;
}

} // namespace

int wmain(int argc, wchar_t** argv)
{
    if (argc != 4)
    {
        std::cerr << "usage: gearsets-tests <gearsets.lua> <fixtures-directory> <scratch-directory>\n";
        return 2;
    }
    const fs::path  addonScript(argv[1]);
    const fs::path  fixtures(argv[2]);
    const fs::path  scratch = fs::path(argv[3]) / "gearsets-addon";
    std::error_code error;
    fs::remove_all(scratch, error);
    bool ok = TestItemNames();
    ok      = TestRecordModel(addonScript, fixtures, scratch) && ok;
    ok      = TestItemLookup(addonScript, scratch) && ok;
    ok      = TestHostBindingsAndPersistence(addonScript, fixtures, scratch) && ok;
    ok      = TestWithoutServicesIsSessionOnly(addonScript, scratch) && ok;
    ok      = TestDebugEquipGates(addonScript, scratch) && ok;
    ok      = TestInterruptedLoadRetries(addonScript, scratch) && ok;
    ok      = TestInventoryReadRecoveryKeepsSessionSets(addonScript, scratch) && ok;
    ok      = TestWindow(addonScript, scratch) && ok;
    ok      = RunGearsetsTransactionTests() && ok;
    return ok ? 0 : 1;
}
