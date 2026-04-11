//! FLUX PoC002 — Multi-Server
//!
//! Runs 4 independent GStreamer encode-and-send pipelines in a single process,
//! each sourcing a different videotestsrc pattern and transmitting on a
//! dedicated QUIC port.
//!
//! Stream layout:
//!   Stream 0  pattern=pinwheel   port 7400   keys: 1 / shift+1
//!   Stream 1  pattern=pinwheel   port 7401   keys: 2 / shift+2
//!   Stream 2  pattern=pinwheel   port 7402   keys: 3 / shift+3
//!   Stream 3  pattern=pinwheel   port 7403   keys: 4 / shift+4
//!
//! Each pipeline:
//!   videotestsrc(pattern=N) is-live=true
//!     → videoconvertscale
//!     → video/x-raw,width=640,height=360,framerate=30/1
//!     → clockoverlay(HH:MM:SS, centred, large)
//!     → timeoverlay(running-time, bottom-left)
//!     → textoverlay("CAM N", top-left)
//!     → identity(sleep-time=D_N µs)        ← artificial delay for sync demo
//!     → vtenc_h265(realtime=true)
//!     → h265parse(config-interval=-1)
//!     → fluxframer(channel-id=N, group-id=1)
//!     → fluxsink(port=740N)
//!
//! ── Keyboard controls ─────────────────────────────────────────────────────────
//!
//!   1–4      — select active stream (1=stream0 … 4=stream3)
//!   + / =    — increase delay on selected stream by 10 ms
//!   -        — decrease delay on selected stream by 10 ms (min 0)
//!   R / r    — reset all delays to 0
//!   S / s    — show current delay table
//!   Q / q    — quit
//!   H / ?    — help

use gstreamer as gst;
use gstreamer::prelude::*;
use std::io::Write;
use std::sync::{Arc, Mutex};

// ── Serialized stderr logger ──────────────────────────────────────────────────

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

// Number of source streams.
const N: usize = 4;

// Per-stream videotestsrc patterns.
// All 4 streams use the same spinning pattern so the mosaic viewer can
// immediately spot sync mismatches: if the tiles are in sync the rotation
// looks identical across all 4; any delay shows as a visible phase offset.
// "pinwheel" gives fast, unambiguous rotation against a bright background.
const PATTERNS: [&str; N] = ["pinwheel", "pinwheel", "pinwheel", "pinwheel"];

// Human-readable label burned into each tile.
const LABELS: [&str; N] = ["CAM 1", "CAM 2", "CAM 3", "CAM 4"];

// Base QUIC port for stream 0; stream N uses BASE_PORT + N.
const BASE_PORT: u32 = 7400;

// group-id broadcast to all streams (matches the fluxsync group= property on client).
const GROUP_ID: u32 = 1;

// Delay step in microseconds for keyboard adjustments (10 ms).
const DELAY_STEP_US: i64 = 10_000;

// Maximum delay we allow (500 ms).
const MAX_DELAY_US: i64 = 500_000;

// ── Per-stream state ──────────────────────────────────────────────────────────

struct Stream {
    identity: gst::Element,
    delay_us: i64,
}

fn main() {
    env_logger::init();

    gst::init().expect("GStreamer init failed");

    // Register plugins from statically linked crates.
    gstfluxframer::plugin_register_static().expect("fluxframer register");
    gstfluxsink::plugin_register_static().expect("fluxsink register");

    // Open /dev/tty in raw mode before spinning up Cocoa.
    let tty = open_tty_raw();
    if tty.is_none() {
        eprintln!("[multi-server] WARNING: could not open /dev/tty — keyboard disabled");
    }

    gst::macos_main(move || run(tty));
}

