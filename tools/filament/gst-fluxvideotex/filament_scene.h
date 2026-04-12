/*
 * filament_scene.h — plain C interface to the Filament offscreen renderer
 *
 * Used by fluxvideotex.c (GStreamer element) to keep C/C++ clean separation.
 * The implementation is in filament_scene.cpp.
 *
 * The renderer loads a GLB asset (cube.glb, embedded via xxd at build time)
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
 * Create a Filament offscreen renderer at the given pixel dimensions.
 * glb_data / glb_size: the raw GLB bytes (embedded via xxd).
 * Returns NULL on failure.
 */
FilamentScene* filament_scene_create(int width, int height,
                                     const uint8_t* glb_data,
                                     size_t         glb_size);

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
 */
void filament_scene_render(FilamentScene* scene,
                           const uint8_t* in_rgba, int in_w, int in_h,
                           double elapsed_s,
                           double period_x, double period_y, double period_z,
                           uint8_t* out_rgba);

#ifdef __cplusplus
} /* extern "C" */
#endif
