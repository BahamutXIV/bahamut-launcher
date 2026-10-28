cmake_minimum_required(VERSION 3.25)

project(bahamut_client LANGUAGES C CXX)

if(NOT WIN32)
    message(FATAL_ERROR "The client module is a Windows x86 Win32 project; cross-compile it with client/cmake/llvm-mingw-i686.cmake")
endif()

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

if(NOT CMAKE_SIZEOF_VOID_P EQUAL 4)
    message(FATAL_ERROR "The bootstrap helper, target, and runtime must be x86")
endif()

if(MSVC)
    add_compile_options(/W4 /WX /EHsc /permissive-)
else()
    # The clang analog of /W4 /WX; the disabled groups are ones /W4 does not raise.
    add_compile_options(
        -Wno-unknown-warning-option
        -Wall -Wextra -Werror
        -Wno-missing-field-initializers
        -Wno-deprecated-volatile
        -Wno-unused-but-set-variable
        -Wno-cast-function-type-mismatch
        -Wno-cast-calling-convention
    )
endif()

add_compile_definitions(UNICODE _UNICODE WIN32_LEAN_AND_MEAN NOMINMAX _WIN32_WINNT=0x0601)

# lld expects module-definition internal names without the i386 underscore
# that link.exe expects, so the tracked .def files are rewritten at configure
# time for non-MSVC toolchains.
function(bahamut_module_definition variable input)
    if(MSVC)
        set(${variable} "${input}" PARENT_SCOPE)
        return()
    endif()
    get_filename_component(name "${input}" NAME)
    set(output "${CMAKE_CURRENT_BINARY_DIR}/gnu-def/${name}")
    file(READ "${input}" content)
    string(REGEX REPLACE "=_([A-Za-z0-9_]+@[0-9]+)" "=\\1" content "${content}")
    set(existing "")
    if(EXISTS "${output}")
        file(READ "${output}" existing)
    endif()
    if(NOT existing STREQUAL content)
        file(WRITE "${output}" "${content}")
    endif()
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${input}")
    set(${variable} "${output}" PARENT_SCOPE)
endfunction()

# mingw needs -municode for wmain and wWinMain entry points; link.exe infers them.
function(bahamut_unicode_entry target)
    if(NOT MSVC)
        target_link_options(${target} PRIVATE -municode)
    endif()
endfunction()

# Non-MSVC toolchains compile a raw __try without a handler frame, so every
# guarded read must go through src/fault_guard.h on every toolchain.
file(GLOB _bahamut_first_party CONFIGURE_DEPENDS
    "${CMAKE_CURRENT_SOURCE_DIR}/src/*.cpp"
    "${CMAKE_CURRENT_SOURCE_DIR}/src/*.h"
    "${CMAKE_CURRENT_SOURCE_DIR}/tests/*.cpp"
    "${CMAKE_CURRENT_SOURCE_DIR}/api/*.h"
)
foreach(_bahamut_source IN LISTS _bahamut_first_party)
    if(_bahamut_source MATCHES "/fault_guard\\.h$")
        continue()
    endif()
    file(STRINGS "${_bahamut_source}" _bahamut_seh REGEX "__(try|except|finally)")
    if(_bahamut_seh)
        message(FATAL_ERROR "${_bahamut_source} uses __try; use BAHAMUT_FAULT_TRY from src/fault_guard.h")
    endif()
endforeach()

bahamut_module_definition(BAHAMUT_RUNTIME_DEF "${CMAKE_CURRENT_SOURCE_DIR}/src/bahamut.def")
bahamut_module_definition(BAHAMUT_SCREENSHOT_DEF "${CMAKE_CURRENT_SOURCE_DIR}/src/screenshot.def")
bahamut_module_definition(BAHAMUT_DISCORD_RPC_DEF "${CMAKE_CURRENT_SOURCE_DIR}/src/discord_rpc.def")
bahamut_module_definition(BAHAMUT_TEST_XINPUT_DEF "${CMAKE_CURRENT_SOURCE_DIR}/tests/xinput_stub.def")

add_library(minhook STATIC
    vendor/minhook/src/buffer.c
    vendor/minhook/src/hook.c
    vendor/minhook/src/trampoline.c
    vendor/minhook/src/hde/hde32.c
)
target_include_directories(minhook SYSTEM PUBLIC vendor/minhook/include)
if(MSVC)
    target_compile_options(minhook PRIVATE /W3 /WX-)