fn run(tty: Option<Tty>) {
    let streams: Arc<Mutex<[Stream; N]>> = {
        let arr: [Stream; N] = std::array::from_fn(|i| {
            // Identity element is created separately per-pipeline below;
            // we'll fill these in after building.
            let _ = i;
            Stream {
                identity: gst::ElementFactory::make("identity").build().unwrap(),
                delay_us: 0,
            }
        });
        Arc::new(Mutex::new(arr))
    };

    // Build the 4 pipelines.
    let mut pipelines: Vec<gst::Pipeline> = Vec::with_capacity(N);

    for i in 0..N {
        let pipeline = build_stream_pipeline(i);
        // Retrieve the identity element we stored.
        let identity = pipeline
            .by_name(&format!("identity_{}", i))
            .expect("identity element not found");
        streams.lock().unwrap()[i].identity = identity;
        pipelines.push(pipeline);
    }

    // Start all pipelines.
    for (i, pl) in pipelines.iter().enumerate() {
        pl.set_state(gst::State::Playing)
            .expect("pipeline set_state Playing");
        log!(
            "[multi-server] Stream {} started → FLUX on port {}",
            i,
            BASE_PORT + i as u32
        );
    }

    log!(
        "[multi-server] All {} streams running — group_id={}\n\
         Keys: 1–4 select stream, +/- adjust delay (10ms step), R reset, S status, Q quit, H help",
        N,
        GROUP_ID
    );

    // Ctrl-C handler — send EOS to all pipelines.
    {
        let pl_clones: Vec<_> = pipelines.iter().map(|p| p.downgrade()).collect();
        ctrlc::set_handler(move || {
            log!("[multi-server] Ctrl-C — sending EOS");
            for pw in &pl_clones {
                if let Some(p) = pw.upgrade() {
                    p.send_event(gst::event::Eos::new());
                }
            }
        })
        .expect("ctrlc handler");
    }

    // Keyboard input: background thread does blocking reads, main loop drains via idle_add_local.
    let main_loop = glib::MainLoop::new(None, false);
    let streams_for_keys = streams.clone();
    let mut selected: usize = 0;

    if let Some(tty) = tty {
        let (tx, rx) = std::sync::mpsc::channel::<char>();
        std::thread::spawn(move || {
            let fd = tty.fd;
            log!("[multi-server] keyboard thread started (fd={})", fd);
            loop {
                let mut buf = [0u8; 1];
                let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 1) };
                if n <= 0 {
                    log!(
                        "[multi-server] keyboard thread: read returned {} — exiting",
                        n
                    );
                    break;
                }
                log!("[multi-server] key: 0x{:02x} '{}'", buf[0], buf[0] as char);
                if tx.send(buf[0] as char).is_err() {
                    break;
                }
            }
            drop(tty); // keep Tty alive until thread exits so raw mode persists
        });

        let ml = main_loop.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(20), move || {
            while let Ok(ch) = rx.try_recv() {
                if ch == 'Q' || ch == 'q' || ch == '\x03' {
                    handle_key(ch, &mut selected, &streams_for_keys);
                    ml.quit();
                    return glib::ControlFlow::Break;
                }
                handle_key(ch, &mut selected, &streams_for_keys);
            }
            glib::ControlFlow::Continue
        });
    }

    // Bus watcher: quit on EOS or error from any pipeline.
    let mut _bus_watches = Vec::new();
    for pl in &pipelines {
        let ml = main_loop.clone();
        let bus = pl.bus().unwrap();
        let w = bus
            .add_watch(move |_, msg| {
                use gst::MessageView;
                match msg.view() {
                    MessageView::Eos(..) => {
                        log!("[multi-server] EOS");
                        ml.quit();
                        return glib::ControlFlow::Break;
                    }
                    MessageView::Error(err) => {
                        log!("[multi-server] ERROR: {} ({:?})", err.error(), err.debug());
                        ml.quit();
                        return glib::ControlFlow::Break;
                    }
                    _ => {}
                }
                glib::ControlFlow::Continue
            })
            .unwrap();
        _bus_watches.push(w);
    }

    main_loop.run();

    for pl in &pipelines {
        pl.set_state(gst::State::Null).unwrap();
    }
    log!("[multi-server] Stopped");
}

