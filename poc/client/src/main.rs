//! FLUX PoC — Client
//!
//! Pipeline:
//!   fluxsrc address=127.0.0.1 port=7400
//!     → fluxdemux  (routes media_0 and control/cdbc pads)
//!         media_0 → fluxdeframer
//!                     → h265parse
//!                     → video/x-h265,stream-format=hvc1,alignment=au
//!                     → vtdec_hw
//!                     → videoconvertscale
//!                     → osxvideosink
//!         cdbc    → fluxcdbc server-address=127.0.0.1 server-port=7400 → fakesink
//!
//! Fixed decode chain: vtdec_hw requires hvc1/hev1 format (not byte-stream).
//! h265parse converts byte-stream → hvc1 with codec_data in caps.
//!
//! The CDBC element observes frames on the cdbc pad (which in the PoC also
//! passes through MEDIA_DATA) and sends CDBC_FEEDBACK back to the server.

use glib::ControlFlow;
use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;

fn main() {
    env_logger::init();

    gst::init().expect("GStreamer init failed");

    // On macOS, GStreamer video sinks require NSApplication to be running on
    // the main thread. gst::macos_main() sets this up and runs our closure on
    // the main thread's run loop, which is required for osxvideosink.
    gst::macos_main(run);
}

fn run() {
    // Register custom elements
    gstfluxsrc::plugin_register_static().expect("fluxsrc register");
    gstfluxdemux::plugin_register_static().expect("fluxdemux register");
    gstfluxdeframer::plugin_register_static().expect("fluxdeframer register");
    gstfluxcdbc::plugin_register_static().expect("fluxcdbc register");

    // ── Build pipeline programmatically ───────────────────────────────────────

    let pipeline = gst::Pipeline::new();

    // Source
    let fluxsrc = gst::ElementFactory::make("fluxsrc")
        .property("address", "127.0.0.1")
        .property("port", 7400u32)
        .build()
        .expect("fluxsrc element");

    // Demux
    let fluxdemux = gst::ElementFactory::make("fluxdemux")
        .build()
        .expect("fluxdemux element");

    // Media branch: deframe → parse → decode → convert → display
    let fluxdeframer = gst::ElementFactory::make("fluxdeframer")
        .build()
        .expect("fluxdeframer element");

    // h265parse converts byte-stream → hvc1 (required by vtdec_hw)
    let h265parse = gst::ElementFactory::make("h265parse")
        .build()
        .expect("h265parse element");

    // vtdec_hw: Apple VideoToolbox HW-only H.265 decoder
    // Requires stream-format=hvc1 or hev1 with codec_data in caps
    let vtdec_hw = gst::ElementFactory::make("vtdec_hw")
        .build()
        .expect("vtdec_hw element");

    let convert = gst::ElementFactory::make("videoconvertscale")
        .build()
        .expect("videoconvertscale element");

    let sink = gst::ElementFactory::make("osxvideosink")
        .property("sync", false)
        .property("async", false)
        .build()
        .expect("osxvideosink element");

    // CDBC branch: measure → fakesink
    let fluxcdbc = gst::ElementFactory::make("fluxcdbc")
        .property("server-address", "127.0.0.1")
        .property("server-port", 7400u32)
        .property("cdbc-interval", 50u64)
        .property("cdbc-min-interval", 10u64)
        .build()
        .expect("fluxcdbc element");

    let fakesink = gst::ElementFactory::make("fakesink")
        .property("sync", false)
        .build()
        .expect("fakesink element");

    // Add all elements to the pipeline
    pipeline
        .add_many([
            &fluxsrc,
            &fluxdemux,
            &fluxdeframer,
            &h265parse,
            &vtdec_hw,
            &convert,
            &sink,
            &fluxcdbc,
            &fakesink,
        ])
        .expect("add elements");

    // Link the fixed media decode chain
    // fluxsrc → fluxdemux (dynamic pads linked below via pad-added)
    fluxsrc.link(&fluxdemux).expect("fluxsrc → fluxdemux");
    // fluxdeframer → h265parse → (hvc1 caps) → vtdec_hw → videoconvertscale → osxvideosink
    fluxdeframer.link(&h265parse).expect("deframer → h265parse");

    // Force h265parse output to hvc1 (required by vtdec_hw)
    let hvc1_caps = gst::Caps::builder("video/x-h265")
        .field("stream-format", "hvc1")
        .field("alignment", "au")
        .build();
    h265parse
        .link_filtered(&vtdec_hw, &hvc1_caps)
        .expect("h265parse → vtdec_hw (hvc1)");

    vtdec_hw
        .link(&convert)
        .expect("vtdec_hw → videoconvertscale");
    convert
        .link(&sink)
        .expect("videoconvertscale → osxvideosink");
    fluxcdbc.link(&fakesink).expect("fluxcdbc → fakesink");

    // Connect to pad-added for the dynamic pads from fluxdemux
    let deframer_clone = fluxdeframer.clone();
    let cdbc_clone = fluxcdbc.clone();
    fluxdemux.connect_pad_added(move |_elem, pad| {
        let pad_name = pad.name();
        eprintln!("[flux-client] fluxdemux pad added: {}", pad_name);

        match pad_name.as_str() {
            "media_0" => {
                let sink_pad = deframer_clone
                    .static_pad("sink")
                    .expect("deframer sink pad");
                if sink_pad.is_linked() {
                    eprintln!("[flux-client] deframer sink already linked, skipping");
                    return;
                }
                pad.link(&sink_pad).expect("link media_0 → fluxdeframer");
                eprintln!("[flux-client] media_0 → fluxdeframer linked");
            }
            "cdbc" => {
                let cdbc_sink = cdbc_clone.static_pad("sink").expect("cdbc sink pad");
                if cdbc_sink.is_linked() {
                    return;
                }
                pad.link(&cdbc_sink).expect("link cdbc → fluxcdbc");
                eprintln!("[flux-client] cdbc → fluxcdbc linked");
            }
            _ => {
                eprintln!("[flux-client] unhandled pad '{}' — ignoring", pad_name);
            }
        }
    });

    // ── Run ───────────────────────────────────────────────────────────────────
    pipeline
        .set_state(gst::State::Playing)
        .expect("Unable to start pipeline");
    eprintln!("[flux-client] Pipeline started — waiting for FLUX stream on :7400");

    // Use a GLib main loop to drive async state changes.
    // gst::macos_main() runs run() on the GLib main thread, so MainLoop::new(None, false)
    // attaches to the default (main) GLib main context.
    let main_loop = glib::MainLoop::new(None, false);
    let main_loop_clone = main_loop.clone();
    let pipeline_clone = pipeline.clone();

    let bus = pipeline.bus().unwrap();
    let _bus_watch = bus
        .add_watch(move |_bus, msg| {
            use gst::MessageView;
            match msg.view() {
                MessageView::Eos(..) => {
                    eprintln!("[flux-client] EOS");
                    main_loop_clone.quit();
                    return ControlFlow::Break;
                }
                MessageView::Error(err) => {
                    eprintln!(
                        "[flux-client] ERROR from {:?}: {} ({:?})",
                        err.src().map(|s| s.path_string()),
                        err.error(),
                        err.debug()
                    );
                    main_loop_clone.quit();
                    return ControlFlow::Break;
                }
                MessageView::StateChanged(sc) => {
                    if msg
                        .src()
                        .map(|s| s == pipeline_clone.upcast_ref::<gst::Object>())
                        .unwrap_or(false)
                    {
                        eprintln!("[flux-client] State: {:?} → {:?}", sc.old(), sc.current());
                    }
                }
                _ => {}
            }
            ControlFlow::Continue
        })
        .expect("bus watch");

    main_loop.run();

    pipeline.set_state(gst::State::Null).unwrap();
    eprintln!("[flux-client] Stopped");
}
