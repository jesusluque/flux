//! FLUX PoC002 — Mosaic Client
//!
//! Receives 4 FLUX streams (ports 7400–7403) and assembles them into a 2×2
//! mosaic using `compositor`, with `fluxsync` enforcing the MSS sync barrier
//! (spec §6.3) on each stream before decoding.
//!
//! Per-stream receive pipeline (stream N):
//!   fluxsrc(port=740N)
//!     → fluxdemux
//!         media_0 → fluxcdbc (passthrough, sends CDBC_FEEDBACK)
//!                     → fluxsync(group=1, stream=N, nstreams=4)
//!                     → fluxdeframer
//!                     → h265parse
//!                     → vtdec_hw
//!                     → videoconvertscale
//!                     → compositor.sink_N
//!         cdbc → fakesink
//!
//! Compositor sink pad positions:
//!   sink_0: top-left     (  0,   0) 640×360
//!   sink_1: top-right    (640,   0) 640×360
//!   sink_2: bottom-left  (  0, 360) 640×360
//!   sink_3: bottom-right (640, 360) 640×360
//!
//! Output: compositor → videoconvertscale → fpsdisplaysink(osxvideosink)
//!
//! ── Keyboard controls ─────────────────────────────────────────────────────────
//!
//!   Space   — pause / resume
//!   Q / q   — quit
//!   D / d   — toggle fps overlay
//!   S / s   — print live sync stats per stream
//!   H / ?   — help

use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const N: usize = 4;
const BASE_PORT: u32 = 7400;
const GROUP_ID: u32 = 1;

// Mosaic tile size.
const TILE_W: i32 = 640;
const TILE_H: i32 = 360;

fn main() {
    env_logger::init();

    if std::env::var("GST_DEBUG").is_err() {
        std::env::set_var("GST_DEBUG", "2");
    }

    gst::init().expect("GStreamer init failed");

    let tty = open_tty_raw();

    gst::macos_main(move || run(tty));
}

