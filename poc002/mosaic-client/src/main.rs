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
//! Output: compositor → videoconvertscale → osxvideosink
//!
//! ── Keyboard controls ─────────────────────────────────────────────────────────
//!
//!   Space   — pause / resume
//!   Q / q   — quit
//!   S / s   — print live sync stats per stream
//!   H / ?   — help (also shown on startup)

use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// ── Serialized stderr logger ──────────────────────────────────────────────────
//
// log! expands to multiple write() calls and interleaves across threads.
// This macro serializes each line into a single write_all() under a global mutex.

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
    if tty.is_none() {
        log!("[mosaic-client] WARNING: could not open /dev/tty — keyboard disabled");
    }

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

    // min-upstream-latency must match TOTAL_LATENCY_NS in fluxdeframer (400 ms).
    // The compositor uses this as its aggregation window: it waits up to this
    // duration for frames before emitting output.  Setting it equal to the PTS
    // headroom means the compositor fires exactly when the first frame's PTS is
    // due.  Must be >= fluxdeframer TOTAL_LATENCY_NS.
    let compositor = gst::ElementFactory::make("compositor")
        .property_from_str("background", "black")
        .property("min-upstream-latency", 400_000_000u64) // 400 ms — must match fluxdeframer TOTAL_LATENCY_NS
        // force-live: always operate in live mode and aggregate on timeout.
        // Without this the aggregator blocks the Paused→Playing transition
        // waiting for pre-roll buffers that can't arrive until the pipeline
        // is already Playing (live-source deadlock with dynamic demux pads).
        .property("force-live", true)
        .build()
        .expect("compositor element");

    // Lock compositor output caps to the exact mosaic size (4 × 640×360 = 1280×720)
    // and a concrete format.  Without this the compositor negotiates prematurely on
    // the first stream-start event (before any input caps arrive) and fixates to
    // 1×1 @ 25 fps, after which renegotiation with the real 640×360 NV12 frames
    // stalls because osxvideosink already accepted 1×1 caps.
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

    // Queue between compositor and the rest of the output chain.
    // Without this, osxvideosink (or any downstream element) blocking on
    // clock/sync back-pressures directly into compositor's aggregator output
    // thread.  When that thread is blocked, compositor stops scheduling
    // aggregation timeouts and ceases to pull from its input pads — freezing
    // tile_caps_src at the first couple of buffers.  The queue decouples
    // compositor's output thread so it can always keep aggregating.
    let out_queue = gst::ElementFactory::make("queue")
        .property("max-size-buffers", 4u32)
        .property("max-size-bytes", 0u32)
        .property("max-size-time", 0u64)
        .build()
        .expect("output queue");

    let out_convert = gst::ElementFactory::make("videoconvertscale")
        .build()
        .expect("output videoconvertscale");

    // sync=false: render frames as fast as they arrive without clock-based
    // synchronisation.  For a live multi-stream mosaic we never want the
    // sink to drop or delay frames due to PTS mismatch; the queue above
    // already decouples us from timing back-pressure.
    // async=false: do not block Paused→Playing waiting for a pre-roll buffer
    // (live-source pipeline deadlock).
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

    // Probe counters: [stream][probe_point]
    // probe_points: 0=fluxsrc_src, 1=fluxdeframer_src, 2=vtdec_src, 3=tile_caps_src
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

        // Queue between fluxcdbc and fluxsync to decouple the GStreamer
        // streaming thread (which also runs fluxsrc::create()) from the
        // blocking condvar wait inside fluxsync::transform_ip().
        //
        // Without this queue, fluxsync blocks the streaming thread while
        // waiting for all 4 streams to deposit into the same slot.  Because
        // fluxsrc::create() runs on that same thread, the raw_tx channel
        // backs up (all 4 QUIC recv tasks keep pushing at 30 fps) and frames
        // are dropped with "raw_tx full or closed".
        //
        // With the queue, the streaming thread (fluxsrc::create) runs freely
        // up to fluxcdbc, deposits into the queue, and returns immediately.
        // The queue spawns its own thread which calls chain() on fluxsync —
        // that thread may block on the condvar, but it never blocks create().
        //
        // leaky=downstream (2): when the queue fills, drop the OLDEST buffer
        // rather than blocking fluxsrc's streaming thread.  This is the
        // correct behaviour for a live stream under artificial delay: we prefer
        // to drop old frames (already stale) rather than back-pressuring into
        // the QUIC receive path and eventually corrupting the H.265 bitstream
        // with "raw_tx full or closed" drops.
        //
        // max-size-buffers=600 (~20 s at 30 fps) gives enough headroom for
        // delays up to ~500 ms before any dropping occurs, while still
        // allowing fluxsrc to run freely.
        let sync_queue = gst::ElementFactory::make("queue")
            .property("max-size-buffers", 600u32) // 20 s at 30 fps
            .property("max-size-bytes", 0u32)
            .property("max-size-time", 0u64)
            .property_from_str("leaky", "downstream") // drop-oldest on overflow
            .name(format!("sync_queue_{}", i).as_str())
            .build()
            .expect("sync queue");

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
            // Disable QoS: vtdec_hw honors GstQosEvents from osxvideosink and
            // drops frames whose PTS is in the past.  At startup the pipeline
            // clock has already advanced past the PTS of the first few frames
            // (vtdec's own decode latency can exceed our PTS headroom), so
            // vtdec would otherwise drop every frame until the PTS catches up.
            // For a live mosaic we never want decoder-level frame drops.
            .property("qos", false)
            .build()
            .expect("vtdec_hw element");

        let convert = gst::ElementFactory::make("videoconvertscale")
            .name(format!("convert_{}", i).as_str())
            .build()
            .expect("videoconvertscale element");

        // Lock each tile to the expected size and format so the compositor sees
        // consistent caps.  vtdec_hw reports framerate=0/1 (VFR); omit the
        // framerate field here — videoconvertscale cannot do framerate
        // conversion, and the compositor's own output capsfilter already pins
        // the final output to 30 fps.
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

        // Static links.
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
        // Let h265parse and vtdec_hw negotiate caps freely.
        // h265parse receives byte-stream from fluxdeframer and will output in
        // whatever format vtdec_hw's sink pad template accepts.  Using
        // link_filtered here caused "Failed to link" errors because vtdec_hw's
        // pad template includes a memory:SystemMemory qualifier that conflicts
        // with a plain byte-stream caps filter.
        h265parse.link(&vtdec).expect("h265parse → vtdec");
        vtdec.link(&convert).expect("vtdec → convert");
        convert
            .link(&tile_capsfilter)
            .expect("convert → tile_capsfilter");

        // tile_capsfilter src → compositor.sink_i
        let tile_src = tile_capsfilter.static_pad("src").unwrap();
        tile_src
            .link(&comp_sink_pads[i])
            .expect("tile_capsfilter → compositor");

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

        // ── Pad probes for black-screen diagnostics ───────────────────────────
        // Count buffers passing each key src pad; printed every 3 s.

        // 0: fluxsrc src pad (first src pad — "src_0" from fluxsrc, or "src")
        {
            let cnt = probe_counts[i][0].clone();
            // fluxsrc exposes a static "src" pad
            if let Some(pad) = fluxsrc_elem.static_pad("src") {
                pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    cnt.fetch_add(1, Ordering::Relaxed);
                    gst::PadProbeReturn::Ok
                });
            } else {
                log!(
                    "[mosaic-client] stream {}: fluxsrc has no static 'src' pad (dynamic)",
                    i
                );
            }
        }
        // 1: fluxdeframer src pad
        {
            let cnt = probe_counts[i][1].clone();
            if let Some(pad) = fluxdeframer.static_pad("src") {
                pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    cnt.fetch_add(1, Ordering::Relaxed);
                    gst::PadProbeReturn::Ok
                });
            }
        }
        // 2: vtdec src pad
        {
            let cnt = probe_counts[i][2].clone();
            if let Some(pad) = vtdec.static_pad("src") {
                pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    cnt.fetch_add(1, Ordering::Relaxed);
                    gst::PadProbeReturn::Ok
                });
            }
        }
        // 3: tile_capsfilter src pad
        {
            let cnt = probe_counts[i][3].clone();
            if let Some(pad) = tile_capsfilter.static_pad("src") {
                pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    cnt.fetch_add(1, Ordering::Relaxed);
                    gst::PadProbeReturn::Ok
                });
            }
        }

        fluxsync_elems.push(fluxsync);
    }

    // ── Start ─────────────────────────────────────────────────────────────────

    let sc = pipeline
        .set_state(gst::State::Playing)
        .expect("Unable to start pipeline");
    log!(
        "[mosaic-client] Started — receiving {} streams, group_id={} (set_state={:?})",
        N,
        GROUP_ID,
        sc
    );
    print_help();

    // ── GLib main loop ────────────────────────────────────────────────────────

    let main_loop = glib::MainLoop::new(None, false);
    let paused = Arc::new(AtomicBool::new(false));

    // Bus watcher — must be kept alive until main loop exits.
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

    // ── 3-second diagnostic timer — buffer counts per probe point ────────────
    {
        let pc = probe_counts.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(3), move || {
            for i in 0..N {
                let fs = pc[i][0].load(Ordering::Relaxed);
                let df = pc[i][1].load(Ordering::Relaxed);
                let vt = pc[i][2].load(Ordering::Relaxed);
                let tc = pc[i][3].load(Ordering::Relaxed);
                log!(
                    "[probe] stream {}: fluxsrc_src={:5}  deframer_src={:5}  vtdec_src={:5}  tile_caps_src={:5}",
                    i, fs, df, vt, tc
                );
            }
            glib::ControlFlow::Continue
        });
    }

    // Keyboard input: background thread does blocking reads, main loop drains via idle_add_local.
    if let Some(tty) = tty {
        let (tx, rx) = std::sync::mpsc::channel::<char>();
        std::thread::spawn(move || {
            log!("[mosaic-client] keyboard thread started (fd={})", tty.fd);
            let fd = tty.fd;
            loop {
                let mut buf = [0u8; 1];
                let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 1) };
                if n <= 0 {
                    let err = unsafe { *libc::__error() };
                    log!(
                        "[mosaic-client] keyboard thread: read returned {} errno={}",
                        n,
                        err
                    );
                    break;
                }
                log!(
                    "[mosaic-client] key read: 0x{:02x} '{}'",
                    buf[0],
                    buf[0] as char
                );
                if tx.send(buf[0] as char).is_err() {
                    break;
                }
            }
            drop(tty);
        });

        let pipeline_weak = pipeline.downgrade();
        let ml = main_loop.clone();
        let fluxsync_weak: Vec<_> = fluxsync_elems.iter().map(|e| e.downgrade()).collect();
        let paused_flag = paused.clone();

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
                    'D' | 'd' => {
                        log!("[mosaic-client] FPS overlay not available (fpsdisplaysink removed)");
                    }
                    'S' | 's' => {
                        for (i, sw) in fluxsync_weak.iter().enumerate() {
                            if let Some(s) = sw.upgrade() {
                                let synced: u64 = s.property("frames-synced");
                                let dropped: u64 = s.property("frames-dropped");
                                let skew: u64 = s.property("max-skew-ns");
                                log!(
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

fn print_help() {
    log!(
        "\n[mosaic-client] Keys:\n\
         Space  pause / resume\n\
         S      print sync stats per stream\n\
         H / ?  show this help\n\
         Q      quit\n"
    );
}
