cmake_minimum_required(VERSION 3.20)
project(flux-filament-tools LANGUAGES C CXX)

set(CMAKE_C_STANDARD   11)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# ── macOS: ensure libc++ headers are found ────────────────────────────────────
# The CommandLineTools compiler may not find its own C++ stdlib headers without
# an explicit sysroot when CMAKE_OSX_SYSROOT is not set by the generator.
if(APPLE AND NOT CMAKE_OSX_SYSROOT)
    execute_process(
        COMMAND xcrun --sdk macosx --show-sdk-path
        OUTPUT_VARIABLE APPLE_SDK_PATH
        OUTPUT_STRIP_TRAILING_WHITESPACE
    )
    if(APPLE_SDK_PATH)
        set(CMAKE_OSX_SYSROOT "${APPLE_SDK_PATH}" CACHE PATH "macOS SDK sysroot" FORCE)
        message(STATUS "Set CMAKE_OSX_SYSROOT = ${APPLE_SDK_PATH}")
    endif()
endif()

# The macOS 26.x SDK ships libc++ headers but they may not be on the default
# search path when using /usr/bin/c++ from CommandLineTools.  Add them explicitly.
if(APPLE AND CMAKE_OSX_SYSROOT)
    set(LIBCXX_INCLUDE "${CMAKE_OSX_SYSROOT}/usr/include/c++/v1")
    if(EXISTS "${LIBCXX_INCLUDE}")
        add_compile_options($<$<COMPILE_LANGUAGE:CXX>:-I${LIBCXX_INCLUDE}>)
        message(STATUS "Added libc++ include: ${LIBCXX_INCLUDE}")
    endif()
endif()

# ── Fetch prebuilt Filament ────────────────────────────────────────────────────
list(APPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/cmake")
include(FetchFilament)

# ── Sub-projects ──────────────────────────────────────────────────────────────
add_subdirectory(gst-fluxvideotex)
