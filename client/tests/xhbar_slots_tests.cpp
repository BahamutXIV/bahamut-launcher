#include "xhbar_slots.h"

#include <array>
#include <iostream>
#include <limits>
#include <map>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

using namespace bahamut_client::xhbar;

namespace
{

void Require(bool condition, const char* reason)
{
    if (!condition)
        throw std::runtime_error(reason);
}

class Space
{
public:
    std::uint32_t Alloc(std::uint32_t bytes)
    {
        const std::uint32_t address = next_;
        next_ += (bytes + 15) & ~15u;
        for (std::uint32_t offset = 0; offset < bytes; ++offset)
            bytes_[address + offset] = 0;
        return address;
    }

    void Map(std::uint32_t address, std::uint32_t bytes)
    {
        for (std::uint32_t offset = 0; offset < bytes; ++offset)
            bytes_.try_emplace(address + offset, std::uint8_t{ 0 });
    }

    void Unmap(std::uint32_t address, std::uint32_t bytes)
    {
        for (std::uint32_t offset = 0; offset < bytes; ++offset)
            bytes_.erase(address + offset);
    }

    void Put8(std::uint32_t address, std::uint8_t value)
    {
        Map(address, 1);
        bytes_[address] = value;
    }

    void Put32(std::uint32_t address, std::uint32_t value)
    {
        for (std::uint32_t index = 0; index != 4; ++index)
            Put8(address + index, static_cast<std::uint8_t>(value >> (8 * index)));
    }

    std::uint32_t Get32(std::uint32_t address) const
    {
        std::uint32_t value = 0;
        for (std::uint32_t index = 0; index != 4; ++index)
            value |= static_cast<std::uint32_t>(bytes_.at(address + index)) << (8 * index);
        return value;
    }

    static bool Copy(void* context, std::uint32_t address, void* output, std::size_t bytes) noexcept
    {
        auto& self = *static_cast<Space*>(context);
        auto* out  = static_cast<std::uint8_t*>(output);
        for (std::size_t index = 0; index != bytes; ++index)
        {
            const auto found = self.bytes_.find(address + static_cast<std::uint32_t>(index));
            if (found == self.bytes_.end())
                return false;
            out[index] = found->second;
        }
        return true;
    }

private:
    std::map<std::uint32_t, std::uint8_t> bytes_;
    std::uint32_t                         next_ = 0x20000000;
};

class Fixture
{
public:
    Space space;

    std::uint32_t Typed(std::uint32_t type, std::uint32_t bytes)
    {
        const std::uint32_t locator = space.Alloc(0x14);
        space.Put32(locator + 0x0c, type);
        const std::uint32_t table = space.Alloc(0x20) + 4;
        space.Put32(table - 4, locator);
        const std::uint32_t object = space.Alloc(bytes);
        space.Put32(object, table);
        return object;
    }

    void FillMap(std::uint32_t map, std::uint32_t nilOffset, std::uint32_t nodeBytes, const std::vector<std::uint32_t>& nodes)
    {
        const std::uint32_t head = space.Alloc(nodeBytes);
        space.Put8(head + nilOffset, 1);
        space.Put32(map + 4, head);
        space.Put32(head + 4, Balanced(head, nodes, 0, nodes.size()));
    }

    std::uint32_t Balanced(std::uint32_t head, const std::vector<std::uint32_t>& nodes, std::size_t begin, std::size_t end)
    {
        if (begin == end)
            return head;
        const std::size_t   middle = begin + (end - begin) / 2;
        const std::uint32_t node   = nodes[middle];
        space.Put32(node, Balanced(head, nodes, begin, middle));
        space.Put32(node + 8, Balanced(head, nodes, middle + 1, end));
        return node;
    }

    // Native string: 0x54 bytes; pointer +0, capacity +4, NUL-inclusive length +8, inline buffer +0x12.
    void NativeString(std::uint32_t address, const std::string& text, bool countNul = true)
    {
        space.Map(address, 0x54);
        const std::uint32_t characters = address + 0x12;
        for (std::size_t index = 0; index != text.size(); ++index)
            space.Put8(characters + static_cast<std::uint32_t>(index), static_cast<std::uint8_t>(text[index]));
        space.Put8(characters + static_cast<std::uint32_t>(text.size()), 0);
        space.Put32(address, characters);
        space.Put32(address + 4, 0x40);
        space.Put32(address + 8, static_cast<std::uint32_t>(text.size() + (countNul ? 1 : 0)));
    }

    std::uint32_t Interned(const std::string& text)
    {
        const std::uint32_t entry = space.Alloc(8 + 0x58);
        NativeString(entry + 8, text);
        space.Put32(entry + 8 + 0x54, 1);
        return entry + 8;
    }

    std::uint32_t NameNode(const std::string& name, std::uint32_t element, bool countNul = true)
    {
        const std::uint32_t node = space.Alloc(0x68);
        NativeString(node + 0x0c, name, countNul);
        space.Put32(node + 0x60, element);
        return node;
    }

    std::uint32_t InternedNode(std::uint32_t nodeBytes, std::uint32_t pool, std::uint32_t string, std::uint32_t value)
    {
        const std::uint32_t node   = space.Alloc(nodeBytes);
        std::uint32_t       length = 0;
        Space::Copy(&space, string + 8, &length, 4);
        space.Put32(node + 0x0c, pool);
        space.Put32(node + 0x10, string);
        space.Put32(node + 0x14, length - 1);
        space.Put32(node + 0x1c, value);
        return node;
    }

