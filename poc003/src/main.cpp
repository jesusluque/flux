/*
 * poc003/src/main.cpp — FLUX poc003: fluxvideotex demo
 *
 * Demonstrates FLUX Protocol Spec v0.6.3 §16 fluxvideotex element:
 *   - videotestsrc generates an animated smpte pattern
 *   - fluxvideotex uploads each frame as a GPU texture onto a Filament-
 *     rendered unit cube, with slow multi-axis rotation over 5 minutes
 *   - osxvideosink displays the rendered output
 *
 * Usage:
 *   poc003 [--color-space <mode>] [--ycbcr] [--glb <path>]
 *
 *   --color-space  One of: srgb (default), bt709, rec709-linear,
 *                          rec2020-linear, rec2020-pq, rec2020-hlg
 *   --ycbcr        Enable Y'CbCr output encoding (ycbcr-output=true)
 *   --glb <path>   Path to a GLB file to use instead of the built-in cube
 *   --duration N   Run for N seconds (default 10)
 *
 * Without arguments runs in the original 5-minute mode.
 *
 * Pipeline:
 *   videotestsrc pattern=smpte is-live=true
 *     ! videoconvert
 *     ! video/x-raw,format=RGBA,width=1280,height=720,framerate=30/1
 *     ! fluxvideotex width=1280 height=720 color-space=<mode> ycbcr-output=<bool>
 *     ! video/x-raw,format=<RGBA|AYUV>,width=1280,height=720
 *     ! videoconvert
 *     ! osxvideosink sync=false
 */

#include <gst/gst.h>
#include <gst/gstmacos.h>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>

static GMainLoop* g_loop = nullptr;

static void on_sigint(int)
{
    if (g_loop)
        g_main_loop_quit(g_loop);
}

static gboolean on_bus_message(GstBus* /*bus*/, GstMessage* msg, gpointer /*data*/)
{
    switch (GST_MESSAGE_TYPE(msg)) {
    case GST_MESSAGE_EOS:
        g_print("poc003: EOS received\n");
        g_main_loop_quit(g_loop);
        break;
    case GST_MESSAGE_ERROR: {
        GError* err = nullptr;
        gchar*  dbg = nullptr;
        gst_message_parse_error(msg, &err, &dbg);
        g_printerr("poc003 ERROR from %s: %s\n%s\n",
                   GST_OBJECT_NAME(msg->src), err->message, dbg ? dbg : "");
        g_error_free(err);
        g_free(dbg);
        g_main_loop_quit(g_loop);
        break;
    }
    case GST_MESSAGE_WARNING: {
        GError* err = nullptr;
        gchar*  dbg = nullptr;
        gst_message_parse_warning(msg, &err, &dbg);
        g_printerr("poc003 WARNING from %s: %s\n%s\n",
                   GST_OBJECT_NAME(msg->src), err->message, dbg ? dbg : "");
        g_error_free(err);
        g_free(dbg);
        break;
    }
    default:
        break;
    }
    return TRUE;
}

static gboolean on_timeout(gpointer pipeline)
{
    g_print("poc003: demo duration reached, stopping.\n");
    gst_element_send_event(GST_ELEMENT(pipeline), gst_event_new_eos());
    return G_SOURCE_REMOVE;
}

static int real_main(int argc, char* argv[])
{
    /* ── Init GStreamer first with original argv (gst_macos_main requires
     *    argv[0] to survive; do NOT pass a stack-local copy) ─────────── */
    gst_init(&argc, &argv);

    /* ── Parse our own arguments after gst_init (GStreamer strips its own) */
    const char* color_space  = "srgb";
    bool        ycbcr        = false;
    const char* glb_file     = nullptr;
    guint       duration_ms  = 300U * 1000U;  /* default: 5 min */

    for (int i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--color-space") == 0 && i + 1 < argc) {
            color_space = argv[++i];
        } else if (strcmp(argv[i], "--ycbcr") == 0) {
            ycbcr = true;
        } else if (strcmp(argv[i], "--glb") == 0 && i + 1 < argc) {
            glb_file = argv[++i];
        } else if (strcmp(argv[i], "--duration") == 0 && i + 1 < argc) {
            duration_ms = (guint)(atoi(argv[++i]) * 1000);
        }
    }

    /* Output format on the src pad depends on ycbcr flag */
    const char* out_fmt = ycbcr ? "AYUV" : "RGBA";

    g_print("poc003: FLUX fluxvideotex demo — Filament textured cube\n");
    g_print("         FLUX Protocol Spec v0.6.3 §16\n");
    g_print("         color-space: %s   ycbcr-output: %s   duration: %u s\n",
            color_space, ycbcr ? "true" : "false", duration_ms / 1000);
    g_print("         glb-file: %s\n\n", glb_file ? glb_file : "(built-in cube)");

    /* ── Build pipeline ─────────────────────────────────────────────────── */
    GError* err      = nullptr;
    gchar*  glb_prop = glb_file
        ? g_strdup_printf("glb-file=\"%s\"", glb_file)
        : g_strdup("");

    gchar*  pipe_str = g_strdup_printf(
        "videotestsrc pattern=smpte is-live=true "
        "! videoconvert "
        "! video/x-raw,format=RGBA,width=1280,height=720,framerate=30/1 "
        "! fluxvideotex name=vt width=1280 height=720 "
        "    rotation-period-x=150 rotation-period-y=200 rotation-period-z=300 "
        "    color-space=%s ycbcr-output=%s %s "
        "! video/x-raw,format=%s,width=1280,height=720 "
        "! videoconvert "
        "! glimagesink sync=false",
        color_space,
        ycbcr ? "true" : "false",
        glb_prop,
        out_fmt);
    g_free(glb_prop);

    g_print("poc003: pipeline: %s\n\n", pipe_str);

    GstElement* pipeline = gst_parse_launch(pipe_str, &err);
    g_free(pipe_str);

    if (!pipeline || err) {
        g_printerr("poc003: failed to build pipeline: %s\n",
                   err ? err->message : "(unknown)");
        if (err) g_error_free(err);
        return EXIT_FAILURE;
    }

    /* ── Bus ────────────────────────────────────────────────────────────── */
    GstBus* bus = gst_element_get_bus(pipeline);
    gst_bus_add_watch(bus, on_bus_message, nullptr);
    gst_object_unref(bus);

    /* ── Main loop ──────────────────────────────────────────────────────── */
    g_loop = g_main_loop_new(nullptr, FALSE);
    std::signal(SIGINT, on_sigint);

    /* ── Start ──────────────────────────────────────────────────────────── */
    GstStateChangeReturn ret = gst_element_set_state(pipeline, GST_STATE_PLAYING);
    if (ret == GST_STATE_CHANGE_FAILURE) {
        g_printerr("poc003: could not set pipeline to PLAYING\n");
        gst_object_unref(pipeline);
        return EXIT_FAILURE;
    }

    g_timeout_add(duration_ms, on_timeout, pipeline);

    g_print("poc003: pipeline running — press Ctrl-C to stop early\n");
    g_main_loop_run(g_loop);

    /* ── Teardown ────────────────────────────────────────────────────────── */
    g_print("poc003: stopping pipeline\n");
    gst_element_set_state(pipeline, GST_STATE_NULL);
    gst_object_unref(pipeline);
    g_main_loop_unref(g_loop);

    g_print("poc003: done.\n");
    return EXIT_SUCCESS;
}

int main(int argc, char* argv[])
{
    return gst_macos_main((GstMainFunc)real_main, argc, argv, nullptr);
}
