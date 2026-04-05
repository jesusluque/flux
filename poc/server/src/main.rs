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
//!
//! FLUX-C commands received from clients:
//!   test_pattern  — switch videotestsrc pattern live (T key on client)
//!   ptz           — logged only (no real camera in PoC)
//!   audio_mix     — logged only
//!   routing       — logged only

use flux_framing::ControlType;
use gstreamer as gst;
use gstreamer::prelude::*;

fn main() {
    env_logger::init();

    gst::init().expect("GStreamer init failed");

    // Register our custom elements by linking the plugin crates
    gstfluxframer::plugin_register_static().expect("fluxframer register");
    gstfluxsink::plugin_register_static().expect("fluxsink register");

    // ── Build pipeline ────────────────────────────────────────────────────────

    let pipeline = gst::Pipeline::new();

    let videotestsrc = gst::ElementFactory::make("videotestsrc")
        .property_from_str("pattern", "smpte")
        .property("is-live", true)
        .build()
        .expect("videotestsrc");

    let convert = gst::ElementFactory::make("videoconvertscale")
        .build()
        .expect("videoconvertscale");

    let vtenc = gst::ElementFactory::make("vtenc_h265")
        .property("realtime", true)
        .property("allow-frame-reordering", false)
        .property("bitrate", 4000u32)
        .build()
        .expect("vtenc_h265");

    let h265parse = gst::ElementFactory::make("h265parse")
        .property("config-interval", -1i32)
        .build()
        .expect("h265parse");

    let fluxframer = gst::ElementFactory::make("fluxframer")
        .property("channel-id", 0u32)
        .property("group-id", 1u32)
        .build()
        .expect("fluxframer");

    let fluxsink_elem = gst::ElementFactory::make("fluxsink")
        .property("port", 7400u32)
        .build()
        .expect("fluxsink");

    // Downcast to FluxSink so we can subscribe to FLUX-C control messages.
    let fluxsink = fluxsink_elem
        .clone()
        .downcast::<gstfluxsink::FluxSink>()
        .expect("fluxsink downcast");

    let flux_ctrl_rx = fluxsink.subscribe_flux_control();

    pipeline
        .add_many([
            &videotestsrc,
            &convert,
            &vtenc,
            &h265parse,
            &fluxframer,
            &fluxsink_elem,
        ])
        .expect("add elements");

    videotestsrc.link(&convert).expect("videotestsrc → convert");

    let caps_720p60 = gst::Caps::builder("video/x-raw")
        .field("width", 1280i32)
        .field("height", 720i32)
        .field("framerate", gst::Fraction::new(60, 1))
        .build();
    convert
        .link_filtered(&vtenc, &caps_720p60)
        .expect("convert → vtenc_h265 (720p60)");

    let bytestream_caps = gst::Caps::builder("video/x-h265")
        .field("stream-format", "byte-stream")
        .field("alignment", "au")
        .build();
    vtenc.link(&h265parse).expect("vtenc_h265 → h265parse");
    h265parse
        .link_filtered(&fluxframer, &bytestream_caps)
        .expect("h265parse → fluxframer");
    fluxframer
        .link(&fluxsink_elem)
        .expect("fluxframer → fluxsink");

    // ── Run ───────────────────────────────────────────────────────────────────
    match pipeline.set_state(gst::State::Playing) {
        Ok(s) => eprintln!("[flux-server] set_state(Playing) → {:?}", s),
        Err(e) => eprintln!(
            "[flux-server] set_state(Playing) failed: {:?} — waiting for bus error",
            e
        ),
    }
    eprintln!(
        "[flux-server] Pipeline started — encoding H.265 and sending FLUX datagrams on :7400 (QUIC)"
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

    // ── FLUX-C control dispatcher ─────────────────────────────────────────────
    // Runs in a background thread: receives parsed FluxControl messages forwarded
    // by fluxsink and acts on them (pattern switch, etc.).
    {
        let videotestsrc_clone = videotestsrc.clone();
        let vtenc_clone = vtenc.clone();
        std::thread::spawn(move || {
            for cmd in flux_ctrl_rx {
                match cmd.control_type {
                    ControlType::TestPattern => {
                        let pat = cmd.pattern_id.unwrap_or(0);
                        eprintln!("[flux-server] FLUX-C test_pattern → {}", pat);
                        // set_property("pattern", i32) panics in gstreamer-rs because
                        // the GLib property type is GstVideoTestSrcPattern (an enum),
                        // not gint.  set_property_from_str accepts both nick names
                        // ("snow") and decimal integer strings ("1") and converts them
                        // correctly to the enum type.
                        videotestsrc_clone.set_property_from_str("pattern", &pat.to_string());
                        // Force vtenc_h265 to emit an IDR immediately.
                        // Without this the encoder keeps sending P-frames that
                        // reference the old reference picture; the client decoder
                        // sees "error flag" warnings until it happens to get a
                        // natural IDR (~every 2 s at default GOP size).
                        // UpstreamForceKeyUnitEvent travels upstream from the
                        // encoder's sink pad toward the source and tells the
                        // encoder to flush and produce a keyframe on the very
                        // next buffer it receives.
                        let fku = gstreamer_video::UpstreamForceKeyUnitEvent::builder()
                            .all_headers(true)
                            .build();
                        if let Some(sink_pad) = vtenc_clone.static_pad("sink") {
                            sink_pad.send_event(fku);
                        }
                    }
                    ControlType::Ptz => {
                        eprintln!(
                            "[flux-server] FLUX-C ptz — pan={:?} tilt={:?} zoom={:?}",
                            cmd.pan_deg, cmd.tilt_deg, cmd.zoom_pos
                        );
                    }
                    ControlType::AudioMix => {
                        eprintln!(
                            "[flux-server] FLUX-C audio_mix — mute={:?} gain_db={:?}",
                            cmd.mute, cmd.gain_db
                        );
                    }
                    ControlType::Routing => {
                        eprintln!("[flux-server] FLUX-C routing — target={:?}", cmd.target_id);
                    }
                }
            }
        });
    }

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
