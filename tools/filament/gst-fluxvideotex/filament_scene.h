/*
 * filament_scene.h — plain C interface to the Filament offscreen renderer
 *
 * Used by fluxvideotex.c (GStreamer element) to keep C/C++ clean separation.
 * The implementation is in filament_scene.cpp.
 *
 * The renderer loads a GLB asset (cube.glb, embedded at build time via xxd)
 * whose baseColorTexture has uri "flux://channel/0" per FLUX Protocol Spec
 * v0.6.3 §10.10.  Each frame, the live video buffer replaces that texture.
 */

#pragma once

#ifdef __cplusplus
extern "C" {
#endif

#include <stddef.h>
#include <stdint.h>

/* Opaque handle to the Filament scene + offscreen swap chain */
typedef struct FilamentScene FilamentScene;

/*
 * Color-space mode constants — must match FluxColorSpaceMode in fluxvideotex.h.
 * Passed as an int to keep the C interface free of C++ types.
 *
 *   FILAMENT_CS_SRGB           Rec709  - sRGB   - D65  (default)
 *   FILAMENT_CS_BT709          Rec709  - BT709  - D65
 *   FILAMENT_CS_REC709_LINEAR  Rec709  - Linear - D65
 *   FILAMENT_CS_REC2020_LINEAR Rec2020 - Linear - D65
 *   FILAMENT_CS_REC2020_PQ     Rec2020 - PQ     - D65
 *   FILAMENT_CS_REC2020_HLG    Rec2020 - HLG    - D65
 */
#define FILAMENT_CS_SRGB           0
#define FILAMENT_CS_BT709          1
#define FILAMENT_CS_REC709_LINEAR  2
#define FILAMENT_CS_REC2020_LINEAR 3
#define FILAMENT_CS_REC2020_PQ     4
#define FILAMENT_CS_REC2020_HLG    5

/*
 * Create a Filament offscreen renderer at the given pixel dimensions.
 *
 * glb_data / glb_size : raw GLB bytes (embedded via xxd).
 * color_space_mode    : one of FILAMENT_CS_* constants above.
 * ycbcr_output        : non-zero → encode output as Y'CbCr via ColorGrading.
 *
 * Returns NULL on failure.
 */
FilamentScene* filament_scene_create(int width, int height,
                                     const uint8_t* glb_data,
                                     size_t         glb_size,
                                     int            color_space_mode,
                                     int            ycbcr_output);

/*
 * Destroy the renderer and free all Filament resources.
 */
void filament_scene_destroy(FilamentScene* scene);

/*
 * Upload in_rgba (in_w × in_h × 4 bytes, RGBA8) as the live video texture,
 * animate the cube rotation based on elapsed_s, render offscreen, and read
 * back the result into out_rgba (scene->width × scene->height × 4 bytes).
 *
 * period_{x,y,z}: seconds per full 2π rotation around each axis.
 *
 * When ycbcr_output was set at create time the output buffer contains packed
 * Y'CbCr (R=Y', G=Cb, B=Cr, A=1) rather than RGBA.
 */
void filament_scene_render(FilamentScene* scene,
                           const uint8_t* in_rgba, int in_w, int in_h,
                           double elapsed_s,
                           double period_x, double period_y, double period_z,
                           uint8_t* out_rgba);

#ifdef __cplusplus
} /* extern "C" */
#endif
