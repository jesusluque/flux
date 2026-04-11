//! FLUX PoC002 — Mosaic Client
//!
//! Receives 4 FLUX streams (ports 7400–7403) and assembles them into a 2×2
//! mosaic using `compositor`, with `fluxsync` enforcing the MSS sync barrier
//! (spec §6.3) on each stream before decoding.
//!
//! Per-stream receive pipeline (stream N):
//!   fluxsrc(port=740N, sim-delay-ms=0)
//!     → fluxdemux
//!         media_0 → fluxcdbc
//!                     → sync_queue
//!                     → fluxsync(group=1, stream=N, nstreams=4, latency=500)
//!                     → fluxdeframer
//!                     → h265parse
//!                     → vtdec_hw
//!                     → videoconvertscale
//!                     → capsfilter(NV12 640×360)
//!                     → compositor.sink_N
//!         cdbc → fakesink
//!
//! Compositor sink pad positions:
//!   sink_0: top-left     (  0,   0) 640×360
//!   sink_1: top-right    (640,   0) 640×360
//!   sink_2: bottom-left  (  0, 360) 640×360
//!   sink_3: bottom-right (640, 360) 640×360
//!
//! Output: compositor → capsfilter(NV12 1280×720 30fps) → queue → videoconvertscale → osxvideosink
//!
//! ── Keyboard controls ─────────────────────────────────────────────────────────
//!
//!   Space     — pause / resume
//!   1–4       — select stream for delay adjustment
//!   + / =     — increase sim-delay-ms on selected stream by 10 ms
//!   -         — decrease sim-delay-ms on selected stream by 10 ms (min 0)
//!   R / r     — reset all delays to 0
//!   S / s     — print live sync stats + delay table
//!   H / ?     — help (shown on startup)
//!   Q / q     — quit

use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

const N: usize = 4;
const BASE_PORT: u32 = 7400;
const GROUP_ID: u32 = 1;

const TILE_W: i32 = 640;
const TILE_H: i32 = 360;

// fluxsync alignment window (ms).
// Must be >= max useful sim-delay-ms.  We support up to 450 ms of artificial
// delay; 500 ms gives a 50 ms margin.
const FLUXSYNC_LATENCY_MS: u64 = 500;

// Total downstream latency budget added to every output PTS by fluxdeframer.
// Covers: fluxsync alignment window + vtdec_hw decode time + margin.
// Must match TOTAL_LATENCY_NS in gst-fluxdeframer/src/lib.rs AND
// min-upstream-latency on the compositor below.
const COMPOSITOR_LATENCY_NS: u64 = 900_000_000; // 900 ms

// Delay adjustment step (ms).
const DELAY_STEP_MS: u32 = 10;
// Maximum sim-delay-ms we allow (ms).
const MAX_DELAY_MS: u32 = 450;

fn main() {
    env_logger::init();

    if std::env::var("GST_DEBUG").is_err() {
        std::env::set_var("GST_DEBUG", "2");
    }

    gst::init().expect("GStreamer init failed");

    let tty = open_tty_raw();
    if tty.is_none() {
        log!("[mosaic-client] WARNING: could not open /dev/tty — keyboard disabled");
    }

    gst::macos_main(move || run(tty));
}