else()
    target_compile_options(minhook PRIVATE -w)
endif()

add_library(miniz STATIC vendor/miniz/miniz.c)
target_include_directories(miniz SYSTEM PUBLIC vendor/miniz)
if(MSVC)
    target_compile_options(miniz PRIVATE /W3 /WX-)
else()
    target_compile_options(miniz PRIVATE -w)
endif()

add_library(imgui STATIC
    vendor/imgui/imgui.cpp
    vendor/imgui/imgui_draw.cpp
    vendor/imgui/imgui_tables.cpp
    vendor/imgui/imgui_widgets.cpp
    vendor/imgui/backends/imgui_impl_dx9.cpp
    vendor/imgui/backends/imgui_impl_win32.cpp
)
target_include_directories(imgui SYSTEM PUBLIC
    vendor/imgui
    vendor/imgui/backends
)
target_compile_definitions(imgui PRIVATE IMGUI_IMPL_WIN32_DISABLE_GAMEPAD)
if(MSVC)
    target_compile_options(imgui PRIVATE /W3 /WX-)
else()
    target_compile_options(imgui PRIVATE -w)
endif()

add_library(lua51 STATIC
    vendor/lua/src/lapi.c
    vendor/lua/src/lauxlib.c
    vendor/lua/src/lbaselib.c
    vendor/lua/src/lcode.c
    vendor/lua/src/ldebug.c
    vendor/lua/src/ldo.c
    vendor/lua/src/ldump.c
    vendor/lua/src/lfunc.c
    vendor/lua/src/lgc.c
    vendor/lua/src/llex.c
    vendor/lua/src/lmathlib.c
    vendor/lua/src/lmem.c
    vendor/lua/src/lobject.c
    vendor/lua/src/lopcodes.c
    vendor/lua/src/lparser.c
    vendor/lua/src/lstate.c
    vendor/lua/src/lstring.c
    vendor/lua/src/lstrlib.c
    vendor/lua/src/ltable.c
    vendor/lua/src/ltablib.c
    vendor/lua/src/ltm.c
    vendor/lua/src/lundump.c
    vendor/lua/src/lvm.c
    vendor/lua/src/lzio.c
)
target_include_directories(lua51 SYSTEM PUBLIC vendor/lua/src)
if(MSVC)
    target_compile_options(lua51 PRIVATE /W3 /WX-)
else()
    target_compile_options(lua51 PRIVATE -w)
endif()

add_library(bahamut SHARED
    src/action_movement.cpp
    src/action_movement_admission.cpp
    src/action_movement_capture.cpp
    src/action_movement_hook.cpp
    src/action_movement_hook.h
    src/action_movement_runtime.cpp
    src/action_movement_runtime.h
    src/action_movement_target.cpp
    src/action_movement.h
    src/native_state.h
    api/native_plugin_api.h
    api/runtime_api.h
    ${BAHAMUT_RUNTIME_DEF}
    src/actor_name.cpp
    src/actor_name.h
    src/autologin.cpp
    src/autologin.h
    src/autologin_machine.cpp
    src/autologin_machine.h
    src/camera_zoom.cpp
    src/camera_zoom.h
    src/addon_host.cpp
    src/addon_host.h
    src/chat_boundary.cpp
    src/chat_boundary.h
    src/command_boundary.cpp
    src/command_boundary.h
    src/custom_timer.cpp
    src/custom_timer.h
    src/enemy_cast.cpp
    src/enemy_cast.h
    src/login_reader.cpp
    src/login_reader.h
    src/player_state.cpp
    src/player_state.h
    src/packet_capture.cpp
    src/packet_capture.h
    src/packet_capture_policy.h
    src/packet_observer.cpp
    src/packet_observer.h
    src/target_distance.cpp
    src/target_distance.h
    src/targetlines.cpp
    src/targetlines.h
    src/targetlines_arc.cpp
    src/targetlines_arc.h
    src/targetlines_mobs.cpp
    src/targetlines_mobs.h
    src/targetlines_probe.cpp
    src/targetlines_probe.h
    src/targetlines_projection.cpp
    src/targetlines_projection.h
    src/world_projection.cpp
    src/world_projection.h
    src/render_boundary.cpp
    src/render_boundary.h
    src/overlay.cpp
    src/overlay.h
    src/input.cpp
    src/input.h
    src/input_policy.cpp
    src/input_policy.h
    src/bahamut.cpp
    src/native_plugin_host.cpp
    src/native_plugin_host.h
    src/object_distance.cpp
    src/object_distance.h
    src/monitor_selection.cpp
    src/monitor_selection.h
    src/dat_overlay.cpp
    src/dat_overlay.h
    src/retail_identity.cpp
    src/retail_identity.h
    src/runtime_commands.cpp
    src/runtime_commands.h
    src/startup_script.cpp
    src/startup_script.h
)
target_include_directories(bahamut PUBLIC api PRIVATE src)
target_link_libraries(bahamut PRIVATE
    bcrypt d3d9 dwmapi imgui lua51 minhook miniz ole32 shell32 user32 windowscodecs ws2_32
)
set_target_properties(bahamut PROPERTIES PREFIX "")

