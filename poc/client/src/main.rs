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
//!                     → osxvideosink
//!         cdbc    → fakesink  (server→client CDBC echo — not used in PoC)
//!
//! ── Keyboard controls (stdin) ─────────────────────────────────────────────────
//!
//!   Space     — pause / resume (PAUSED ↔ PLAYING)
//!   Q / q     — quit cleanly
//!   S / s     — print live session stats
//!   P / p     — send a FLUX-C PTZ preset to the server (ch 0, pan=0°, tilt=0°)
//!   A / a     — toggle audio mute on channel 0 via FLUX-C audio_mix command
//!   R / r     — send a FLUX-C routing info request (prints current session only;
//!               routing redirect itself would require a target known at runtime)
//!
//! All FLUX-C commands are sent as MetadataFrame (0xC) datagrams over UDP to the
//! server media port (spec §12 / §14).  The server logs receipt.

use glib::ControlFlow;
use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;
use std::net::UdpSocket;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

fn main() {
    env_logger::init();

    // Suppress GStreamer debug spam unless the caller has already set GST_DEBUG.
    // Level 2 = WARNING: serious but non-fatal issues only.
    if std::env::var("GST_DEBUG").is_err() {
        std::env::set_var("GST_DEBUG", "2");
    }

    gst::init().expect("GStreamer init failed");

    // On macOS, GStreamer video sinks require NSApplication to be running on
    // the main thread.  gst::macos_main() sets this up.
    gst::macos_main(run);
}

fn run() {
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

    let sink = gst::ElementFactory::make("osxvideosink")
        .property("sync", false)
        .property("async", false)
        .build()
        .expect("osxvideosink element");

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
            &sink,
            &fluxcdbc,
            &fakesink,
        ])
        .expect("add elements");

    fluxsrc.link(&fluxdemux).expect("fluxsrc → fluxdemux");
    // fluxcdbc is passthrough — it observes raw FLUX frames and sends CDBC_FEEDBACK.
    // Wire it in-line on media_0: fluxcdbc → fluxdeframer → h265parse → ...
    fluxcdbc
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
                eprintln!("[flux-client] unhandled pad '{}' — ignoring", pad_name);
            }
        }
    });

    // ── Run ───────────────────────────────────────────────────────────────────
    pipeline
        .set_state(gst::State::Playing)
        .expect("Unable to start pipeline");
    eprintln!("[flux-client] Pipeline started — waiting for FLUX stream on :7400");
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

    // ── Keyboard input via GLib fd watch ────────────────────────────────────
    //
    // On macOS, gst::macos_main() owns the main thread (Cocoa run loop).
    // A blocking stdin.read() in a background thread is unreliable there.
    // The correct approach is to register stdin fd=0 with the GLib main loop
    // via glib::unix_fd_add so keystrokes are delivered on the same thread
    // as main_loop.run(), with no blocking or thread-safety issues.

    // Put terminal into raw mode (single-keypress, no echo, no line buffer).
    let old_termios = set_raw_mode();
    let old_termios_cell = Arc::new(Mutex::new(old_termios));

    // Mute state (channel 0) toggled by 'A'
    let audio_muted = Arc::new(AtomicBool::new(false));
    // FLUX-C sequence counter
    let ctrl_seq = Arc::new(Mutex::new(0u32));

    let pipeline_ctl = pipeline.clone();
    let fluxsrc_ctl = fluxsrc.clone();
    let fluxcdbc_ctl = fluxcdbc.clone();
    let main_loop_ctl = main_loop.clone();
    let termios_ctl = old_termios_cell.clone();

    let stdin_fd = std::io::stdin().as_raw_fd();
    glib::unix_fd_add(stdin_fd, glib::IOCondition::IN, move |_fd, _cond| {
        use std::io::Read;
        let mut buf = [0u8; 1];
        if std::io::stdin().lock().read(&mut buf).is_err() {
            return ControlFlow::Break;
        }
        let key = buf[0];

        let session_id: String = fluxsrc_ctl.property("session-id");

        match key {
            b' ' => {
                let (_, cur, _) = pipeline_ctl.state(gst::ClockTime::NONE);
                if cur == gst::State::Playing {
                    pipeline_ctl.set_state(gst::State::Paused).ok();
                    eprintln!("[flux-client] PAUSED");
                } else {
                    pipeline_ctl.set_state(gst::State::Playing).ok();
                    eprintln!("[flux-client] PLAYING");
                }
            }

            b'q' | b'Q' | 0x03 /* Ctrl-C */ => {
                eprintln!("[flux-client] Quit");
                restore_termios(*termios_ctl.lock().unwrap());
                main_loop_ctl.quit();
                return ControlFlow::Break;
            }

            b's' | b'S' => {
                print_stats(&fluxsrc_ctl, &fluxcdbc_ctl);
            }

            b'p' | b'P' => {
                send_flux_c(
                    flux_framing::FluxControl::ptz(
                        &session_id, 0, 0.0, 0.0, 0.5, 0.5, 1.0,
                    ),
                    &ctrl_seq,
                );
                eprintln!("[flux-client] FLUX-C PTZ preset sent (ch 0 pan=0 tilt=0 zoom=0.5)");
            }

            b'a' | b'A' => {
                let muted = !audio_muted.load(Ordering::Relaxed);
                audio_muted.store(muted, Ordering::Relaxed);
                let gain = if muted { -96.0f64 } else { 0.0f64 };
                send_flux_c(
                    flux_framing::FluxControl::audio_mix(
                        &session_id, vec![muted], vec![gain],
                    ),
                    &ctrl_seq,
                );
                eprintln!(
                    "[flux-client] FLUX-C audio ch 0 -> {}",
                    if muted { "MUTED" } else { "UNMUTED" }
                );
            }

            b'r' | b'R' => {
                eprintln!(
                    "[flux-client] FLUX-C routing — session_id={}  server=127.0.0.1:7400",
                    if session_id.is_empty() { "<not yet negotiated>" } else { &session_id }
                );
                if !session_id.is_empty() {
                    send_flux_c(
                        flux_framing::FluxControl::routing(&session_id, "current"),
                        &ctrl_seq,
                    );
                }
            }

            b'h' | b'H' | b'?' => print_help(),

            _ => {}
        }
        ControlFlow::Continue
    });

    main_loop.run();

    // Restore terminal before exiting (normal path — quit via bus EOS/error)
    restore_termios(*old_termios_cell.lock().unwrap());

    pipeline.set_state(gst::State::Null).unwrap();
    eprintln!("[flux-client] Stopped");
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn print_help() {
    eprintln!();
    eprintln!("[flux-client] Controls:");
    eprintln!("  Space — pause / resume");
    eprintln!("  Q     — quit");
    eprintln!("  S     — print live stats");
    eprintln!("  P     — send FLUX-C PTZ preset (ch 0)");
    eprintln!("  A     — toggle audio mute ch 0 via FLUX-C");
    eprintln!("  R     — show routing / session info");
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
    eprintln!("[flux-client] ─────────────────────────────────────────────");
    eprintln!();
}

