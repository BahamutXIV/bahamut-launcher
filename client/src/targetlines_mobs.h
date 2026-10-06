#pragma once

#include "packet_observer.h"

namespace bahamut_client
{

[[nodiscard]] bool IsTargetlinesMobInstantiation(const packet_observer::GameMessage& message);

} // namespace bahamut_client
