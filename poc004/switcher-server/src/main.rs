//! FLUX PoC004 — Switcher Server
//!
//! Runs 4 independent GStreamer encode pipelines (one per camera), each
//! terminating in an `appsink`.  A router task reads from all four sinks and
//! forwards only the **active camera**'s FLUX-framed buffers into a single
//! `appsrc → fluxsink` output pipeline on port 7410.
//!
//! The router waits for a keyframe (IDR, `!DELTA_UNIT`) before committing
//! a pending channel switch, so the client decoder never receives a
//! non-decodeable partial GOP.
//!
//! Bidirectional tally (spec §8):
//!   - Receives `TALLY_UPDATE (0xA)` datagrams from the director client via
//!     `fluxsink.subscribe_flux_control()` (parsed as raw MetadataFrame control).
//!   - On every committed switch, sends a `tally_confirm` JSON datagram
//!     back to the client via `fluxsink.send_datagram()`.
//!
//! FLUX-C routing (spec §12):
//!   - Receives `FluxControl { type: routing, target_id: "cam-N" }` from the
//!     client; treated identically to a keyboard cut.
//!
//! Keyboard controls:
//!   1–4  cut to camera N (queues pending switch)
//!   T    show tally state
//!   Q    quit
//!   H    help

use flux_framing::flags as flux_flags;
use gst::glib;
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

// ── Constants ─────────────────────────────────────────────────────────────────

const N: usize = 4;

// videotestsrc patterns — visually distinct so cuts are obvious.
const PATTERNS: [&str; N] = ["smpte", "pinwheel", "ball", "snow"];

// Human labels.
const LABELS: [&str; N] = ["CAM 1", "CAM 2", "CAM 3", "CAM 4"];

// group-id used by fluxframer on all cameras.
const GROUP_ID: u32 = 4;

// Output port.
const OUT_PORT: u32 = 7410;

// Tally confirm colours per state.
const PGM_COLOR: &str = "#FF0000";

// ── Logger ────────────────────────────────────────────────────────────────────

static STDERR_LOCK: std::sync::OnceLock<Mutex<std::io::Stderr>> = std::sync::OnceLock::new();

macro_rules! log {
    ($($arg:tt)*) => {{
        let lock = STDERR_LOCK.get_or_init(|| Mutex::new(std::io::stderr()));
        let mut line = format!($($arg)*);
        line.push('\n');
        let mut stderr = lock.lock().unwrap();
        let _ = stderr.write_all(line.as_bytes());
    }};
}

// ── Shared switcher state ─────────────────────────────────────────────────────

struct SwitcherState {
    /// The camera currently on air (0–3).
    active: u32,
    /// Human-readable tally: per-camera state label.
    tally: [&'static str; N],
}

impl SwitcherState {
    fn new() -> Self {
        let mut tally = ["idle"; N];
        tally[0] = "program";
        SwitcherState { active: 0, tally }
    }

