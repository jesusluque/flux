//! FLUX PoC — Server
//!
//! Pipeline:
//!   videotestsrc pattern=smpte is-live=true
//!     → videoconvertscale
//!     → video/x-raw,width=1280,height=720,framerate=60/1
//!     → vtenc_h265  (Apple VideoToolbox HW encoder)
//!     → h265parse
//!     → fluxframer channel-id=0 group-id=1
//!     → fluxsink    port=7400
//!
//! GStreamer plugins are registered statically from the linked crates.
//! GST_PLUGIN_PATH is not required when running this binary.

use gstreamer as gst;
use gstreamer::prelude::*;

fn main() {
    env_logger::init();

    gst::init().expect("GStreamer init failed");

    // Register our custom elements by linking the plugin crates
    gstfluxframer::plugin_register_static().expect("fluxframer register");
    gstfluxsink::plugin_register_static().expect("fluxsink register");

    // ── Build pipeline ────────────────────────────────────────────────────────
    let pipeline_desc = "\
        videotestsrc pattern=smpte is-live=true ! \
        videoconvertscale ! \
        video/x-raw,width=1280,height=720,framerate=60/1 ! \
        vtenc_h265 realtime=true allow-frame-reordering=false bitrate=4000 ! \
        h265parse config-interval=-1 ! \
        video/x-h265,stream-format=byte-stream,alignment=au ! \
        fluxframer channel-id=0 group-id=1 ! \
        fluxsink port=7400\
    ";

    let pipeline = gst::parse::launch(pipeline_desc).expect("Failed to parse pipeline description");

    let pipeline = pipeline
        .downcast::<gst::Pipeline>()
        .expect("Expected a Pipeline");

    // ── Run ───────────────────────────────────────────────────────────────────
    pipeline
        .set_state(gst::State::Playing)
        .expect("Unable to start pipeline");
    eprintln!(
        "[flux-server] Pipeline started — encoding H.265 and sending FLUX datagrams on :7400"
    );

    // Install Ctrl-C handler: send EOS so the pipeline shuts down cleanly.
    let pipeline_weak = pipeline.downgrade();
    ctrlc::set_handler(move || {
        eprintln!("[flux-server] Ctrl-C — sending EOS");
        if let Some(p) = pipeline_weak.upgrade() {
            p.send_event(gst::event::Eos::new());
        }
    })
    .expect("ctrlc handler");

    let bus = pipeline.bus().unwrap();
    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        use gst::MessageView;
        match msg.view() {
            MessageView::Eos(..) => {
                eprintln!("[flux-server] EOS");
                break;
            }
            MessageView::Error(err) => {
                eprintln!(
                    "[flux-server] ERROR from {:?}: {} ({:?})",
                    err.src().map(|s| s.path_string()),
                    err.error(),
                    err.debug()
                );
                break;
            }
            MessageView::StateChanged(sc) => {
                if msg
                    .src()
                    .map(|s| s == pipeline.upcast_ref::<gst::Object>())
                    .unwrap_or(false)
                {
                    eprintln!("[flux-server] State: {:?} → {:?}", sc.old(), sc.current());
                }
            }
            _ => {}
        }
    }

    pipeline.set_state(gst::State::Null).unwrap();
    eprintln!("[flux-server] Stopped");
}
