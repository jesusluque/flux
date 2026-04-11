//! FLUX PoC002 — Multi-Server
//!
//! Runs 4 independent GStreamer encode-and-send pipelines in a single process,
//! each sourcing a spinning pinwheel pattern with wall-clock + running-time
//! overlays and transmitting on a dedicated QUIC port.
//!
//! Stream layout:
//!   Stream 0  pattern=pinwheel   port 7400
//!   Stream 1  pattern=pinwheel   port 7401
//!   Stream 2  pattern=pinwheel   port 7402
//!   Stream 3  pattern=pinwheel   port 7403
//!
//! Each pipeline:
//!   videotestsrc(pattern=pinwheel, is-live=true)
//!     → videoconvertscale
//!     → video/x-raw,width=640,height=360,framerate=30/1
//!     → clockoverlay(HH:MM:SS.cc, centred, large)
//!     → timeoverlay(running-time, bottom-left)
//!     → textoverlay("CAM N", top-left)
//!     → vtenc_h265(realtime=true)
//!     → h265parse(config-interval=-1)
//!     → fluxframer(channel-id=N, group-id=1)
//!     → fluxsink(port=740N)
//!
//! Keyboard controls: Q/q — quit, H/? — help

use gstreamer as gst;
use gstreamer::prelude::*;
use std::io::Write;
use std::sync::Mutex;

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
const LABELS: [&str; N] = ["CAM 1", "CAM 2", "CAM 3", "CAM 4"];
const BASE_PORT: u32 = 7400;
const GROUP_ID: u32 = 1;

fn main() {
    env_logger::init();

    gst::init().expect("GStreamer init failed");

    gstfluxframer::plugin_register_static().expect("fluxframer register");
    gstfluxsink::plugin_register_static().expect("fluxsink register");

    let tty = open_tty_raw();
    if tty.is_none() {
        eprintln!("[multi-server] WARNING: could not open /dev/tty — keyboard disabled");
    }

    gst::macos_main(move || run(tty));
}

fn run(tty: Option<Tty>) {
    let mut pipelines: Vec<gst::Pipeline> = Vec::with_capacity(N);

    for i in 0..N {
        pipelines.push(build_stream_pipeline(i));
    }

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
         Keys: Q quit, H help",
        N,
        GROUP_ID
    );

    // Ctrl-C handler.
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

    let main_loop = glib::MainLoop::new(None, false);

    if let Some(tty) = tty {
        let (tx, rx) = std::sync::mpsc::channel::<char>();
        std::thread::spawn(move || {
            let fd = tty.fd;
            log!("[multi-server] keyboard thread started (fd={})", fd);
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
        let pl_weak: Vec<_> = pipelines.iter().map(|p| p.downgrade()).collect();
        glib::timeout_add_local(std::time::Duration::from_millis(20), move || {
            while let Ok(ch) = rx.try_recv() {
                match ch {
                    'Q' | 'q' | '\x03' => {
                        for pw in &pl_weak {
                            if let Some(p) = pw.upgrade() {
                                p.send_event(gst::event::Eos::new());
                            }
                        }
                        ml.quit();
                        return glib::ControlFlow::Break;
                    }
                    'H' | 'h' | '?' => print_help(),
                    _ => {}
                }
            }
            glib::ControlFlow::Continue
        });
    }

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
//   videotestsrc(pattern=pinwheel, is-live=true)
//     → videoconvertscale
//     → video/x-raw I420 640×360 30fps
//     → clockoverlay  (wall-clock HH:MM:SS.cc, centred, large)
//     → timeoverlay   (running-time, bottom-left)
//     → textoverlay   ("CAM N", top-left)
//     → vtenc_h265(realtime=true, allow-frame-reordering=false)
//     → h265parse(config-interval=-1)
//     → fluxframer(channel-id=N, group-id=1)
//     → fluxsink(port=740N)

fn build_stream_pipeline(idx: usize) -> gst::Pipeline {
    let pipeline = gst::Pipeline::with_name(&format!("pipeline_{}", idx));

    let src = gst::ElementFactory::make("videotestsrc")
        .property_from_str("pattern", "pinwheel")
        .property("is-live", true)
        .name(&format!("src_{}", idx))
        .build()
        .expect("videotestsrc");

    let convert = gst::ElementFactory::make("videoconvertscale")
        .name(&format!("convert_{}", idx))
        .build()
        .expect("videoconvertscale");

    // Wall-clock timecode overlay — large, centred.
    // %H:%M:%S.%2N → HH:MM:SS.cc (hundredths of second).
    // Sub-second precision makes frame-level sync mismatch immediately visible.
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
            &vtenc,
            &h265parse,
            &fluxframer,
            &fluxsink,
        ])
        .expect("add elements");

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
    textoverlay.link(&vtenc).expect("textoverlay → vtenc");
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

fn print_help() {
    log!(
        "\n[multi-server] Keys:\n\
         H / ?  show this help\n\
         Q      quit\n"
    );
}

// ── TTY raw-mode helpers ──────────────────────────────────────────────────────

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