option(BAHAMUT_TARGETLINES_PROBE "Build the optional Targetlines scene camera diagnostic" OFF)
if(BAHAMUT_TARGETLINES_PROBE)
    target_sources(bahamut PRIVATE
        src/targetlines_capture.cpp
        src/targetlines_capture.h
    )
    target_compile_definitions(bahamut PRIVATE BAHAMUT_TARGETLINES_PROBE)
endif()

add_library(screenshot SHARED
    api/native_plugin_api.h
    ${BAHAMUT_SCREENSHOT_DEF}
    src/screenshot.cpp
    src/screenshot.h
    src/screenshot_plugin.cpp
)
target_include_directories(screenshot PRIVATE api src)
target_link_libraries(screenshot PRIVATE d3d9 ole32 user32 windowscodecs)
set_target_properties(screenshot PROPERTIES
    OUTPUT_NAME "screenshot"
    PREFIX ""
)

add_library(discord-rpc SHARED
    api/native_plugin_api.h
    ${BAHAMUT_DISCORD_RPC_DEF}
    src/discord_rpc_plugin.cpp
)
target_include_directories(discord-rpc PRIVATE api)
set_target_properties(discord-rpc PROPERTIES
    OUTPUT_NAME "discord-rpc"
    PREFIX ""
)

add_executable(bahamut-loader
    src/loader.cpp
)
target_include_directories(bahamut-loader PRIVATE api)
target_link_libraries(bahamut-loader PRIVATE advapi32 bcrypt psapi)
bahamut_unicode_entry(bahamut-loader)

# Keep shared test objects separate from the runtime DLL's export/probe variants.
add_library(bahamut-test-monitor-selection-objects OBJECT src/monitor_selection.cpp)
target_include_directories(bahamut-test-monitor-selection-objects PRIVATE src)

add_library(bahamut-test-actor-name-objects OBJECT src/actor_name.cpp)
target_include_directories(bahamut-test-actor-name-objects PRIVATE api src)
target_link_libraries(bahamut-test-actor-name-objects PRIVATE minhook)

add_library(bahamut-test-target-distance-objects OBJECT src/target_distance.cpp)
target_include_directories(bahamut-test-target-distance-objects PRIVATE api src)
target_link_libraries(bahamut-test-target-distance-objects PRIVATE minhook)

add_library(bahamut-test-player-state-objects OBJECT src/player_state.cpp src/login_reader.cpp)
target_include_directories(bahamut-test-player-state-objects PRIVATE api src)
target_link_libraries(bahamut-test-player-state-objects PRIVATE minhook)

add_library(bahamut-test-custom-timer-objects OBJECT src/custom_timer.cpp)
target_include_directories(bahamut-test-custom-timer-objects PRIVATE src)

add_library(bahamut-test-enemy-cast-objects OBJECT src/enemy_cast.cpp)
target_include_directories(bahamut-test-enemy-cast-objects PRIVATE api src vendor/minhook/include)
target_link_libraries(bahamut-test-enemy-cast-objects PRIVATE minhook)

add_library(bahamut-test-packet-capture-objects OBJECT src/packet_capture.cpp)
target_include_directories(bahamut-test-packet-capture-objects PRIVATE src)

add_library(bahamut-test-action-movement-objects OBJECT
    src/action_movement.cpp
    src/action_movement_admission.cpp
)
target_include_directories(bahamut-test-action-movement-objects PRIVATE api src)

add_library(bahamut-test-addon-host-objects OBJECT src/addon_host.cpp)
target_include_directories(bahamut-test-addon-host-objects PRIVATE api src vendor/minhook/include)
target_link_libraries(bahamut-test-addon-host-objects PRIVATE lua51 minhook)

