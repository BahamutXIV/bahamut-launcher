#pragma once

// Fault guard for reads of untrusted retail memory and calls into plugin
// callbacks. MSVC compiles the guard to structured exception handling. Clang
// targeting i386 mingw accepts __try only with -fms-extensions and emits no
// handler frame for it, and GCC rejects the keyword, so those builds register
// an explicit frame-based handler and return to the guard after the
// intermediate frames are unwound.
//
//     BAHAMUT_FAULT_TRY
//     {
//         // may return, break or continue
//     }
//     BAHAMUT_FAULT_EXCEPT
//     {
//         // runs after a fault in the block, outside the guard
//     }
//
// A local modified inside the block and read in the except block, or after
// the guard statement on the fault path, must be volatile, as with setjmp.
// C++ objects constructed inside the block are not destroyed on the fault
// path, which matches MSVC __try under /EHsc. A C++ exception thrown inside
// the block propagates past the guard on non-MSVC toolchains, where MSVC
// __except would catch it. A read whose value is never used may be optimized
// away and then cannot fault.

#include <windows.h>

#if defined(_MSC_VER)

#define BAHAMUT_FAULT_TRY    __try
#define BAHAMUT_FAULT_EXCEPT __except (EXCEPTION_EXECUTE_HANDLER)

#elif defined(__GNUC__) && defined(_WIN32) && defined(__i386__)

namespace bahamut_fault
{

// setjmp for the guard: saves ebp, ebx, esi, edi, the caller's esp and the
// return address, and returns 0. returns_twice is what keeps the compiler from
// reusing, inside the block, the stack slots of locals the except block reads;
// __builtin_setjmp does not carry it and loses those locals at -O2.
[[gnu::naked, gnu::returns_twice, gnu::noinline]] inline int Save(void** /*context*/)
{
    __asm__("movl 4(%esp), %eax\n\t"
            "movl %ebp, 0(%eax)\n\t"
            "movl %ebx, 4(%eax)\n\t"
            "movl %esi, 8(%eax)\n\t"
            "movl %edi, 12(%eax)\n\t"
            "leal 4(%esp), %ecx\n\t"
            "movl %ecx, 16(%eax)\n\t"
            "movl (%esp), %ecx\n\t"
            "movl %ecx, 20(%eax)\n\t"
            "xorl %eax, %eax\n\t"
            "ret");
}

// Returns from Save a second time, with 1.
[[gnu::naked, gnu::noreturn, gnu::noinline]] inline void Resume(void** /*context*/)
{
    __asm__("movl 4(%esp), %edx\n\t"
            "movl 0(%edx), %ebp\n\t"
            "movl 4(%edx), %ebx\n\t"
            "movl 8(%edx), %esi\n\t"
            "movl 12(%edx), %edi\n\t"
            "movl 16(%edx), %esp\n\t"
            "movl $1, %eax\n\t"
            "jmp *20(%edx)");
}

struct Frame
{
    struct Registration
    {
        Registration* next;
        void*         handler;
    };

    Registration registration{};
    void*        context[6]{};

    Frame()                        = default;
    Frame(const Frame&)            = delete;
    Frame& operator=(const Frame&) = delete;

    // Every exit from the guard statement unlinks here; the fault path is
    // already unlinked by the handler, so fs:[0] no longer points at this frame.
    ~Frame()
    {
        if (Head() == &registration)
        {
            SetHead(registration.next);
        }
    }

    static Registration* Head() noexcept
    {
        Registration* head = nullptr;
        __asm__ volatile("movl %%fs:0, %0" : "=r"(head));
        return head;
    }

    static void SetHead(Registration* head) noexcept
    {
        __asm__ volatile("movl %0, %%fs:0" : : "r"(head) : "memory");
    }

    bool Arm() noexcept
    {
        registration.next    = Head();
        registration.handler = reinterpret_cast<void*>(&Handler);
        SetHead(&registration);
        return true;
    }

    static EXCEPTION_DISPOSITION __cdecl Handler(EXCEPTION_RECORD* record,
                                                 void*             establisher,
                                                 CONTEXT*,
                                                 void*)
    {
        if ((record->ExceptionFlags & (EXCEPTION_UNWINDING | EXCEPTION_EXIT_UNWIND)) != 0)
        {
            return ExceptionContinueSearch;
        }
        RtlUnwind(establisher, nullptr, record, nullptr);
        // RtlUnwind stops with the establisher at fs:[0]. Windows does not
        // promise ebx, esi and edi across it, so nothing is carried in them.
        Registration* const self = Head();
        SetHead(self->next);
        Resume(reinterpret_cast<Frame*>(self)->context);
    }
};

// The dispatcher passes the fs:[0] record as the establisher frame.
static_assert(__is_standard_layout(Frame) && __builtin_offsetof(Frame, registration) == 0,
              "the registration record must be the Frame address");

} // namespace bahamut_fault

#define BAHAMUT_FAULT_CONCAT_(a, b) a##b
#define BAHAMUT_FAULT_CONCAT(a, b)  BAHAMUT_FAULT_CONCAT_(a, b)
// An if/else, so a site whose block and except block both return satisfies
// -Wreturn-type; the Frame destructor unlinks on every exit.
#define BAHAMUT_FAULT_TRY_(name) \
    if (bahamut_fault::Frame name; bahamut_fault::Save(name.context) == 0 && name.Arm())
#define BAHAMUT_FAULT_TRY    BAHAMUT_FAULT_TRY_(BAHAMUT_FAULT_CONCAT(bahamut_fault_frame_, __LINE__))
#define BAHAMUT_FAULT_EXCEPT else

#else
#error "fault_guard.h supports MSVC and i386 mingw toolchains only"
#endif
