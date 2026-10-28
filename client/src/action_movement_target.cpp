#include "action_movement_admission.h"

#include "fault_guard.h"
#include "retail_identity.h"
#include "runtime_contract.h"

#include <windows.h>

#include <array>
#include <cstring>

namespace bahamut_client
{
namespace
{

bool CopyTarget(std::uintptr_t address, void* output, std::size_t size)
{
    BAHAMUT_FAULT_TRY
    {
        std::memcpy(output, reinterpret_cast<const void*>(address), size);
        return true;
    }
    BAHAMUT_FAULT_EXCEPT
    {
        return false;
    }
}

} // namespace

bool VerifyActionMovementRetailTarget()
{
    const auto imageBase = reinterpret_cast<std::uintptr_t>(GetModuleHandleW(nullptr));
    if (imageBase != bahamut_runtime_contract::kRetailImageBase || !VerifyRuntimeHostIdentity(false))
    {
        return false;
    }
    std::array<std::uint8_t, 22> setter{};
    std::array<std::uint8_t, 45> wrapper{};
    std::array<std::uint8_t, 5>  producerCall{};
    return CopyTarget(kActionMovementSetterVa, setter.data(), setter.size()) &&
           CopyTarget(kActionMovementWrapperVa, wrapper.data(), wrapper.size()) &&
           CopyTarget(kActionMovementProducerCallVa, producerCall.data(), producerCall.size()) &&
           MatchActionMovementTargetBytes(imageBase, setter, wrapper, producerCall);
}

} // namespace bahamut_client
