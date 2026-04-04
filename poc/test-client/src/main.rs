//! FLUX PoC — End-to-End Test Client
//!
//! Runs the same receive pipeline as `client`, but with:
//!   - Pad probes to count flux datagrams rx, fragments seen, and reassembly errors
//!   - `fpsdisplaysink` wrapping `osxvideosink` to measure rendered fps
//!   - Ctrlc handler for clean shutdown
//!   - Full structured pass/fail report printed at exit
//!
//! PASS criteria (all must be true):
//!   1. SESSION handshake succeeded (session_id non-empty)
//!   2. Frames rendered > 0  (aus_decoded > 0)
//!   3. CDBC reports sent > 0
//!   4. Reassembly errors == 0
//!   5. Datagram loss < 1%
//!   6. Average fps >= 55.0
//!
//! Run in Terminal 1:  cargo run --release --bin flux-server
//! Run in Terminal 2:  cargo run --release --bin flux-test-client
//! Press Ctrl-C in either terminal to stop.

use glib::ControlFlow;
use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Instant;

// ─── Stats collected via pad probes ──────────────────────────────────────────

struct Stats {
    /// Raw FLUX datagrams coming off the wire (fluxsrc src pad)
    flux_datagrams_rx: u64,
    /// Fragment payloads seen entering fluxdeframer
    fragments_seen: u64,
    /// Complete AUs emitted by fluxdeframer (reassembled frames)
    aus_decoded: u64,
    /// Incomplete / stale fragment sequences dropped by fluxdeframer
    reassembly_errors: u64,
    /// Wall-clock time the pipeline first produced a complete AU
    first_frame_at: Option<Instant>,
    /// Wall-clock time we started
    start: Instant,
}

impl Stats {
    fn new() -> Self {
        Stats {
            flux_datagrams_rx: 0,
            fragments_seen: 0,
            aus_decoded: 0,
            reassembly_errors: 0,
            first_frame_at: None,
            start: Instant::now(),
        }
    }
}

// ─── entry point ─────────────────────────────────────────────────────────────

fn main() {
    env_logger::init();
    gst::init().expect("GStreamer init failed");
    // osxvideosink requires NSApplication on the main thread.
    gst::macos_main(run);
}