add_library(bahamut-test-world-projection-objects OBJECT src/world_projection.cpp)
target_include_directories(bahamut-test-world-projection-objects PRIVATE src)

add_library(bahamut-test-targetlines-mobs-objects OBJECT src/targetlines_mobs.cpp)
target_include_directories(bahamut-test-targetlines-mobs-objects PRIVATE src)

add_executable(bahamut-test-client WIN32
    src/test_client.cpp
    $<TARGET_OBJECTS:bahamut-test-monitor-selection-objects>
)
target_include_directories(bahamut-test-client PRIVATE api src)
target_link_libraries(bahamut-test-client PRIVATE d3d9 user32)
bahamut_unicode_entry(bahamut-test-client)
# The stub must load at the retail image base so RVA-addressed patches apply.
if(MSVC)
    set_property(TARGET bahamut-test-client APPEND_STRING PROPERTY LINK_FLAGS " /DYNAMICBASE:NO /FIXED")
else()
    # --kill-at drops the @N suffix from the stub's stdcall exports; input.cpp
    # resolves that name after the link.exe _Name@N form.
    target_link_options(bahamut-test-client PRIVATE -Wl,--disable-dynamicbase -Wl,--disable-reloc-section -Wl,--image-base,0x400000 -Wl,--kill-at)
endif()

add_library(bahamut-test-xinput SHARED
    tests/xinput_stub.cpp
    ${BAHAMUT_TEST_XINPUT_DEF}
)
set_target_properties(bahamut-test-xinput PROPERTIES
    OUTPUT_NAME "xinput1_3"
    PREFIX ""
)
add_dependencies(bahamut-test-client bahamut-test-xinput)

add_executable(bahamut-loader-tests
    tests/loader_tests.cpp
    $<TARGET_OBJECTS:bahamut-test-monitor-selection-objects>
)
target_include_directories(bahamut-loader-tests PRIVATE src)
target_link_libraries(bahamut-loader-tests PRIVATE user32)
bahamut_unicode_entry(bahamut-loader-tests)

add_executable(bahamut-monitor-selection-tests
    $<TARGET_OBJECTS:bahamut-test-monitor-selection-objects>
    tests/monitor_selection_tests.cpp
)
target_include_directories(bahamut-monitor-selection-tests PRIVATE src)
target_link_libraries(bahamut-monitor-selection-tests PRIVATE user32)

add_executable(bahamut-discord-rpc-tests
    tests/discord_rpc_plugin_tests.cpp
)
target_include_directories(bahamut-discord-rpc-tests PRIVATE api)

add_executable(bahamut-input-policy-tests
    src/input_policy.cpp
    src/input_policy.h
    tests/input_policy_tests.cpp
)
target_include_directories(bahamut-input-policy-tests PRIVATE src)

add_executable(bahamut-action-movement-tests
    src/action_movement.cpp
    src/action_movement_admission.cpp
    src/action_movement_capture.cpp
    src/action_movement.h
    src/action_movement_hook.cpp
    src/action_movement_hook.h
    src/action_movement_runtime.cpp
    src/action_movement_runtime.h
    src/action_movement_target.cpp
    src/retail_identity.cpp
    tests/action_movement_tests.cpp
)
target_include_directories(bahamut-action-movement-tests PRIVATE api src vendor/minhook/include)
target_link_libraries(bahamut-action-movement-tests PRIVATE bcrypt minhook)

add_executable(bahamut-action-movement-admission-tests
    src/action_movement.cpp
    src/action_movement_admission.cpp
    src/action_movement_capture.cpp
    src/action_movement_target.cpp
    src/retail_identity.cpp
    tests/action_movement_admission_tests.cpp
)
target_include_directories(bahamut-action-movement-admission-tests PRIVATE api src)
target_link_libraries(bahamut-action-movement-admission-tests PRIVATE bcrypt)
bahamut_unicode_entry(bahamut-action-movement-admission-tests)

add_executable(bahamut-action-movement-observer-tests
    src/action_movement_observer.cpp
    tests/action_movement_observer_tests.cpp
)
target_include_directories(bahamut-action-movement-observer-tests PRIVATE api src)
target_link_libraries(bahamut-action-movement-observer-tests PRIVATE minhook)

add_executable(bahamut-autologin-tests
    src/autologin.h
    src/autologin_machine.cpp
    src/autologin_machine.h
    src/login_reader.h
    tests/autologin_tests.cpp
)
target_include_directories(bahamut-autologin-tests PRIVATE src)