    std::uint32_t KeyNode(std::uint32_t key, std::uint32_t value)
    {
        const std::uint32_t node = space.Alloc(0x18);
        space.Put32(node + 0x0c, key);
        space.Put32(node + 0x10, value);
        return node;
    }
};

struct Scenario
{
    Fixture                       fixture;
    std::uint32_t                 pool           = 0;
    std::uint32_t                 declarations   = 0;
    std::uint32_t                 mainModule     = 0;
    std::uint32_t                 tlsArray       = 0;
    std::uint32_t                 element        = 0;
    std::uint32_t                 space          = 0;
    std::uint32_t                 otherSpace     = 0;
    std::uint32_t                 window         = 0;
    std::uint32_t                 record         = 0;
    std::uint32_t                 buffer         = 0;
    std::uint32_t                 context        = 0;
    std::uint32_t                 main           = 0; // Main at [0x013232B8]
    std::uint32_t                 network        = 0;
    std::uint32_t                 engine         = 0; // GameEngine, player control, record
    std::uint32_t                 player         = 0;
    std::uint32_t                 playerRecord   = 0;
    std::uint32_t                 shared         = 0; // player's shared work record
    std::uint32_t                 charaWork      = 0;
    std::uint32_t                 syncContainer  = 0;
    std::uint32_t                 syncData       = 0; // first byte of chara work's sync group
    std::uint32_t                 otherElement   = 0;
    std::uint32_t                 desktop        = 0; // Desktop element, control, record
    std::uint32_t                 desktopControl = 0;
    std::uint32_t                 desktopRecord  = 0;
    std::uint32_t                 desktopBuffer  = 0;
    std::uint32_t                 modeField      = 0; // temp-scope field descriptors
    std::uint32_t                 lockField      = 0;
    std::uint32_t                 rootsField     = 0; // work.rootWidget array descriptor
    std::array<std::uint32_t, 10> buttons{};
    std::array<std::uint32_t, 30> labels{};
};

std::uint32_t PropertyStorage(Fixture& fixture, std::uint32_t control, std::uint32_t descriptor, std::uint32_t value)
{
    const std::uint32_t storage = fixture.space.Alloc(0x34);
    fixture.space.Put32(storage, 0x0106f4fc);
    fixture.space.Put32(storage + 0x24, value);
    fixture.space.Put32(storage + 0x2c, control);
    fixture.space.Put32(storage + 0x30, descriptor);
    return storage;
}

void Manager(Fixture& fixture, std::uint32_t context, std::uint32_t offset, std::uint32_t descriptor, std::uint32_t handlers, std::uint32_t crc, const std::vector<std::pair<std::uint32_t, std::uint32_t>>& values)
{
    const std::uint32_t manager = context + offset;
    fixture.space.Put32(manager, 0x01078554);
    fixture.space.Put32(manager + 0x1c, crc);
    fixture.space.Put32(manager + 0x44, handlers);
    fixture.space.Put32(descriptor + 8, crc);
    std::vector<std::uint32_t> nodes;
    for (const auto& [control, value] : values)
        nodes.push_back(fixture.KeyNode(control, PropertyStorage(fixture, control, descriptor, value)));
    fixture.FillMap(manager + 4, 0x15, 0x18, nodes);
}

void Names(Scenario& scenario, std::uint32_t omitLabel = 0, bool omitWindow = false)
{
    Fixture&                   f = scenario.fixture;
    std::vector<std::uint32_t> nodes;
    if (!omitWindow)
        nodes.push_back(f.NameNode("Window_ActionMenuWidget", scenario.window));
    for (std::uint32_t button = 1; button <= scenario.buttons.size(); ++button)
        nodes.push_back(f.NameNode("Button_Command_" + std::to_string(button), scenario.buttons[button - 1], button != 1));
    for (std::uint32_t slot = 1; slot <= scenario.labels.size(); ++slot)
        if (slot != omitLabel)
            nodes.push_back(f.NameNode("Label_CommandIcon_" + std::to_string(slot), scenario.labels[slot - 1]));
    f.FillMap(scenario.space + 4, 0x65, 0x68, nodes);
}

void Values(Scenario& scenario, std::uint32_t omitLabel = 0)
{
    std::vector<std::pair<std::uint32_t, std::uint32_t>> values;
    for (std::uint32_t button = 1; button <= scenario.buttons.size(); ++button)
        values.emplace_back(scenario.buttons[button - 1], button);
    for (std::uint32_t slot = 1; slot <= scenario.labels.size(); ++slot)
        if (slot != omitLabel)
            values.emplace_back(scenario.labels[slot - 1], 100 + slot);
    Manager(scenario.fixture, scenario.context, 0x21bc, 0x0135f908, 0x0135f92c, 0xab860104, values);
}

void Children(Fixture& f, std::uint32_t parent, const std::vector<std::uint32_t>& children)
{
    const std::uint32_t vector = f.space.Alloc(static_cast<std::uint32_t>(4 * children.size() + 4));
    for (std::size_t index = 0; index != children.size(); ++index)
        f.space.Put32(vector + static_cast<std::uint32_t>(4 * index), children[index]);
    f.space.Put32(parent + 0x264, children.empty() ? 0 : vector);
    f.space.Put32(parent + 0x268, children.empty() ? 0 : vector + static_cast<std::uint32_t>(4 * children.size()));
}

std::uint32_t TimerIcon(Fixture& f, std::uint32_t icon)
{
    const std::uint32_t timer = f.space.Alloc(0x3a8);
    f.space.Put32(timer, 0x010824b4);
    const std::uint32_t datas = f.space.Alloc(4);
    f.space.Put32(datas, icon);
    f.space.Put32(timer + 0x2b0, datas);
    f.space.Put32(timer + 0x2b4, datas + 4);
    return timer;
}

void Icons(Scenario& scenario)
{
    Fixture& f = scenario.fixture;
    for (std::uint32_t slot = 1; slot <= scenario.labels.size(); ++slot)
    {
        const std::uint32_t templ = f.space.Alloc(0xb4);
        const std::uint32_t root  = f.space.Alloc(0x270);
        const std::uint32_t frame = f.space.Alloc(0x270);
        const std::uint32_t group = f.space.Alloc(0x270);
        f.space.Put32(root, 0x01099990);
        f.space.Put32(frame, 0x01099994);
        f.space.Put32(group, 0x01099998);
        f.space.Put32(scenario.labels[slot - 1] + 0x318, templ);
        f.space.Put32(templ + 0xb0, root);
        Children(f, root, { frame, group });
        Children(f, frame, {});
        Children(f, group, { TimerIcon(f, 30000 + slot) });
    }
}

void SetPage(Scenario& scenario, std::uint8_t page)
{
    scenario.fixture.space.Put8(scenario.buffer + 0x10 + 1, page);
}

void SetMoving(Scenario& scenario, bool moving)
{
    scenario.fixture.space.Put8(scenario.buffer + 0x10 + 3, moving ? 0x01 : 0xfe);
}

void SetMacroType(Scenario& scenario, std::uint8_t macroType)
{
    scenario.fixture.space.Put8(scenario.buffer + 0x10 + 9, macroType);
}

constexpr std::int32_t  kNow       = 1700000000;
constexpr std::uint32_t kSaveStart = 776;
constexpr std::uint32_t kTempStart = 4872;
constexpr std::uint32_t kGroupBase = 0x20;
constexpr std::uint32_t kSyncBytes = 0x400;

std::uint32_t Field(Fixture& f, std::uint32_t pool, const std::string& name, std::uint32_t start)
{
    const std::uint32_t descriptor = f.space.Alloc(0x10);
    f.space.Put32(descriptor, 0x0110f974);
    f.space.Put32(descriptor + 8, start);
    return f.InternedNode(0x28, pool, f.Interned(name), descriptor);
}

void Declare(Fixture& f, std::uint32_t work, const std::vector<std::uint32_t>& fields)
{
    const std::uint32_t declarations = f.space.Alloc(0x14);
    f.space.Put32(work + 0x68, declarations);
    f.FillMap(declarations + 8, 0x25, 0x28, fields);
}

void PlayerWorks(Scenario& scenario, const std::vector<std::uint32_t>& extra)
{
    Fixture&            f     = scenario.fixture;
    const std::uint32_t pool  = scenario.pool;
    const std::uint32_t other = f.space.Alloc(0x90);
    const std::uint32_t empty = f.space.Alloc(0x90);
    Declare(f, other, { Field(f, pool, "unrelatedField", 0) });
    std::vector<std::uint32_t> nodes = { f.InternedNode(0x24, pool, f.Interned("EmptyWork"), empty), f.InternedNode(0x24, pool, f.Interned("CharaWork"), scenario.charaWork), f.InternedNode(0x24, pool, f.Interned("OtherWork"), other) };
    nodes.insert(nodes.end(), extra.begin(), extra.end());
    f.FillMap(scenario.shared + 0x1c, 0x21, 0x24, nodes);
}

// Layout-derived bit offsets, independent of the reader's constants.
void PutRecast(Scenario& scenario, std::uint32_t slot, std::int32_t end)
{
    scenario.fixture.space.Put32(scenario.syncData + (kSaveStart + 368 + 32 * slot) / 8, static_cast<std::uint32_t>(end));
}

void PutLimit(Scenario& scenario, std::uint32_t slot, std::int16_t limit)
{
    const std::uint32_t address = scenario.syncData + (kTempStart + 48 + 16 * slot) / 8;
    scenario.fixture.space.Put8(address, static_cast<std::uint8_t>(static_cast<std::uint16_t>(limit)));
    scenario.fixture.space.Put8(address + 1, static_cast<std::uint8_t>(static_cast<std::uint16_t>(limit) >> 8));
}

void PutCompatible(Scenario& scenario, std::uint32_t slot, bool compatible)
{
    const std::uint32_t bit     = kSaveStart + 1648 + slot;
    const std::uint32_t address = scenario.syncData + bit / 8;
    std::uint8_t        byte    = 0;
    Space::Copy(&scenario.fixture.space, address, &byte, 1);
    byte = static_cast<std::uint8_t>(compatible ? byte | (1u << (bit & 7)) : byte & ~(1u << (bit & 7)));
    scenario.fixture.space.Put8(address, byte);
}

std::int32_t PatternRemaining(std::uint32_t slot)
{
    return static_cast<std::int32_t>(slot % 2 == 1 ? 5 + slot : 10 + slot);
}

bool PatternCompatible(std::uint32_t slot)
{
    return slot % 3 != 1;
}

void BuildPlayer(Scenario& scenario, std::uint32_t actor)
{
    Fixture& f = scenario.fixture;

    f.space.Put32(scenario.mainModule, 0x00f9142c);
    scenario.main    = f.space.Alloc(0xa0);
    scenario.network = f.space.Alloc(0x320);
    f.space.Put32(0x013232b8, scenario.main);
    f.space.Put32(scenario.main, 0x00f54a24);
    f.space.Put32(scenario.main + 0x94, scenario.network);
    f.space.Put32(scenario.main + 0x98, scenario.mainModule);
    f.space.Put32(scenario.network, 0x00f91b5c);
    f.space.Put32(scenario.network + 8, scenario.mainModule);
    f.space.Put32(scenario.network + 0x30c, static_cast<std::uint32_t>(kNow));

    scenario.engine = f.space.Alloc(0x230);
    f.space.Put32(actor + 4, scenario.engine);
    const std::uint32_t outer = f.space.Alloc(8);
    const std::uint32_t inner = f.space.Alloc(0x10);
    scenario.player           = f.space.Alloc(0x10);
    f.space.Put32(scenario.engine + 0x220, outer);
    f.space.Put32(outer + 4, inner);
    f.space.Put32(inner + 0x0c, scenario.player);
    f.space.Put32(scenario.player, 0x00fd785c);
    scenario.playerRecord = f.space.Alloc(0x20);
    f.space.Put32(scenario.player + 4, scenario.playerRecord);
    f.space.Put32(scenario.playerRecord, scenario.player);
    const std::uint32_t link = f.space.Alloc(4);
    scenario.shared          = f.space.Alloc(0x40);
    f.space.Put32(scenario.playerRecord + 0x14, link);
    f.space.Put32(link, scenario.shared);

    scenario.charaWork = f.space.Alloc(0x90);
    f.space.Put8(scenario.charaWork + 0x18, 1);
    Declare(f, scenario.charaWork, { Field(f, scenario.pool, "parameterSave", kSaveStart), Field(f, scenario.pool, "parameterTemp", kTempStart) });
    PlayerWorks(scenario, {});

    const std::uint32_t scopes = f.space.Alloc(0x30);
    f.space.Put32(scenario.shared + 0x2c, scopes);
    f.space.Put32(scenario.shared + 0x30, scopes + 0x30);
    const std::uint32_t groups = f.space.Alloc(8);
    f.space.Put32(scopes + 0x20 + 4, groups);
    f.space.Put32(scopes + 0x20 + 8, groups + 8);
    f.space.Put32(groups + 4, kGroupBase);
    const std::uint32_t allocator  = f.space.Alloc(0x14);
    const std::uint32_t containers = f.space.Alloc(0x0c);
    f.space.Put32(scenario.shared + 4, allocator);
    f.space.Put32(allocator + 0x0c, containers);
    f.space.Put32(allocator + 0x10, containers + 0x0c);
    scenario.syncContainer = f.space.Alloc(0x24);
    f.space.Put32(containers + 8, scenario.syncContainer);
    f.space.Put32(scenario.syncContainer, 0x0110ef34);
    const std::uint32_t access = f.space.Alloc(0x10);
    f.space.Put32(scenario.syncContainer + 8, access);
    f.space.Put32(access, 0x0110ecdc);
    const std::uint32_t payload = f.space.Alloc(kSyncBytes);
    f.space.Put32(scenario.syncContainer + 0x1c, payload);
    f.space.Put32(scenario.syncContainer + 0x20, payload + kSyncBytes);
    scenario.syncData = payload + kGroupBase;

    for (std::uint32_t slot = 0; slot != 30; ++slot)
    {
        PutRecast(scenario, slot, kNow + 10 + static_cast<std::int32_t>(slot));
        PutLimit(scenario, slot, static_cast<std::int16_t>(slot % 2 == 1 ? 5 + slot : 100 + slot));
        PutCompatible(scenario, slot, PatternCompatible(slot));
    }
}

constexpr std::uint32_t kDesktopGroup = 0x10;
constexpr std::uint32_t kModeBit      = 24;
constexpr std::uint32_t kLockBit      = 35;
// work.rootWidget: 17 identity cells starting at byte 8.
constexpr std::uint32_t kRootsBit = 64;

std::uint32_t ControlElement(Fixture& f, std::uint32_t key, std::uint32_t table, std::uint32_t& control)
{
    const std::uint32_t element = f.Typed(0x0126da0c, 0xe8);
    f.space.Put32(element + 0x88, key);
    const std::uint32_t holder = f.space.Alloc(4);
    f.space.Put32(element + 0x80, holder);
    const std::uint32_t actor = f.Typed(0x01270b10, 0x20);
    f.space.Put32(holder, actor);
    control = f.space.Alloc(0x80);
    f.space.Put32(control, table);
    f.space.Put32(actor + 8, control);
    return element;
}

void Elements(Scenario& scenario, const std::vector<std::uint32_t>& extra)
{
    Fixture&                   f     = scenario.fixture;
    std::vector<std::uint32_t> nodes = { f.KeyNode(0x56, scenario.otherElement), f.KeyNode(0x55, scenario.element), f.KeyNode(0x57, scenario.desktop) };
    nodes.insert(nodes.end(), extra.begin(), extra.end());
    f.FillMap(scenario.mainModule + 0x17814, 0x15, 0x18, nodes);
}

// Set other lock-byte bits oppositely so only bit 3 can pass.
void SetDesktop(Scenario& scenario, std::int8_t mode, bool locked)
{
    scenario.fixture.space.Put8(scenario.desktopBuffer + kDesktopGroup + kModeBit / 8, static_cast<std::uint8_t>(mode));
    scenario.fixture.space.Put8(scenario.desktopBuffer + kDesktopGroup + kLockBit / 8, locked ? 0x08 : 0xf7);
}

void BuildDesktop(Scenario& scenario)
{
    Fixture& f       = scenario.fixture;
    scenario.desktop = ControlElement(f, 0x57, 0x00fd5b8c, scenario.desktopControl);
    Elements(scenario, {});

    scenario.desktopRecord       = f.space.Alloc(0x84);
    const std::uint32_t userdata = f.space.Alloc(4);
    f.space.Put32(scenario.desktopControl + 4, scenario.desktopRecord);
    f.space.Put32(scenario.desktopRecord, scenario.desktopControl);
    f.space.Put32(scenario.desktopRecord + 4, userdata);
    f.space.Put32(userdata, scenario.desktopRecord);
    const std::uint32_t handle       = f.space.Alloc(4);
    const std::uint32_t workOperator = f.space.Alloc(0x40);
    f.space.Put32(scenario.desktopRecord + 0x14, handle);
    f.space.Put32(handle, workOperator);

    const std::uint32_t pool = scenario.pool;
    const std::uint32_t work = f.space.Alloc(0x90);
    f.FillMap(workOperator + 0x1c, 0x21, 0x24, { f.InternedNode(0x24, pool, f.Interned("WidgetBaseClass"), f.space.Alloc(0x90)), f.InternedNode(0x24, pool, f.Interned("DesktopWidget"), work) });
    scenario.modeField = f.space.Alloc(0x10);
    scenario.lockField = f.space.Alloc(0x10);
    f.space.Put32(scenario.modeField, 0x0110fc3c);
    f.space.Put32(scenario.modeField + 8, kModeBit);
    f.space.Put32(scenario.lockField, 0x0110fbc4);
    f.space.Put32(scenario.lockField + 8, kLockBit);
    scenario.rootsField       = f.space.Alloc(0x18);
    const std::uint32_t cells = f.space.Alloc(0x10);
    f.space.Put32(scenario.rootsField, 0x0110f804);
    f.space.Put32(scenario.rootsField + 8, kRootsBit);
    f.space.Put32(scenario.rootsField + 0x10, 17);
    f.space.Put32(scenario.rootsField + 0x14, cells);
    f.space.Put32(cells, 0x0110ff0c);
    f.space.Put32(cells + 8, kRootsBit);
    const std::uint32_t declarations = f.space.Alloc(0x14);
    f.space.Put32(work + 0x64, declarations);
    f.FillMap(declarations + 8,
              0x25,
              0x28,
              { f.InternedNode(0x28, pool, f.Interned("mode"), scenario.modeField),
                f.InternedNode(0x28, pool, f.Interned("lockUserControl"), scenario.lockField),
                f.InternedNode(0x28, pool, f.Interned("rootWidget"), scenario.rootsField) });

    f.space.Put8(work + 0x18, 1);
    const std::uint32_t scopes = f.space.Alloc(0x20);
    f.space.Put32(workOperator + 0x2c, scopes);
    f.space.Put32(workOperator + 0x30, scopes + 0x20);
    const std::uint32_t groups = f.space.Alloc(8);
    f.space.Put32(scopes + 0x14, groups);
    f.space.Put32(scopes + 0x18, groups + 8);
    f.space.Put32(groups + 4, kDesktopGroup);
    const std::uint32_t allocator  = f.space.Alloc(0x14);
    const std::uint32_t containers = f.space.Alloc(8);
    f.space.Put32(workOperator + 4, allocator);
    f.space.Put32(allocator + 0x0c, containers);
    f.space.Put32(allocator + 0x10, containers + 8);
    const std::uint32_t container = f.space.Alloc(0x24);
    f.space.Put32(containers + 4, container);
    f.space.Put32(container, 0x0110ee6c);
    const std::uint32_t access = f.space.Alloc(0x10);
    f.space.Put32(container + 8, access);
    f.space.Put32(access, 0x0110ecdc);
    scenario.desktopBuffer = f.space.Alloc(0x80);
    f.space.Put32(container + 0x1c, scenario.desktopBuffer);
    f.space.Put32(container + 0x20, scenario.desktopBuffer + 0x80);
    SetDesktop(scenario, 16, true);
}

void PutRoot(Scenario& scenario, std::uint32_t index, std::uint32_t identity)
{
    scenario.fixture.space.Put32(scenario.desktopBuffer + kDesktopGroup + kRootsBit / 8 + 4 * index, identity);
}

std::uint32_t Widget(Fixture& f, std::uint32_t key, const std::string& name, std::uint32_t identity)
{
    std::uint32_t       control  = 0;
    const std::uint32_t element  = ControlElement(f, key, 0x00fd0000, control);
    const std::uint32_t record   = f.space.Alloc(0x80);
    const std::uint32_t userdata = f.space.Alloc(4);
    f.space.Put32(control + 4, record);
    f.space.Put32(record, control);
    f.space.Put32(record + 4, userdata);
    f.space.Put32(userdata, record);
    f.space.Put32(record + 0x70, identity);
    const std::uint32_t prototype = f.space.Alloc(0x58);
    const std::uint32_t string    = f.space.Alloc(0x54);
    f.NativeString(string, name);
    f.space.Put32(record + 0x0c, prototype);
    f.space.Put32(prototype + 0x50, 1);
    f.space.Put32(prototype + 0x54, string);
    f.space.Put32(element + 0xe0, 0);
    f.space.Put32(element + 0xe4, 0);
    return element;
}

void ElementChildren(Fixture& f, std::uint32_t parent, const std::vector<std::uint32_t>& children)
{
    const std::uint32_t vector = f.space.Alloc(static_cast<std::uint32_t>(4 * children.size() + 4));
    for (std::size_t index = 0; index != children.size(); ++index)
        f.space.Put32(vector + static_cast<std::uint32_t>(4 * index), children[index]);
    f.space.Put32(parent + 0xe0, children.empty() ? 0 : vector);
    f.space.Put32(parent + 0xe4, children.empty() ? 0 : vector + static_cast<std::uint32_t>(4 * children.size()));
}

void Build(Scenario& scenario)
{
    Fixture& f          = scenario.fixture;
    scenario.mainModule = f.space.Alloc(0x17820);

    scenario.element = f.Typed(0x0126da0c, 0xe0);
    f.space.Put32(scenario.element + 0x88, 0x55);
    const std::uint32_t other = f.Typed(0x0126da0c, 0xe0);
    f.space.Put32(other + 0x88, 0x56);
    scenario.otherElement = other;
    f.FillMap(scenario.mainModule + 0x17814, 0x15, 0x18, { f.KeyNode(0x56, other), f.KeyNode(0x55, scenario.element) });

    const std::uint32_t spaces = f.space.Alloc(0x10);
    f.space.Put32(scenario.element + 0xd4, spaces);
    scenario.space      = f.Typed(0x012e4670, 0x40);
    scenario.otherSpace = f.Typed(0x012e4670, 0x40);
    f.FillMap(spaces, 0x15, 0x18, { f.KeyNode(1, scenario.otherSpace), f.KeyNode(2, scenario.space) });
    f.FillMap(scenario.otherSpace + 4, 0x65, 0x68, { f.NameNode("Window_LogWidget", f.Typed(0x01269e20, 0x100)) });
    scenario.window = f.Typed(0x01269e20, 0x120);
    for (std::uint32_t& button : scenario.buttons)
        button = f.Typed(0x0126aaaa, 0x40);
    for (std::uint32_t& label : scenario.labels)
        label = f.Typed(0x0126bbbb, 0x320);
    Names(scenario);

    const std::uint32_t holder = f.space.Alloc(4);
    f.space.Put32(scenario.element + 0x80, holder);
    const std::uint32_t actor = f.Typed(0x01270b10, 0x20);
    f.space.Put32(holder, actor);
    const std::uint32_t control = f.Typed(0x012c1964, 0x80);
    f.space.Put32(actor + 8, control);
    scenario.record              = f.space.Alloc(0x84);
    const std::uint32_t userdata = f.space.Alloc(4);
    f.space.Put32(control + 4, scenario.record);
    f.space.Put32(scenario.record, control);
    f.space.Put32(scenario.record + 4, userdata);
    f.space.Put32(userdata, scenario.record);
    const std::uint32_t handle       = f.space.Alloc(4);
    const std::uint32_t workOperator = f.space.Alloc(0x40);
    f.space.Put32(scenario.record + 0x14, handle);
    f.space.Put32(handle, workOperator);

    const std::uint32_t pool = f.space.Alloc(0x20);
    const std::uint32_t work = f.space.Alloc(0x90);
    scenario.pool            = pool;
    f.FillMap(workOperator + 0x1c, 0x21, 0x24, { f.InternedNode(0x24, pool, f.Interned("WidgetBaseClass"), f.space.Alloc(0x90)), f.InternedNode(0x24, pool, f.Interned("ActionMenuWidget"), work) });
    scenario.declarations = f.space.Alloc(0x14);
    f.space.Put32(work + 0x64, scenario.declarations);
    const std::uint32_t page   = f.space.Alloc(0x10);
    const std::uint32_t macro  = f.space.Alloc(0x10);
    const std::uint32_t moving = f.space.Alloc(0x10);
    f.space.Put32(page, 0x0110fc3c);
    f.space.Put32(page + 8, 8);
    f.space.Put32(macro, 0x0110fc3c);
    f.space.Put32(macro + 8, 72);
    // Retail slotMoving is bit 0 of byte 3.
    f.space.Put32(moving, 0x0110fbc4);
    f.space.Put32(moving + 8, 24);
    f.FillMap(scenario.declarations + 8, 0x25, 0x28, { f.InternedNode(0x28, pool, f.Interned("slotPage"), page), f.InternedNode(0x28, pool, f.Interned("userMacroType"), macro), f.InternedNode(0x28, pool, f.Interned("slotMoving"), moving) });

    f.space.Put8(work + 0x18, 1);
    const std::uint32_t scopes = f.space.Alloc(0x20);
    f.space.Put32(workOperator + 0x2c, scopes);
    f.space.Put32(workOperator + 0x30, scopes + 0x20);
    const std::uint32_t groups = f.space.Alloc(8);
    f.space.Put32(scopes + 0x14, groups);
    f.space.Put32(scopes + 0x18, groups + 8);
    f.space.Put32(groups + 4, 0x10);
    const std::uint32_t allocator  = f.space.Alloc(0x14);
    const std::uint32_t containers = f.space.Alloc(8);
    f.space.Put32(workOperator + 4, allocator);
    f.space.Put32(allocator + 0x0c, containers);
    f.space.Put32(allocator + 0x10, containers + 8);
    const std::uint32_t container = f.space.Alloc(0x24);
    f.space.Put32(containers + 4, container);
    f.space.Put32(container, 0x0110ee6c);
    const std::uint32_t access = f.space.Alloc(0x10);
    f.space.Put32(container + 8, access);
    f.space.Put32(access, 0x0110ecdc);
    scenario.buffer = f.space.Alloc(0x40);
    f.space.Put32(container + 0x1c, scenario.buffer);
    f.space.Put32(container + 0x20, scenario.buffer + 0x40);
    SetPage(scenario, 2);
    SetMacroType(scenario, 0);

    f.space.Put32(0x01363f38, 3);
    scenario.tlsArray        = f.space.Alloc(0x40);
    const std::uint32_t slot = f.space.Alloc(0x20);
    scenario.context         = f.space.Alloc(0x2300);
    f.space.Put32(scenario.tlsArray + 12, slot);
    f.space.Put32(slot + 0x1c, scenario.context);
    Values(scenario);
    Icons(scenario);
    BuildPlayer(scenario, actor);
    BuildDesktop(scenario);
}

SlotCommands Run(Scenario& scenario)
{
    return ReadSlotCommands(scenario.mainModule, scenario.tlsArray, &Space::Copy, &scenario.fixture.space);
}

DesktopState Desktop(Scenario& scenario)
{
    return ReadDesktop(scenario.mainModule, &Space::Copy, &scenario.fixture.space);
}

SlotCommands Run(Scenario& scenario, SlotStates* states, bool labels = true)
{
    return ReadSlotCommands(scenario.mainModule, scenario.tlsArray, &Space::Copy, &scenario.fixture.space, labels, states);
}

bool AllZero(const SlotCommands& result)
{
    for (const std::int32_t command : result.commands)
        if (command != 0)
            return false;
    for (const std::uint32_t icon : result.icons)
        if (icon != 0)
            return false;
    return true;
}

void RequireLabels(const SlotCommands& result, std::size_t skipped, const char* reason)
{
    for (std::size_t index = 0; index != result.commands.size(); ++index)
    {
        const std::int32_t expected = index == skipped ? 0 : 101 + static_cast<std::int32_t>(index);
        Require(result.commands[index] == expected, reason);
    }
}

void TestComplete()
{
    Scenario scenario;
    Build(scenario);
    SetMoving(scenario, false);
    const SlotCommands result = Run(scenario);
    Require(result.valid && result.page == 2 && result.macroType == 0 && !result.slotMoving, "complete read");
    Require(result.pageAddress == scenario.buffer + 0x10 + 1, "page byte address");
    RequireLabels(result, result.commands.size(), "absolute slot commands");
    for (std::size_t index = 0; index != result.icons.size(); ++index)
        Require(result.icons[index] == 30001u + index, "absolute slot icons");
}

void TestSkipLabels()
{
    Scenario scenario;
    Build(scenario);
    SetMoving(scenario, false);
    const SlotCommands result = ReadSlotCommands(scenario.mainModule, scenario.tlsArray, &Space::Copy, &scenario.fixture.space, false);
    Require(result.valid && result.page == 2 && result.macroType == 0 && !result.slotMoving, "labels=false page fields");
    Require(result.pageAddress == scenario.buffer + 0x10 + 1, "labels=false page byte address");
    Require(AllZero(result), "labels=false leaves commands and icons zero");
}

void TestPageMapping()
{
    Scenario scenario;
    Build(scenario);
    for (std::uint8_t page = 1; page <= 3; ++page)
    {
        SetPage(scenario, page);
        const SlotCommands result = Run(scenario);
        Require(result.valid && result.page == page && result.macroType == 0, "page read");
        Require(result.pageAddress == scenario.buffer + 0x10 + 1, "page byte address");
        RequireLabels(result, result.commands.size(), "page labels");
    }
    for (const std::uint8_t page : { std::uint8_t{ 0 }, std::uint8_t{ 4 }, std::uint8_t{ 0xff } })
    {
        SetPage(scenario, page);
        const SlotCommands result = Run(scenario);
        Require(result.valid && result.page == static_cast<std::int8_t>(page) && result.pageAddress == 0, "unmapped page has no page byte");
        RequireLabels(result, result.commands.size(), "unmapped page labels");
        Require(result.icons[0] == 30001 && result.icons[29] == 30030, "unmapped page icons");
    }
}

void TestMacroBar()
{
    Scenario scenario;
    Build(scenario);
    for (const std::uint8_t macroType : { std::uint8_t{ 1 }, std::uint8_t{ 0xff } })
    {
        SetMacroType(scenario, macroType);
        const SlotCommands result = Run(scenario);
        Require(result.valid && result.page == 2 && result.macroType == static_cast<std::int8_t>(macroType), "macro bar read");
        Require(AllZero(result), "macro bar leaves commands and icons zero");
    }
}

// A failed label read zeroes only its own command.
void TestLabelFailureIsLocal()
{
    {
        Scenario scenario;
        Build(scenario);
        Names(scenario, 13);
        const SlotCommands result = Run(scenario);
        Require(result.valid && result.commands[12] == 0 && result.icons[12] == 0, "missing label name");
        RequireLabels(result, 12, "labels beside missing label name");
    }
    {
        Scenario scenario;
        Build(scenario);
        Values(scenario, 14);
        const SlotCommands result = Run(scenario);
        Require(result.valid && result.commands[13] == 0 && result.icons[13] == 0, "unmaterialized label");
        RequireLabels(result, 13, "labels beside unmaterialized label");
    }
}

void TestIcons()
{
    {
        Scenario scenario;
        Build(scenario);
        Fixture& f = scenario.fixture;
        f.space.Put32(scenario.labels[11] + 0x318, 0);
        f.space.Put32(scenario.labels[12] + 0x318, 0x7ff00000);
        const std::uint32_t root = f.space.Alloc(0x270);
        f.space.Put32(root, 0x01099990);
        Children(f, root, { TimerIcon(f, 1), TimerIcon(f, 2) });
        const std::uint32_t templ = f.space.Alloc(0xb4);
        f.space.Put32(templ + 0xb0, root);
        f.space.Put32(scenario.labels[13] + 0x318, templ);
        const SlotCommands result = Run(scenario);
        Require(result.valid, "bad templates valid");
        RequireLabels(result, result.commands.size(), "commands beside bad templates");
        Require(result.icons[11] == 0 && result.icons[12] == 0 && result.icons[13] == 0, "missing, unmapped or ambiguous template");
        Require(result.icons[10] == 30011 && result.icons[14] == 30015, "icons beside bad templates");
    }
    {
        Scenario scenario;
        Build(scenario);
        Fixture&            f    = scenario.fixture;
        const std::uint32_t root = f.space.Alloc(0x270);
        f.space.Put32(root, 0x01099990);
        Children(f, root, { root });
        const std::uint32_t templ = f.space.Alloc(0xb4);
        f.space.Put32(templ + 0xb0, root);
        for (const std::uint32_t label : scenario.labels)
            f.space.Put32(label + 0x318, templ);
        const SlotCommands result = Run(scenario);
        Require(result.valid, "cyclic templates valid");
        RequireLabels(result, result.commands.size(), "commands beside cyclic templates");
        for (const std::uint32_t icon : result.icons)
            Require(icon == 0, "cyclic template icons");
    }
}

void TestSlotState()
{
    {
        Scenario scenario;
        Build(scenario);
        SetMoving(scenario, true);
        const SlotCommands result = Run(scenario);
        Require(result.valid && result.slotMoving, "moving bar read");
    }
    {
        Scenario scenario;
        Build(scenario);
        SetMoving(scenario, false);
        Fixture&            f     = scenario.fixture;
        const std::uint32_t page  = f.space.Alloc(0x10);
        const std::uint32_t macro = f.space.Alloc(0x10);
        f.space.Put32(page, 0x0110fc3c);
        f.space.Put32(page + 8, 8);
        f.space.Put32(macro, 0x0110fc3c);
        f.space.Put32(macro + 8, 72);
        f.FillMap(scenario.declarations + 8, 0x25, 0x28, { f.InternedNode(0x28, scenario.pool, f.Interned("slotPage"), page), f.InternedNode(0x28, scenario.pool, f.Interned("userMacroType"), macro) });
        const SlotCommands result = Run(scenario);
        Require(result.valid && result.slotMoving, "missing slotMoving counts as moving");
        RequireLabels(result, result.commands.size(), "commands without slotMoving");
    }
    {
        Scenario scenario;
        Build(scenario);
        SetMoving(scenario, false);
        std::uint32_t container = 0;
        for (std::uint32_t address = scenario.buffer - 0x400; address != scenario.buffer; address += 4)
        {
            std::uint32_t value = 0;
            if (Space::Copy(&scenario.fixture.space, address, &value, 4) && value == scenario.buffer)
                container = address - 0x1c;
        }
        Require(container != 0, "container located");
        std::uint32_t access = 0;
        Require(Space::Copy(&scenario.fixture.space, container + 8, &access, 4), "access located");
        scenario.fixture.space.Put32(access, 0x0110ed84);
        scenario.fixture.space.Put8(access + 0x0c, 7);
        scenario.fixture.space.Put8(access + 0x0d, 0);
        scenario.fixture.space.Put8(0x0130d420, 7);
        scenario.fixture.space.Put8(0x0130d421, 0);
        const SlotCommands result = Run(scenario);
        Require(result.valid && !result.slotMoving && result.pageAddress == 0, "SharedTemp page byte not writable");
        RequireLabels(result, result.commands.size(), "SharedTemp labels read");
    }
}

void TestMissingWindow()
{
    Scenario scenario;
    Build(scenario);
    Names(scenario, 0, true);
    const SlotCommands result = Run(scenario);
    Require(!result.valid && result.page == 0 && result.macroType == 0 && AllZero(result), "missing window");
}

void TestDuplicateWindow()
{
    Scenario scenario;
    Build(scenario);
    Fixture& f = scenario.fixture;
    f.FillMap(scenario.otherSpace + 4, 0x65, 0x68, { f.NameNode("Window_ActionMenuWidget", f.Typed(0x01269e20, 0x100)) });
    Require(!Run(scenario).valid, "duplicate window");
}

void TestFaultingCopy()
{
    Scenario scenario;
    Build(scenario);
    scenario.fixture.space.Unmap(scenario.record, 0x84);
    const SlotCommands result = Run(scenario);
    Require(!result.valid && AllZero(result), "faulting copy");
}

void TestStageGuards()
{
    {
        Scenario scenario;
        Build(scenario);
        Require(!ReadSlotCommands(0, scenario.tlsArray, &Space::Copy, &scenario.fixture.space).valid, "no main module");
        Require(!ReadSlotCommands(scenario.mainModule, scenario.tlsArray, nullptr, nullptr).valid, "no copy function");
        Require(!ReadSlotCommands(scenario.mainModule, 0, &Space::Copy, &scenario.fixture.space).valid, "missing TLS array");
    }
    {
        Scenario scenario;
        Build(scenario);
        Fixture&            f    = scenario.fixture;
        const std::uint32_t page = f.space.Alloc(0x10);
        f.space.Put32(page, 0x0110fc3c);
        f.space.Put32(page + 8, 8);
        f.FillMap(scenario.declarations + 8, 0x25, 0x28, { f.InternedNode(0x28, scenario.pool, f.Interned("slotPage"), page) });
        Require(!Run(scenario).valid, "missing userMacroType");
    }
    {
        // userMacroType at groupBase 0x10 + 9 is excluded by end 0x19.
        Scenario scenario;
        Build(scenario);
        std::uint32_t container = 0;
        for (std::uint32_t address = scenario.buffer - 0x400; address != scenario.buffer; address += 4)
        {
            std::uint32_t value = 0;
            if (Space::Copy(&scenario.fixture.space, address, &value, 4) && value == scenario.buffer)
                container = address - 0x1c;
        }
        Require(container != 0, "container located");
        scenario.fixture.space.Put32(container + 0x20, scenario.buffer + 0x19);
        Require(!Run(scenario).valid, "value past buffer end");
    }
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Put32(scenario.context + 0x21bc + 0x1c, 1);
        const SlotCommands result = Run(scenario);
        Require(result.valid && AllZero(result), "manager mismatch leaves commands zero");
    }
}

