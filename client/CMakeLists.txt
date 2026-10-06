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
    api/native_plugin_api.h
    api/runtime_api.h
    ${BAHAMUT_RUNTIME_DEF}
    src/actor_name.cpp
    src/actor_name.h
    src/camera_zoom.cpp
    src/camera_zoom.h
    src/addon_host.cpp
    src/addon_host.h
    src/chat_boundary.cpp
    src/chat_boundary.h
    src/command_boundary.cpp
    src/command_boundary.h
    src/player_state.cpp
    src/player_state.h
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

add_executable(bahamut-test-client WIN32
    src/test_client.cpp
    src/monitor_selection.cpp
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
    src/monitor_selection.cpp
)
target_include_directories(bahamut-loader-tests PRIVATE src)
target_link_libraries(bahamut-loader-tests PRIVATE user32)
bahamut_unicode_entry(bahamut-loader-tests)

add_executable(bahamut-monitor-selection-tests
    src/monitor_selection.cpp
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

add_executable(bahamut-addon-host-tests
    src/actor_name.cpp
    src/actor_name.h
    src/addon_host.cpp
    src/addon_host.h
    src/player_state.cpp
    src/player_state.h
    src/target_distance.cpp
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

add_executable(bahamut-command-boundary-tests
    src/actor_name.cpp
    src/actor_name.h
    src/addon_host.cpp
    src/addon_host.h
    src/chat_boundary.cpp
    src/chat_boundary.h
    src/command_boundary.cpp
    src/command_boundary.h
    src/player_state.cpp
    src/player_state.h
    src/retail_identity.cpp
    src/retail_identity.h
    src/target_distance.cpp
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
    src/player_state.cpp
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
    src/actor_name.cpp
    src/actor_name.h
    src/player_state.cpp
    src/player_state.h
    src/target_distance.cpp
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
    src/world_projection.cpp
    src/world_projection.h
    tests/world_projection_tests.cpp
)
target_include_directories(bahamut-world-projection-tests PRIVATE src)

add_executable(bahamut-targetlines-arc-tests
    src/targetlines_arc.cpp
    src/targetlines_arc.h
    src/world_projection.cpp
    tests/targetlines_arc_tests.cpp
)
target_include_directories(bahamut-targetlines-arc-tests PRIVATE src)

add_executable(bahamut-targetlines-tests
    src/actor_name.cpp
    src/player_state.cpp
    src/target_distance.cpp
    src/targetlines.cpp
    src/targetlines_mobs.cpp
    tests/targetlines_tests.cpp
)
target_include_directories(bahamut-targetlines-tests PRIVATE api src vendor/minhook/include)
target_link_libraries(bahamut-targetlines-tests PRIVATE minhook)

add_executable(bahamut-targetlines-mobs-tests
    src/targetlines_mobs.cpp
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
    src/world_projection.cpp
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
    tests/packet_observer_tests.cpp
)
target_include_directories(bahamut-packet-observer-tests PRIVATE src vendor/minhook/include vendor/miniz)
target_link_libraries(bahamut-packet-observer-tests PRIVATE minhook miniz ws2_32)

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
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/targethp/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/combatparser/addon.toml
        ${CMAKE_CURRENT_SOURCE_DIR}/../addons/targetlines/addon.toml
        ${CMAKE_CURRENT_BINARY_DIR}/addon-host-test-settings
        ${CMAKE_CURRENT_BINARY_DIR}/addon-host-test-chat-logs
)
add_test(NAME bahamut-command-boundary COMMAND bahamut-command-boundary-tests)
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
