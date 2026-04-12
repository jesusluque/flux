cmake_minimum_required(VERSION 3.20)
project(gst-fluxvideotex LANGUAGES C CXX)

set(CMAKE_C_STANDARD   11)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# ── GStreamer (via pkg-config) ──────────────────────────────────────────────
find_package(PkgConfig REQUIRED)
pkg_check_modules(GST      REQUIRED gstreamer-1.0>=1.20)
pkg_check_modules(GST_BASE REQUIRED gstreamer-base-1.0>=1.20)
pkg_check_modules(GST_VID  REQUIRED gstreamer-video-1.0>=1.20)

# ── Embed cube.glb via xxd ─────────────────────────────────────────────────
# FILAMENT_ROOT and FILAMENT_LIB_DIR are set by FetchFilament.cmake in parent.
set(CUBE_GLB_SRC "${CMAKE_CURRENT_SOURCE_DIR}/assets/cube.glb")
set(CUBE_GLB_HDR "${CMAKE_CURRENT_BINARY_DIR}/cube_glb.h")

add_custom_command(
    OUTPUT  "${CUBE_GLB_HDR}"
    COMMAND xxd -i cube.glb > "${CUBE_GLB_HDR}"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/assets"
    DEPENDS "${CUBE_GLB_SRC}"
    COMMENT "Embedding cube.glb as C byte array: cube_glb.h"
)

# ── Filament import targets ────────────────────────────────────────────────
# Core rendering libs + gltfio for GLB loading
foreach(_lib
        filament backend bluegl bluevk utils filaflat filabridge geometry shaders
        gltfio gltfio_core
        ktxreader stb image dracodec meshoptimizer
        uberarchive uberzlib abseil
        smol-v zstd)
    if(EXISTS "${FILAMENT_LIB_DIR}/lib${_lib}.a")
        add_library(filament::${_lib} STATIC IMPORTED GLOBAL)
        set_target_properties(filament::${_lib} PROPERTIES
            IMPORTED_LOCATION "${FILAMENT_LIB_DIR}/lib${_lib}.a")
    endif()
endforeach()

# ── Plugin target ──────────────────────────────────────────────────────────
add_library(gstfluxvideotex MODULE
    fluxvideotex.c
    filament_scene.cpp
    "${CUBE_GLB_HDR}"         # ensures the custom command runs
)

# GStreamer expects the plugin to be named libgstfluxvideotex.so / .dylib
set_target_properties(gstfluxvideotex PROPERTIES
    PREFIX      "lib"
    OUTPUT_NAME "gstfluxvideotex"
)

target_include_directories(gstfluxvideotex PRIVATE
    "${CMAKE_CURRENT_SOURCE_DIR}"
    "${CMAKE_CURRENT_BINARY_DIR}"       # for cube_glb.h
    "${FILAMENT_INCLUDE_DIR}"
    ${GST_INCLUDE_DIRS}
    ${GST_BASE_INCLUDE_DIRS}
    ${GST_VID_INCLUDE_DIRS}
)

target_compile_options(gstfluxvideotex PRIVATE
    ${GST_CFLAGS_OTHER}
    ${GST_BASE_CFLAGS_OTHER}
    ${GST_VID_CFLAGS_OTHER}
)

# ── Link Filament + system frameworks + GStreamer ──────────────────────────
# Static link order matters: higher-level libs first.
target_link_libraries(gstfluxvideotex PRIVATE
    filament::gltfio
    filament::gltfio_core
    filament::filament
    filament::backend
    filament::bluegl
    filament::shaders
    filament::filaflat
    filament::filabridge
    filament::geometry
    filament::utils
    filament::ktxreader
    filament::stb
    filament::image
    filament::dracodec
    filament::meshoptimizer
    filament::uberarchive
    filament::uberzlib
    filament::abseil
    filament::bluevk
    filament::smol-v
    filament::zstd
    # GStreamer
    ${GST_LDFLAGS}
    ${GST_BASE_LDFLAGS}
    ${GST_VID_LDFLAGS}
)

if(APPLE)
    target_link_libraries(gstfluxvideotex PRIVATE
        "-framework Metal"
        "-framework CoreGraphics"
        "-framework Foundation"
        "-framework CoreVideo"
        "-framework IOSurface"
        "-framework QuartzCore"
        "-framework OpenGL"
        "-framework AppKit"
    )
elseif(UNIX)
    target_link_libraries(gstfluxvideotex PRIVATE
        GL EGL dl pthread
    )
endif()

# ── Install (optional — for GST_PLUGIN_PATH use) ───────────────────────────
install(TARGETS gstfluxvideotex
    LIBRARY DESTINATION "${CMAKE_INSTALL_PREFIX}/lib/gstreamer-1.0")