add_executable(bahamut-custom-timer-tests
    $<TARGET_OBJECTS:bahamut-test-custom-timer-objects>
    src/custom_timer.h
    tests/custom_timer_tests.cpp
)
target_include_directories(bahamut-custom-timer-tests PRIVATE src)

add_executable(bahamut-enemy-cast-tests
    $<TARGET_OBJECTS:bahamut-test-actor-name-objects>
    src/actor_name.h
    $<TARGET_OBJECTS:bahamut-test-enemy-cast-objects>
    src/enemy_cast.h
    $<TARGET_OBJECTS:bahamut-test-player-state-objects>
    src/player_state.h
    tests/enemy_cast_tests.cpp
)
target_include_directories(bahamut-enemy-cast-tests PRIVATE
    api
    src
    vendor/minhook/include
)
target_link_libraries(bahamut-enemy-cast-tests PRIVATE minhook)

add_executable(bahamut-addon-host-tests
    $<TARGET_OBJECTS:bahamut-test-action-movement-objects>
    src/action_movement.h
    $<TARGET_OBJECTS:bahamut-test-actor-name-objects>
    $<TARGET_OBJECTS:bahamut-test-target-distance-objects>
    src/actor_name.h
    $<TARGET_OBJECTS:bahamut-test-addon-host-objects>
    src/addon_host.h
    $<TARGET_OBJECTS:bahamut-test-custom-timer-objects>
    src/custom_timer.h
    $<TARGET_OBJECTS:bahamut-test-enemy-cast-objects>
    src/enemy_cast.h
    $<TARGET_OBJECTS:bahamut-test-player-state-objects>
    $<TARGET_OBJECTS:bahamut-test-packet-capture-objects>
    src/packet_capture.h
    src/packet_capture_policy.h
    src/player_state.h
    src/target_distance.h
    tests/addon_host_tests.cpp
)
target_include_directories(bahamut-addon-host-tests PRIVATE
    api
    src
    vendor/minhook/include
)
target_link_libraries(bahamut-addon-host-tests PRIVATE lua51 minhook)
bahamut_unicode_entry(bahamut-addon-host-tests)

add_executable(bahamut-runtime-commands-tests
    $<TARGET_OBJECTS:bahamut-test-action-movement-objects>
    src/action_movement.h
    $<TARGET_OBJECTS:bahamut-test-actor-name-objects>
    $<TARGET_OBJECTS:bahamut-test-target-distance-objects>
    src/actor_name.h
    $<TARGET_OBJECTS:bahamut-test-addon-host-objects>
    src/addon_host.h
    $<TARGET_OBJECTS:bahamut-test-custom-timer-objects>
    src/custom_timer.h
    $<TARGET_OBJECTS:bahamut-test-enemy-cast-objects>
    src/enemy_cast.h
    $<TARGET_OBJECTS:bahamut-test-player-state-objects>
    $<TARGET_OBJECTS:bahamut-test-packet-capture-objects>
    src/packet_capture.h
    src/packet_capture_policy.h
    src/player_state.h
    src/runtime_commands.cpp
    src/runtime_commands.h
    src/target_distance.h
    tests/runtime_commands_tests.cpp
)
target_include_directories(bahamut-runtime-commands-tests PRIVATE
    api
    src
    vendor/minhook/include
)
target_link_libraries(bahamut-runtime-commands-tests PRIVATE lua51 minhook)
bahamut_unicode_entry(bahamut-runtime-commands-tests)

add_executable(bahamut-action-movement-addon-tests
    $<TARGET_OBJECTS:bahamut-test-action-movement-objects>
    src/action_movement.h
    $<TARGET_OBJECTS:bahamut-test-actor-name-objects>
    $<TARGET_OBJECTS:bahamut-test-addon-host-objects>
    src/addon_host.h
    $<TARGET_OBJECTS:bahamut-test-custom-timer-objects>
    $<TARGET_OBJECTS:bahamut-test-enemy-cast-objects>
    $<TARGET_OBJECTS:bahamut-test-player-state-objects>
    $<TARGET_OBJECTS:bahamut-test-packet-capture-objects>
    $<TARGET_OBJECTS:bahamut-test-target-distance-objects>
    tests/action_movement_addon_tests.cpp
)
target_include_directories(bahamut-action-movement-addon-tests PRIVATE api src vendor/minhook/include)
target_link_libraries(bahamut-action-movement-addon-tests PRIVATE lua51 minhook)
bahamut_unicode_entry(bahamut-action-movement-addon-tests)