/// Send a `FluxControl` command as a UDP datagram to the server media port.
fn send_flux_c(cmd: flux_framing::FluxControl, seq_store: &Arc<Mutex<u32>>) {
    let seq = {
        let mut s = seq_store.lock().unwrap();
        let v = *s;
        *s = s.wrapping_add(1);
        v
    };
    let datagram = cmd.encode_datagram(seq);
    // Best-effort send; bind an ephemeral socket for this one datagram.
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        let _ = sock.send_to(&datagram, "127.0.0.1:7400");
    }
}

// ─── Raw terminal mode (macOS / POSIX) ───────────────────────────────────────
//
// We need single-keypress input without waiting for Enter.  We switch stdin to
// raw (non-canonical, no echo) mode using libc termios calls.

#[cfg(unix)]
type Termios = libc::termios;

#[cfg(unix)]
fn set_raw_mode() -> Option<Termios> {
    use std::os::unix::io::AsRawFd;
    let fd = std::io::stdin().as_raw_fd();
    let mut old = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(fd, &mut old) } != 0 {
        return None;
    }
    let mut raw = old;
    unsafe {
        libc::cfmakeraw(&mut raw);
        libc::tcsetattr(fd, libc::TCSANOW, &raw);
    }
    Some(old)
}

#[cfg(unix)]
fn restore_termios(saved: Option<Termios>) {
    if let Some(old) = saved {
        use std::os::unix::io::AsRawFd;
        let fd = std::io::stdin().as_raw_fd();
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &old) };
    }
}

#[cfg(not(unix))]
fn set_raw_mode() -> Option<()> {
    None
}
#[cfg(not(unix))]
fn restore_termios(_: Option<()>) {}