    fn print_tally(&self) {
        let row: Vec<String> = (0..N)
            .map(|i| format!("CAM {}: {:>7}", i + 1, self.tally[i].to_uppercase()))
            .collect();
        log!("[tally]  {}", row.join("   "));
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

fn main() {
    env_logger::init();
    gst::init().expect("GStreamer init failed");

    gstfluxframer::plugin_register_static().expect("fluxframer register");
    gstfluxsink::plugin_register_static().expect("fluxsink register");

    let tty = open_tty_raw();
    if tty.is_none() {
        eprintln!("[switcher-server] WARNING: /dev/tty unavailable — keyboard disabled");
    }

    gst::macos_main(move || run(tty));
}

fn run(tty: Option<Tty>) {
    // ── Build 4 encode pipelines ──────────────────────────────────────────────
    let mut cam_pipelines: Vec<gst::Pipeline> = Vec::with_capacity(N);
    let mut cam_sinks_vec: Vec<gst_app::AppSink> = Vec::with_capacity(N);

    for i in 0..N {
        let (pipeline, appsink) = build_cam_pipeline(i);
        cam_pipelines.push(pipeline);
        cam_sinks_vec.push(appsink);
    }

    let cam_sinks = Arc::new(cam_sinks_vec);

    // ── Build output pipeline: appsrc → fluxsink ──────────────────────────────
    let (out_pipeline, router_src, flux_sink) = build_output_pipeline();

    // ── Switcher state ────────────────────────────────────────────────────────
    let state = Arc::new(Mutex::new(SwitcherState::new()));

    // ── Pending-switch signals (written by keyboard/FLUX-C, read by router) ───
    let pending_switch = Arc::new(AtomicBool::new(false));
    let pending_channel = Arc::new(AtomicU32::new(0));

    // ── Start all pipelines ───────────────────────────────────────────────────
    // set_state() is async for live pipelines — call get_state() with a 5 s
    // timeout so the router thread never starts polling appsinks before the
    // encode pipelines have actually reached PLAYING (which causes all
    // try_pull_sample() calls to return NULL indefinitely → green screen).
    for (i, pl) in cam_pipelines.iter().enumerate() {
        pl.set_state(gst::State::Playing)
            .expect("cam pipeline Playing");
        let (change, cur, _pend) = pl.state(gst::ClockTime::from_seconds(5));
        match change {
            Ok(_) => {}
            Err(e) => panic!("CAM {} pipeline failed to reach PLAYING: {:?}", i + 1, e),
        }
        log!(
            "[switcher-server] CAM {} encode pipeline PLAYING (cur={:?})",
            i + 1,
            cur
        );
    }
    out_pipeline
        .set_state(gst::State::Playing)
        .expect("output pipeline Playing");
    log!(
        "[switcher-server] Output pipeline started (port {})",
        OUT_PORT
    );

    // ── FLUX-C / tally receive task ───────────────────────────────────────────
    // subscribe_flux_control() gives us FluxControl messages (routing, etc.)
    let ctrl_rx = flux_sink.subscribe_flux_control();
    {
        let pending_switch = pending_switch.clone();
        let pending_channel = pending_channel.clone();
        let state = state.clone();
        let flux_sink_weak = flux_sink.downgrade();
        let cam_sinks_ctrl = cam_sinks.clone();
        std::thread::spawn(move || {
            run_control_task(
                ctrl_rx,
                pending_switch,
                pending_channel,
                state,
                flux_sink_weak,
                cam_sinks_ctrl,
            );
        });
    }

    // ── Router task ───────────────────────────────────────────────────────────
    {
        let state = state.clone();
        let pending_switch = pending_switch.clone();
        let pending_channel = pending_channel.clone();
        let flux_sink_weak = flux_sink.downgrade();
        let router_src = router_src.clone();
        let cam_sinks_router = cam_sinks.clone();
        std::thread::spawn(move || {
            run_router(
                cam_sinks_router,
                router_src,
                state,
                pending_switch,
                pending_channel,
                flux_sink_weak,
            );
        });
    }

    // ── Ctrl-C ────────────────────────────────────────────────────────────────
    {
        let pl_clones: Vec<_> = cam_pipelines.iter().map(|p| p.downgrade()).collect();
        let out_weak = out_pipeline.downgrade();
        ctrlc::set_handler(move || {
            log!("[switcher-server] Ctrl-C — shutting down");
            for pw in &pl_clones {
                if let Some(p) = pw.upgrade() {
                    p.send_event(gst::event::Eos::new());
                }
            }
            if let Some(p) = out_weak.upgrade() {
                p.send_event(gst::event::Eos::new());
            }
        })
        .expect("ctrlc handler");
    }

    // ── GLib main loop ────────────────────────────────────────────────────────
    let main_loop = glib::MainLoop::new(None, false);

    // Keyboard input thread → channel → idle timeout.
    if let Some(tty) = tty {
        let (tx, rx) = std::sync::mpsc::channel::<char>();
        std::thread::spawn(move || {
            let fd = tty.fd;
            loop {
                let mut buf = [0u8; 1];
                let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 1) };
                if n <= 0 {
                    break;
                }
                if tx.send(buf[0] as char).is_err() {
                    break;
                }
            }
            drop(tty);
        });

        let ml = main_loop.clone();
        let state2 = state.clone();
        let ps = pending_switch.clone();
        let pc = pending_channel.clone();
        let cs = cam_sinks.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(20), move || {
            while let Ok(ch) = rx.try_recv() {
                match ch {
                    'Q' | 'q' | '\x03' => {
                        log!("[switcher-server] Quit");
                        ml.quit();
                        return glib::ControlFlow::Break;
                    }
                    _ => handle_key(ch, &state2, &ps, &pc, &cs),
                }
            }
            glib::ControlFlow::Continue
        });
    }

