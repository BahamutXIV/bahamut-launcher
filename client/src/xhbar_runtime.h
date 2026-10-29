#pragma once

#include "input_policy.h"
#include "xhbar_pad.h"
#include "xhbar_policy.h"

#include <cstddef>
#include <cstdint>
#include <filesystem>

namespace bahamut_client::xhbar
{

// See client/README.md#xhbar.
bool InstallRuntime();

// Run on the pad thread after the native pad update, before consumed controls reach the client.
void OnPadUpdate(ClientPadState* state, bool connected, bool foreground);

Side ActiveSide();

// GetTickCount64 at last dispatch for edge 0-31; 0 if never fired.
std::uint64_t LastDispatchTick(std::size_t slot);

bool BarAvailable();

// Call after each rendered bar frame to keep native-bar hiding alive.
void MarkBarDrawn();

// One snapshot per edge; 0 icon means unknown/empty/macro, 0 page means unbound.
struct SlotView
{
    std::uint32_t icon         = 0;
    int           page         = 0;
    int           remaining    = 0;     // whole recast seconds left
    bool          incompatible = false; // class cannot use it
};

SlotView SlotDisplay(std::size_t slot);

// Call once at startup, before the overlay or commands run.
void ConfigureSettings(const std::filesystem::path& file);

LabelStyle Labels();

// Applies immediately; returns whether saving succeeded.
bool SaveLabels(LabelStyle style);

// Updates visible immediately; returns whether saving succeeded.
bool ToggleVisible(bool& visible);

} // namespace bahamut_client::xhbar