// ── Build a single send pipeline ─────────────────────────────────────────────
//
// Full pipeline per stream:
//   videotestsrc(pattern=X, is-live=true)
//     → videoconvertscale
//     → video/x-raw I420 640×360 30fps
//     → clockoverlay  (wall-clock HH:MM:SS, centred, large font)
//     → timeoverlay   (running-time, bottom-left, smaller font)
//     → textoverlay   (stream label "CAM N", top-left)
//     → identity(sleep-time=0)   ← artificial delay knob
//     → vtenc_h265(realtime=true, allow-frame-reordering=false, bitrate=2000)
//     → h265parse(config-interval=-1)
//     → fluxframer(channel-id=N, group-id=1)
//     → fluxsink(port=740N)

fn build_stream_pipeline(idx: usize) -> gst::Pipeline {
    let pipeline = gst::Pipeline::with_name(&format!("pipeline_{}", idx));

    let src = gst::ElementFactory::make("videotestsrc")
        .property_from_str("pattern", PATTERNS[idx])
        .property("is-live", true)
        .name(&format!("src_{}", idx))
        .build()
        .expect("videotestsrc");

    let convert = gst::ElementFactory::make("videoconvertscale")
        .name(&format!("convert_{}", idx))
        .build()
        .expect("videoconvertscale");

    // Wall-clock timecode overlay — large, centred.
    // %H:%M:%S.%2N  →  HH:MM:SS.cc  (hundredths of second)
    // Sub-second precision makes frame-level sync mismatch immediately visible:
    // in a well-synced mosaic all 4 tiles show the exact same digit; any delay
    // shows as a different hundredths value.
    let clockoverlay = gst::ElementFactory::make("clockoverlay")
        .property_from_str("time-format", "%H:%M:%S.%2N")
        .property_from_str("halignment", "center")
        .property_from_str("valignment", "center")
        .property_from_str("font-desc", "Sans Bold 36")
        .property("shaded-background", true)
        .name(&format!("clockoverlay_{}", idx))
        .build()
        .expect("clockoverlay");

    // Running-time overlay — smaller, bottom-left.
    let timeoverlay = gst::ElementFactory::make("timeoverlay")
        .property_from_str("time-mode", "running-time")
        .property_from_str("halignment", "left")
        .property_from_str("valignment", "bottom")
        .property_from_str("font-desc", "Monospace Bold 18")
        .property("shaded-background", true)
        .name(&format!("timeoverlay_{}", idx))
        .build()
        .expect("timeoverlay");

    // Stream-label overlay — top-left.
    let textoverlay = gst::ElementFactory::make("textoverlay")
        .property("text", LABELS[idx])
        .property_from_str("halignment", "left")
        .property_from_str("valignment", "top")
        .property_from_str("font-desc", "Sans Bold 24")
        .property("shaded-background", true)
        .name(&format!("textoverlay_{}", idx))
        .build()
        .expect("textoverlay");

    // identity element used for artificial delay injection.
    // sleep-time property: pause this many microseconds after each buffer.
    let identity = gst::ElementFactory::make("identity")
        .property("sleep-time", 0u32)
        .name(&format!("identity_{}", idx))
        .build()
        .expect("identity");

    let vtenc = gst::ElementFactory::make("vtenc_h265")
        .property("realtime", true)
        .property("allow-frame-reordering", false)
        .property("bitrate", 2000u32)
        .name(&format!("vtenc_{}", idx))
        .build()
        .expect("vtenc_h265");

    let h265parse = gst::ElementFactory::make("h265parse")
        .property("config-interval", -1i32)
        .name(&format!("h265parse_{}", idx))
        .build()
        .expect("h265parse");

    let fluxframer = gst::ElementFactory::make("fluxframer")
        .property("channel-id", idx as u32)
        .property("group-id", GROUP_ID)
        .name(&format!("fluxframer_{}", idx))
        .build()
        .expect("fluxframer");

    let fluxsink = gst::ElementFactory::make("fluxsink")
        .property("port", BASE_PORT + idx as u32)
        .name(&format!("fluxsink_{}", idx))
        .build()
        .expect("fluxsink");

    pipeline
        .add_many([
            &src,
            &convert,
            &clockoverlay,
            &timeoverlay,
            &textoverlay,
            &identity,
            &vtenc,
            &h265parse,
            &fluxframer,
            &fluxsink,
        ])
        .expect("add elements");

    // Caps after convert: I420 640×360 30fps (overlay elements accept this).
    let caps_360p30 = gst::Caps::builder("video/x-raw")
        .field("width", 640i32)
        .field("height", 360i32)
        .field("framerate", gst::Fraction::new(30, 1))
        .build();

    src.link(&convert).expect("src → convert");
    convert
        .link_filtered(&clockoverlay, &caps_360p30)
        .expect("convert → clockoverlay (360p30)");
    clockoverlay
        .link(&timeoverlay)
        .expect("clockoverlay → timeoverlay");
    timeoverlay
        .link(&textoverlay)
        .expect("timeoverlay → textoverlay");
    textoverlay.link(&identity).expect("textoverlay → identity");
    identity.link(&vtenc).expect("identity → vtenc");
    vtenc.link(&h265parse).expect("vtenc → h265parse");
    let bs_caps = gst::Caps::builder("video/x-h265")
        .field("stream-format", "byte-stream")
        .field("alignment", "au")
        .build();
    h265parse
        .link_filtered(&fluxframer, &bs_caps)
        .expect("h265parse → fluxframer");
    fluxframer.link(&fluxsink).expect("fluxframer → fluxsink");

    pipeline
}

