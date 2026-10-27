#pragma once

#include <windows.h>

#include <mutex>

// Locks a recursive mutex shared by the game's render, window and command threads.
// Waiters pump sent messages so a holder's cross-thread SendMessage (clipboard_set) cannot deadlock.
class ServicingLock
{
public:
    explicit ServicingLock(std::recursive_mutex& mutex)
    : mutex_(mutex)
    {
        for (unsigned spins = 0; !mutex_.try_lock(); ++spins)
        {
            MSG message;
            PeekMessageW(&message, nullptr, 0, 0, PM_NOREMOVE | PM_QS_SENDMESSAGE);
            if (spins < 64)
            {
                SwitchToThread();
            }
            else
            {
                Sleep(1);
            }
        }
    }

    ~ServicingLock()
    {
        mutex_.unlock();
    }

    ServicingLock(const ServicingLock&)            = delete;
    ServicingLock& operator=(const ServicingLock&) = delete;

private:
    std::recursive_mutex& mutex_;
};