    // Bus watchers.
    let mut _bus_watches = Vec::new();
    let ml = main_loop.clone();
    for pl in cam_pipelines.iter().chain(std::iter::once(&out_pipeline)) {
        let ml2 = ml.clone();
        let w = pl
            .bus()
            .unwrap()
            .add_watch(move |_, msg| {
                use gst::MessageView;
                match msg.view() {
                    MessageView::Eos(..) => {
                        ml2.quit();
                        return glib::ControlFlow::Break;
                    }
                    MessageView::Error(e) => {
                        log!(
                            "[switcher-server] PIPELINE ERROR: {} ({:?})",
                            e.error(),
                            e.debug()
                        );
                        ml2.quit();
                        return glib::ControlFlow::Break;
                    }
                    _ => {}
                }
                glib::ControlFlow::Continue
            })
            .unwrap();
        _bus_watches.push(w);
    }

    log!("[switcher-server] Running — active: CAM 1 | Keys: 1–4 cut, T tally, Q quit, H help");
    state.lock().unwrap().print_tally();

    main_loop.run();

    for pl in &cam_pipelines {
        pl.set_state(gst::State::Null).unwrap();
    }
    out_pipeline.set_state(gst::State::Null).unwrap();
    log!("[switcher-server] Stopped");
}

// ── Camera encode pipeline ────────────────────────────────────────────────────
//
// videotestsrc(pattern, is-live)
//   → videoconvertscale
//   → video/x-raw,640×360,30fps
//   → clockoverlay (HH:MM:SS.cc, centred)
//   → textoverlay  ("CAM N", top-left)
//   → vtenc_h265(realtime, no-reorder)
//   → h265parse(config-interval=-1)
//   → fluxframer(channel-id=N, group-id=GROUP_ID)
//   → appsink(sync=false, max-buffers=4, drop=true)

fn build_cam_pipeline(idx: usize) -> (gst::Pipeline, gst_app::AppSink) {
    let pipeline = gst::Pipeline::with_name(&format!("cam_{}", idx));

    macro_rules! make {
        ($factory:expr, $name:expr) => {
            gst::ElementFactory::make($factory)
                .name($name)
                .build()
                .unwrap_or_else(|_| panic!("Could not create {}", $factory))
        };
    }

    let src = gst::ElementFactory::make("videotestsrc")
        .property_from_str("pattern", PATTERNS[idx])
        .property("is-live", true)
        .name(&format!("src_{}", idx))
        .build()
        .expect("videotestsrc");

    let convert = make!("videoconvertscale", &format!("convert_{}", idx));

    let clockoverlay = gst::ElementFactory::make("clockoverlay")
        .property_from_str("time-format", "%H:%M:%S.%2N")
        .property_from_str("halignment", "center")
        .property_from_str("valignment", "center")
        .property_from_str("font-desc", "Sans Bold 36")
        .property("shaded-background", true)
        .name(&format!("clock_{}", idx))
        .build()
        .expect("clockoverlay");

    let textoverlay = gst::ElementFactory::make("textoverlay")
        .property("text", LABELS[idx])
        .property_from_str("halignment", "left")
        .property_from_str("valignment", "top")
        .property_from_str("font-desc", "Sans Bold 24")
        .property("shaded-background", true)
        .name(&format!("text_{}", idx))
        .build()
        .expect("textoverlay");

    let vtenc = gst::ElementFactory::make("vtenc_h265")
        .property("realtime", true)
        .property("allow-frame-reordering", false)
        .property("bitrate", 2000u32)
        .name(&format!("vtenc_{}", idx))
        .build()
        .expect("vtenc_h265");

    let h265parse = gst::ElementFactory::make("h265parse")
        .property("config-interval", -1i32)
        .name(&format!("parse_{}", idx))
        .build()
        .expect("h265parse");

    let fluxframer = gst::ElementFactory::make("fluxframer")
        .property("channel-id", idx as u32)
        .property("group-id", GROUP_ID)
        .name(&format!("framer_{}", idx))
        .build()
        .expect("fluxframer");

    let appsink = gst_app::AppSink::builder()
        .name(&format!("appsink_{}", idx))
        .sync(false)
        .max_buffers(4u32)
        .drop(true)
        .build();

    pipeline
        .add_many([
            &src,
            &convert,
            &clockoverlay,
            &textoverlay,
            &vtenc,
            &h265parse,
            &fluxframer,
            appsink.upcast_ref(),
        ])
        .expect("add elements");

    let caps_360p30 = gst::Caps::builder("video/x-raw")
        .field("width", 640i32)
        .field("height", 360i32)
        .field("framerate", gst::Fraction::new(30, 1))
        .build();

    src.link(&convert).expect("src→convert");
    convert
        .link_filtered(&clockoverlay, &caps_360p30)
        .expect("convert→clock");
    clockoverlay.link(&textoverlay).expect("clock→text");
    textoverlay.link(&vtenc).expect("text→vtenc");
    vtenc.link(&h265parse).expect("vtenc→parse");
    let flux_caps = gst::Caps::builder("application/x-flux").build();
    h265parse
        .link_filtered(
            &fluxframer,
            &gst::Caps::builder("video/x-h265")
                .field("stream-format", "byte-stream")
                .field("alignment", "au")
                .build(),
        )
        .expect("parse→framer");
    fluxframer
        .link_filtered(&appsink, &flux_caps)
        .expect("framer→appsink");

    (pipeline, appsink)
}

