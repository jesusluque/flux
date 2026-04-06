//! FLUX PoC002 — Multi-Server
//!
//! Runs 4 independent GStreamer encode-and-send pipelines in a single process,
//! each sourcing a different videotestsrc pattern and transmitting on a
//! dedicated QUIC port.
//!
//! Stream layout:
//!   Stream 0  pattern=0 (SMPTE bars)      port 7400   keys: 1 / shift+1
//!   Stream 1  pattern=1 (snow)            port 7401   keys: 2 / shift+2
//!   Stream 2  pattern=18 (ball)           port 7402   keys: 3 / shift+3
//!   Stream 3  pattern=24 (checkers-8)     port 7403   keys: 4 / shift+4
//!
//! Each pipeline:
//!   videotestsrc(pattern=N) is-live=true
//!     → videoconvertscale
//!     → video/x-raw,width=640,height=360,framerate=30/1
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
use std::sync::{Arc, Mutex};

// Number of source streams.
const N: usize = 4;

// Per-stream starting patterns (videotestsrc "pattern" property).
const PATTERNS: [u32; N] = [0, 1, 18, 24];

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
        eprintln!(
            "[multi-server] Stream {} started → FLUX on port {}",
            i,
            BASE_PORT + i as u32
        );
    }

    eprintln!(
        "[multi-server] All {} streams running — group_id={}\n\
         Keys: 1–4 select stream, +/- adjust delay (10ms step), R reset, S status, Q quit, H help",
        N, GROUP_ID
    );

    // Ctrl-C handler — send EOS to all pipelines.
    {
        let pl_clones: Vec<_> = pipelines.iter().map(|p| p.downgrade()).collect();
        ctrlc::set_handler(move || {
            eprintln!("[multi-server] Ctrl-C — sending EOS");
            for pw in &pl_clones {
                if let Some(p) = pw.upgrade() {
                    p.send_event(gst::event::Eos::new());
                }
            }
        })
        .expect("ctrlc handler");
    }

    // Keyboard input loop (raw /dev/tty).
    let streams_for_keys = streams.clone();
    let mut selected: usize = 0;

    if let Some(tty) = tty {
        let fd = tty.fd;
        glib::unix_fd_add_local(fd, glib::IOCondition::IN, move |_, _| {
            let mut buf = [0u8; 1];
            let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, 1) };
            if n <= 0 {
                return glib::ControlFlow::Continue;
            }
            let ch = buf[0] as char;
            handle_key(ch, &mut selected, &streams_for_keys);
            glib::ControlFlow::Continue
        });
    }

    // Run main loop (needed for glib::unix_fd_add_local).
    let main_loop = glib::MainLoop::new(None, false);

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
                        eprintln!("[multi-server] EOS");
                        ml.quit();
                        return glib::ControlFlow::Break;
                    }
                    MessageView::Error(err) => {
                        eprintln!("[multi-server] ERROR: {} ({:?})", err.error(), err.debug());
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
    eprintln!("[multi-server] Stopped");
}

// ── Build a single send pipeline ─────────────────────────────────────────────

fn build_stream_pipeline(idx: usize) -> gst::Pipeline {
    let pipeline = gst::Pipeline::with_name(&format!("pipeline_{}", idx));

    let src = gst::ElementFactory::make("videotestsrc")
        .property("pattern", PATTERNS[idx])
        .property("is-live", true)
        .name(&format!("src_{}", idx))
        .build()
        .expect("videotestsrc");

    let convert = gst::ElementFactory::make("videoconvertscale")
        .name(&format!("convert_{}", idx))
        .build()
        .expect("videoconvertscale");

    // identity element used for artificial delay injection.
    // sleep-time property: pause this many microseconds after each buffer.
    let identity = gst::ElementFactory::make("identity")
        .property("sleep-time", 0u64)
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
            &identity,
            &vtenc,
            &h265parse,
            &fluxframer,
            &fluxsink,
        ])
        .expect("add elements");

    // Link: src → convert → (360p30 caps) → identity → vtenc
    let caps_360p30 = gst::Caps::builder("video/x-raw")
        .field("width", 640i32)
        .field("height", 360i32)
        .field("framerate", gst::Fraction::new(30, 1))
        .build();
    src.link(&convert).expect("src → convert");
    convert
        .link_filtered(&identity, &caps_360p30)
        .expect("convert → identity (360p30)");
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
            eprintln!("[multi-server] Selected stream {}", *selected);
        }
        '+' | '=' => {
            let mut st = streams.lock().unwrap();
            let s = &mut st[*selected];
            s.delay_us = (s.delay_us + DELAY_STEP_US).min(MAX_DELAY_US);
            s.identity.set_property("sleep-time", s.delay_us as u64);
            eprintln!(
                "[multi-server] Stream {} delay → {} ms",
                *selected,
                s.delay_us / 1000
            );
        }
        '-' => {
            let mut st = streams.lock().unwrap();
            let s = &mut st[*selected];
            s.delay_us = (s.delay_us - DELAY_STEP_US).max(0);
            s.identity.set_property("sleep-time", s.delay_us as u64);
            eprintln!(
                "[multi-server] Stream {} delay → {} ms",
                *selected,
                s.delay_us / 1000
            );
        }
        'R' | 'r' => {
            let mut st = streams.lock().unwrap();
            for (i, s) in st.iter_mut().enumerate() {
                s.delay_us = 0;
                s.identity.set_property("sleep-time", 0u64);
                eprintln!("[multi-server] Stream {} delay reset", i);
            }
        }
        'S' | 's' => {
            let st = streams.lock().unwrap();
            eprintln!("[multi-server] Delay table:");
            for (i, s) in st.iter().enumerate() {
                eprintln!(
                    "  stream {} (port {}): {} ms",
                    i,
                    BASE_PORT + i as u32,
                    s.delay_us / 1000
                );
            }
        }
        'Q' | 'q' | '\x03' => {
            eprintln!("[multi-server] Quit");
            std::process::exit(0);
        }
        'H' | 'h' | '?' => print_help(),
        _ => {}
    }
}

fn print_help() {
    eprintln!(
        "\n[multi-server] Keys:\n\
         1–4    select stream\n\
         + / =  increase delay on selected stream (10 ms)\n\
         -      decrease delay on selected stream (10 ms)\n\
         R      reset all delays\n\
         S      show delay table\n\
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
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK) };
    if fd < 0 {
        return None;
    }
    let mut orig: libc::termios = unsafe { std::mem::zeroed() };
    unsafe { libc::tcgetattr(fd, &mut orig) };
    let mut raw = orig;
    unsafe { libc::cfmakeraw(&mut raw) };
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) };
    Some(Tty { fd, orig })
}