// ── Keyboard handler ──────────────────────────────────────────────────────────

fn handle_key(ch: char, selected: &mut usize, streams: &Arc<Mutex<[Stream; N]>>) {
    match ch {
        '1'..='4' => {
            *selected = ch as usize - '1' as usize;
            log!("[multi-server] Selected stream {}", *selected);
        }
        '+' | '=' => {
            let mut st = streams.lock().unwrap();
            let s = &mut st[*selected];
            s.delay_us = (s.delay_us + DELAY_STEP_US).min(MAX_DELAY_US);
            s.identity.set_property("sleep-time", s.delay_us as u32);
            log!(
                "[multi-server] Stream {} delay → {} ms",
                *selected,
                s.delay_us / 1000
            );
        }
        '-' => {
            let mut st = streams.lock().unwrap();
            let s = &mut st[*selected];
            s.delay_us = (s.delay_us - DELAY_STEP_US).max(0);
            s.identity.set_property("sleep-time", s.delay_us as u32);
            log!(
                "[multi-server] Stream {} delay → {} ms",
                *selected,
                s.delay_us / 1000
            );
        }
        'R' | 'r' => {
            let mut st = streams.lock().unwrap();
            for (i, s) in st.iter_mut().enumerate() {
                s.delay_us = 0;
                s.identity.set_property("sleep-time", 0u32);
                log!("[multi-server] Stream {} delay reset", i);
            }
        }
        'S' | 's' => {
            let st = streams.lock().unwrap();
            log!("[multi-server] Delay table:");
            for (i, s) in st.iter().enumerate() {
                log!(
                    "  stream {} (port {}): {} ms",
                    i,
                    BASE_PORT + i as u32,
                    s.delay_us / 1000
                );
            }
        }
        'Q' | 'q' | '\x03' => {
            log!("[multi-server] Quit");
            // main loop quit is handled by the receiver in run()
        }
        'H' | 'h' | '?' => print_help(),
        _ => {}
    }
}

fn print_help() {
    log!(
        "\n[multi-server] Keys:\n\
         1–4    select stream\n\
         + / =  increase delay on selected stream (10 ms)\n\
         -      decrease delay on selected stream (10 ms)\n\
         R      reset all delays\n\
         S      show delay table\n\
         H / ?  show this help\n\
         Q      quit\n"
    );
}

// ── TTY raw-mode helpers (same pattern as poc001 client) ──────────────────────

use gst::glib;
use libc;

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
    // Open in blocking mode — reads happen on a dedicated thread.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR) };
    if fd < 0 {
        return None;
    }
    let mut orig: libc::termios = unsafe { std::mem::zeroed() };
    unsafe { libc::tcgetattr(fd, &mut orig) };
    let mut raw = orig;
    // cfmakeraw disables OPOST which turns off \n→\r\n mapping, causing
    // staircase output.  Instead set only the input flags we need manually:
    // disable canonical mode, echo, and signal generation — but leave output
    // processing (OPOST / ONLCR) intact so \n still moves to column 0.
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