// ── Output pipeline ───────────────────────────────────────────────────────────
//
// appsrc(format=Time, is-live=true, caps=application/x-flux)
//   → fluxsink(port=7410)

fn build_output_pipeline() -> (gst::Pipeline, gst_app::AppSrc, gstfluxsink::FluxSink) {
    let pipeline = gst::Pipeline::with_name("output");

    let appsrc = gst_app::AppSrc::builder()
        .name("router_src")
        .format(gst::Format::Time)
        .is_live(true)
        .caps(&gst::Caps::builder("application/x-flux").build())
        .build();

    let fluxsink = gst::ElementFactory::make("fluxsink")
        .property("port", OUT_PORT)
        .name("fluxsink_out")
        .build()
        .expect("fluxsink");

    pipeline
        .add_many([appsrc.upcast_ref(), &fluxsink])
        .expect("add output elements");
    appsrc.link(&fluxsink).expect("appsrc→fluxsink");

    let fs = fluxsink
        .dynamic_cast::<gstfluxsink::FluxSink>()
        .expect("FluxSink cast");
    (pipeline, appsrc, fs)
}

// ── Router task ───────────────────────────────────────────────────────────────

fn run_router(
    cam_sinks: Arc<Vec<gst_app::AppSink>>,
    router_src: gst_app::AppSrc,
    state: Arc<Mutex<SwitcherState>>,
    pending_switch: Arc<AtomicBool>,
    pending_channel: Arc<AtomicU32>,
    flux_sink_weak: gst::glib::WeakRef<gstfluxsink::FluxSink>,
) {
    use flux_framing::{now_ns, TallyConfirm};

    log!("[router] started");
    let mut diag_tick = std::time::Instant::now();
    let mut diag_pulled = [0u32; N];
    let mut diag_pushed: u32 = 0;
    // Tracks when the current go-silent window began.  The switch-IDR is only
    // committed once the silence has lasted ≥600 ms, which guarantees that
    // vtdec_hw's 16-frame async VideoToolbox pipeline (~533 ms at 30 fps) is
    // fully drained before the DISCONT switch-IDR arrives.  An empty pipeline
    // means the DISCONT-triggered flush has nothing to abandon → zero error frames.
    let mut silence_started: Option<std::time::Instant> = None;
    loop {
        let active = state.lock().unwrap().active as usize;
        let is_pending = pending_switch.load(Ordering::Acquire);
        let new_cam = if is_pending {
            pending_channel.load(Ordering::Acquire) as usize
        } else {
            active
        };

        // Record when the go-silent window starts (first iteration is_pending=true).
        if is_pending {
            if silence_started.is_none() {
                silence_started = Some(std::time::Instant::now());
                log!("[router] silence window started (waiting ≥600 ms before cut)");
            }
        } else {
            // No pending switch — clear the timer.
            silence_started = None;
        }

        // Pull only the cameras we actually need this iteration:
        //   - always pull the current active camera (keep it drained so its
        //     appsink never fills up and drops while we wait for an IDR)
        //   - when a switch is pending, also pull the incoming camera
        //   - skip all other cameras entirely to avoid wasting time on 1ms
        //     timeouts that would stall the active-camera push path
        let active_sample: Option<gst_app::gst::Sample>;
        let mut incoming_sample: Option<gst_app::gst::Sample> = None;

        active_sample = cam_sinks[active].try_pull_sample(gst::ClockTime::from_mseconds(1));
        if active_sample.is_some() {
            diag_pulled[active] += 1;
        }

        if is_pending && new_cam != active {
            incoming_sample = cam_sinks[new_cam].try_pull_sample(gst::ClockTime::from_mseconds(1));
            if incoming_sample.is_some() {
                diag_pulled[new_cam] += 1;
            }
        }

        // Print diagnostics every 2 seconds.
        if diag_tick.elapsed().as_secs() >= 2 {
            log!(
                "[router/diag] pulled per-cam: {:?}  pushed: {}",
                diag_pulled,
                diag_pushed
            );
            diag_pulled = [0u32; N];
            diag_pushed = 0;
            diag_tick = std::time::Instant::now();
        }

        // Check if a pending switch can be committed: we need a keyframe from
        // the *incoming* camera (not the currently active one) AND the silence
        // window must have lasted at least 600 ms so vtdec_hw's async pipeline
        // is fully drained before the DISCONT switch-IDR arrives.
        if is_pending {
            let silence_ok = silence_started
                .map(|t| t.elapsed() >= std::time::Duration::from_millis(600))
                .unwrap_or(false);

            if let Some(ref inc) = incoming_sample {
                if let Some(incoming_buf) = inc.buffer() {
                    let incoming_keyframe =
                        !incoming_buf.flags().contains(gst::BufferFlags::DELTA_UNIT);
                    if incoming_keyframe && silence_ok {
                        pending_switch.store(false, Ordering::Release);
                        silence_started = None;

                        // Commit the switch.
                        let confirmed_cam;
                        {
                            let mut st = state.lock().unwrap();
                            for i in 0..N {
                                st.tally[i] = "idle";
                            }
                            st.tally[new_cam] = "program";
                            st.active = new_cam as u32;
                            confirmed_cam = new_cam;
                            st.print_tally();
                        }

                        // Send tally_confirm datagram to client (spec §8.3).
                        if let Some(fs) = flux_sink_weak.upgrade() {
                            let confirm = TallyConfirm {
                                msg_type: "tally_confirm".into(),
                                channel: confirmed_cam as u8,
                                state: "program".into(),
                                color: PGM_COLOR.into(),
                                label: format!("PGM CAM {}", confirmed_cam + 1),
                            };
                            let dg = confirm.encode_datagram(now_ns());
                            fs.send_datagram(dg);
                            log!(
                                "[router] cut committed → CAM {} | tally_confirm sent",
                                confirmed_cam + 1
                            );
                        }

                        // Push the incoming IDR immediately (active is now new_cam).
                        // Set DISCONT both in the GstBuffer flags (for in-process
                        // elements) and in byte 1 of the FLUX header (FLAGS field,
                        // bit 2 = DISCONT) so the client can distinguish this
                        // switch-IDR from a normal periodic keyframe.
                        let mut out_buf = incoming_buf.copy();
                        {
                            let ob = out_buf.get_mut().unwrap();
                            ob.set_flags(gst::BufferFlags::DISCONT);
                            // Patch FLUX FLAGS byte (byte 1) in-band.
                            let mut map = ob.map_writable().unwrap();
                            if map.len() > 1 {
                                map[1] |= flux_flags::DISCONT;
                            }
                        }
                        let _ = router_src.push_buffer(out_buf);
                        diag_pushed += 1;
                        continue;
                    }
                }
            }
        }

        // Push a frame from the active camera.  During a pending switch we
        // continue forwarding ALL active-camera frames (both IDRs and delta
        // frames) so the client decoder never stalls and vtdec_hw's async
        // pipeline stays busy — its 16 in-flight frames drain naturally as they
        // complete.  The silence_ok gate above ensures the DISCONT switch-IDR
        // is only committed once that pipeline is empty.
        if let Some(ref sample) = active_sample {
            let buf = match sample.buffer() {
                Some(b) => b,
                None => continue,
            };

            let out_buf = buf.copy();
            let _ = router_src.push_buffer(out_buf);
            diag_pushed += 1;
        } else {
            // No sample ready on active camera — brief sleep to avoid spinning.
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

// ── FLUX-C / tally control task ───────────────────────────────────────────────

fn run_control_task(
    ctrl_rx: std::sync::mpsc::Receiver<flux_framing::FluxControl>,
    pending_switch: Arc<AtomicBool>,
    pending_channel: Arc<AtomicU32>,
    state: Arc<Mutex<SwitcherState>>,
    _flux_sink_weak: gst::glib::WeakRef<gstfluxsink::FluxSink>,
    cam_sinks: Arc<Vec<gst_app::AppSink>>,
) {
    use flux_framing::ControlType;

    log!("[control] FLUX-C listener started");
    loop {
        match ctrl_rx.recv() {
            Ok(cmd) => {
                match cmd.control_type {
                    ControlType::Routing => {
                        // target_id = "cam-N" where N is 1-indexed.
                        if let Some(ref target) = cmd.target_id {
                            if let Some(n) = parse_cam_target(target) {
                                log!("[control] FLUX-C routing → CAM {}", n + 1);
                                request_switch(
                                    n,
                                    &pending_switch,
                                    &pending_channel,
                                    &state,
                                    &cam_sinks,
                                );
                            }
                        }
                    }
                    other => {
                        log!("[control] FLUX-C {:?} (ignored in poc004)", other);
                    }
                }
            }
            Err(_) => {
                log!("[control] FLUX-C channel closed — task exiting");
                break;
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Queue a camera switch. The router thread will commit it on the next IDR.
/// To minimise cut latency, we immediately signal the incoming camera's encoder
/// to produce an IDR on its very next frame via an UpstreamForceKeyUnit event.
fn request_switch(
    cam: usize,
    pending_switch: &Arc<AtomicBool>,
    pending_channel: &Arc<AtomicU32>,
    state: &Arc<Mutex<SwitcherState>>,
    cam_sinks: &Arc<Vec<gst_app::AppSink>>,
) {
    let active = state.lock().unwrap().active as usize;
    if cam == active {
        log!("[switcher] CAM {} is already active — ignoring", cam + 1);
        return;
    }
    pending_channel.store(cam as u32, Ordering::Release);
    pending_switch.store(true, Ordering::Release);
    // Mark the incoming camera as preview while the switch is pending.
    {
        let mut st = state.lock().unwrap();
        st.tally[cam] = "preview";
    }

    // Force the incoming camera's encoder to emit an IDR immediately.
    // Without this, the router would spin waiting for the next natural IDR
    // (up to one GOP away), and frames from both cameras would be seen by
    // the decoder while it waits — producing dissolve-style encoding errors.
    let fku = gst_video::UpstreamForceKeyUnitEvent::builder()
        .all_headers(true)
        .build();
    let sink_pad = cam_sinks[cam].static_pad("sink").unwrap();
    sink_pad.push_event(fku);

    log!("[switcher] Cut queued → CAM {} (IDR requested)", cam + 1);
}

/// Parse "cam-N" (1-indexed) → 0-indexed index.
fn parse_cam_target(s: &str) -> Option<usize> {
    let lower = s.to_ascii_lowercase();
    let n: usize = lower.strip_prefix("cam-")?.parse().ok()?;
    if n >= 1 && n <= N {
        Some(n - 1)
    } else {
        None
    }
}

// ── Keyboard handler ──────────────────────────────────────────────────────────

fn handle_key(
    ch: char,
    state: &Arc<Mutex<SwitcherState>>,
    pending_switch: &Arc<AtomicBool>,
    pending_channel: &Arc<AtomicU32>,
    cam_sinks: &Arc<Vec<gst_app::AppSink>>,
) {
    match ch {
        '1'..='4' => {
            let cam = ch as usize - '1' as usize;
            request_switch(cam, pending_switch, pending_channel, state, cam_sinks);
        }
        'T' | 't' => {
            state.lock().unwrap().print_tally();
        }
        'H' | 'h' | '?' => print_help(),
        _ => {}
    }
}

fn print_help() {
    log!(
        "\n[switcher-server] Keys:\n\
         1–4  cut to camera (queued on next IDR)\n\
         T    show tally table\n\
         H    this help\n\
         Q    quit\n"
    );
}

// ── TTY raw-mode helpers ──────────────────────────────────────────────────────

struct Tty {
    fd: std::os::unix::io::RawFd,
    orig: libc::termios,
}

impl Drop for Tty {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.orig);
            libc::close(self.fd);
        }
    }
}

fn open_tty_raw() -> Option<Tty> {
    let path = std::ffi::CString::new("/dev/tty").unwrap();
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR) };
    if fd < 0 {
        return None;
    }
    let mut orig: libc::termios = unsafe { std::mem::zeroed() };
    unsafe { libc::tcgetattr(fd, &mut orig) };
    let mut raw = orig;
    raw.c_lflag &= !(libc::ICANON
        | libc::ECHO
        | libc::ECHOE
        | libc::ECHOK
        | libc::ECHONL
        | libc::ISIG
        | libc::IEXTEN);
    raw.c_iflag &= !(libc::IXON | libc::ICRNL | libc::INLCR | libc::IGNCR);
    raw.c_cc[libc::VMIN] = 1;
    raw.c_cc[libc::VTIME] = 0;
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) };
    Some(Tty { fd, orig })
}
