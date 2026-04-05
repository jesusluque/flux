//! FLUX PoC — Client
//!
//! Pipeline:
//!   fluxsrc address=127.0.0.1 port=7400
//!     → fluxdemux  (routes media_0 and control/cdbc pads)
//!         media_0 → fluxcdbc (passthrough observer — sends CDBC_FEEDBACK)
//!                     → fluxdeframer
//!                     → h265parse
//!                     → video/x-h265,stream-format=hvc1,alignment=au
//!                     → vtdec_hw
//!                     → videoconvertscale
//!                     → fpsdisplaysink (wraps osxvideosink)
//!         cdbc    → fakesink  (server→client CDBC echo — not used in PoC)
//!
//! ── Keyboard controls (stdin) ─────────────────────────────────────────────────
//!
//!   Space     — pause / resume (PAUSED ↔ PLAYING)
//!   Q / q     — quit cleanly
//!   S / s     — print live session stats (includes NetSim state)
//!   P / p     — send a FLUX-C PTZ preset to the server (ch 0, pan=0°, tilt=0°)
//!   A / a     — toggle audio mute on channel 0 via FLUX-C audio_mix command
//!   R / r     — send a FLUX-C routing info request
//!   D / d     — toggle fps overlay on video window
//!   T / t     — cycle videotestsrc pattern on server via FLUX-C test_pattern
//!   L / l     — NetSim loss: L = +5%, l = -5% (clamped 0–100%)
//!   Y / y     — NetSim delay: Y = +20 ms, y = -20 ms (clamped 0–500 ms)
//!   B / b     — NetSim bandwidth: B = +1000 kbps, b = -1000 kbps (0 = off)
//!   H / ? / h — show this help
//!
//! All FLUX-C commands are sent as MetadataFrame (0xC) QUIC Datagrams via
//! `fluxsrc.send_datagram()` to the server (spec §12 / §14).

use glib::ControlFlow;
use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

fn main() {
    env_logger::init();

    if std::env::var("GST_DEBUG").is_err() {
        std::env::set_var("GST_DEBUG", "2");
    }

    gst::init().expect("GStreamer init failed");

    // Open /dev/tty and put it in raw mode NOW — before gst::macos_main()
    // calls [NSApp run] which makes the process a foreground Cocoa app and
    // may change how the OS delivers TTY input.
    let tty = open_tty_raw();

    // gst::macos_main() spawns our run() on a background thread and blocks
    // the main thread in [NSApp run] forever (required for osxvideosink).
    gst::macos_main(move || run(tty));
}