add_executable(bahamut-action-movement-lua-tests
    $<TARGET_OBJECTS:bahamut-test-action-movement-objects>
    $<TARGET_OBJECTS:bahamut-test-actor-name-objects>
    $<TARGET_OBJECTS:bahamut-test-addon-host-objects>
    $<TARGET_OBJECTS:bahamut-test-custom-timer-objects>
    $<TARGET_OBJECTS:bahamut-test-enemy-cast-objects>
    src/chat_boundary.cpp
    src/command_boundary.cpp
    $<TARGET_OBJECTS:bahamut-test-player-state-objects>
    $<TARGET_OBJECTS:bahamut-test-packet-capture-objects>
    src/retail_identity.cpp
    src/runtime_commands.cpp
    $<TARGET_OBJECTS:bahamut-test-target-distance-objects>
    tests/action_movement_lua_tests.cpp
)
target_include_directories(bahamut-action-movement-lua-tests PRIVATE api src vendor/minhook/include)
target_link_libraries(bahamut-action-movement-lua-tests PRIVATE bcrypt lua51 minhook)
bahamut_unicode_entry(bahamut-action-movement-lua-tests)

add_executable(bahamut-command-boundary-tests
    $<TARGET_OBJECTS:bahamut-test-action-movement-objects>
    src/action_movement.h
    $<TARGET_OBJECTS:bahamut-test-actor-name-objects>
    $<TARGET_OBJECTS:bahamut-test-target-distance-objects>
    src/actor_name.h
    $<TARGET_OBJECTS:bahamut-test-addon-host-objects>
    src/addon_host.h
    src/chat_boundary.cpp
    src/chat_boundary.h
    src/command_boundary.cpp
    src/command_boundary.h
    $<TARGET_OBJECTS:bahamut-test-custom-timer-objects>
    src/custom_timer.h
    $<TARGET_OBJECTS:bahamut-test-enemy-cast-objects>
    src/enemy_cast.h
    $<TARGET_OBJECTS:bahamut-test-player-state-objects>
    $<TARGET_OBJECTS:bahamut-test-packet-capture-objects>
    src/packet_capture.h
    src/packet_capture_policy.h
    src/player_state.h
    src/retail_identity.cpp
    src/retail_identity.h
    src/target_distance.h
    tests/command_boundary_tests.cpp
)
target_include_directories(bahamut-command-boundary-tests PRIVATE
    api
    src
    vendor/minhook/include
)
target_link_libraries(bahamut-command-boundary-tests PRIVATE bcrypt lua51 minhook)
bahamut_unicode_entry(bahamut-command-boundary-tests)

add_executable(bahamut-player-state-tests
    $<TARGET_OBJECTS:bahamut-test-player-state-objects>
    src/player_state.h
    tests/player_state_tests.cpp
)
target_include_directories(bahamut-player-state-tests PRIVATE
    api
    src
    vendor/minhook/include
)
target_link_libraries(bahamut-player-state-tests PRIVATE minhook)
bahamut_unicode_entry(bahamut-player-state-tests)

add_executable(bahamut-target-distance-tests
    $<TARGET_OBJECTS:bahamut-test-actor-name-objects>
    $<TARGET_OBJECTS:bahamut-test-target-distance-objects>
    src/actor_name.h
    $<TARGET_OBJECTS:bahamut-test-player-state-objects>
    src/player_state.h
    src/target_distance.h
    tests/target_distance_tests.cpp
)
target_include_directories(bahamut-target-distance-tests PRIVATE
    api
    src
    vendor/minhook/include
)
target_link_libraries(bahamut-target-distance-tests PRIVATE minhook)

add_executable(bahamut-startup-script-tests
    src/startup_script.cpp
    src/startup_script.h
    tests/startup_script_tests.cpp
)
target_include_directories(bahamut-startup-script-tests PRIVATE src)
bahamut_unicode_entry(bahamut-startup-script-tests)

add_executable(bahamut-world-projection-tests
    $<TARGET_OBJECTS:bahamut-test-world-projection-objects>
    src/world_projection.h
    tests/world_projection_tests.cpp
)
target_include_directories(bahamut-world-projection-tests PRIVATE src)

add_executable(bahamut-targetlines-arc-tests
    src/targetlines_arc.cpp
    src/targetlines_arc.h
    $<TARGET_OBJECTS:bahamut-test-world-projection-objects>
    tests/targetlines_arc_tests.cpp
)
target_include_directories(bahamut-targetlines-arc-tests PRIVATE src)

