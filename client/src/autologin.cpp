#include "autologin.h"
#include "autologin_machine.h"
#include "login_reader.h"

#include <windows.h>

#include <cstdio>
#include <cstring>
#include <cwchar>

namespace bahamut_client
{
namespace
{

class ScopedLock
{
public:
    explicit ScopedLock(SRWLOCK& lock)
    : lock_(lock)
    {
        AcquireSRWLockExclusive(&lock_);
    }

    ~ScopedLock()
    {
        ReleaseSRWLockExclusive(&lock_);
    }

    ScopedLock(const ScopedLock&)            = delete;
    ScopedLock& operator=(const ScopedLock&) = delete;

private:
    SRWLOCK& lock_;
};

// Appends timestamped transitions, request results and reasons to
// autologin.log beside this DLL.
class LogFile
{
public:
    void Write(const char* text)
    {
        if (file_ == INVALID_HANDLE_VALUE && !tried_)
        {
            tried_ = true;
            Open();
        }
        if (file_ == INVALID_HANDLE_VALUE)
        {
            return;
        }
        char line[256];
        int  used = std::snprintf(line, sizeof(line), "t=%llu %s\r\n", static_cast<unsigned long long>(GetTickCount64()), text);
        if (used <= 0)
        {
            return;
        }
        const DWORD length  = static_cast<DWORD>(used < static_cast<int>(sizeof(line)) ? used : sizeof(line) - 1);
        DWORD       written = 0;
        WriteFile(file_, line, length, &written, nullptr);
    }

private:
    void Open()
    {
        HMODULE owner = nullptr;
        wchar_t path[MAX_PATH]{};
        if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                                reinterpret_cast<LPCWSTR>(&StepAutologin),
                                &owner))
        {
            return;
        }
        const DWORD       length  = GetModuleFileNameW(owner, path, ARRAYSIZE(path));
        wchar_t*          slash   = length > 0 && length < ARRAYSIZE(path) ? wcsrchr(path, L'\\') : nullptr;
        constexpr wchar_t kName[] = L"autologin.log";
        if (slash == nullptr || ARRAYSIZE(path) - static_cast<std::size_t>(slash + 1 - path) < ARRAYSIZE(kName))
        {
            return;
        }
        std::memcpy(slash + 1, kName, sizeof(kName));
        file_ = CreateFileW(path, FILE_APPEND_DATA, FILE_SHARE_READ, nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    }

    HANDLE file_  = INVALID_HANDLE_VALUE;
    bool   tried_ = false;
};

// Step runs on the render thread; service calls may arrive on other threads.
// The lock protects the machine and log. Only the frame-tick thread runs requests.
class ProductionService final : public AutologinService
{
public:
    ProductionService()
    : machine_(
          [](LoginRequest request, std::uint32_t slot)
          {
              return RequestLogin(request, slot);
          },
          [this](const char* line)
          {
              log_.Write(line);
          })
    {
    }

    AutologinStatus Start(std::uint32_t slot) override
    {
        const LoginScreen screen = ReadLoginScreen();
        ScopedLock        lock(lock_);
        return machine_.Start(slot, GetTickCount64(), screen);
    }

    AutologinStatus Cancel() override
    {
        ScopedLock lock(lock_);
        return machine_.Cancel(GetTickCount64());
    }

    void Step()
    {
        AutologinInputs inputs;
        inputs.now   = GetTickCount64();
        bool running = false;
        {
            ScopedLock lock(lock_);
            running = machine_.Running();
        }
        if (running)
        {
            inputs.screen = ReadLoginScreen();
        }
        ScopedLock lock(lock_);
        machine_.Step(inputs);
    }

private:
    SRWLOCK          lock_ = SRWLOCK_INIT;
    LogFile          log_;
    AutologinMachine machine_;
};

// Process lifetime avoids destruction under the loader lock during DLL detach.
ProductionService& Production()
{
    static ProductionService* const service = new ProductionService();
    return *service;
}

} // namespace

AutologinService& ProductionAutologinService()
{
    return Production();
}

void StepAutologin()
{
    Production().Step();
}

} // namespace bahamut_client