void TestCycleStops()
{
    Scenario scenario;
    Build(scenario);
    Fixture&            f    = scenario.fixture;
    const std::uint32_t node = f.KeyNode(0x57, 0);
    f.FillMap(scenario.mainModule + 0x17814, 0x15, 0x18, { node });
    f.space.Put32(node + 8, node);
    Require(!Run(scenario).valid, "cyclic tree bounded");
}

void RequireMenuIntact(const SlotCommands& result, const char* reason)
{
    Require(result.valid && result.page == 2 && result.macroType == 0 && !result.slotMoving, reason);
    RequireLabels(result, result.commands.size(), reason);
    for (std::size_t index = 0; index != result.icons.size(); ++index)
        Require(result.icons[index] == 30001u + index, reason);
}

void TestStatesComplete()
{
    Scenario scenario;
    Build(scenario);
    SetMoving(scenario, false);
    PutRecast(scenario, 0, kNow - 50);
    PutRecast(scenario, 1, kNow + 30);
    PutLimit(scenario, 1, 600);
    PutRecast(scenario, 2, kNow + 500);
    PutLimit(scenario, 2, 45);
    PutRecast(scenario, 3, kNow);
    PutLimit(scenario, 3, 7);
    PutRecast(scenario, 29, kNow + 12);
    PutLimit(scenario, 29, 0);
    SlotStates         states;
    const SlotCommands result = Run(scenario, &states);
    RequireMenuIntact(result, "menu read beside states");
    Require(result.window == scenario.window, "window object");
    Require(states.valid && states.now == kNow && states.shared == scenario.shared, "states header");
    for (std::uint32_t slot = 0; slot != 30; ++slot)
    {
        std::int32_t expected = PatternRemaining(slot);
        if (slot == 0 || slot == 3)
            expected = 0;
        else if (slot == 1)
            expected = 30;
        else if (slot == 2)
            expected = 45;
        else if (slot == 29)
            expected = 12;
        Require(states.remaining[slot] == expected, "slot remaining");
        Require(states.compatible[slot] == PatternCompatible(slot), "slot compatibility");
    }
}

