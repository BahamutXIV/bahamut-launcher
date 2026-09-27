#pragma once

#include "packet_observer.h"

#include <chrono>
#include <cstdint>
#include <mutex>
#include <optional>
#include <string>

namespace bahamut_client
{

struct CustomTimerSnapshot
{
    std::string   title;
    std::string   objective;
    std::uint32_t remainingSeconds = 0;
};

class CustomTimerService
{
public:
    void                                             Observe(const packet_observer::GameMessage& message);
    [[nodiscard]] std::optional<CustomTimerSnapshot> Snapshot();

private:
    void ClearLocked();

    std::mutex                            mutex_;
    bool                                  active_ = false;
    std::string                           title_;
    std::string                           objective_;
    std::chrono::steady_clock::time_point deadline_{};
};

} // namespace bahamut_client