fn run(tty: Option<Tty>) {
    // Register all needed plugins.
    gstfluxsrc::plugin_register_static().expect("fluxsrc register");
    gstfluxdemux::plugin_register_static().expect("fluxdemux register");
    gstfluxdeframer::plugin_register_static().expect("fluxdeframer register");
    gstfluxcdbc::plugin_register_static().expect("fluxcdbc register");
    gstfluxsync::plugin_register_static().expect("fluxsync register");

    // ── Build pipeline ────────────────────────────────────────────────────────

    let pipeline = gst::Pipeline::new();

    // ── Output chain: compositor → convert → fpsdisplaysink ──────────────────

    let compositor = gst::ElementFactory::make("compositor")
        .property("background", 1u32) // black background
        .build()
        .expect("compositor element");

    let out_convert = gst::ElementFactory::make("videoconvertscale")
        .build()
        .expect("output videoconvertscale");

    let osxvideosink = gst::ElementFactory::make("osxvideosink")
        .property("sync", false)
        .property("async", false)
        .build()
        .expect("osxvideosink");

    let fpssink = gst::ElementFactory::make("fpsdisplaysink")
        .property("video-sink", &osxvideosink)
        .property("sync", false)
        .property("text-overlay", true)
        .build()
        .expect("fpsdisplaysink");

    pipeline
        .add_many([&compositor, &out_convert, &fpssink])
        .expect("add compositor/output elements");

    compositor
        .link(&out_convert)
        .expect("compositor → out_convert");
    out_convert.link(&fpssink).expect("out_convert → fpssink");

    // Request compositor sink pads (one per stream) with position/size.
    let mut comp_sink_pads: Vec<gst::Pad> = Vec::with_capacity(N);
    for i in 0..N {
        let col = (i % 2) as i32;
        let row = (i / 2) as i32;
        let pad = compositor
            .request_pad_simple("sink_%u")
            .expect("compositor sink pad");
        pad.set_property("xpos", col * TILE_W);
        pad.set_property("ypos", row * TILE_H);
        pad.set_property("width", TILE_W);
        pad.set_property("height", TILE_H);
        comp_sink_pads.push(pad);
    }

    // ── Per-stream receive elements ───────────────────────────────────────────

    let mut fluxsync_elems: Vec<gst::Element> = Vec::with_capacity(N);

    for i in 0..N {
        // fluxsrc
        let fluxsrc = gst::ElementFactory::make("fluxsrc")
            .property("address", "127.0.0.1")
            .property("port", BASE_PORT + i as u32)
            .name(format!("fluxsrc_{}", i).as_str())
            .build()
            .expect("fluxsrc element")
            .downcast::<gstfluxsrc::FluxSrc>()
            .expect("fluxsrc downcast");
        let fluxsrc_elem = fluxsrc.upcast::<gst::Element>();

        let fluxdemux = gst::ElementFactory::make("fluxdemux")
            .name(format!("fluxdemux_{}", i).as_str())
            .build()
            .expect("fluxdemux element");

        let fluxcdbc = gst::ElementFactory::make("fluxcdbc")
            .property("cdbc-interval", 50u64)
            .property("cdbc-min-interval", 10u64)
            .name(format!("fluxcdbc_{}", i).as_str())
            .build()
            .expect("fluxcdbc element")
            .downcast::<gstfluxcdbc::FluxCdbc>()
            .expect("fluxcdbc downcast");
        let fluxcdbc_elem = fluxcdbc.upcast::<gst::Element>();

        let fluxsync = gst::ElementFactory::make("fluxsync")
            .property("group", GROUP_ID)
            .property("stream", i as u32)
            .property("nstreams", N as u32)
            .property("latency", 200u64)
            .name(format!("fluxsync_{}", i).as_str())
            .build()
            .expect("fluxsync element");

        let fluxdeframer = gst::ElementFactory::make("fluxdeframer")
            .name(format!("fluxdeframer_{}", i).as_str())
            .build()
            .expect("fluxdeframer element");

        let h265parse = gst::ElementFactory::make("h265parse")
            .name(format!("h265parse_{}", i).as_str())
            .build()
            .expect("h265parse element");

        let vtdec = gst::ElementFactory::make("vtdec_hw")
            .name(format!("vtdec_{}", i).as_str())
            .build()
            .expect("vtdec_hw element");

        let convert = gst::ElementFactory::make("videoconvertscale")
            .name(format!("convert_{}", i).as_str())
            .build()
            .expect("videoconvertscale element");

        let fakesink = gst::ElementFactory::make("fakesink")
            .property("sync", false)
            .name(format!("fakesink_{}", i).as_str())
            .build()
            .expect("fakesink element");

        pipeline
            .add_many([
                &fluxsrc_elem,
                &fluxdemux,
                &fluxcdbc_elem,
                &fluxsync,
                &fluxdeframer,
                &h265parse,
                &vtdec,
                &convert,
                &fakesink,
            ])
            .expect("add stream elements");

        // Static links.
        fluxsrc_elem.link(&fluxdemux).expect("fluxsrc → fluxdemux");
        fluxcdbc_elem.link(&fluxsync).expect("fluxcdbc → fluxsync");
        fluxsync
            .link(&fluxdeframer)
            .expect("fluxsync → fluxdeframer");
        fluxdeframer
            .link(&h265parse)
            .expect("fluxdeframer → h265parse");
        let hvc1_caps = gst::Caps::builder("video/x-h265")
            .field("stream-format", "hvc1")
            .field("alignment", "au")
            .build();
        h265parse
            .link_filtered(&vtdec, &hvc1_caps)
            .expect("h265parse → vtdec (hvc1)");
        vtdec.link(&convert).expect("vtdec → convert");

        // convert src → compositor.sink_i
        let convert_src = convert.static_pad("src").unwrap();
        convert_src
            .link(&comp_sink_pads[i])
            .expect("convert → compositor");

        // Dynamic demux pad routing.
        let cdbc_clone = fluxcdbc_elem.clone();
        let fs_clone = fakesink.clone();
        fluxdemux.connect_pad_added(move |_, pad| match pad.name().as_str() {
            "media_0" => {
                let sink = cdbc_clone.static_pad("sink").expect("fluxcdbc sink");
                if !sink.is_linked() {
                    pad.link(&sink).expect("media_0 → fluxcdbc");
                }
            }
            "cdbc" => {
                let sink = fs_clone.static_pad("sink").expect("fakesink sink");
                if !sink.is_linked() {
                    pad.link(&sink).expect("cdbc → fakesink");
                }
            }
            _ => {}
        });

        fluxsync_elems.push(fluxsync);
    }

    // ── Start ─────────────────────────────────────────────────────────────────

    pipeline
        .set_state(gst::State::Playing)
        .expect("Unable to start pipeline");
    eprintln!(
        "[mosaic-client] Started — receiving {} streams, group_id={}",
        N, GROUP_ID
    );
    print_help();

    // ── GLib main loop ────────────────────────────────────────────────────────

    let main_loop = glib::MainLoop::new(None, false);
    let paused = Arc::new(AtomicBool::new(false));
    let fps_overlay = Arc::new(AtomicBool::new(true));

    // Bus watcher.
    {
        let ml = main_loop.clone();
        let pipeline_for_bus = pipeline.clone();
        let bus = pipeline.bus().unwrap();
        let _bus_watch = bus
            .add_watch(move |_, msg| {
                use gst::MessageView;
                match msg.view() {
                    MessageView::Eos(..) => {
                        eprintln!("[mosaic-client] EOS");
                        ml.quit();
                        return glib::ControlFlow::Break;
                    }
                    MessageView::Error(err) => {
                        eprintln!("[mosaic-client] ERROR: {} ({:?})", err.error(), err.debug());
                        ml.quit();
                        return glib::ControlFlow::Break;
                    }
                    MessageView::StateChanged(sc) => {
                        if msg
                            .src()
                            .map(|s| s == pipeline_for_bus.upcast_ref::<gst::Object>())
                            .unwrap_or(false)
                        {
                            eprintln!("[mosaic-client] State: {:?} → {:?}", sc.old(), sc.current());
                        }
                    }
                    _ => {}
                }
                glib::ControlFlow::Continue
            })
            .unwrap();

        // Keep _bus_watch alive for the duration of the main loop.
        let _ = _bus_watch;
    }

    // Keyboard input via raw /dev/tty.
    if let Some(tty) = tty {
        let fd = tty.fd;
        let pipeline_weak = pipeline.downgrade();
        let ml = main_loop.clone();
        let fluxsync_weak: Vec<_> = fluxsync_elems.iter().map(|e| e.downgrade()).collect();
        let paused_flag = paused.clone();
        let fps_flag = fps_overlay.clone();
        let fpssink_weak = fpssink.downgrade();

        glib::unix_fd_add_local(fd, glib::IOCondition::IN, move |_, _| {
            let mut buf = [0u8; 1];
            let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, 1) };
            if n <= 0 {
                return glib::ControlFlow::Continue;
            }
            match buf[0] as char {
                ' ' => {
                    if let Some(pl) = pipeline_weak.upgrade() {
                        if paused_flag.load(Ordering::Relaxed) {
                            pl.set_state(gst::State::Playing).ok();
                            paused_flag.store(false, Ordering::Relaxed);
                            eprintln!("[mosaic-client] Resumed");
                        } else {
                            pl.set_state(gst::State::Paused).ok();
                            paused_flag.store(true, Ordering::Relaxed);
                            eprintln!("[mosaic-client] Paused");
                        }
                    }
                }
                'Q' | 'q' | '\x03' => {
                    if let Some(pl) = pipeline_weak.upgrade() {
                        pl.send_event(gst::event::Eos::new());
                    }
                    ml.quit();
                }
                'D' | 'd' => {
                    let cur = fps_flag.load(Ordering::Relaxed);
                    fps_flag.store(!cur, Ordering::Relaxed);
                    if let Some(s) = fpssink_weak.upgrade() {
                        s.set_property("text-overlay", !cur);
                    }
                    eprintln!(
                        "[mosaic-client] FPS overlay {}",
                        if !cur { "on" } else { "off" }
                    );
                }
                'S' | 's' => {
                    for (i, sw) in fluxsync_weak.iter().enumerate() {
                        if let Some(s) = sw.upgrade() {
                            let synced: u64 = s.property("frames-synced");
                            let dropped: u64 = s.property("frames-dropped");
                            let skew: u64 = s.property("max-skew-ns");
                            eprintln!(
                                "[mosaic-client] stream {}: synced={} dropped={} max_skew={}µs",
                                i,
                                synced,
                                dropped,
                                skew / 1000
                            );
                        }
                    }
                }
                'H' | 'h' | '?' => print_help(),
                _ => {}
            }
            glib::ControlFlow::Continue
        });

        // Prevent Tty from being dropped until the main loop exits.
        let _tty = tty;
        main_loop.run();
    } else {
        main_loop.run();
    }

    pipeline.set_state(gst::State::Null).unwrap();
    eprintln!("[mosaic-client] Stopped");
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

fn print_help() {
    eprintln!(
        "\n[mosaic-client] Keys:\n\
         Space  pause / resume\n\
         D      toggle FPS overlay\n\
         S      print sync stats per stream\n\
         Q      quit\n"
    );
}