void TestStatesBesideMenuModes()
{
    {
        Scenario scenario;
        Build(scenario);
        SlotStates         states;
        const SlotCommands result = Run(scenario, &states, false);
        Require(result.valid && result.page == 2 && AllZero(result), "labels=false menu with states");
        Require(states.valid && states.shared == scenario.shared && states.remaining[1] == PatternRemaining(1), "labels=false states");
    }
    {
        Scenario scenario;
        Build(scenario);
        SetMacroType(scenario, 1);
        SlotStates         states;
        const SlotCommands result = Run(scenario, &states);
        Require(result.valid && result.macroType == 1 && AllZero(result), "macro bar menu with states");
        Require(states.valid && states.remaining[2] == PatternRemaining(2), "macro bar states");
    }
}

void TestRecastRemaining()
{
    constexpr std::int32_t kMin = std::numeric_limits<std::int32_t>::min();
    constexpr std::int32_t kMax = std::numeric_limits<std::int32_t>::max();
    Require(RecastRemaining(100, 100, 0) == 0, "recast ends now");
    Require(RecastRemaining(50, 100, 0) == 0, "recast ended");
    Require(RecastRemaining(50, 100, 30) == 0, "ended recast ignores max");
    Require(RecastRemaining(130, 100, 0) == 30, "time left, no max");
    Require(RecastRemaining(130, 100, -5) == 30, "negative max ignored");
    Require(RecastRemaining(130, 100, 29) == 29, "max below time left");
    Require(RecastRemaining(130, 100, 30) == 30, "max equal to time left");
    Require(RecastRemaining(130, 100, 31) == 30, "max above time left");
    Require(RecastRemaining(130, 100, 1) == 1, "smallest max");
    Require(RecastRemaining(kMin, kMax, 0) == 0, "difference does not wrap");
    Require(RecastRemaining(kMax, 0, 0) == kMax, "largest time left");
    Require(RecastRemaining(kMax, kMin, 5) == 5, "wide difference capped");
}

