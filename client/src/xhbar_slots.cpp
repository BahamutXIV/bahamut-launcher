#include "xhbar_slots.h"

#include <algorithm>
#include <charconv>
#include <cstring>
#include <limits>
#include <string_view>

namespace bahamut_client::xhbar
{
namespace
{

// Retail 2012.09.19.0001 layouts.
constexpr std::uint32_t kElementMapOffset      = 0x10 + 0x17804;
constexpr std::uint32_t kFormElementType       = 0x0126da0c;
constexpr std::uint32_t kNamespaceType         = 0x012e4670;
constexpr std::uint32_t kWindowType            = 0x01269e20;
constexpr std::uint32_t kLuaActorType          = 0x01270b10;
constexpr std::uint32_t kWidgetBaseType        = 0x012c1964;
constexpr std::uint32_t kInteger8Table         = 0x0110fc3c;
constexpr std::uint32_t kBooleanTable          = 0x0110fbc4;
constexpr std::uint32_t kSingleThreadContainer = 0x0110ee6c;
constexpr std::uint32_t kSimpleAccess          = 0x0110ecdc;
constexpr std::uint32_t kSharedTempAccess      = 0x0110ed84;
constexpr std::uint32_t kSharedTempLocalTag    = 0x0130d420;
constexpr std::uint32_t kSqwtTlsIndex          = 0x01363f38;
constexpr std::uint32_t kPropertyManagerTable  = 0x01078554;
constexpr std::uint32_t kPropertyStorageTable  = 0x0106f4fc;
constexpr std::uint32_t kTimerIconTable        = 0x010824b4;
constexpr std::uint32_t kWorkScope             = 1;
constexpr std::size_t   kMaxVisits             = 65536;
constexpr std::size_t   kMaxStack              = 128;
constexpr std::size_t   kMaxName               = 64;

constexpr std::string_view kWindowName = "Window_ActionMenuWidget";
constexpr std::string_view kLabelStem  = "Label_CommandIcon_";
constexpr std::string_view kWorkClass  = "ActionMenuWidget";
constexpr std::string_view kSlotPage   = "slotPage";
constexpr std::string_view kMacroType  = "userMacroType";
constexpr std::string_view kSlotMoving = "slotMoving";

constexpr std::uint32_t    kDesktopControlTable = 0x00fd5b8c;
constexpr std::string_view kDesktopClass        = "DesktopWidget";
constexpr std::string_view kDesktopMode         = "mode";
constexpr std::string_view kLockUserControl     = "lockUserControl";

// work.rootWidget descriptor: start bit +8, count +0x10, element descriptor
// +0x14; actor cells hold root record identities [R+0x70], 0 for nil.
constexpr std::uint32_t    kArrayTable     = 0x0110f804;
constexpr std::uint32_t    kActorTable     = 0x0110ff0c;
constexpr std::uint32_t    kRootWidgets    = 17;
constexpr std::string_view kRootWidget     = "rootWidget";
constexpr std::string_view kMapClass       = "MapNavigationWidget";
constexpr std::size_t      kMaxRegistered  = 512;
constexpr std::size_t      kMaxMapWalk     = 256;
constexpr std::uint32_t    kMaxMapChildren = 256;
constexpr std::uint32_t    kMaxMapDepth    = 8;

struct AttachedProperty
{
    std::uint32_t managerOffset;
    std::uint32_t descriptor;
    std::uint32_t handlers;
};

// IntData.Value0 manager, metadata descriptor and handler record in the Sqwt context.
constexpr AttachedProperty kValue0 = { 0x21bc, 0x0135f908, 0x0135f92c };

class Memory
{
public:
    Memory(ReadMemory copy, void* context) noexcept
    : copy_(copy)
    , context_(context)
    {
    }

    // A failed copy poisons the read; callers must check Faulted().
    bool Faulted() const noexcept
    {
        return faulted_;
    }

    // Nested walks share one node budget.
    bool Visit() noexcept
    {
        return ++visits_ <= kMaxVisits;
    }

    // Failure leaves the rest of the read intact.
    bool TryU32(std::uint32_t address, std::uint32_t& value) noexcept
    {
        value = 0;
        return !faulted_ && copy_(context_, address, &value, sizeof(value));
    }

    bool Bytes(std::uint32_t address, void* output, std::size_t bytes) noexcept
    {
        if (!faulted_ && !copy_(context_, address, output, bytes))
            faulted_ = true;
        if (faulted_)
            std::memset(output, 0, bytes);
        return !faulted_;
    }

    std::uint32_t U32(std::uint32_t address) noexcept
    {
        std::uint32_t value = 0;
        Bytes(address, &value, sizeof(value));
        return value;
    }

    std::uint16_t U16(std::uint32_t address) noexcept
    {
        std::uint16_t value = 0;
        Bytes(address, &value, sizeof(value));
        return value;
    }