fn run(tty: Option<Tty>) {
    // Register custom elements
    gstfluxsrc::plugin_register_static().expect("fluxsrc register");
    gstfluxdemux::plugin_register_static().expect("fluxdemux register");
    gstfluxdeframer::plugin_register_static().expect("fluxdeframer register");
    gstfluxcdbc::plugin_register_static().expect("fluxcdbc register");

    // ── Build pipeline ────────────────────────────────────────────────────────

    let pipeline = gst::Pipeline::new();

    let fluxsrc = gst::ElementFactory::make("fluxsrc")
        .property("address", "127.0.0.1")
        .property("port", 7400u32)
        .build()
        .expect("fluxsrc element")
        .downcast::<gstfluxsrc::FluxSrc>()
        .expect("fluxsrc downcast to FluxSrc");
    // Keep a base-class reference for APIs that take &gst::Element.
    let fluxsrc_elem: &gst::Element = fluxsrc.upcast_ref();

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

    let osxvideosink = gst::ElementFactory::make("osxvideosink")
        .property("sync", false)
        .property("async", false)
        .build()
        .expect("osxvideosink element");

    let sink = gst::ElementFactory::make("fpsdisplaysink")
        .property("video-sink", &osxvideosink)
        .property("sync", false)
        .property("text-overlay", true)
        .build()
        .expect("fpsdisplaysink element");

    let fluxcdbc = gst::ElementFactory::make("fluxcdbc")
        .property("cdbc-interval", 50u64)
        .property("cdbc-min-interval", 10u64)
        .build()
        .expect("fluxcdbc element")
        .downcast::<gstfluxcdbc::FluxCdbc>()
        .expect("fluxcdbc downcast to FluxCdbc");

    // Route CDBC_FEEDBACK over QUIC (spec §4.4).  Install the send callback
    // now so it is ready before the pipeline transitions to PLAYING.
    {
        let src_for_cdbc = fluxsrc.clone();
        fluxcdbc.set_send_callback(move |pkt| {
            src_for_cdbc.send_datagram(pkt);
        });
    }
    // Keep a base-class reference for GStreamer APIs that take &gst::Element.
    let fluxcdbc_elem: &gst::Element = fluxcdbc.upcast_ref();

    let fakesink = gst::ElementFactory::make("fakesink")
        .property("sync", false)
        .build()
        .expect("fakesink element");

    pipeline
        .add_many([
            fluxsrc_elem,
            &fluxdemux,
            &fluxdeframer,
            &h265parse,
            &vtdec_hw,
            &convert,
            &sink,
            fluxcdbc_elem,
            &fakesink,
        ])
        .expect("add elements");

    fluxsrc_elem.link(&fluxdemux).expect("fluxsrc → fluxdemux");
    // fluxcdbc is passthrough — it observes raw FLUX frames and sends CDBC_FEEDBACK.
    // Wire it in-line on media_0: fluxcdbc → fluxdeframer → h265parse → ...
    fluxcdbc_elem
        .link(&fluxdeframer)
        .expect("fluxcdbc → fluxdeframer");
    fluxdeframer.link(&h265parse).expect("deframer → h265parse");

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

    // The demux `cdbc` pad carries server→client CDBC echoes (unused in PoC).
    // We wire it to fakesink so the pad doesn't stall the pipeline.

    let cdbc_fakesink_clone = fakesink.clone();
    let cdbc_element_clone = fluxcdbc.clone();
    fluxdemux.connect_pad_added(move |_elem, pad| {
        let pad_name = pad.name();
        eprintln!("[flux-client] fluxdemux pad added: {}", pad_name);
        match pad_name.as_str() {
            "media_0" => {
                // media_0 → fluxcdbc (passthrough observer) → fluxdeframer → ...
                let cdbc_sink = cdbc_element_clone
                    .static_pad("sink")
                    .expect("fluxcdbc sink pad");
                if cdbc_sink.is_linked() {
                    eprintln!("[flux-client] fluxcdbc sink already linked, skipping");
                    return;
                }
                pad.link(&cdbc_sink).expect("link media_0 → fluxcdbc");
                eprintln!("[flux-client] media_0 → fluxcdbc → fluxdeframer linked");
            }
            "cdbc" => {
                // Server→client CDBC echo pad — drain into fakesink
                let fs_sink = cdbc_fakesink_clone
                    .static_pad("sink")
                    .expect("fakesink sink pad");
                if fs_sink.is_linked() {
                    return;
                }
                pad.link(&fs_sink).expect("link cdbc → fakesink");
                eprintln!("[flux-client] cdbc (server echo) → fakesink linked");
            }
            _ => {
                // misc, control etc. — fluxdemux handles not-linked gracefully.
                eprintln!(
                    "[flux-client] pad '{}' — unlinked (not-linked is non-fatal)",
                    pad_name
                );
            }
        }
    });

    // ── Run ───────────────────────────────────────────────────────────────────
    pipeline
        .set_state(gst::State::Playing)
        .expect("Unable to start pipeline");
    eprintln!("[flux-client] Pipeline started — waiting for FLUX stream on :7400 (QUIC)");
    print_help();

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

    // ── Keyboard input thread ────────────────────────────────────────────────
    //
    // Two problems on macOS we work around here:
    //
    // 1. osxvideosink opens a Cocoa window that grabs keyboard focus.  While
    //    that window is focused, keypresses never arrive on stdin (fd 0).
    //    Fix: open /dev/tty — the controlling terminal — which always receives
    //    TTY input regardless of Cocoa focus.
    //
    // 2. ctx.invoke() posts a GLib idle source that may never fire on macOS
    //    because the Cocoa run loop — not GLib — drives the main thread.
    //    Fix: call GStreamer APIs directly from the keyboard thread.
    //    gst::Element::set_property, set_state, and glib::MainLoop::quit are
    //    all internally thread-safe and need no main-thread dispatch.

    let audio_muted = Arc::new(AtomicBool::new(false));
    let is_playing = Arc::new(AtomicBool::new(true)); // pipeline starts in Playing
    let ctrl_seq = Arc::new(Mutex::new(0u32));
    // Cycle through all videotestsrc pattern ids in order.
    // Start at 1 so the first T press moves off the server's default (smpte=0).
    const PATTERNS: &[u32] = &[
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25,
    ];
    let pattern_idx = Arc::new(AtomicU32::new(1));
    let fps_overlay_on = Arc::new(AtomicBool::new(true));

    {
        let pipeline_ctl = pipeline.clone();
        let fluxsrc_ctl = fluxsrc.clone();
        let fluxcdbc_ctl = fluxcdbc.clone();
        let main_loop_ctl = main_loop.clone();
        let audio_muted = audio_muted.clone();
        let is_playing = is_playing.clone();
        let ctrl_seq = ctrl_seq.clone();
        let pattern_idx = pattern_idx.clone();
        let fps_overlay_on = fps_overlay_on.clone();
        let sink_ctl = sink.clone();

        #[cfg(unix)]
        let tty_fd = tty.as_ref().map(|t| t.fd).unwrap_or(-1);
        #[cfg(not(unix))]
        let tty_fd = -1i32;

        std::thread::spawn(move || {
            if tty_fd < 0 {
                eprintln!("[flux-client] /dev/tty unavailable — hotkeys disabled");
                return;
            }
            let mut buf = [0u8; 1];
            loop {
                let n = unsafe { libc::read(tty_fd, buf.as_mut_ptr() as *mut libc::c_void, 1) };
                if n <= 0 {
                    break;
                }
                let key = buf[0];

                // All GStreamer calls below are thread-safe — no main-context
                // dispatch needed.
                let session_id: String = fluxsrc_ctl.property("session-id");

                match key {
                    b' ' => {
                        // Toggle playing/paused using a local AtomicBool rather
                        // than querying pipeline state — the state query blocks
                        // up to 100 ms and on macOS returns a stale value because
                        // the GLib main loop (same thread as run()) services the
                        // query, causing a consistent false-Paused result.
                        let was_playing = is_playing.fetch_xor(true, Ordering::Relaxed);
                        if was_playing {
                            pipeline_ctl.set_state(gst::State::Paused).ok();
                            eprintln!("[flux-client] PAUSED");
                        } else {
                            pipeline_ctl.set_state(gst::State::Playing).ok();
                            eprintln!("[flux-client] PLAYING");
                        }
                    }
                    b'q' | b'Q' | 0x03 => {
                        eprintln!("[flux-client] Quit");
                        main_loop_ctl.quit();
                        break;
                    }
                    b's' | b'S' => print_stats(fluxsrc_ctl.upcast_ref(), fluxcdbc_ctl.upcast_ref()),
                    b'p' | b'P' => {
                        send_flux_c(
                            flux_framing::FluxControl::ptz(&session_id, 0, 0.0, 0.0, 0.5, 0.5, 1.0),
                            &ctrl_seq,
                            &fluxsrc_ctl,
                        );
                        eprintln!("[flux-client] FLUX-C PTZ sent");
                    }
                    b'a' | b'A' => {
                        let muted = !audio_muted.load(Ordering::Relaxed);
                        audio_muted.store(muted, Ordering::Relaxed);
                        let gain = if muted { -96.0f64 } else { 0.0f64 };
                        send_flux_c(
                            flux_framing::FluxControl::audio_mix(
                                &session_id,
                                vec![muted],
                                vec![gain],
                            ),
                            &ctrl_seq,
                            &fluxsrc_ctl,
                        );
                        eprintln!(
                            "[flux-client] audio ch0 -> {}",
                            if muted { "MUTED" } else { "UNMUTED" }
                        );
                    }
                    b'r' | b'R' => {
                        eprintln!(
                            "[flux-client] routing session={} server=127.0.0.1:7400",
                            if session_id.is_empty() {
                                "<pending>"
                            } else {
                                &session_id
                            }
                        );
                        if !session_id.is_empty() {
                            send_flux_c(
                                flux_framing::FluxControl::routing(&session_id, "current"),
                                &ctrl_seq,
                                &fluxsrc_ctl,
                            );
                        }
                    }
                    b'd' | b'D' => {
                        let on = !fps_overlay_on.load(Ordering::Relaxed);
                        fps_overlay_on.store(on, Ordering::Relaxed);
                        sink_ctl.set_property("text-overlay", on);
                        eprintln!(
                            "[flux-client] fps overlay {}",
                            if on { "ON" } else { "OFF" }
                        );
                    }
                    b't' | b'T' => {
                        if session_id.is_empty() {
                            eprintln!("[flux-client] T: no session yet");
                        } else {
                            let idx = pattern_idx.fetch_add(1, Ordering::Relaxed) as usize;
                            let pat_id = PATTERNS[idx % PATTERNS.len()];
                            send_flux_c(
                                flux_framing::FluxControl::test_pattern(&session_id, pat_id),
                                &ctrl_seq,
                                &fluxsrc_ctl,
                            );
                            eprintln!("[flux-client] FLUX-C test_pattern → {}", pat_id);
                        }
                    }
                    // ── NetSim hotkeys ────────────────────────────────────────
                    b'L' => {
                        let cur: f64 = fluxsrc_ctl.property("sim-loss-pct");
                        let new = (cur + 5.0).min(100.0);
                        fluxsrc_ctl.set_property("sim-loss-pct", new);
                        eprintln!("[flux-client] NetSim loss → {:.1}%", new);
                    }
                    b'l' => {
                        let cur: f64 = fluxsrc_ctl.property("sim-loss-pct");
                        let new = (cur - 5.0).max(0.0);
                        fluxsrc_ctl.set_property("sim-loss-pct", new);
                        eprintln!("[flux-client] NetSim loss → {:.1}%", new);
                    }
                    b'Y' => {
                        let cur: u32 = fluxsrc_ctl.property("sim-delay-ms");
                        let new = (cur + 20).min(500);
                        fluxsrc_ctl.set_property("sim-delay-ms", new);
                        eprintln!("[flux-client] NetSim delay → {} ms", new);
                    }
                    b'y' => {
                        let cur: u32 = fluxsrc_ctl.property("sim-delay-ms");
                        let new = cur.saturating_sub(20);
                        fluxsrc_ctl.set_property("sim-delay-ms", new);
                        eprintln!("[flux-client] NetSim delay → {} ms", new);
                    }
                    b'B' => {
                        let cur: u32 = fluxsrc_ctl.property("sim-bw-kbps");
                        let new = cur + 1000;
                        fluxsrc_ctl.set_property("sim-bw-kbps", new);
                        eprintln!("[flux-client] NetSim bw → {} kbps", new);
                    }
                    b'b' => {
                        let cur: u32 = fluxsrc_ctl.property("sim-bw-kbps");
                        let new = cur.saturating_sub(1000);
                        fluxsrc_ctl.set_property("sim-bw-kbps", new);
                        if new == 0 {
                            eprintln!("[flux-client] NetSim bw → OFF (unlimited)");
                        } else {
                            eprintln!("[flux-client] NetSim bw → {} kbps", new);
                        }
                    }
                    b'h' | b'H' | b'?' => print_help(),
                    _ => {}
                }
            }
        });
    }

    main_loop.run();

    drop(tty); // restores termios
    pipeline.set_state(gst::State::Null).unwrap();
    eprintln!("[flux-client] Stopped");
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn print_help() {
    eprintln!();
    eprintln!("[flux-client] Controls:");
    eprintln!("  Space — pause / resume");
    eprintln!("  Q     — quit");
    eprintln!("  S     — print live stats (includes NetSim state)");
    eprintln!("  P     — send FLUX-C PTZ preset (ch 0)");
    eprintln!("  A     — toggle audio mute ch 0 via FLUX-C");
    eprintln!("  R     — show routing / session info");
    eprintln!("  D     — toggle fps overlay on video window");
    eprintln!(
        "  T     — cycle server test pattern via FLUX-C (0=smpte … 25=smpte-rp-219, wraps around)"
    );
    eprintln!("  L/l   — NetSim packet loss  +5% / -5%  (clamped 0–100%)");
    eprintln!("  Y/y   — NetSim delay       +20ms / -20ms  (clamped 0–500 ms)");
    eprintln!("  B/b   — NetSim bandwidth +1000 / -1000 kbps  (0 = off)");
    eprintln!("  H / ? — show this help");
    eprintln!();
}

fn print_stats(fluxsrc: &gst::Element, fluxcdbc: &gst::Element) {
    let session_id: String = fluxsrc.property("session-id");
    let ka_sent: u64 = fluxsrc.property("keepalives-sent");
    let ka_interval: u32 = fluxsrc.property("keepalive-interval-ms");
    let ka_timeout: u32 = fluxsrc.property("keepalive-timeout-count");
    let reports_sent: u64 = fluxcdbc.property("reports-sent");
    let lost_total: u64 = fluxcdbc.property("datagrams-lost-total");
    let loss_pct: f64 = fluxcdbc.property("loss-pct");
    let jitter_ms: f64 = fluxcdbc.property("jitter-ms");
    let rx_bps: u64 = fluxcdbc.property("rx-bps");
    // NetSim state
    let sim_loss: f64 = fluxsrc.property("sim-loss-pct");
    let sim_delay: u32 = fluxsrc.property("sim-delay-ms");
    let sim_bw: u32 = fluxsrc.property("sim-bw-kbps");
    eprintln!();
    eprintln!("[flux-client] ── Live Stats ──────────────────────────────");
    eprintln!(
        "  Session ID          : {}",
        if session_id.is_empty() {
            "<not yet>"
        } else {
            &session_id
        }
    );
    eprintln!(
        "  KA interval         : {} ms  (timeout after {} missed)",
        ka_interval, ka_timeout
    );
    eprintln!("  Keepalives sent     : {}", ka_sent);
    eprintln!("  CDBC reports sent   : {}", reports_sent);
    eprintln!(
        "  Datagrams lost      : {}  (loss {:.2}%)",
        lost_total, loss_pct
    );
    eprintln!("  Jitter              : {:.2} ms", jitter_ms);
    eprintln!(
        "  RX bitrate          : {} bps  ({:.1} Mbps)",
        rx_bps,
        rx_bps as f64 / 1_000_000.0
    );
    eprintln!("[flux-client] ── NetSim ──────────────────────────────────");
    eprintln!("  Loss                : {:.1}%", sim_loss);
    eprintln!("  Delay               : {} ms", sim_delay);
    if sim_bw == 0 {
        eprintln!("  Bandwidth cap       : OFF (unlimited)");
    } else {
        eprintln!("  Bandwidth cap       : {} kbps", sim_bw);
    }
    eprintln!("[flux-client] ─────────────────────────────────────────────");
    eprintln!();
}

/// Send a `FluxControl` command as a QUIC datagram to the server via the
/// live fluxsrc connection.  Falls back to a no-op if not yet connected.
fn send_flux_c(
    cmd: flux_framing::FluxControl,
    seq_store: &Arc<Mutex<u32>>,
    fluxsrc: &gstfluxsrc::FluxSrc,
) {
    let seq = {
        let mut s = seq_store.lock().unwrap();
        let v = *s;
        *s = s.wrapping_add(1);
        v
    };
    let datagram = cmd.encode_datagram(seq);
    if !fluxsrc.send_datagram(datagram) {
        eprintln!("[flux-client] send_flux_c: no active QUIC connection — dropped");
    }
}

// ─── Raw terminal mode (macOS / POSIX) ───────────────────────────────────────
//
// We open /dev/tty directly — the controlling terminal — rather than reading
// from stdin (fd 0).  On macOS, osxvideosink opens a Cocoa window that grabs
// keyboard focus; while that window is focused, keypresses never arrive on
// stdin.  /dev/tty always receives TTY input regardless of which window has
// Cocoa focus, so this approach works reliably.

#[cfg(unix)]
pub struct Tty {
    fd: std::os::unix::io::RawFd,
    old: libc::termios,
}

#[cfg(unix)]
impl Drop for Tty {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.old) };
        unsafe { libc::close(self.fd) };
    }
}

#[cfg(unix)]
fn open_tty_raw() -> Option<Tty> {
    use std::ffi::CString;
    let path = CString::new("/dev/tty").unwrap();
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if fd < 0 {
        eprintln!("[flux-client] could not open /dev/tty");
        return None;
    }
    let mut old = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(fd, &mut old) } != 0 {
        unsafe { libc::close(fd) };
        return None;
    }
    let mut raw = old;
    unsafe {
        // Only disable canonical mode and echo — do NOT call cfmakeraw, which
        // also turns off OPOST/ONLCR output processing and causes eprintln!
        // to print bare \n (no carriage return) producing a staircase effect.
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        libc::tcsetattr(fd, libc::TCSANOW, &raw);
    }
    Some(Tty { fd, old })
}

#[cfg(not(unix))]
pub struct Tty;
#[cfg(not(unix))]
fn open_tty_raw() -> Option<Tty> {
    None
}
