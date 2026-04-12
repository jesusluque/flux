# FetchFilament.cmake
# Uses a locally-built Filament dist if available at FILAMENT_LOCAL_DIST,
# otherwise downloads the prebuilt Filament v1.71.0 release tarball.
#
# Exposes:
#   FILAMENT_ROOT        — prefix directory
#   FILAMENT_INCLUDE_DIR — <prefix>/include
#   FILAMENT_LIB_DIR     — <prefix>/lib/<arch>
#   MATC_PATH            — <prefix>/bin/matc

# ── Local build override ──────────────────────────────────────────────────────
# If a locally-built dist exists (from jesusluque/filament source build),
# use it directly without downloading anything.
set(_FILAMENT_LOCAL_DIST "$ENV{HOME}/luc/filament/out/release/filament")

if(EXISTS "${_FILAMENT_LOCAL_DIST}/include/filament/Engine.h")
    message(STATUS "FetchFilament: using local source build at ${_FILAMENT_LOCAL_DIST}")
    set(FILAMENT_ROOT "${_FILAMENT_LOCAL_DIST}" CACHE PATH "Filament root")
else()
    # ── Pre-built binary fallback ─────────────────────────────────────────────
    include(FetchContent)

    if(APPLE)
        set(_FILAMENT_TARBALL "filament-v1.71.0-mac.tgz")
    elseif(UNIX)
        # Distinguish arm64 Linux from x86_64 Linux
        if(CMAKE_SYSTEM_PROCESSOR MATCHES "aarch64|arm64")
            set(_FILAMENT_TARBALL "filament-v1.71.0-arm-linux.tgz")
        else()
            set(_FILAMENT_TARBALL "filament-v1.71.0-linux.tgz")
        endif()
    elseif(WIN32)
        set(_FILAMENT_TARBALL "filament-v1.71.0-windows.tgz")
    else()
        message(FATAL_ERROR "FetchFilament: unsupported platform")
    endif()

    set(_FILAMENT_URL
        "https://github.com/google/filament/releases/download/v1.71.0/${_FILAMENT_TARBALL}")

    message(STATUS "FetchFilament: fetching ${_FILAMENT_TARBALL}")

    FetchContent_Declare(
        filament_dist
        URL            "${_FILAMENT_URL}"
        DOWNLOAD_EXTRACT_TIMESTAMP TRUE
    )

    # Populate (download + extract) but do NOT add as a CMake subdirectory —
    # it's a prebuilt binary distribution, not a CMake source tree.
    FetchContent_GetProperties(filament_dist)
    if(NOT filament_dist_POPULATED)
        FetchContent_Populate(filament_dist)
    endif()

    set(FILAMENT_ROOT "${filament_dist_SOURCE_DIR}" CACHE PATH "Filament root")
endif()

set(FILAMENT_INCLUDE_DIR "${FILAMENT_ROOT}/include"   CACHE PATH     "Filament include dir")
set(FILAMENT_LIB_DIR     "${FILAMENT_ROOT}/lib/arm64" CACHE PATH     "Filament lib dir")
set(MATC_PATH            "${FILAMENT_ROOT}/bin/matc"  CACHE FILEPATH "Filament matc compiler")

# On Linux the lib subdir is different
if(UNIX AND NOT APPLE)
    set(FILAMENT_LIB_DIR "${FILAMENT_ROOT}/lib/aarch64" CACHE PATH "Filament lib dir" FORCE)
    if(CMAKE_SYSTEM_PROCESSOR MATCHES "x86_64")
        set(FILAMENT_LIB_DIR "${FILAMENT_ROOT}/lib/x86_64" CACHE PATH "Filament lib dir" FORCE)
    endif()
endif()

message(STATUS "FetchFilament: FILAMENT_ROOT     = ${FILAMENT_ROOT}")
message(STATUS "FetchFilament: FILAMENT_LIB_DIR  = ${FILAMENT_LIB_DIR}")
message(STATUS "FetchFilament: MATC_PATH         = ${MATC_PATH}")
