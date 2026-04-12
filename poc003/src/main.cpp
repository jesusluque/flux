/*
 * poc003/src/main.cpp — FLUX poc003: fluxvideotex demo
 *
 * Demonstrates FLUX Protocol Spec v0.6.3 §16 fluxvideotex element:
 *   - videotestsrc generates an animated smpte pattern
 *   - fluxvideotex uploads each frame as a GPU texture onto a Filament-
 *     rendered unit cube, with slow multi-axis rotation over 5 minutes
 *   - osxvideosink displays the rendered output
 *
 * Pipeline:
 *   videotestsrc pattern=smpte is-live=true
 *     ! videoconvert
 *     ! video/x-raw,format=RGBA,width=1280,height=720,framerate=30/1
 *     ! fluxvideotex width=1280 height=720
 *     ! video/x-raw,format=RGBA,width=1280,height=720
 *     ! osxvideosink sync=false
 *
 * Run for 300 seconds (5 minutes) then exit cleanly.  Ctrl-C also works.
 */

#include <gst/gst.h>
#include <gst/gstmacos.h>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>

/* Demo duration in milliseconds (5 minutes) */
static const guint DEMO_DURATION_MS = 300U * 1000U;

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

/* Timeout callback: stop after DEMO_DURATION_MS */
static gboolean on_timeout(gpointer pipeline)
{
    g_print("poc003: 5-minute demo complete, stopping.\n");
    gst_element_send_event(GST_ELEMENT(pipeline), gst_event_new_eos());
    return G_SOURCE_REMOVE;
}

static int real_main(int argc, char* argv[])
{
    gst_init(&argc, &argv);

    g_print("poc003: FLUX fluxvideotex demo — Filament textured cube\n");
    g_print("         FLUX Protocol Spec v0.6.3 §16\n");
    g_print("         Pattern: smpte   Duration: 5 min\n\n");

    /* ── Build pipeline ─────────────────────────────────────────────────── */
    GError*  err      = nullptr;
    gchar*   pipe_str = g_strdup_printf(
        "videotestsrc pattern=smpte is-live=true "
        "! videoconvert "
        "! video/x-raw,format=RGBA,width=1280,height=720,framerate=30/1 "
        "! fluxvideotex name=vt width=1280 height=720 "
        "    rotation-period-x=150 rotation-period-y=200 rotation-period-z=300 "
        "! video/x-raw,format=RGBA,width=1280,height=720 "
        "! videoconvert "
        "! osxvideosink sync=false");

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

    /* Schedule auto-stop after 5 minutes */
    g_timeout_add(DEMO_DURATION_MS, on_timeout, pipeline);

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