add_executable(bahamut-targetlines-tests
    $<TARGET_OBJECTS:bahamut-test-actor-name-objects>
    $<TARGET_OBJECTS:bahamut-test-target-distance-objects>
    $<TARGET_OBJECTS:bahamut-test-player-state-objects>
    src/targetlines.cpp
    $<TARGET_OBJECTS:bahamut-test-targetlines-mobs-objects>
    tests/targetlines_tests.cpp
)
target_include_directories(bahamut-targetlines-tests PRIVATE api src vendor/minhook/include)
target_link_libraries(bahamut-targetlines-tests PRIVATE minhook)

add_executable(bahamut-targetlines-mobs-tests
    $<TARGET_OBJECTS:bahamut-test-targetlines-mobs-objects>
    tests/targetlines_mobs_tests.cpp
)
target_include_directories(bahamut-targetlines-mobs-tests PRIVATE src)

if(BAHAMUT_TARGETLINES_PROBE)
    add_executable(bahamut-targetlines-capture-tests
        src/targetlines_capture.cpp
        src/targetlines_capture.h
        tests/targetlines_capture_tests.cpp
    )
    target_include_directories(bahamut-targetlines-capture-tests PRIVATE src)
endif()
add_executable(bahamut-targetlines-projection-tests
    src/targetlines_projection.cpp
    src/targetlines_projection.h
    $<TARGET_OBJECTS:bahamut-test-world-projection-objects>
    tests/targetlines_projection_tests.cpp
)
target_include_directories(bahamut-targetlines-projection-tests PRIVATE src)

add_executable(bahamut-dat-overlay-tests
    src/dat_overlay.cpp
    src/dat_overlay.h
    tests/dat_overlay_tests.cpp
)
target_include_directories(bahamut-dat-overlay-tests PRIVATE
    api
    src
    vendor/minhook/include
)
target_compile_definitions(bahamut-dat-overlay-tests PRIVATE BAHAMUT_DAT_OVERLAY_TEST)
target_link_libraries(bahamut-dat-overlay-tests PRIVATE minhook)

add_executable(bahamut-camera-zoom-tests
    src/camera_zoom.cpp
    src/camera_zoom.h
    tests/camera_zoom_tests.cpp
)
target_include_directories(bahamut-camera-zoom-tests PRIVATE api src vendor/minhook/include)
target_link_libraries(bahamut-camera-zoom-tests PRIVATE minhook)

add_executable(bahamut-object-distance-tests
    src/object_distance.cpp
    src/object_distance.h
    tests/object_distance_tests.cpp
)
target_include_directories(bahamut-object-distance-tests PRIVATE api src vendor/minhook/include)
target_link_libraries(bahamut-object-distance-tests PRIVATE minhook)

add_executable(bahamut-packet-observer-tests
    src/packet_observer.cpp
    src/packet_observer.h
    src/packet_capture_policy.h
    tests/packet_observer_tests.cpp
)
target_include_directories(bahamut-packet-observer-tests PRIVATE src vendor/minhook/include vendor/miniz)
target_link_libraries(bahamut-packet-observer-tests PRIVATE minhook miniz ws2_32)

add_executable(bahamut-packet-capture-tests
    $<TARGET_OBJECTS:bahamut-test-packet-capture-objects>
    src/packet_capture.h
    src/packet_capture_policy.h
    tests/packet_capture_tests.cpp
)
target_include_directories(bahamut-packet-capture-tests PRIVATE src)

enable_testing()
add_test(
    NAME bahamut-loader
    COMMAND bahamut-loader-tests
        $<TARGET_FILE:bahamut-loader>
        $<TARGET_FILE:bahamut-test-client>
        $<TARGET_FILE:bahamut>
        $<TARGET_FILE:screenshot>
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/fps/addon.toml
        ${CMAKE_CURRENT_BINARY_DIR}/bootstrap-addon-settings
        ${CMAKE_CURRENT_BINARY_DIR}/bootstrap-chat-logs
)
add_test(NAME bahamut-monitor-selection COMMAND bahamut-monitor-selection-tests)
add_test(NAME bahamut-input-policy COMMAND bahamut-input-policy-tests)
add_test(NAME bahamut-action-movement-admission
    COMMAND bahamut-action-movement-admission-tests ${CMAKE_CURRENT_BINARY_DIR}/action-movement-admission-fixtures/$<CONFIG>)
