# Cross-compile the Windows x86 client module from macOS or Linux with llvm-mingw.
#
# llvm-mingw is the only supported non-MSVC toolchain: it ships one
# self-consistent clang, lld, mingw-w64 CRT, and libc++. Clang objects mixed
# with a GCC libstdc++ fail to link, and the fault guard in src/fault_guard.h
# replaces __try/__except, which clang parses for this target without emitting
# a handler frame.
#
# Usage:
#   cmake -S client -B out/client-mingw \
#     -DCMAKE_TOOLCHAIN_FILE="$PWD/client/cmake/llvm-mingw-i686.cmake" \
#     -DCMAKE_BUILD_TYPE=Release
#
# The toolchain root is taken from -DLLVM_MINGW_ROOT=<dir>, then the
# LLVM_MINGW_ROOT environment variable, then the first
# i686-w64-mingw32-clang on PATH.

set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR X86)

set(LLVM_MINGW_ROOT "" CACHE PATH "llvm-mingw installation root")
# try_compile re-reads this file in a fresh cache.
list(APPEND CMAKE_TRY_COMPILE_PLATFORM_VARIABLES LLVM_MINGW_ROOT)
if(NOT LLVM_MINGW_ROOT AND DEFINED ENV{LLVM_MINGW_ROOT})
    set(LLVM_MINGW_ROOT "$ENV{LLVM_MINGW_ROOT}")
endif()
if(NOT LLVM_MINGW_ROOT)
    find_program(_bahamut_mingw_clang NAMES i686-w64-mingw32-clang)
    if(_bahamut_mingw_clang)
        get_filename_component(_bahamut_mingw_bin "${_bahamut_mingw_clang}" REALPATH)
        get_filename_component(_bahamut_mingw_bin "${_bahamut_mingw_bin}" DIRECTORY)
        get_filename_component(LLVM_MINGW_ROOT "${_bahamut_mingw_bin}" DIRECTORY)
    endif()
endif()
if(LLVM_MINGW_ROOT AND EXISTS "${LLVM_MINGW_ROOT}/bin/i686-w64-mingw32-clang++")
    # Cache the resolved root so later reconfigures do not depend on the shell.
    set(LLVM_MINGW_ROOT "${LLVM_MINGW_ROOT}" CACHE PATH "llvm-mingw installation root" FORCE)
endif()
if(NOT LLVM_MINGW_ROOT OR NOT EXISTS "${LLVM_MINGW_ROOT}/bin/i686-w64-mingw32-clang++")
    message(FATAL_ERROR
        "llvm-mingw was not found. Install a release from "
        "https://github.com/mstorsjo/llvm-mingw/releases and pass "
        "-DLLVM_MINGW_ROOT=<dir> or put its bin directory on PATH.")
endif()

set(_bahamut_triple i686-w64-mingw32)
set(CMAKE_C_COMPILER "${LLVM_MINGW_ROOT}/bin/${_bahamut_triple}-clang")
set(CMAKE_CXX_COMPILER "${LLVM_MINGW_ROOT}/bin/${_bahamut_triple}-clang++")
set(CMAKE_RC_COMPILER "${LLVM_MINGW_ROOT}/bin/${_bahamut_triple}-windres")
set(CMAKE_AR "${LLVM_MINGW_ROOT}/bin/llvm-ar")
set(CMAKE_RANLIB "${LLVM_MINGW_ROOT}/bin/llvm-ranlib")

set(CMAKE_FIND_ROOT_PATH "${LLVM_MINGW_ROOT}/${_bahamut_triple}")
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)

# __uuidof and _ReturnAddress are Microsoft extensions in clang.
set(CMAKE_C_FLAGS_INIT "-fms-extensions")
set(CMAKE_CXX_FLAGS_INIT "-fms-extensions")

# The DLLs are injected into the game and the helper runs before any
# runtime is installed, so the C++ library and unwinder link statically.
set(CMAKE_EXE_LINKER_FLAGS_INIT "-static")
set(CMAKE_SHARED_LINKER_FLAGS_INIT "-static")
set(CMAKE_MODULE_LINKER_FLAGS_INIT "-static")
