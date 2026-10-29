#pragma once

#include <array>
#include <cstddef>
#include <cstdint>

namespace bahamut_client::xhbar
{

// Return false on an unreadable client address; never fault.
using ReadMemory = bool (*)(void* context, std::uint32_t address, void* output, std::size_t bytes) noexcept;

struct SlotCommands
{
    bool                          valid       = false; // valid through the Sqwt context
    std::int32_t                  page        = 0;     // work.slotPage
    std::int32_t                  macroType   = 0;     // work.userMacroType; nonzero means macro bar
    bool                          slotMoving  = true;  // work.slotMoving; true when unread
    std::uint32_t                 pageAddress = 0;     // writable slotPage byte only for SimpleAccess pages 1-3
    std::array<std::int32_t, 30>  commands{};          // Label_CommandIcon_S Value0 at S-1; 0 if unread, empty or macro
    std::array<std::uint32_t, 30> icons{};             // TimerIcon icon ID at S-1; 0 if unread
    std::uint32_t                 window = 0;          // unique Window_ActionMenuWidget; 0 if absent
};

struct SlotStates
{
    bool                         valid  = false; // all state reads succeeded
    std::uint32_t                shared = 0;     // player's shared work record
    std::int32_t                 now    = 0;     // server whole seconds
    std::array<std::int32_t, 30> remaining{};    // recast seconds at S-1; 0 if ready
    std::array<bool, 30>         compatible{};   // class compatibility at S-1
};

// getRecastTimeForCustomCommand caps remaining seconds at a positive limit.
std::int32_t RecastRemaining(std::int32_t end, std::int32_t now, std::int32_t limit) noexcept;

// Pure, bounded reads; tlsArray is the calling thread's TEB ThreadLocalStoragePointer.
// labels=false skips labels; states is read independently after the Sqwt context, even on the macro bar.
SlotCommands ReadSlotCommands(std::uint32_t mainModule, std::uint32_t tlsArray, ReadMemory copy, void* context, bool labels = true, SlotStates* states = nullptr) noexcept;

struct DesktopState
{
    bool         valid  = false; // both fields read
    bool         locked = false;
    std::int32_t mode   = 0;
    bool         map    = false; // MapNavigationWidget under Desktop roots; false if unread
};

DesktopState ReadDesktop(std::uint32_t mainModule, ReadMemory copy, void* context) noexcept;

} // namespace bahamut_client::xhbar