void RequireNoStates(Scenario& scenario, const char* reason)
{
    SlotStates         states;
    const SlotCommands result = Run(scenario, &states);
    RequireMenuIntact(result, reason);
    Require(!states.valid, reason);
}

void TestStatesGuards()
{
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Put32(scenario.player, 0x00fd7860);
        RequireNoStates(scenario, "wrong player vtable");
    }
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Put32(scenario.main + 0x98, scenario.mainModule + 0x10);
        RequireNoStates(scenario, "Main names another MainModule");
    }
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Put32(scenario.mainModule, 0x00f91430);
        RequireNoStates(scenario, "MainModule vtable");
    }
    {
        // parameterSave declared by two works is ambiguous.
        Scenario scenario;
        Build(scenario);
        Fixture&            f    = scenario.fixture;
        const std::uint32_t twin = f.space.Alloc(0x90);
        Declare(f, twin, { Field(f, scenario.pool, "parameterSave", 0) });
        PlayerWorks(scenario, { f.InternedNode(0x24, scenario.pool, f.Interned("TwinWork"), twin) });
        RequireNoStates(scenario, "parameterSave declared twice");
    }
    {
        // parameterTemp in another work cannot share the group.
        Scenario scenario;
        Build(scenario);
        Fixture&            f    = scenario.fixture;
        const std::uint32_t temp = f.space.Alloc(0x90);
        Declare(f, scenario.charaWork, { Field(f, scenario.pool, "parameterSave", kSaveStart) });
        Declare(f, temp, { Field(f, scenario.pool, "parameterTemp", kTempStart) });
        PlayerWorks(scenario, { f.InternedNode(0x24, scenario.pool, f.Interned("TempWork"), temp) });
        RequireNoStates(scenario, "parameterTemp in another work");
    }
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Put32(scenario.syncContainer, 0x0110ee6c);
        RequireNoStates(scenario, "container is not a SyncContainer");
    }
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Unmap(scenario.engine + 0x220, 4);
        RequireNoStates(scenario, "fault in the engine");
    }
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Unmap(scenario.syncData + (kSaveStart + 368 + 32 * 29) / 8, 4);
        RequireNoStates(scenario, "fault in a slot recast");
    }
}

