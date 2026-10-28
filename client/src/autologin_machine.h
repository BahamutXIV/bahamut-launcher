#pragma once

#include "autologin.h"
#include "login_reader.h"

#include <cstdint>
#include <functional>

namespace bahamut_client
{

inline constexpr std::uint32_t kAutologinWorldScene = 21;

struct AutologinInputs
{
    LoginScreen   screen;
    std::uint64_t now = 0; // monotonic milliseconds, same clock as screen.capturedAt
};

// Hands one login request to the frame-tick thread and returns its id; None
// clears it and returns 0.
using AutologinRequest = std::function<std::uint32_t(LoginRequest request, std::uint32_t slot)>;
using AutologinLog     = std::function<void(const char* line)>;

// The caller serializes access to the machine and its callbacks.
class AutologinMachine
{
public:
    AutologinMachine(AutologinRequest request, AutologinLog log);

    // Arms the single run. Later calls return the status unchanged.
    AutologinStatus Start(std::uint32_t slot, std::uint64_t now, const LoginScreen& screen);
    // Stops an armed run; no-op when the run is not active.
    AutologinStatus Cancel(std::uint64_t now);
    void            Step(const AutologinInputs& inputs);
    AutologinStatus Status() const;
    bool            Running() const;

private:
    void Enter(AutologinPhase phase, std::uint64_t now, const char* reason);
    void Request(LoginRequest request, std::uint32_t slot);

    AutologinRequest request_;
    AutologinLog     log_;
    AutologinStatus  status_;
    bool             started_ = false;

    std::uint64_t     since_       = 0;
    std::uint64_t     titleSince_  = 0;
    std::uint64_t     listSince_   = 0;
    std::uint32_t     requestId_   = 0;
    LoginRequestState lastRequest_ = LoginRequestState::None;
};

} // namespace bahamut_client
