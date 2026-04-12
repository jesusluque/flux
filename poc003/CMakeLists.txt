cmake_minimum_required(VERSION 3.20)
project(poc003 LANGUAGES C CXX)

set(CMAKE_C_STANDARD   11)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# ── macOS SDK / libc++ fix (same as tools/filament) ───────────────────────
if(APPLE)
    execute_process(
        COMMAND xcrun --sdk macosx --show-sdk-path
        OUTPUT_VARIABLE _SDK_PATH OUTPUT_STRIP_TRAILING_WHITESPACE)
    if(_SDK_PATH)
        set(CMAKE_OSX_SYSROOT "${_SDK_PATH}" CACHE PATH "macOS SDK" FORCE)
        message(STATUS "macOS SDK: ${_SDK_PATH}")
        add_compile_options("-I${_SDK_PATH}/usr/include/c++/v1")
    endif()
endif()

# ── GStreamer ──────────────────────────────────────────────────────────────
find_package(PkgConfig REQUIRED)
pkg_check_modules(GST      REQUIRED gstreamer-1.0>=1.20)
pkg_check_modules(GST_BASE REQUIRED gstreamer-base-1.0>=1.20)

# ── poc003 demo binary ─────────────────────────────────────────────────────
add_executable(poc003 src/main.cpp)

target_include_directories(poc003 PRIVATE
    ${GST_INCLUDE_DIRS}
    ${GST_BASE_INCLUDE_DIRS}
)

target_compile_options(poc003 PRIVATE
    ${GST_CFLAGS_OTHER}
)

target_link_libraries(poc003 PRIVATE
    ${GST_LDFLAGS}
    ${GST_BASE_LDFLAGS}
)

# ── Print plugin path hint at configure time ───────────────────────────────
message(STATUS "")
message(STATUS "poc003 build configured.")
message(STATUS "Before running poc003, set:")
message(STATUS "  export GST_PLUGIN_PATH=<build-dir-of-tools/filament>/gst-fluxvideotex")
message(STATUS "")