    std::uint8_t U8(std::uint32_t address) noexcept
    {
        std::uint8_t value = 0;
        Bytes(address, &value, sizeof(value));
        return value;
    }

private:
    ReadMemory  copy_;
    void*       context_;
    bool        faulted_ = false;
    std::size_t visits_  = 0;
};

enum class Walk
{
    Done,
    Stopped,
    Failed
};

// MSVC std::map: head at map+4, sentinel byte at nilOffset; false stops the visitor.
template <typename Visit>
Walk ForEachNode(Memory& memory, std::uint32_t map, std::uint32_t nilOffset, Visit&& visit) noexcept
{
    if (map == 0)
        return Walk::Done;
    const std::uint32_t head = memory.U32(map + 4);
    if (memory.Faulted() || head == 0)
        return Walk::Failed;
    std::array<std::uint32_t, kMaxStack> stack{};
    std::size_t                          size = 0;
    stack[size++]                             = memory.U32(head + 4);
    while (size != 0)
    {
        const std::uint32_t node = stack[--size];
        if (node == head)
            continue;
        if (node == 0 || memory.U8(node + nilOffset) != 0 || memory.Faulted() || !memory.Visit())
            return Walk::Failed;
        if (!visit(node))
            return Walk::Stopped;
        const std::uint32_t left  = memory.U32(node);
        const std::uint32_t right = memory.U32(node + 8);
        if (memory.Faulted())
            return Walk::Failed;
        for (const std::uint32_t child : { left, right })
        {
            if (child == head)
                continue;
            if (size == stack.size())
                return Walk::Failed;
            stack[size++] = child;
        }
    }
    return Walk::Done;
}

// Complete-object RTTI: the locator at table-4 has offset 0 and names type.
bool HasType(Memory& memory, std::uint32_t object, std::uint32_t type) noexcept
{
    if (object == 0)
        return false;
    const std::uint32_t table = memory.U32(object);
    if (table < 4 || memory.Faulted())
        return false;
    const std::uint32_t locator = memory.U32(table - 4);
    return locator != 0 && memory.U32(locator + 4) == 0 && memory.U32(locator + 0x0c) == type && !memory.Faulted();
}

// Native string: pointer +0, inline +0x12 up to capacity 0x40, NUL-inclusive length +8 (0x00447260, 0x004451F0).
// Lengths without the NUL are also accepted.
bool StringEquals(Memory& memory, std::uint32_t string, std::string_view text) noexcept
{
    const std::uint32_t characters = memory.U32(string);
    const std::uint32_t length     = memory.U32(string + 8);
    if (memory.Faulted() || characters == 0 || length > kMaxName || (length != text.size() && length != text.size() + 1))
        return false;
    std::array<char, kMaxName> buffer{};
    if (!memory.Bytes(characters, buffer.data(), length))
        return false;
    return (length == text.size() || buffer[text.size()] == '\0') && std::memcmp(buffer.data(), text.data(), text.size()) == 0;
}

// Interned-name tree: pool string pointer at node+0x10, value at node+0x1C
// (0x00CEDEDD, 0x00CD2A9E, 0x00D1F85D). Pool-list strings sit at list
// node+8 with the refcount at string+0x54. Returns 0 unless exactly one matches.
std::uint32_t FindInterned(Memory& memory, std::uint32_t map, std::uint32_t nilOffset, std::string_view name) noexcept
{
    std::uint32_t value   = 0;
    std::uint32_t matches = 0;
    auto          visit   = [&](std::uint32_t node)
    {
        if (StringEquals(memory, memory.U32(node + 0x10), name))
        {
            ++matches;
            value = memory.U32(node + 0x1c);
        }
        return !memory.Faulted();
    };
    const bool done = ForEachNode(memory, map, nilOffset, visit) == Walk::Done;
    return done && matches == 1 ? value : 0;
}

// Name tree X+4: inline name node+0x0C, element node+0x60; incomplete walks cannot establish uniqueness.
std::uint32_t FindName(Memory& memory, std::uint32_t container, std::string_view name, std::uint32_t& matches, bool& complete) noexcept
{
    std::uint32_t found = 0;
    matches             = 0;
    auto visit          = [&](std::uint32_t node)
    {
        if (StringEquals(memory, node + 0x0c, name))
        {
            ++matches;
            found = memory.U32(node + 0x60);
        }
        return !memory.Faulted();
    };
    complete = ForEachNode(memory, container + 4, 0x65, visit) == Walk::Done;
    return complete ? found : 0;
}

// Avoid the client getter: it creates storage on a miss.
bool ReadAttached(Memory& memory, std::uint32_t context, const AttachedProperty& property, std::uint32_t control, std::uint32_t& value, std::uint32_t& stored) noexcept
{
    const std::uint32_t manager = context + property.managerOffset;
    if (memory.U32(manager) != kPropertyManagerTable || memory.U32(manager + 0x44) != property.handlers ||
        memory.U32(manager + 0x1c) != memory.U32(property.descriptor + 8) || memory.Faulted())
        return false;
    std::uint32_t storage = 0;
    std::uint32_t matches = 0;
    auto          visit   = [&](std::uint32_t node)
    {
        if (memory.U32(node + 0x0c) == control)
        {
            ++matches;
            storage = memory.U32(node + 0x10);
        }
        return !memory.Faulted();
    };
    if (ForEachNode(memory, manager + 4, 0x15, visit) != Walk::Done || matches > 1)
        return false;
    value  = 0;
    stored = matches;
    if (matches == 0)
        return true;
    if (memory.U32(storage) != kPropertyStorageTable || memory.U32(storage + 0x2c) != control ||
        memory.U32(storage + 0x30) != property.descriptor || (memory.U8(storage + 0x20) & 4) != 0)
        return false;
    value = memory.U32(storage + 0x24);
    return !memory.Faulted();
}

std::string_view Numbered(std::array<char, kMaxName>& buffer, std::string_view stem, std::uint32_t number) noexcept
{
    std::memcpy(buffer.data(), stem.data(), stem.size());
    const auto result = std::to_chars(buffer.data() + stem.size(), buffer.data() + buffer.size(), number);
    return std::string_view(buffer.data(), static_cast<std::size_t>(result.ptr - buffer.data()));
}

bool InRange(std::uint32_t begin, std::uint32_t end, std::uint32_t stride, std::uint32_t index) noexcept
{
    return begin != 0 && end >= begin && (end - begin) / stride > index;
}

struct WindowSearch
{
    std::uint32_t matches  = 0;
    std::uint32_t element  = 0;
    std::uint32_t space    = 0;
    std::uint32_t window   = 0;
    bool          complete = false;
};

// The named window must be unique across all FormElement namespace containers.
WindowSearch FindWindow(Memory& memory, std::uint32_t mainModule, std::string_view name) noexcept
{
    WindowSearch  search;
    bool          innerError = false;
    std::uint32_t candidate  = 0;
    auto          visitSpace = [&](std::uint32_t spaceNode)
    {
        const std::uint32_t container = memory.U32(spaceNode + 0x10);
        if (!HasType(memory, container, kNamespaceType))
            return !memory.Faulted();
        std::uint32_t       matches  = 0;
        bool                complete = false;
        const std::uint32_t found    = FindName(memory, container, name, matches, complete);
        if (!complete)
            return false;
        if (matches != 0 && search.matches == 0)
        {
            search.element = candidate;
            search.space   = container;
            search.window  = found;
        }
        search.matches += matches;
        return true;
    };
    auto visitElement = [&](std::uint32_t node)
    {
        candidate = memory.U32(node + 0x10);
        if (candidate == 0 || memory.U32(candidate + 0x88) != memory.U32(node + 0x0c) || !HasType(memory, candidate, kFormElementType))
            return !memory.Faulted();
        innerError = ForEachNode(memory, memory.U32(candidate + 0xd4), 0x15, visitSpace) != Walk::Done;
        return !innerError;
    };
    const Walk elements = ForEachNode(memory, mainModule + kElementMapOffset, 0x15, visitElement);
    search.complete     = elements == Walk::Done && !innerError && !memory.Faulted();
    return search;
}

struct WorkSearch
{
    std::uint32_t work         = 0;
    std::uint32_t workOperator = 0;
};

// A nonzero controlTable requires that vftable instead of complete type controlType.
bool FindWork(Memory& memory, std::uint32_t element, std::uint32_t controlType, std::string_view workClass, WorkSearch& search, std::uint32_t controlTable = 0) noexcept
{
    // FormElement actor: [[E+0x80]]; LuaActorImpl control: +8.
    const std::uint32_t holder = memory.U32(element + 0x80);
    const std::uint32_t actor  = holder == 0 ? 0 : memory.U32(holder);
    if (!HasType(memory, actor, kLuaActorType))
        return false;
    const std::uint32_t control = memory.U32(actor + 8);
    if (controlTable != 0 ? control == 0 || memory.U32(control) != controlTable || memory.Faulted() : !HasType(memory, control, controlType))
        return false;

    const std::uint32_t record   = memory.U32(control + 4);
    const std::uint32_t userdata = record == 0 ? 0 : memory.U32(record + 4);
    if (record == 0 || memory.U32(record) != control || userdata == 0 || memory.U32(userdata) != record || memory.Faulted())
        return false;

    const std::uint32_t handle = memory.U32(record + 0x14);
    search.workOperator        = handle == 0 ? 0 : memory.U32(handle);
    // Work is keyed by the instance's Lua class [[record+0x0C]+0x50] (0x00D1CF41-0x00D1CF73,
    // 0x00CC6FCB-0x00CC6FF4, 0x00D1D4E7-0x00D1D4FE -> 0x00D20B3D).
    if (search.workOperator != 0 && !memory.Faulted())
        search.work = FindInterned(memory, search.workOperator + 0x1c, 0x21, workClass);
    return search.work != 0 && !memory.Faulted();
}

struct WorkBuffer
{
    std::uint32_t base   = 0;
    std::uint32_t size   = 0;
    bool          simple = false;
};

bool FindBuffer(Memory& memory, std::uint32_t work, std::uint32_t workOperator, WorkBuffer& out) noexcept
{
    const std::int8_t   group      = static_cast<std::int8_t>(memory.U8(work + 0x18));
    const std::uint32_t scopeBegin = memory.U32(workOperator + 0x2c);
    const std::uint32_t scopeEnd   = memory.U32(workOperator + 0x30);
    if (group < 0 || !InRange(scopeBegin, scopeEnd, 0x10, kWorkScope))
        return false;
    const std::uint32_t scope      = scopeBegin + 0x10 * kWorkScope;
    const std::uint32_t groupBegin = memory.U32(scope + 4);
    const std::uint32_t groupEnd   = memory.U32(scope + 8);
    if (!InRange(groupBegin, groupEnd, 4, static_cast<std::uint32_t>(group)))
        return false;
    const std::uint32_t groupBase      = memory.U32(groupBegin + 4 * static_cast<std::uint32_t>(group));
    const std::uint32_t allocator      = memory.U32(workOperator + 4);
    const std::uint32_t containerBegin = allocator == 0 ? 0 : memory.U32(allocator + 0x0c);
    const std::uint32_t containerEnd   = allocator == 0 ? 0 : memory.U32(allocator + 0x10);
    if (!InRange(containerBegin, containerEnd, 4, kWorkScope))
        return false;
    const std::uint32_t container = memory.U32(containerBegin + 4 * kWorkScope);
    const std::uint32_t access    = container == 0 ? 0 : memory.U32(container + 8);
    const std::uint32_t accessTag = access == 0 ? 0 : memory.U32(access);
    // Direct reads require a supported container/access pair.
    const bool          supported = container != 0 && memory.U32(container) == kSingleThreadContainer &&
                                    (accessTag == kSimpleAccess ||
                                     (accessTag == kSharedTempAccess && memory.U16(access + 0x0c) == memory.U16(kSharedTempLocalTag)));
    const std::uint32_t buffer    = supported ? memory.U32(container + 0x1c) : 0;
    const std::uint32_t end       = supported ? memory.U32(container + 0x20) : 0;
    if (!supported || buffer == 0 || end <= buffer || groupBase >= end - buffer || memory.Faulted())
        return false;
    out.base   = buffer + groupBase;
    out.size   = end - buffer - groupBase;
    out.simple = accessTag == kSimpleAccess;
    return true;
}

bool ByteAt(Memory& memory, const WorkBuffer& buffer, std::uint32_t bit, std::uint8_t& value) noexcept
{
    if (bit / 8 >= buffer.size)
        return false;
    value = memory.U8(buffer.base + bit / 8);
    return !memory.Faulted();
}

bool ReadInteger8(Memory& memory, const WorkBuffer& buffer, std::uint32_t bit, std::int32_t& value) noexcept
{
    std::uint8_t byte = 0;
    if (!ByteAt(memory, buffer, bit, byte))
        return false;
    value = static_cast<std::int8_t>(byte);
    return true;
}

// Template [[C+0x318]+0xB0] (0x00936DF0), children +0x264..+0x268 (0x009313C0).
// TimerIcon IconControl_Command_2: IconDatas vector +0x2B0..+0x2B4, first int is icon ID (0x0096A040, 0x009170A0).
// Non-poisoning TryU32 reads and a local node budget isolate icon failures from command reads.
std::uint32_t ReadIcon(Memory& memory, std::uint32_t label) noexcept
{
    constexpr std::size_t kMaxChildren = 64;
    constexpr std::size_t kMaxNodes    = 256;
    std::size_t           visits       = 0;
    std::uint32_t         templ        = 0;
    std::uint32_t         node         = 0;
    if (!memory.TryU32(label + 0x318, templ) || templ == 0 || !memory.TryU32(templ + 0xb0, node) || node == 0)
        return 0;
    std::array<std::uint32_t, kMaxStack> stack{};
    std::size_t                          size    = 0;
    std::uint32_t                        icon    = 0;
    std::uint32_t                        matches = 0;
    stack[size++]                                = node;
    while (size != 0)
    {
        node                = stack[--size];
        std::uint32_t table = 0;
        if (node == 0 || ++visits > kMaxNodes || !memory.TryU32(node, table))
            return 0;
        if (table == kTimerIconTable)
        {
            std::uint32_t begin = 0;
            std::uint32_t end   = 0;
            if (!memory.TryU32(node + 0x2b0, begin) || !memory.TryU32(node + 0x2b4, end))
                return 0;
            ++matches;
            if (begin == 0 || end <= begin || !memory.TryU32(begin, icon))
                icon = 0;
            continue;
        }
        std::uint32_t begin = 0;
        std::uint32_t end   = 0;
        if (!memory.TryU32(node + 0x264, begin) || !memory.TryU32(node + 0x268, end))
            return 0;
        if (begin == 0 && end == 0)
            continue;
        if (begin == 0 || end < begin || (end - begin) % 4 != 0 || (end - begin) / 4 > kMaxChildren || (end - begin) / 4 > stack.size() - size)
            return 0;
        for (std::uint32_t child = begin; child != end; child += 4)
            if (!memory.TryU32(child, stack[size++]))
                return 0;
    }
    return matches == 1 && icon < 100000 ? icon : 0;
}

std::int32_t ReadLabel(Memory& memory, std::uint32_t space, std::uint32_t sqwt, std::uint32_t slot, std::uint32_t& icon) noexcept
{
    icon = 0;
    std::array<char, kMaxName> name{};
    std::uint32_t              matches  = 0;
    bool                       complete = false;
    std::uint32_t              value    = 0;
    std::uint32_t              stored   = 0;
    const std::uint32_t        label    = FindName(memory, space, Numbered(name, kLabelStem, slot), matches, complete);
    if (!complete || matches != 1 || label == 0 || !ReadAttached(memory, sqwt, kValue0, label, value, stored))
        return 0;
    // removeSlot zeroes Value0, so empty slots have no icon.
    if (value != 0)
        icon = ReadIcon(memory, label);
    return static_cast<std::int32_t>(value);
}

// Main [0x013232B8]: NetworkModule +0x94, MainModule +0x98; server seconds N+0x30C (0x0075BD80).
constexpr std::uint32_t kMainGlobal      = 0x013232b8;
constexpr std::uint32_t kMainTable       = 0x00f54a24;
constexpr std::uint32_t kMainModuleTable = 0x00f9142c;
constexpr std::uint32_t kNetworkTable    = 0x00f91b5c;
constexpr std::uint32_t kMyPlayerTable   = 0x00fd785c;
constexpr std::uint32_t kSyncContainer   = 0x0110ef34;
constexpr std::uint32_t kStructTable     = 0x0110f974;
constexpr std::uint32_t kSyncKind        = 2;
constexpr std::uint32_t kSyncScope       = 0x68;

constexpr std::string_view kParameterSave = "parameterSave";
constexpr std::string_view kParameterTemp = "parameterTemp";

// Binder declaration order:
// parameterSave.commandSlot_recastTime int32[40], parameterSave.commandSlot_compatibility boolean[40],
// parameterTemp.maxCommandRecastTime int16[40].
constexpr std::uint32_t kRecastBits        = 368;
constexpr std::uint32_t kCompatibilityBits = 1648;
constexpr std::uint32_t kMaxRecastBits     = 48;

struct SyncField
{
    std::uint32_t work  = 0;
    std::uint32_t start = 0; // bits into the work's sync group
};

bool FindSyncField(Memory& memory, std::uint32_t shared, std::string_view name, SyncField& out) noexcept
{
    std::uint32_t matches = 0;
    auto          visit   = [&](std::uint32_t node)
    {
        const std::uint32_t work         = memory.U32(node + 0x1c);
        const std::uint32_t declarations = work == 0 ? 0 : memory.U32(work + kSyncScope);
        const std::uint32_t descriptor   = declarations == 0 ? 0 : FindInterned(memory, declarations + 8, 0x25, name);
        if (descriptor != 0)
        {
            ++matches;
            out.work  = work;
            out.start = memory.U32(descriptor + 8);
            if (memory.U32(descriptor) != kStructTable)
                out.work = 0;
        }
        return !memory.Faulted();
    };
    return ForEachNode(memory, shared + 0x1c, 0x21, visit) == Walk::Done && matches == 1 && out.work != 0 && !memory.Faulted();
}

// Kind 2: shared scope vector (0x00CE4550), operator pool vector (0x00CCB430); direct bytes require SyncContainer with SimpleAccess.
bool FindSyncBuffer(Memory& memory, std::uint32_t shared, std::uint32_t work, WorkBuffer& out) noexcept
{
    const std::int8_t   group      = static_cast<std::int8_t>(memory.U8(work + 0x18));
    const std::uint32_t scopeBegin = memory.U32(shared + 0x2c);
    const std::uint32_t scopeEnd   = memory.U32(shared + 0x30);
    if (group < 0 || !InRange(scopeBegin, scopeEnd, 0x10, kSyncKind))
        return false;
    const std::uint32_t scope      = scopeBegin + 0x10 * kSyncKind;
    const std::uint32_t groupBegin = memory.U32(scope + 4);
    const std::uint32_t groupEnd   = memory.U32(scope + 8);
    if (!InRange(groupBegin, groupEnd, 4, static_cast<std::uint32_t>(group)))
        return false;
    const std::uint32_t groupBase      = memory.U32(groupBegin + 4 * static_cast<std::uint32_t>(group));
    const std::uint32_t allocator      = memory.U32(shared + 4);
    const std::uint32_t containerBegin = allocator == 0 ? 0 : memory.U32(allocator + 0x0c);
    const std::uint32_t containerEnd   = allocator == 0 ? 0 : memory.U32(allocator + 0x10);
    if (!InRange(containerBegin, containerEnd, 4, kSyncKind))
        return false;
    const std::uint32_t container = memory.U32(containerBegin + 4 * kSyncKind);
    const std::uint32_t access    = container == 0 ? 0 : memory.U32(container + 8);
    const bool          supported = container != 0 && memory.U32(container) == kSyncContainer && access != 0 && memory.U32(access) == kSimpleAccess;
    const std::uint32_t buffer    = supported ? memory.U32(container + 0x1c) : 0;
    const std::uint32_t end       = supported ? memory.U32(container + 0x20) : 0;
    if (!supported || buffer == 0 || end <= buffer || groupBase >= end - buffer || memory.Faulted())
        return false;
    out.base   = buffer + groupBase;
    out.size   = end - buffer - groupBase;
    out.simple = true;
    return true;
}

template <typename T>
bool ReadAligned(Memory& memory, const WorkBuffer& buffer, std::uint32_t bit, std::int32_t& value) noexcept
{
    T raw{};
    if ((bit & 7) != 0 || bit / 8 > buffer.size || sizeof(T) > buffer.size - bit / 8 || !memory.Bytes(buffer.base + bit / 8, &raw, sizeof(raw)))
        return false;
    value = static_cast<std::int32_t>(raw);
    return true;
}

// Accept the clock only when Main's MainModule is this read's receiver.
bool ReadClock(Memory& memory, std::uint32_t mainModule, std::int32_t& now) noexcept
{
    const std::uint32_t main    = memory.U32(kMainGlobal);
    const std::uint32_t network = main == 0 || memory.U32(main) != kMainTable ? 0 : memory.U32(main + 0x94);
    if (network == 0 || memory.U32(main + 0x98) != mainModule || memory.U32(mainModule) != kMainModuleTable ||
        memory.U32(network) != kNetworkTable || memory.U32(network + 8) != mainModule || memory.Faulted())
        return false;
    now = static_cast<std::int32_t>(memory.U32(network + 0x30c));
    return !memory.Faulted();
}

// ActionMenu LuaActorImpl I -> GameEngine [I+4] -> player [[[G+0x220]+4]+0x0C] (0x006F564C, _getMyPlayer 0x00707970).
// Player record (0x00CD7A30) -> shared work [[R+0x14]].
void ReadStates(Memory& memory, std::uint32_t mainModule, std::uint32_t element, SlotStates& states) noexcept
{
    std::int32_t now = 0;
    if (!ReadClock(memory, mainModule, now))
        return;
    const std::uint32_t holder = memory.U32(element + 0x80);
    const std::uint32_t actor  = holder == 0 ? 0 : memory.U32(holder);
    if (!HasType(memory, actor, kLuaActorType))
        return;
    const std::uint32_t engine = memory.U32(actor + 4);
    const std::uint32_t outer  = engine == 0 ? 0 : memory.U32(engine + 0x220);
    const std::uint32_t inner  = outer == 0 ? 0 : memory.U32(outer + 4);
    const std::uint32_t player = inner == 0 ? 0 : memory.U32(inner + 0x0c);
    if (player == 0 || memory.U32(player) != kMyPlayerTable || memory.Faulted())
        return;
    const std::uint32_t first  = memory.U32(engine + 0x1bc);
    const std::uint32_t record = first != 0 && memory.U32(first) == player ? first : memory.U32(player + 4);
    const std::uint32_t link   = record == 0 ? 0 : memory.U32(record + 0x14);
    const std::uint32_t shared = link == 0 ? 0 : memory.U32(link);
    if (shared == 0 || memory.Faulted())
        return;

    SyncField  save;
    SyncField  temp;
    WorkBuffer buffer;
    if (!FindSyncField(memory, shared, kParameterSave, save) || !FindSyncField(memory, shared, kParameterTemp, temp) || temp.work != save.work ||
        !FindSyncBuffer(memory, shared, save.work, buffer))
        return;
    for (std::uint32_t slot = 0; slot < states.remaining.size(); ++slot)
    {
        std::int32_t        end           = 0;
        std::int32_t        limit         = 0;
        std::uint8_t        byte          = 0;
        const std::uint32_t compatibility = save.start + kCompatibilityBits + slot;
        if (!ReadAligned<std::int32_t>(memory, buffer, save.start + kRecastBits + 32 * slot, end) ||
            !ReadAligned<std::int16_t>(memory, buffer, temp.start + kMaxRecastBits + 16 * slot, limit) || !ByteAt(memory, buffer, compatibility, byte))
            return;
        states.remaining[slot]  = RecastRemaining(end, now, limit);
        states.compatible[slot] = ((byte >> (compatibility & 7)) & 1) != 0;
    }
    states.shared = shared;
    states.now    = now;
    states.valid  = !memory.Faulted();
}

std::uint32_t FindControlElement(Memory& memory, std::uint32_t mainModule, std::uint32_t table) noexcept
{
    std::uint32_t found   = 0;
    std::uint32_t matches = 0;
    auto          visit   = [&](std::uint32_t node)
    {
        const std::uint32_t element = memory.U32(node + 0x10);
        if (element == 0 || memory.U32(element + 0x88) != memory.U32(node + 0x0c) || !HasType(memory, element, kFormElementType))
            return !memory.Faulted();
        const std::uint32_t holder = memory.U32(element + 0x80);
        const std::uint32_t actor  = holder == 0 ? 0 : memory.U32(holder);
        if (!HasType(memory, actor, kLuaActorType))
            return !memory.Faulted();
        const std::uint32_t control = memory.U32(actor + 8);
        if (control != 0 && memory.U32(control) == table)
        {
            ++matches;
            found = element;
        }
        return !memory.Faulted();
    };
    const bool done = ForEachNode(memory, mainModule + kElementMapOffset, 0x15, visit) == Walk::Done;
    return done && matches == 1 && !memory.Faulted() ? found : 0;
}

struct Registered
{
    std::uint32_t element  = 0;
    std::uint32_t record   = 0;
    std::uint32_t identity = 0;
};

// Lua class: Utf8String at [[R+0x0C]+0x54] when [[R+0x0C]+0x50] is set.
bool IsMap(Memory& memory, std::uint32_t record) noexcept
{
    const std::uint32_t prototype = memory.U32(record + 0x0c);
    if (prototype == 0 || memory.Faulted() || memory.U32(prototype + 0x50) == 0)
        return false;
    const std::uint32_t name = memory.U32(prototype + 0x54);
    return name != 0 && !memory.Faulted() && StringEquals(memory, name, kMapClass);
}

bool MapOpen(Memory& memory, std::uint32_t mainModule, std::uint32_t declarations, const WorkBuffer& buffer) noexcept
{
    const std::uint32_t array = FindInterned(memory, declarations + 8, 0x25, kRootWidget);
    if (array == 0 || memory.U32(array) != kArrayTable || memory.U32(array + 0x10) != kRootWidgets)
        return false;
    const std::uint32_t                     actor = memory.U32(array + 0x14);
    const std::uint32_t                     start = memory.U32(array + 8);
    std::array<std::uint32_t, kRootWidgets> roots{};
    if (actor == 0 || memory.Faulted() || memory.U32(actor) != kActorTable || memory.U32(actor + 8) != start || (start & 7) != 0 ||
        start / 8 > buffer.size || sizeof(roots) > buffer.size - start / 8 || !memory.Bytes(buffer.base + start / 8, roots.data(), sizeof(roots)))
        return false;

    std::array<Registered, kMaxRegistered> registry{};
    std::size_t                            count = 0;
    auto                                   visit = [&](std::uint32_t node)
    {
        const std::uint32_t element = memory.U32(node + 0x10);
        if (element == 0 || memory.U32(element + 0x88) != memory.U32(node + 0x0c) || !HasType(memory, element, kFormElementType))
            return !memory.Faulted();
        const std::uint32_t holder = memory.U32(element + 0x80);
        const std::uint32_t bound  = holder == 0 ? 0 : memory.U32(holder);
        if (!HasType(memory, bound, kLuaActorType))
            return !memory.Faulted();
        const std::uint32_t control  = memory.U32(bound + 8);
        const std::uint32_t record   = control == 0 ? 0 : memory.U32(control + 4);
        const std::uint32_t userdata = record == 0 ? 0 : memory.U32(record + 4);
        if (record == 0 || memory.U32(record) != control || userdata == 0 || memory.U32(userdata) != record)
            return !memory.Faulted();
        if (count == registry.size())
            return false;
        registry[count++] = { element, record, memory.U32(record + 0x70) };
        return !memory.Faulted();
    };
    if (ForEachNode(memory, mainModule + kElementMapOffset, 0x15, visit) != Walk::Done || memory.Faulted())
        return false;

    struct Step
    {
        const Registered* entry;
        std::uint32_t     depth;
    };

    std::array<Step, kMaxMapWalk> queue{};
    std::size_t                   head = 0, tail = 0;
    for (const std::uint32_t identity : roots)
    {
        const Registered* match   = nullptr;
        std::uint32_t     matches = 0;
        for (std::size_t index = 0; identity != 0 && index < count; ++index)
            if (registry[index].identity == identity)
            {
                match = &registry[index];
                ++matches;
            }
        if (matches == 1)
            queue[tail++] = { match, 0 };
    }
    while (head < tail)
    {
        const Step step = queue[head++];
        if (IsMap(memory, step.entry->record))
            return !memory.Faulted();
        const std::uint32_t begin = memory.U32(step.entry->element + 0xe0);
        const std::uint32_t end   = memory.U32(step.entry->element + 0xe4);
        if (memory.Faulted() || (begin == 0) != (end == 0) || end < begin || (end - begin) % 4 != 0 || (end - begin) / 4 > kMaxMapChildren)
            return false;
        for (std::uint32_t slot = begin; step.depth < kMaxMapDepth && slot < end; slot += 4)
        {
            const std::uint32_t child = memory.U32(slot);
            if (memory.Faulted())
                return false;
            for (std::size_t index = 0; child != 0 && index < count; ++index)
                if (registry[index].element == child)
                {
                    if (tail == queue.size())
                        return false;
                    queue[tail++] = { &registry[index], step.depth + 1 };
                    break;
                }
        }
    }
    return false;
}

} // namespace

DesktopState ReadDesktop(std::uint32_t mainModule, ReadMemory copy, void* context) noexcept
{
    DesktopState state;
    if (mainModule == 0 || copy == nullptr)
        return state;
    Memory              memory(copy, context);
    const std::uint32_t element = FindControlElement(memory, mainModule, kDesktopControlTable);
    WorkSearch          work;
    if (element == 0 || !FindWork(memory, element, 0, kDesktopClass, work, kDesktopControlTable))
        return state;
    // Temp-scope fields: desktopwidget_connector.lua work._temp.
    const std::uint32_t declarations = memory.U32(work.work + 0x64);
    if (declarations == 0 || memory.Faulted())
        return state;
    const std::uint32_t modeField = FindInterned(memory, declarations + 8, 0x25, kDesktopMode);
    const std::uint32_t lockField = FindInterned(memory, declarations + 8, 0x25, kLockUserControl);
    if (modeField == 0 || lockField == 0 || memory.U32(modeField) != kInteger8Table || memory.U32(lockField) != kBooleanTable)
        return state;
    const std::uint32_t modeBit = memory.U32(modeField + 8);
    const std::uint32_t lockBit = memory.U32(lockField + 8);
    if ((modeBit & 7) != 0 || memory.Faulted())
        return state;
    WorkBuffer   buffer;
    std::uint8_t lock = 0;
    if (!FindBuffer(memory, work.work, work.workOperator, buffer) || !ReadInteger8(memory, buffer, modeBit, state.mode) || !ByteAt(memory, buffer, lockBit, lock))
        return state;
    state.locked = ((lock >> (lockBit & 7)) & 1) != 0;
    state.valid  = !memory.Faulted();
    state.map    = state.valid && MapOpen(memory, mainModule, declarations, buffer);
    return state;
}

std::int32_t RecastRemaining(std::int32_t end, std::int32_t now, std::int32_t limit) noexcept
{
    std::int64_t left = static_cast<std::int64_t>(end) - now;
    if (limit > 0 && limit < left)
        left = limit;
    return static_cast<std::int32_t>(std::clamp<std::int64_t>(left, 0, std::numeric_limits<std::int32_t>::max()));
}

SlotCommands ReadSlotCommands(std::uint32_t mainModule, std::uint32_t tlsArray, ReadMemory copy, void* context, bool labels, SlotStates* states) noexcept
{
    SlotCommands result;
    if (states != nullptr)
        *states = {};
    if (mainModule == 0 || copy == nullptr)
        return result;
    Memory memory(copy, context);

    const WindowSearch window = FindWindow(memory, mainModule, kWindowName);
    if (!window.complete || window.matches != 1 || !HasType(memory, window.window, kWindowType))
        return result;
    result.window = window.window;
    WorkSearch work;
    if (!FindWork(memory, window.element, kWidgetBaseType, kWorkClass, work))
        return result;

    // Only temp scope; other client scopes at [W+0x60]/[W+0x68] are unsupported (0x00D16CD0).
    const std::uint32_t declarations = memory.U32(work.work + 0x64);
    if (declarations == 0 || memory.Faulted())
        return result;
    const std::uint32_t pageField   = FindInterned(memory, declarations + 8, 0x25, kSlotPage);
    const std::uint32_t macroField  = FindInterned(memory, declarations + 8, 0x25, kMacroType);
    const std::uint32_t movingField = FindInterned(memory, declarations + 8, 0x25, kSlotMoving);
    if (pageField == 0 || macroField == 0 || memory.U32(pageField) != kInteger8Table || memory.U32(macroField) != kInteger8Table)
        return result;
    const std::uint32_t pageBit  = memory.U32(pageField + 8);
    const std::uint32_t macroBit = memory.U32(macroField + 8);
    // integer8 rounds partial bytes up (0x00D11C79-0x00D11C8E); this reader requires byte alignment.
    if ((pageBit & 7) != 0 || (macroBit & 7) != 0 || memory.Faulted())
        return result;

    WorkBuffer buffer;
    if (!FindBuffer(memory, work.work, work.workOperator, buffer))
        return result;
    std::int32_t page      = 0;
    std::int32_t macroType = 0;
    if (!ReadInteger8(memory, buffer, pageBit, page) || !ReadInteger8(memory, buffer, macroBit, macroType))
        return result;

    // Sqwt lives in the calling thread's client TLS slot.
    const std::uint32_t tlsIndex = memory.U32(kSqwtTlsIndex);
    const std::uint32_t tlsSlot  = tlsArray == 0 ? 0 : memory.U32(tlsArray + 4 * tlsIndex);
    const std::uint32_t sqwt     = tlsSlot == 0 ? 0 : memory.U32(tlsSlot + 0x1c);
    if (sqwt == 0 || memory.Faulted())
        return result;

    result.valid     = true;
    result.page      = page;
    result.macroType = macroType;
    // Boolean: bit (position & 7), byte position / 8 (0x00D11C00-0x00D11C25, 0x00CFDFC0-0x00CFE015).
    std::uint8_t moving = 0;
    if (movingField != 0 && memory.U32(movingField) == kBooleanTable && !memory.Faulted())
    {
        const std::uint32_t movingBit = memory.U32(movingField + 8);
        if (!memory.Faulted() && ByteAt(memory, buffer, movingBit, moving))
            result.slotMoving = ((moving >> (movingBit & 7)) & 1) != 0;
    }
    // Only SimpleAccess is writable (0x00CFE020); slotMaxPage is 3 from init (actionmenuwidget.lua:195).
    if (buffer.simple && page >= 1 && page <= 3 && !memory.Faulted())
        result.pageAddress = buffer.base + pageBit / 8;
    if (states != nullptr)
    {
        // Separate budget and fault state isolate player-work failures from the menu read.
        Memory player(copy, context);
        ReadStates(player, mainModule, window.element, *states);
    }
    if (!labels || macroType != 0)
        return result;
    for (std::uint32_t slot = 1; slot <= result.commands.size(); ++slot)
        result.commands[slot - 1] = ReadLabel(memory, window.space, sqwt, slot, result.icons[slot - 1]);
    return result;
}

} // namespace bahamut_client::xhbar
