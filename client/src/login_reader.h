#pragma once

#include <cstdint>

namespace bahamut_client
{

enum class LoginRequest : std::uint32_t
{
    None,
    Title, // accept the title screen's Start
    Slot,  // submit the character in a one-based list slot
};

enum class LoginRequestState : std::uint32_t
{
    None,
    Pending,
    Queued,
    Rejected,
};

// Scene, phase, character-list and last-request state copied on the frame-tick thread.
struct LoginScreen
{
    std::uint64_t     capturedAt    = 0; // GetTickCount64 at capture; 0 until the first capture
    std::uint32_t     sceneId       = 0; // 0 when unreadable
    bool              bootup        = false;
    std::uint32_t     phase         = 0;
    bool              roster        = false; // character-selection phase object readable
    std::uint8_t      closing       = 0;
    std::uint32_t     requestId     = 0; // request the two fields below describe; 0 for none
    LoginRequestState request       = LoginRequestState::None;
    const char*       requestReason = ""; // static text; set when queued or rejected
};

void        CaptureLoginScreen(const void* container);
LoginScreen ReadLoginScreen();
// Replaces a pending request and returns its nonzero id; the frame-tick thread
// runs it once its screen is showing. None clears a pending request and returns 0.
// Both operations wait for any running request to finish.
std::uint32_t RequestLogin(LoginRequest request, std::uint32_t slot);

} // namespace bahamut_client