fn run(tty: Option<Tty>) {
    gstfluxsrc::plugin_register_static().expect("fluxsrc register");
    gstfluxdemux::plugin_register_static().expect("fluxdemux register");
    gstfluxdeframer::plugin_register_static().expect("fluxdeframer register");
    gstfluxcdbc::plugin_register_static().expect("fluxcdbc register");
    gstfluxsync::plugin_register_static().expect("fluxsync register");

    let pipeline = gst::Pipeline::new();

    // ── Output chain ──────────────────────────────────────────────────────────

    let compositor = gst::ElementFactory::make("compositor")
        .property_from_str("background", "black")
        // min-upstream-latency: must be >= fluxdeframer TOTAL_LATENCY_NS.
        // The compositor waits this long for frames before emitting output.
        .property("min-upstream-latency", COMPOSITOR_LATENCY_NS)
        // force-live: prevents Paused→Playing deadlock with live sources.
        .property("force-live", true)
        .build()
        .expect("compositor element");

    let mosaic_w = TILE_W * 2; // 1280
    let mosaic_h = TILE_H * 2; // 720
    let comp_caps = gst::Caps::builder("video/x-raw")
        .field("format", "NV12")
        .field("width", mosaic_w)
        .field("height", mosaic_h)
        .field("framerate", gst::Fraction::new(30, 1))
        .build();
    let comp_capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &comp_caps)
        .build()
        .expect("compositor capsfilter");

    // Queue decouples compositor's output thread from osxvideosink so the
    // compositor keeps aggregating even when the sink is blocked on the clock.
    let out_queue = gst::ElementFactory::make("queue")
        .property("max-size-buffers", 4u32)
        .property("max-size-bytes", 0u32)
        .property("max-size-time", 0u64)
        .build()
        .expect("output queue");

    let out_convert = gst::ElementFactory::make("videoconvertscale")
        .build()
        .expect("output videoconvertscale");

    let osxvideosink = gst::ElementFactory::make("osxvideosink")
        .name("osxvideosink")
        .property("sync", false)
        .property("async", false)
        .build()
        .expect("osxvideosink");

    pipeline
        .add_many([
            &compositor,
            &comp_capsfilter,
            &out_queue,
            &out_convert,
            &osxvideosink,
        ])
        .expect("add compositor/output elements");

    compositor
        .link(&comp_capsfilter)
        .expect("compositor → comp_capsfilter");
    comp_capsfilter
        .link(&out_queue)
        .expect("comp_capsfilter → out_queue");
    out_queue
        .link(&out_convert)
        .expect("out_queue → out_convert");
    out_convert
        .link(&osxvideosink)
        .expect("out_convert → osxvideosink");

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

    let probe_counts: Vec<[Arc<AtomicU64>; 4]> = (0..N)
        .map(|_| {
            [
                Arc::new(AtomicU64::new(0)),
                Arc::new(AtomicU64::new(0)),
                Arc::new(AtomicU64::new(0)),
                Arc::new(AtomicU64::new(0)),
            ]
        })
        .collect();

    let mut fluxsrc_elems: Vec<gst::Element> = Vec::with_capacity(N);
    let mut fluxsync_elems: Vec<gst::Element> = Vec::with_capacity(N);

    // Per-stream sim-delay-ms state (shared with keyboard handler).
    let delays: Arc<Mutex<[u32; N]>> = Arc::new(Mutex::new([0u32; N]));

    for i in 0..N {
        let fluxsrc = gst::ElementFactory::make("fluxsrc")
            .property("address", "127.0.0.1")
            .property("port", BASE_PORT + i as u32)
            .property("sim-delay-ms", 0u32)
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

        // Queue between fluxcdbc and fluxsync.
        //
        // Decouples the fluxsrc streaming thread from fluxsync's blocking
        // condvar wait.  leaky=downstream (drop-oldest) ensures that when one
        // stream's sim-delay-ms exceeds the fluxsync latency window the queue
        // absorbs the throughput deficit rather than back-pressuring into the
        // QUIC receive path.
        //
        // max-size-buffers=600 (~20 s at 30 fps) gives headroom for any
        // practically useful delay value (≤450 ms << 20 s).
        let sync_queue = gst::ElementFactory::make("queue")
            .property("max-size-buffers", 600u32)
            .property("max-size-bytes", 0u32)
            .property("max-size-time", 0u64)
            .property_from_str("leaky", "downstream")
            .name(format!("sync_queue_{}", i).as_str())
            .build()
            .expect("sync queue");

        let fluxsync = gst::ElementFactory::make("fluxsync")
            .property("group", GROUP_ID)
            .property("stream", i as u32)
            .property("nstreams", N as u32)
            .property("latency", FLUXSYNC_LATENCY_MS)
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
            .property("qos", false)
            .build()
            .expect("vtdec_hw element");

        let convert = gst::ElementFactory::make("videoconvertscale")
            .name(format!("convert_{}", i).as_str())
            .build()
            .expect("videoconvertscale element");

        let tile_caps = gst::Caps::builder("video/x-raw")
            .field("format", "NV12")
            .field("width", TILE_W)
            .field("height", TILE_H)
            .build();
        let tile_capsfilter = gst::ElementFactory::make("capsfilter")
            .property("caps", &tile_caps)
            .name(format!("tile_caps_{}", i).as_str())
            .build()
            .expect("tile capsfilter");

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
                &sync_queue,
                &fluxsync,
                &fluxdeframer,
                &h265parse,
                &vtdec,
                &convert,
                &tile_capsfilter,
                &fakesink,
            ])
            .expect("add stream elements");

        fluxsrc_elem.link(&fluxdemux).expect("fluxsrc → fluxdemux");
        fluxcdbc_elem
            .link(&sync_queue)
            .expect("fluxcdbc → sync_queue");
        sync_queue.link(&fluxsync).expect("sync_queue → fluxsync");
        fluxsync
            .link(&fluxdeframer)
            .expect("fluxsync → fluxdeframer");
        fluxdeframer
            .link(&h265parse)
            .expect("fluxdeframer → h265parse");
        h265parse.link(&vtdec).expect("h265parse → vtdec");
        vtdec.link(&convert).expect("vtdec → convert");
        convert
            .link(&tile_capsfilter)
            .expect("convert → tile_capsfilter");

        let tile_src = tile_capsfilter.static_pad("src").unwrap();
        tile_src
            .link(&comp_sink_pads[i])
            .expect("tile_capsfilter → compositor");

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

        // ── Pad probes ────────────────────────────────────────────────────────
        {
            let cnt = probe_counts[i][0].clone();
            if let Some(pad) = fluxsrc_elem.static_pad("src") {
                pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    cnt.fetch_add(1, Ordering::Relaxed);
                    gst::PadProbeReturn::Ok
                });
            }
        }
        {
            let cnt = probe_counts[i][1].clone();
            if let Some(pad) = fluxdeframer.static_pad("src") {
                pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    cnt.fetch_add(1, Ordering::Relaxed);
                    gst::PadProbeReturn::Ok
                });
            }
        }
        {
            let cnt = probe_counts[i][2].clone();
            if let Some(pad) = vtdec.static_pad("src") {
                pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    cnt.fetch_add(1, Ordering::Relaxed);
                    gst::PadProbeReturn::Ok
                });
            }
        }
        {
            let cnt = probe_counts[i][3].clone();
            if let Some(pad) = tile_capsfilter.static_pad("src") {
                pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    cnt.fetch_add(1, Ordering::Relaxed);
                    gst::PadProbeReturn::Ok
                });
            }
        }

        fluxsrc_elems.push(fluxsrc_elem);
        fluxsync_elems.push(fluxsync);
    }

    // ── Start ─────────────────────────────────────────────────────────────────

    let sc = pipeline
        .set_state(gst::State::Playing)
        .expect("Unable to start pipeline");
    log!(
        "[mosaic-client] Started — {} streams, group_id={} (set_state={:?})",
        N,
        GROUP_ID,
        sc
    );
    print_help();

    // ── GLib main loop ────────────────────────────────────────────────────────

    let main_loop = glib::MainLoop::new(None, false);
    let paused = Arc::new(AtomicBool::new(false));

    let ml = main_loop.clone();
    let pipeline_for_bus = pipeline.clone();
    let bus = pipeline.bus().unwrap();
    let _bus_watch = bus
        .add_watch(move |_, msg| {
            use gst::MessageView;
            match msg.view() {
                MessageView::Eos(..) => {
                    log!("[mosaic-client] EOS");
                    ml.quit();
                    return glib::ControlFlow::Break;
                }
                MessageView::Error(err) => {
                    log!("[mosaic-client] ERROR: {} ({:?})", err.error(), err.debug());
                    ml.quit();
                    return glib::ControlFlow::Break;
                }
                MessageView::StateChanged(sc) => {
                    if msg
                        .src()
                        .map(|s| s == pipeline_for_bus.upcast_ref::<gst::Object>())
                        .unwrap_or(false)
                    {
                        log!("[mosaic-client] State: {:?} → {:?}", sc.old(), sc.current());
                    }
                }
                _ => {}
            }
            glib::ControlFlow::Continue
        })
        .unwrap();

    // ── 3-second diagnostic timer ─────────────────────────────────────────────
    {
        let pc = probe_counts.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(3), move || {
            for i in 0..N {
                let fs = pc[i][0].load(Ordering::Relaxed);
                let df = pc[i][1].load(Ordering::Relaxed);
                let vt = pc[i][2].load(Ordering::Relaxed);
                let tc = pc[i][3].load(Ordering::Relaxed);
                log!(
                    "[probe] stream {}: fluxsrc={:5}  deframer={:5}  vtdec={:5}  tile={:5}",
                    i,
                    fs,
                    df,
                    vt,
                    tc
                );
            }
            glib::ControlFlow::Continue
        });
    }

    // ── Keyboard ──────────────────────────────────────────────────────────────
    if let Some(tty) = tty {
        let (tx, rx) = std::sync::mpsc::channel::<char>();
        std::thread::spawn(move || {
            log!("[mosaic-client] keyboard thread started (fd={})", tty.fd);
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

        let pipeline_weak = pipeline.downgrade();
        let ml = main_loop.clone();
        let fluxsrc_weak: Vec<_> = fluxsrc_elems.iter().map(|e| e.downgrade()).collect();
        let fluxsync_weak: Vec<_> = fluxsync_elems.iter().map(|e| e.downgrade()).collect();
        let paused_flag = paused.clone();
        let delays_key = delays.clone();
        let mut selected: usize = 0;

        glib::timeout_add_local(std::time::Duration::from_millis(20), move || {
            while let Ok(ch) = rx.try_recv() {
                match ch {
                    ' ' => {
                        if let Some(pl) = pipeline_weak.upgrade() {
                            if paused_flag.load(Ordering::Relaxed) {
                                pl.set_state(gst::State::Playing).ok();
                                paused_flag.store(false, Ordering::Relaxed);
                                log!("[mosaic-client] Resumed");
                            } else {
                                pl.set_state(gst::State::Paused).ok();
                                paused_flag.store(true, Ordering::Relaxed);
                                log!("[mosaic-client] Paused");
                            }
                        }
                    }
                    'Q' | 'q' | '\x03' => {
                        if let Some(pl) = pipeline_weak.upgrade() {
                            pl.send_event(gst::event::Eos::new());
                        }
                        ml.quit();
                        return glib::ControlFlow::Break;
                    }
                    '1'..='4' => {
                        selected = ch as usize - '1' as usize;
                        log!("[mosaic-client] Selected stream {}", selected);
                    }
                    '+' | '=' => {
                        let new_delay = {
                            let mut d = delays_key.lock().unwrap();
                            d[selected] = (d[selected] + DELAY_STEP_MS).min(MAX_DELAY_MS);
                            d[selected]
                        };
                        if let Some(src) = fluxsrc_weak[selected].upgrade() {
                            src.set_property("sim-delay-ms", new_delay);
                        }
                        log!(
                            "[mosaic-client] Stream {} sim-delay → {} ms",
                            selected,
                            new_delay
                        );
                    }
                    '-' => {
                        let new_delay = {
                            let mut d = delays_key.lock().unwrap();
                            d[selected] = d[selected].saturating_sub(DELAY_STEP_MS);
                            d[selected]
                        };
                        if let Some(src) = fluxsrc_weak[selected].upgrade() {
                            src.set_property("sim-delay-ms", new_delay);
                        }
                        log!(
                            "[mosaic-client] Stream {} sim-delay → {} ms",
                            selected,
                            new_delay
                        );
                    }
                    'R' | 'r' => {
                        let mut d = delays_key.lock().unwrap();
                        for i in 0..N {
                            d[i] = 0;
                            if let Some(src) = fluxsrc_weak[i].upgrade() {
                                src.set_property("sim-delay-ms", 0u32);
                            }
                        }
                        log!("[mosaic-client] All delays reset to 0");
                    }
                    'S' | 's' => {
                        let d = delays_key.lock().unwrap();
                        log!("[mosaic-client] Delay table:");
                        for i in 0..N {
                            log!(
                                "  stream {} (port {}): {} ms",
                                i,
                                BASE_PORT + i as u32,
                                d[i]
                            );
                        }
                        drop(d);
                        for (i, sw) in fluxsync_weak.iter().enumerate() {
                            if let Some(s) = sw.upgrade() {
                                let synced: u64 = s.property("frames-synced");
                                let dropped: u64 = s.property("frames-dropped");
                                let skew: u64 = s.property("max-skew-ns");
                                log!(
                                    "[mosaic-client] fluxsync {}: synced={} dropped={} max_skew={}µs",
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
            }
            glib::ControlFlow::Continue
        });
    }

    main_loop.run();

    pipeline.set_state(gst::State::Null).unwrap();
    log!("[mosaic-client] Stopped");
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

fn print_help() {
    log!(
        "\n[mosaic-client] Keys:\n\
         Space  pause / resume\n\
         1–4    select stream for delay adjustment\n\
         + / =  increase sim-delay on selected stream (10 ms)\n\
         -      decrease sim-delay on selected stream (10 ms)\n\
         R      reset all delays to 0\n\
         S      show delay table + sync stats\n\
         H / ?  show this help\n\
         Q      quit\n"
    );
}