set_tests_properties(bahamut-action-movement-admission PROPERTIES TIMEOUT 15)
add_test(NAME bahamut-action-movement-observer COMMAND bahamut-action-movement-observer-tests)
set_tests_properties(bahamut-action-movement-observer PROPERTIES TIMEOUT 15)
add_test(NAME bahamut-action-movement COMMAND bahamut-action-movement-tests)
set_tests_properties(bahamut-action-movement PROPERTIES TIMEOUT 15)
add_test(NAME bahamut-action-movement-addon
    COMMAND bahamut-action-movement-addon-tests ${CMAKE_CURRENT_BINARY_DIR}/action-movement-addon-fixtures)
set_tests_properties(bahamut-action-movement-addon PROPERTIES TIMEOUT 15)
add_test(NAME bahamut-action-movement-lua
    COMMAND bahamut-action-movement-lua-tests
        ${CMAKE_CURRENT_BINARY_DIR}/action-movement-lua-fixtures
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/nolock/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/fps/addon.toml)
set_tests_properties(bahamut-action-movement-lua PROPERTIES TIMEOUT 15)
add_test(NAME bahamut-autologin COMMAND bahamut-autologin-tests)
add_test(NAME bahamut-discord-rpc COMMAND bahamut-discord-rpc-tests)
if(CMAKE_CROSSCOMPILING_EMULATOR)
    # A cold Wine prefix can take minutes to initialize on the first test.
    set_tests_properties(bahamut-discord-rpc PROPERTIES TIMEOUT 300)
else()
    set_tests_properties(bahamut-discord-rpc PROPERTIES TIMEOUT 3)
endif()
add_test(NAME bahamut-startup-script COMMAND bahamut-startup-script-tests)
add_test(NAME bahamut-dat-overlay COMMAND bahamut-dat-overlay-tests)
add_test(NAME bahamut-camera-zoom COMMAND bahamut-camera-zoom-tests)
add_test(NAME bahamut-object-distance COMMAND bahamut-object-distance-tests)
add_test(NAME bahamut-packet-observer COMMAND bahamut-packet-observer-tests)
add_test(NAME bahamut-packet-capture COMMAND bahamut-packet-capture-tests)
add_test(
    NAME bahamut-addon-host
    COMMAND bahamut-addon-host-tests
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/fps/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/tests/fixtures/addon-error-isolation/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/pos/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/wiki/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/chatlogs/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/zonename/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/packetlogger/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/tests/fixtures/fault-packetlogger/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/distance/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/enemycast/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/targethp/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/customtimer/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/combatparser/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/targetlines/addon.toml
        ${CMAKE_CURRENT_BINARY_DIR}/addon-host-test-settings
        ${CMAKE_CURRENT_BINARY_DIR}/addon-host-test-chat-logs
)
add_test(NAME bahamut-command-boundary COMMAND bahamut-command-boundary-tests)
add_test(
    NAME bahamut-runtime-commands
    COMMAND bahamut-runtime-commands-tests
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/fps/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/pos/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/tests/fixtures/command-words/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/tests/fixtures/load-fault/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/tests/fixtures/command-table/addon.toml
        ${CMAKE_CURRENT_BINARY_DIR}/runtime-commands-test-settings
        ${CMAKE_CURRENT_BINARY_DIR}/runtime-commands-test-chat-logs
)
add_test(NAME bahamut-custom-timer COMMAND bahamut-custom-timer-tests)
add_test(NAME bahamut-enemy-cast COMMAND bahamut-enemy-cast-tests)
add_test(NAME bahamut-player-state COMMAND bahamut-player-state-tests)
add_test(NAME bahamut-target-distance COMMAND bahamut-target-distance-tests)
add_test(NAME bahamut-world-projection COMMAND bahamut-world-projection-tests)
add_test(NAME bahamut-targetlines-arc COMMAND bahamut-targetlines-arc-tests)
add_test(NAME bahamut-targetlines COMMAND bahamut-targetlines-tests)
add_test(NAME bahamut-targetlines-mobs COMMAND bahamut-targetlines-mobs-tests)
add_test(NAME bahamut-targetlines-projection COMMAND bahamut-targetlines-projection-tests)
if(BAHAMUT_TARGETLINES_PROBE)
    add_test(NAME bahamut-targetlines-capture COMMAND bahamut-targetlines-capture-tests)
endif()