// Clear stale caller states even if the read stops before reaching the player.
void TestStatesCleared()
{
    Scenario scenario;
    Build(scenario);
    SlotStates states;
    states.valid  = true;
    states.shared = 1;
    states.now    = 2;
    states.remaining.fill(3);
    states.compatible.fill(true);
    Require(!ReadSlotCommands(0, scenario.tlsArray, &Space::Copy, &scenario.fixture.space, true, &states).valid, "no main module");
    Require(!states.valid && states.shared == 0 && states.now == 0, "stale header cleared");
    for (std::uint32_t slot = 0; slot != 30; ++slot)
        Require(states.remaining[slot] == 0 && !states.compatible[slot], "stale slots cleared");
}

// Use the engine's first record only if it names the player; otherwise follow the player's link.
void TestPlayerRecordSelection()
{
    {
        Scenario scenario;
        Build(scenario);
        Fixture& f = scenario.fixture;
        f.space.Put32(scenario.engine + 0x1bc, scenario.playerRecord);
        f.space.Put32(scenario.player + 4, f.space.Alloc(0x20));
        SlotStates states;
        Run(scenario, &states);
        Require(states.valid && states.shared == scenario.shared, "engine record used");
    }
    {
        Scenario scenario;
        Build(scenario);
        Fixture& f = scenario.fixture;
        f.space.Put32(scenario.engine + 0x1bc, f.space.Alloc(0x20));
        SlotStates states;
        Run(scenario, &states);
        Require(states.valid && states.shared == scenario.shared, "player record used");
    }
}

