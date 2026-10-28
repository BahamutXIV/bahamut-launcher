#pragma once

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <deque>
#include <vector>

namespace equipment_memory_fixture
{

// Synthetic store memory in the layout equipment_state.cpp documents. These
// tests prove the reader against that layout only; they do not establish
// that a running client uses it.
constexpr std::uint32_t kUnset             = 0xFFFFFFFFu;
constexpr std::size_t   kPlayerStoreOffset = 0xD8u;

class Memory
{
public:
    std::uint32_t Alloc(std::size_t size)
    {
        blocks_.emplace_back(size, std::uint8_t{ 0 });
        return Address(blocks_.back().data());
    }

    static std::uint32_t Address(const void* pointer)
    {
        return static_cast<std::uint32_t>(reinterpret_cast<std::uintptr_t>(pointer));
    }

    static void Put32(std::uint32_t address, std::size_t offset, std::uint32_t value)
    {
        std::memcpy(reinterpret_cast<void*>(static_cast<std::uintptr_t>(address + offset)), &value, sizeof(value));
    }

    static void Put8(std::uint32_t address, std::size_t offset, std::uint8_t value)
    {
        *reinterpret_cast<std::uint8_t*>(static_cast<std::uintptr_t>(address + offset)) = value;
    }

private:
    std::deque<std::vector<std::uint8_t>> blocks_;
};

struct Package
{
    std::uint32_t              object = 0;
    std::uint32_t              vector = 0;
    std::vector<std::uint32_t> entries;
};

struct Fixture
{
    Memory        memory;
    std::uint32_t player = 0;
    std::uint32_t store  = 0;
    std::uint32_t head   = 0;
    Package       equipment;
    Package       bag;

    // Builds the map with the bag package as root and the equipment package as
    // its right child, which satisfies the ordering the reader walks.
    explicit Fixture(std::size_t equipmentCount = 35, std::size_t bagCount = 16, bool includeBag = true)
    {
        player = memory.Alloc(0x100);
        store  = memory.Alloc(0x20);
        head   = memory.Alloc(0x20);
        Memory::Put32(player, kPlayerStoreOffset, store);
        Memory::Put32(store, 0x0C, head); // map at store+8, head at map+4
        Memory::Put8(head, 0x15, 1);

        equipment                         = MakePackage(equipmentCount);
        bag                               = MakePackage(bagCount);
        const std::uint32_t equipmentNode = MakeNode(0xFE, equipment.object);
        const std::uint32_t bagNode       = MakeNode(0, bag.object);
        Memory::Put32(head, 4, includeBag ? bagNode : equipmentNode);
        Memory::Put32(bagNode, 0, head);
        Memory::Put32(bagNode, 8, equipmentNode);
        Memory::Put32(equipmentNode, 0, head);
        Memory::Put32(equipmentNode, 8, head);
        if (!includeBag)
        {
            Memory::Put32(equipmentNode, 4, head);
        }
    }

    Package MakePackage(std::size_t count)
    {
        Package package;
        package.object = memory.Alloc(0x20);
        package.vector = memory.Alloc(std::max<std::size_t>(count, 1) * 4);
        package.entries.assign(count, 0);
        Memory::Put32(package.object, 0x14, package.vector);
        Memory::Put32(package.object, 0x18, package.vector + static_cast<std::uint32_t>(count * 4));
        return package;
    }

    std::uint32_t MakeNode(std::uint32_t key, std::uint32_t value)
    {
        const std::uint32_t node = memory.Alloc(0x20);
        Memory::Put32(node, 0x0C, key);
        Memory::Put32(node, 0x10, value);
        return node;
    }

    std::uint32_t MakeItem(std::uint64_t uid, std::uint32_t catalog)
    {
        const std::uint32_t item = memory.Alloc(0x90);
        Memory::Put32(item, 0x10, static_cast<std::uint32_t>(uid));
        Memory::Put32(item, 0x14, static_cast<std::uint32_t>(uid >> 32));
        Memory::Put32(item, 0x24, catalog);
        return item;
    }

    std::uint32_t MakeEntry(std::uint32_t item, std::uint32_t redirectPackage, std::uint32_t redirectSlot)
    {
        const std::uint32_t entry = memory.Alloc(0x20);
        Memory::Put32(entry, 0x08, item);
        Memory::Put32(entry, 0x0C, redirectPackage);
        Memory::Put32(entry, 0x10, redirectSlot);
        return entry;
    }

    static void SetEntry(Package& package, std::size_t index, std::uint32_t entry)
    {
        Memory::Put32(package.vector, index * 4, entry);
    }

    void Bag(std::size_t index, std::uint64_t uid, std::uint32_t catalog)
    {
        SetEntry(bag, index, MakeEntry(MakeItem(uid, catalog), kUnset, kUnset));
    }

    void Link(std::size_t equipmentSlot, std::uint32_t package, std::uint32_t slot)
    {
        SetEntry(equipment, equipmentSlot, MakeEntry(0, package, slot));
    }
};

} // namespace equipment_memory_fixture