fn run() {
    // Register custom elements
    gstfluxsrc::plugin_register_static().expect("fluxsrc register");
    gstfluxdemux::plugin_register_static().expect("fluxdemux register");
    gstfluxdeframer::plugin_register_static().expect("fluxdeframer register");
    gstfluxcdbc::plugin_register_static().expect("fluxcdbc register");

    // ── Shared stats ──────────────────────────────────────────────────────────
    let stats = Arc::new(Mutex::new(Stats::new()));
    // Atomic fps accumulator set by fpsdisplaysink's fps-measurements signal.
    // We store (sum_fps, count) for computing average.
    let fps_sum = Arc::new(Mutex::new((0.0f64, 0u64)));

    // ── Build pipeline ────────────────────────────────────────────────────────
    let pipeline = gst::Pipeline::new();

    let fluxsrc = gst::ElementFactory::make("fluxsrc")
        .property("address", "127.0.0.1")
        .property("port", 7400u32)
        .build()
        .expect("fluxsrc element");

    let fluxdemux = gst::ElementFactory::make("fluxdemux")
        .build()
        .expect("fluxdemux element");

    let fluxdeframer = gst::ElementFactory::make("fluxdeframer")
        .build()
        .expect("fluxdeframer element");

    let h265parse = gst::ElementFactory::make("h265parse")
        .build()
        .expect("h265parse element");

    let vtdec_hw = gst::ElementFactory::make("vtdec_hw")
        .build()
        .expect("vtdec_hw element");

    let convert = gst::ElementFactory::make("videoconvertscale")
        .build()
        .expect("videoconvertscale element");

    // fpsdisplaysink wrapping osxvideosink
    let video_sink = gst::ElementFactory::make("osxvideosink")
        .property("sync", false)
        .property("async", false)
        .build()
        .expect("osxvideosink element");

    let fpsdisplaysink = gst::ElementFactory::make("fpsdisplaysink")
        .property("video-sink", &video_sink)
        .property("sync", false)
        .property("signal-fps-measurements", true)
        .build()
        .expect("fpsdisplaysink element");

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

    pipeline
        .add_many([
            &fluxsrc,
            &fluxdemux,
            &fluxdeframer,
            &h265parse,
            &vtdec_hw,
            &convert,
            &fpsdisplaysink,
            &fluxcdbc,
            &fakesink,
        ])
        .expect("add elements");

    fluxsrc.link(&fluxdemux).expect("fluxsrc → fluxdemux");
    // fluxcdbc is passthrough — insert it in-line on media_0 so it observes
    // actual MediaData frames and sends CDBC_FEEDBACK to the server.
    fluxcdbc
        .link(&fluxdeframer)
        .expect("fluxcdbc → fluxdeframer");
    fluxdeframer
        .link(&h265parse)
        .expect("fluxdeframer → h265parse");

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
        .link(&fpsdisplaysink)
        .expect("videoconvertscale → fpsdisplaysink");
    // fakesink drains the demux 'cdbc' pad (server→client CDBC echoes, unused in PoC)

    // ── Dynamic pad linking ───────────────────────────────────────────────────
    let cdbc_element_clone = fluxcdbc.clone();
    let fakesink_clone = fakesink.clone();
    fluxdemux.connect_pad_added(move |_elem, pad| {
        let pad_name = pad.name();
        eprintln!("[flux-test-client] fluxdemux pad added: {}", pad_name);
        match pad_name.as_str() {
            "media_0" => {
                // media_0 → fluxcdbc (passthrough observer) → fluxdeframer → ...
                let cdbc_sink = cdbc_element_clone
                    .static_pad("sink")
                    .expect("fluxcdbc sink pad");
                if cdbc_sink.is_linked() {
                    return;
                }
                pad.link(&cdbc_sink).expect("link media_0 → fluxcdbc");
                eprintln!("[flux-test-client] media_0 → fluxcdbc → fluxdeframer linked");
            }
            "cdbc" => {
                let fs_sink = fakesink_clone
                    .static_pad("sink")
                    .expect("fakesink sink pad");
                if fs_sink.is_linked() {
                    return;
                }
                pad.link(&fs_sink).expect("link cdbc → fakesink");
                eprintln!("[flux-test-client] cdbc (server echo) → fakesink linked");
            }
            _ => {
                // misc, control etc. — fluxdemux already handles not-linked gracefully.
                eprintln!(
                    "[flux-test-client] pad '{}' — unlinked (not-linked is non-fatal)",
                    pad_name
                );
            }
        }
    });

    // ── Pad probe: fluxsrc src — count raw datagrams ──────────────────────────
    {
        let stats_ref = stats.clone();
        let src_pad = fluxsrc.static_pad("src").expect("fluxsrc src pad");
        src_pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, _info| {
            stats_ref.lock().unwrap().flux_datagrams_rx += 1;
            gst::PadProbeReturn::Ok
        });
    }

    // ── Pad probe: fluxdeframer sink — count fragment datagrams ──────────────
    {
        let stats_ref = stats.clone();
        let sink_pad = fluxdeframer
            .static_pad("sink")
            .expect("fluxdeframer sink pad");
        sink_pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, _info| {
            stats_ref.lock().unwrap().fragments_seen += 1;
            gst::PadProbeReturn::Ok
        });
    }

    // ── Pad probe: fluxdeframer src — count complete AUs + detect errors ─────
    // A buffer pushed from fluxdeframer represents a successfully reassembled AU.
    // We also watch for GAP events (emitted by fluxdeframer on reassembly error)
    // to count reassembly_errors.
    {
        let stats_ref_buf = stats.clone();
        let stats_ref_evt = stats.clone();
        let src_pad = fluxdeframer
            .static_pad("src")
            .expect("fluxdeframer src pad");

        src_pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, _info| {
            let mut s = stats_ref_buf.lock().unwrap();
            s.aus_decoded += 1;
            if s.first_frame_at.is_none() {
                s.first_frame_at = Some(Instant::now());
            }
            gst::PadProbeReturn::Ok
        });

        src_pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_pad, info| {
            if let Some(gst::PadProbeData::Event(ref ev)) = info.data {
                if ev.type_() == gst::EventType::Gap {
                    stats_ref_evt.lock().unwrap().reassembly_errors += 1;
                }
            }
            gst::PadProbeReturn::Ok
        });
    }

    // ── fps-measurements signal ───────────────────────────────────────────────
    {
        let fps_ref = fps_sum.clone();
        fpsdisplaysink.connect("fps-measurements", false, move |args| {
            // args: [element, fps, droprate, avgfps]
            if let Some(fps) = args.get(1).and_then(|v| v.get::<f64>().ok()) {
                let mut guard = fps_ref.lock().unwrap();
                guard.0 += fps;
                guard.1 += 1;
            }
            None
        });
    }

    // ── Start pipeline ────────────────────────────────────────────────────────
    pipeline
        .set_state(gst::State::Playing)
        .expect("Unable to start pipeline");
    eprintln!("[flux-test-client] Pipeline started — waiting for FLUX stream on :7400");
    eprintln!("[flux-test-client] Press Ctrl-C to stop and print report");

    let main_loop = glib::MainLoop::new(None, false);
    let main_loop_quit = main_loop.clone();

    // Ctrl-C → quit the GLib main loop (pipeline stops below)
    ctrlc::set_handler(move || {
        eprintln!("\n[flux-test-client] Ctrl-C received — stopping…");
        main_loop_quit.quit();
    })
    .expect("ctrlc handler");

    let main_loop_clone = main_loop.clone();
    let pipeline_clone = pipeline.clone();
    let bus = pipeline.bus().unwrap();
    let _bus_watch = bus
        .add_watch(move |_bus, msg| {
            use gst::MessageView;
            match msg.view() {
                MessageView::Eos(..) => {
                    eprintln!("[flux-test-client] EOS");
                    main_loop_clone.quit();
                    return ControlFlow::Break;
                }
                MessageView::Error(err) => {
                    eprintln!(
                        "[flux-test-client] ERROR from {:?}: {} ({:?})",
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
                        eprintln!(
                            "[flux-test-client] State: {:?} → {:?}",
                            sc.old(),
                            sc.current()
                        );
                    }
                }
                _ => {}
            }
            ControlFlow::Continue
        })
        .expect("bus watch");

    main_loop.run();

    // ── Tear down ─────────────────────────────────────────────────────────────
    pipeline.set_state(gst::State::Null).unwrap();

    // ── Collect final stats from element properties ───────────────────────────
    let session_id: String = fluxsrc.property::<String>("session-id");
    let keepalives_sent: u64 = fluxsrc.property::<u64>("keepalives-sent");
    let cdbc_reports_sent: u64 = fluxcdbc.property::<u64>("reports-sent");
    let datagrams_lost_total: u64 = fluxcdbc.property::<u64>("datagrams-lost-total");
    let loss_pct: f64 = fluxcdbc.property::<f64>("loss-pct");

    let final_stats = stats.lock().unwrap();
    let elapsed_s = final_stats.start.elapsed().as_secs_f64();

    let (fps_total, fps_count) = *fps_sum.lock().unwrap();
    let avg_fps = if fps_count > 0 {
        fps_total / fps_count as f64
    } else {
        0.0
    };

    // ── Pass/Fail criteria ────────────────────────────────────────────────────
    let c1_session_ok = !session_id.is_empty();
    let c2_frames_ok = final_stats.aus_decoded > 0;
    let c3_cdbc_ok = cdbc_reports_sent > 0;
    let c4_reassembly_ok = final_stats.reassembly_errors == 0;
    let c5_loss_ok = loss_pct < 1.0;
    let c6_fps_ok = avg_fps >= 55.0;

    let overall =
        c1_session_ok && c2_frames_ok && c3_cdbc_ok && c4_reassembly_ok && c5_loss_ok && c6_fps_ok;

    // ── Print report ──────────────────────────────────────────────────────────
    eprintln!();
    eprintln!("╔══════════════════════════════════════════════════════════╗");
    eprintln!("║          FLUX PoC — End-to-End Test Report               ║");
    eprintln!("╠══════════════════════════════════════════════════════════╣");
    eprintln!("║  Duration            : {:.1}s", elapsed_s);
    eprintln!("║");
    eprintln!("║  Transport");
    eprintln!(
        "║    Session ID        : {}",
        if session_id.is_empty() {
            "<none>"
        } else {
            &session_id
        }
    );
    eprintln!("║    Datagrams RX      : {}", final_stats.flux_datagrams_rx);
    eprintln!("║    Keepalives sent   : {}", keepalives_sent);
    eprintln!("║");
    eprintln!("║  CDBC");
    eprintln!("║    Reports sent      : {}", cdbc_reports_sent);
    eprintln!("║    Datagrams lost    : {}", datagrams_lost_total);
    eprintln!("║    Loss pct          : {:.2}%", loss_pct);
    eprintln!("║");
    eprintln!("║  Video");
    eprintln!("║    Fragments seen    : {}", final_stats.fragments_seen);
    eprintln!("║    AUs decoded       : {}", final_stats.aus_decoded);
    eprintln!("║    Reassembly errors : {}", final_stats.reassembly_errors);
    eprintln!("║    Avg fps (display) : {:.1}", avg_fps);
    eprintln!("║");
    eprintln!("║  Pass/Fail Criteria");
    eprintln!(
        "║    [{}] SESSION handshake ok   (session_id non-empty)",
        if c1_session_ok { "PASS" } else { "FAIL" }
    );
    eprintln!(
        "║    [{}] Frames rendered > 0    (aus_decoded={})",
        if c2_frames_ok { "PASS" } else { "FAIL" },
        final_stats.aus_decoded
    );
    eprintln!(
        "║    [{}] CDBC reports sent > 0  (reports_sent={})",
        if c3_cdbc_ok { "PASS" } else { "FAIL" },
        cdbc_reports_sent
    );
    eprintln!(
        "║    [{}] Reassembly errors == 0 (errors={})",
        if c4_reassembly_ok { "PASS" } else { "FAIL" },
        final_stats.reassembly_errors
    );
    eprintln!(
        "║    [{}] Loss < 1%              (loss={:.2}%)",
        if c5_loss_ok { "PASS" } else { "FAIL" },
        loss_pct
    );
    eprintln!(
        "║    [{}] Avg fps >= 55.0        (avg={:.1})",
        if c6_fps_ok { "PASS" } else { "FAIL" },
        avg_fps
    );
    eprintln!("╠══════════════════════════════════════════════════════════╣");
    eprintln!(
        "║  Overall: {}                                          ║",
        if overall { "PASS ✓" } else { "FAIL ✗" }
    );
    eprintln!("╚══════════════════════════════════════════════════════════╝");

    std::process::exit(if overall { 0 } else { 1 });
}