void TestDesktopComplete()
{
    Scenario scenario;
    Build(scenario);
    DesktopState state = Desktop(scenario);
    Require(state.valid && state.locked && state.mode == 16, "locked desktop");
    SetDesktop(scenario, 8, false);
    state = Desktop(scenario);
    Require(state.valid && !state.locked && state.mode == 8, "unlocked desktop");
    SetDesktop(scenario, -3, true);
    state = Desktop(scenario);
    Require(state.valid && state.locked && state.mode == -3, "mode is a signed byte");
}

void RequireNoDesktop(Scenario& scenario, const char* reason)
{
    Require(!Desktop(scenario).valid, reason);
}

void TestDesktopGuards()
{
    Require(!ReadDesktop(0, &Space::Copy, nullptr).valid, "no main module");
    {
        Scenario scenario;
        Build(scenario);
        Require(!ReadDesktop(scenario.mainModule, nullptr, nullptr).valid, "no copy function");
    }
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Put32(scenario.desktopControl, 0x00fd5b90);
        RequireNoDesktop(scenario, "no Desktop element");
    }
    {
        // Both Desktop elements are individually valid; only uniqueness rejects the pair.
        Scenario scenario;
        Build(scenario);
        Fixture&            f       = scenario.fixture;
        const std::uint32_t element = f.Typed(0x0126da0c, 0xe0);
        f.space.Put32(element + 0x88, 0x58);
        f.space.Put32(element + 0x80, f.space.Get32(scenario.desktop + 0x80));
        Elements(scenario, { f.KeyNode(0x58, element) });
        RequireNoDesktop(scenario, "two Desktop elements");
    }
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Put32(scenario.lockField, 0x0110fc3c);
        RequireNoDesktop(scenario, "lockUserControl with the integer8 table");
    }
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Put32(scenario.modeField, 0x0110fbc4);
        RequireNoDesktop(scenario, "mode with the boolean table");
    }
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Put32(scenario.modeField + 8, kModeBit + 1);
        RequireNoDesktop(scenario, "unaligned mode bit");
    }
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Unmap(scenario.desktopBuffer + kDesktopGroup + kModeBit / 8, 1);
        RequireNoDesktop(scenario, "fault in the mode byte");
    }
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Unmap(scenario.desktopRecord + 0x14, 4);
        RequireNoDesktop(scenario, "fault in the Desktop record");
    }
}

