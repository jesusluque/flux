/*
 * fluxvideotex.h — GStreamer element type declarations for fluxvideotex
 *
 * fluxvideotex — FLUX Protocol Spec v0.6.3 §16
 *   Resolves video_texture_bindings and flux:// image URIs, composites
 *   multi-channel bindings, uploads GPU textures.
 *
 *   poc003 implementation: single sink/src pad BaseTransform that uploads
 *   each incoming RGBA video frame as a Filament texture, renders it onto
 *   a rotating cube (offscreen), and emits the composited RGBA frame.
 */

#pragma once

#include <gst/gst.h>
#include <gst/base/gstbasetransform.h>

G_BEGIN_DECLS

/* ── Type macros ─────────────────────────────────────────────────────────── */
#define FLUX_TYPE_VIDEOTEX            (flux_videotex_get_type())
#define FLUX_VIDEOTEX(obj)            (G_TYPE_CHECK_INSTANCE_CAST((obj), FLUX_TYPE_VIDEOTEX, FluxVideotex))
#define FLUX_VIDEOTEX_CLASS(klass)    (G_TYPE_CHECK_CLASS_CAST((klass),  FLUX_TYPE_VIDEOTEX, FluxVideotexClass))
#define FLUX_IS_VIDEOTEX(obj)         (G_TYPE_CHECK_INSTANCE_TYPE((obj), FLUX_TYPE_VIDEOTEX))
#define FLUX_IS_VIDEOTEX_CLASS(klass) (G_TYPE_CHECK_CLASS_TYPE((klass),  FLUX_TYPE_VIDEOTEX))
#define FLUX_VIDEOTEX_GET_CLASS(obj)  (G_TYPE_INSTANCE_GET_CLASS((obj),  FLUX_TYPE_VIDEOTEX, FluxVideotexClass))

typedef struct _FluxVideotex      FluxVideotex;
typedef struct _FluxVideotexClass FluxVideotexClass;

/* Forward declaration of the opaque Filament renderer */
typedef struct FilamentScene FilamentScene;

struct _FluxVideotex {
    GstBaseTransform parent;

    /* ── Properties ─────────────────────────────────────────────────── */
    guint   out_width;       /* render output width  (default 1280) */
    guint   out_height;      /* render output height (default 720)  */
    gdouble period_x;        /* X-rotation period in seconds (150)  */
    gdouble period_y;        /* Y-rotation period in seconds (200)  */
    gdouble period_z;        /* Z-rotation period in seconds (300)  */

    /* ── Runtime state ──────────────────────────────────────────────── */
    FilamentScene* scene;     /* NULL until first buffer             */
    gint64         start_ns;  /* monotonic clock at first buffer     */
};

struct _FluxVideotexClass {
    GstBaseTransformClass parent_class;
};

GType flux_videotex_get_type(void);

/* Plugin registration helper — called from plugin_init */
gboolean flux_videotex_register(GstPlugin* plugin);

G_END_DECLS