struct MapScenario
{
    Scenario      scenario;
    std::uint32_t root = 0;
};

void BuildMap(MapScenario& map, const std::string& rootClass)
{
    Build(map.scenario);
    Fixture& f = map.scenario.fixture;
    map.root   = Widget(f, 0x60, rootClass, 0xc1000081);
    Elements(map.scenario, { f.KeyNode(0x60, map.root) });
    PutRoot(map.scenario, 2, 0xc1000081);
}

bool MapShown(Scenario& scenario)
{
    const DesktopState state = Desktop(scenario);
    Require(state.valid, "Desktop read beside the map read");
    return state.map;
}

void TestDesktopMap()
{
    {
        Scenario scenario;
        Build(scenario);
        Require(!MapShown(scenario), "no root windows");
    }
    {
        MapScenario map;
        BuildMap(map, "MapNavigationWidget");
        Require(MapShown(map.scenario), "map at a root");
        PutRoot(map.scenario, 2, 0);
        Require(!MapShown(map.scenario), "map no longer a root");
    }
    {
        MapScenario map;
        BuildMap(map, "MiniMapWidget");
        Require(!MapShown(map.scenario), "minimap at a root");
    }
    {
        MapScenario map;
        BuildMap(map, "MapNavigationWidgetX");
        Require(!MapShown(map.scenario), "longer class name");
    }
    {
        MapScenario map;
        BuildMap(map, "JournalListWidget");
        Fixture&            f      = map.scenario.fixture;
        const std::uint32_t detail = Widget(f, 0x61, "JournalDetailWidget", 0xc1000082);
        const std::uint32_t quest  = Widget(f, 0x62, "MapNavigationWidget", 0xc1000083);
        Elements(map.scenario, { f.KeyNode(0x60, map.root), f.KeyNode(0x61, detail), f.KeyNode(0x62, quest) });
        ElementChildren(f, map.root, { 0, detail });
        Require(!MapShown(map.scenario), "journal without a map");
        ElementChildren(f, detail, { quest });
        Require(MapShown(map.scenario), "journal quest map");
        Elements(map.scenario, { f.KeyNode(0x60, map.root), f.KeyNode(0x61, detail) });
        Require(!MapShown(map.scenario), "unregistered child map");
    }
}

void TestDesktopMapGuards()
{
    {
        MapScenario map;
        BuildMap(map, "MapNavigationWidget");
        Fixture&            f     = map.scenario.fixture;
        const std::uint32_t other = Widget(f, 0x61, "MapNavigationWidget", 0xc1000081);
        Elements(map.scenario, { f.KeyNode(0x60, map.root), f.KeyNode(0x61, other) });
        Require(!MapShown(map.scenario), "ambiguous root");
    }
    {
        MapScenario map;
        BuildMap(map, "MapNavigationWidget");
        map.scenario.fixture.space.Put32(map.scenario.rootsField + 0x10, 16);
        Require(!MapShown(map.scenario), "rootWidget count");
    }
    {
        // Group ends at 0x70; mapped bytes beyond it must still be rejected.
        MapScenario map;
        BuildMap(map, "MapNavigationWidget");
        Fixture&            f     = map.scenario.fixture;
        const std::uint32_t cells = f.space.Get32(map.scenario.rootsField + 0x14);
        f.space.Map(map.scenario.desktopBuffer + 0x80, 8);
        for (const std::uint32_t first : { 0x70u - 68u, 0x70u - 64u })
        {
            f.space.Put32(map.scenario.rootsField + 8, first * 8);
            f.space.Put32(cells + 8, first * 8);
            f.space.Put32(map.scenario.desktopBuffer + kDesktopGroup + first, 0xc1000081);
            Require(MapShown(map.scenario) == (first == 0x70u - 68u), "cells end at the buffer");
        }
    }
    {
        // A map-subtree fault leaves Desktop lock and mode valid.
        MapScenario map;
        BuildMap(map, "JournalListWidget");
        Fixture&            f      = map.scenario.fixture;
        const std::uint32_t detail = Widget(f, 0x61, "JournalDetailWidget", 0xc1000082);
        const std::uint32_t quest  = Widget(f, 0x62, "MapNavigationWidget", 0xc1000083);
        Elements(map.scenario, { f.KeyNode(0x60, map.root), f.KeyNode(0x61, detail), f.KeyNode(0x62, quest) });
        ElementChildren(f, map.root, { detail });
        ElementChildren(f, detail, { quest });
        Require(MapShown(map.scenario), "journal quest map");
        f.space.Unmap(detail + 0xe0, 4);
        const DesktopState state = Desktop(map.scenario);
        Require(state.valid && state.locked && state.mode == 16 && !state.map, "fault in a child vector");
    }
    {
        // Cyclic child windows must stop at the map-walk bound.
        MapScenario map;
        BuildMap(map, "JournalListWidget");
        ElementChildren(map.scenario.fixture, map.root, { map.root, map.root });
        Require(!MapShown(map.scenario), "cyclic children bounded");
    }
}

// Menu and Desktop reads must remain independent.
void TestDesktopBesideMenu()
{
    {
        Scenario scenario;
        Build(scenario);
        SlotStates         states;
        const SlotCommands result = Run(scenario, &states);
        RequireMenuIntact(result, "menu read beside Desktop");
        Require(states.valid && Desktop(scenario).valid, "both reads valid");
    }
    {
        Scenario scenario;
        Build(scenario);
        scenario.fixture.space.Unmap(scenario.desktopRecord + 0x14, 4);
        const SlotCommands result = Run(scenario);
        RequireMenuIntact(result, "menu read beside a faulting Desktop");
    }
    {
        Scenario scenario;
        Build(scenario);
        Names(scenario, 0, true);
        Require(!Run(scenario).valid, "Desktop is not a menu window");
        Require(Desktop(scenario).valid, "Desktop without a menu window");
    }
}

} // namespace

int main()
{
    try
    {
        TestComplete();
        TestSkipLabels();
        TestPageMapping();
        TestMacroBar();
        TestLabelFailureIsLocal();
        TestIcons();
        TestSlotState();
        TestMissingWindow();
        TestDuplicateWindow();
        TestFaultingCopy();
        TestStageGuards();
        TestCycleStops();
        TestStatesComplete();
        TestStatesBesideMenuModes();
        TestRecastRemaining();
        TestStatesGuards();
        TestStatesCleared();
        TestPlayerRecordSelection();
        TestDesktopComplete();
        TestDesktopGuards();
        TestDesktopBesideMenu();
        TestDesktopMap();
        TestDesktopMapGuards();
    }
    catch (const std::exception& error)
    {
        std::cerr << error.what() << '\n';
        return 1;
    }
    std::cout << "xhbar slot commands passed\n";
    return 0;
}
