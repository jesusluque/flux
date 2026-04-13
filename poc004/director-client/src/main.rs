//! FLUX PoC004 — Director Client
//!
//! Receives the switched FLUX stream from the switcher-server (port 7410) and
//! displays it full-screen.
//!
//! Pipeline:
//!   fluxsrc(port=7410)
//!     → fluxdemux
//!         media_0 → fluxcdbc → fluxdeframer → h265parse → vtdec_hw
//!                → videoconvertscale → osxvideosink
//!         control → appsink  (receives tally_confirm datagrams)
//!         cdbc    → fakesink
//!
//! Bidirectional tally (spec §8):
//!   - On keyboard 1–4: sends `TALLY_UPDATE (0xA)` + `FluxControl{Routing}`
//!     datagrams to the server via `fluxsrc.send_datagram()`.
//!   - Receives `tally_confirm` JSON from the server on the `control` pad via
//!     an appsink; displays a tally light in the terminal.
//!
//! Keyboard controls:
//!   1–4  cut to camera (sends tally + routing upstream)
//!   T    show tally state
//!   Q    quit
//!   H    help

use gst::glib;
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

// ── Constants ─────────────────────────────────────────────────────────────────

const SERVER_ADDR: &str = "127.0.0.1";
const SERVER_PORT: u32 = 7410;
const N: usize = 4;
const SESSION_ID: &str = "poc004-session-01";
const MIXER_ID: &str = "FLUX-DIRECTOR-01";

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

// ── Tally state ───────────────────────────────────────────────────────────────

struct TallyState {
    /// Currently requested camera (0-indexed). Starts at 0.
    active: u32,
    /// Last confirmed tally label received from server per channel.
    confirmed: [Option<String>; N],
}

impl TallyState {
    fn new() -> Self {
        TallyState {
            active: 0,
            confirmed: Default::default(),
        }
    }

    fn print(&self) {
        let row: Vec<String> = (0..N)
            .map(|i| {
                let marker = if i == self.active as usize {
                    "►"
                } else {
                    " "
                };
                let confirm = self.confirmed[i].as_deref().unwrap_or("---");
                format!("{}CAM {}: {:>10}", marker, i + 1, confirm)
            })
            .collect();
        log!("[tally]  {}", row.join("   "));
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

fn main() {
    env_logger::init();

    if std::env::var("GST_DEBUG").is_err() {
        std::env::set_var("GST_DEBUG", "2");
    }

    gst::init().expect("GStreamer init failed");

    let tty = open_tty_raw();
    if tty.is_none() {
        eprintln!("[director-client] WARNING: /dev/tty unavailable — keyboard disabled");
    }

    gst::macos_main(move || run(tty));
}

fn run(tty: Option<Tty>) {
    gstfluxsrc::plugin_register_static().expect("fluxsrc register");
    gstfluxdemux::plugin_register_static().expect("fluxdemux register");
    gstfluxdeframer::plugin_register_static().expect("fluxdeframer register");
    gstfluxcdbc::plugin_register_static().expect("fluxcdbc register");

    // ── Build pipeline ─────────────────────────────────────────────────────────
    let pipeline = gst::Pipeline::new();

    // ── fluxsrc ────────────────────────────────────────────────────────────────
    let fluxsrc = gst::ElementFactory::make("fluxsrc")
        .property("address", SERVER_ADDR)
        .property("port", SERVER_PORT)
        .name("fluxsrc")
        .build()
        .expect("fluxsrc")
        .downcast::<gstfluxsrc::FluxSrc>()
        .expect("FluxSrc cast");
    let fluxsrc_elem = fluxsrc.upcast_ref::<gst::Element>();

    // ── fluxdemux ──────────────────────────────────────────────────────────────
    let fluxdemux = gst::ElementFactory::make("fluxdemux")
        .name("fluxdemux")
        .build()
        .expect("fluxdemux");

    // ── fluxcdbc ───────────────────────────────────────────────────────────────
    let fluxcdbc = gst::ElementFactory::make("fluxcdbc")
        .property("cdbc-interval", 50u64)
        .property("cdbc-min-interval", 10u64)
        .name("fluxcdbc")
        .build()
        .expect("fluxcdbc")
        .downcast::<gstfluxcdbc::FluxCdbc>()
        .expect("FluxCdbc cast");
    let fluxcdbc_elem = fluxcdbc.upcast_ref::<gst::Element>();

    // Wire CDBC send callback → fluxsrc.send_datagram().
    {
        let src_weak = fluxsrc.downgrade();
        fluxcdbc.set_send_callback(move |pkt| {
            if let Some(src) = src_weak.upgrade() {
                src.send_datagram(pkt);
            }
        });
    }

    // ── Decode chain ───────────────────────────────────────────────────────────
    let fluxdeframer = gst::ElementFactory::make("fluxdeframer")
        .name("fluxdeframer")
        .build()
        .expect("fluxdeframer");

    let h265parse = gst::ElementFactory::make("h265parse")
        .name("h265parse")
        .build()
        .expect("h265parse");

    let vtdec = gst::ElementFactory::make("vtdec_hw")
        .name("vtdec")
        .property("qos", false)
        .build()
        .expect("vtdec_hw");

    let convert = gst::ElementFactory::make("videoconvertscale")
        .name("convert")
        .build()
        .expect("videoconvertscale");

    let osxvideosink = gst::ElementFactory::make("osxvideosink")
        .name("sink")
        .property("sync", false)
        .property("async", false)
        .build()
        .expect("osxvideosink");

    // ── Control appsink (tally_confirm receiver) ───────────────────────────────
    let ctrl_appsink = gst_app::AppSink::builder()
        .name("ctrl_appsink")
        .sync(false)
        .max_buffers(8u32)
        .drop(true)
        .build();

    // ── CDBC / media fakesink (for cdbc demux pad) ─────────────────────────────
    let fakesink = gst::ElementFactory::make("fakesink")
        .property("sync", false)
        .name("fakesink_cdbc")
        .build()
        .expect("fakesink");

    pipeline
        .add_many([
            fluxsrc_elem,
            &fluxdemux,
            fluxcdbc_elem,
            &fluxdeframer,
            &h265parse,
            &vtdec,
            &convert,
            &osxvideosink,
            ctrl_appsink.upcast_ref(),
            &fakesink,
        ])
        .expect("add elements");

    // Static: src → demux
    fluxsrc_elem.link(&fluxdemux).expect("fluxsrc → fluxdemux");

    // Static: decode chain (cdbc→deframer→parse→vtdec→convert→sink)
    fluxcdbc_elem
        .link(&fluxdeframer)
        .expect("fluxcdbc → fluxdeframer");
    fluxdeframer
        .link(&h265parse)
        .expect("fluxdeframer → h265parse");
    h265parse.link(&vtdec).expect("h265parse → vtdec");
    vtdec.link(&convert).expect("vtdec → convert");
    convert.link(&osxvideosink).expect("convert → sink");

    // Dynamic demux pad routing.
    let cdbc_sink_pad = fluxcdbc_elem.static_pad("sink").expect("fluxcdbc sink");
    let ctrl_sink = ctrl_appsink.static_pad("sink").expect("ctrl appsink sink");
    let fakesink_sink = fakesink.static_pad("sink").expect("fakesink sink");

    fluxdemux.connect_pad_added(move |_, pad| match pad.name().as_str() {
        "media_0" => {
            if !cdbc_sink_pad.is_linked() {
                if let Err(e) = pad.link(&cdbc_sink_pad) {
                    log!("[director-client] media_0 → fluxcdbc link failed: {:?}", e);
                }
            }
        }
        "control" => {
            if !ctrl_sink.is_linked() {
                if let Err(e) = pad.link(&ctrl_sink) {
                    log!(
                        "[director-client] control → ctrl_appsink link failed: {:?}",
                        e
                    );
                }
            }
        }
        "cdbc" => {
            if !fakesink_sink.is_linked() {
                if let Err(e) = pad.link(&fakesink_sink) {
                    log!("[director-client] cdbc → fakesink link failed: {:?}", e);
                }
            }
        }
        _ => {}
    });

    // ── Tally state ────────────────────────────────────────────────────────────
    let tally = Arc::new(Mutex::new(TallyState::new()));

    // ── Control appsink callback — parse tally_confirm ─────────────────────────
    {
        let tally2 = tally.clone();
        ctrl_appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buf = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let map = buf.map_readable().map_err(|_| gst::FlowError::Error)?;
                    let data = map.as_slice();

                    // Skip the 32-byte FLUX header; body is JSON.
                    if data.len() > flux_framing::HEADER_SIZE {
                        let body = &data[flux_framing::HEADER_SIZE..];
                        if let Some(confirm) = flux_framing::TallyConfirm::decode_body(body) {
                            let ch = confirm.channel as usize;
                            if ch < N {
                                let mut st = tally2.lock().unwrap();
                                let label = format!("{} ({})", confirm.label, confirm.color);
                                st.confirmed[ch] = Some(label);
                                log!(
                                    "[tally] CONFIRM ch={} state={} label={}",
                                    ch + 1,
                                    confirm.state,
                                    confirm.label
                                );
                                st.print();
                            }
                        }
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
    }

    // ── Keyboard → tally send ──────────────────────────────────────────────────
    let active_cam = Arc::new(AtomicU32::new(0));

    let main_loop = glib::MainLoop::new(None, false);

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
        let tally3 = tally.clone();
        let active_cam2 = active_cam.clone();
        let src_weak = fluxsrc.downgrade();

        glib::timeout_add_local(std::time::Duration::from_millis(20), move || {
            while let Ok(ch) = rx.try_recv() {
                match ch {
                    'Q' | 'q' | '\x03' => {
                        log!("[director-client] Quit");
                        ml.quit();
                        return glib::ControlFlow::Break;
                    }
                    '1'..='4' => {
                        let cam = ch as u32 - b'1' as u32;
                        if cam == active_cam2.load(Ordering::Relaxed) {
                            log!("[director-client] CAM {} is already active", cam + 1);
                        } else {
                            active_cam2.store(cam, Ordering::Relaxed);
                            {
                                let mut st = tally3.lock().unwrap();
                                st.active = cam;
                            }
                            log!("[director-client] Requesting cut → CAM {}", cam + 1);
                            send_tally_and_routing(cam, &src_weak);
                        }
                    }
                    'T' | 't' => {
                        tally3.lock().unwrap().print();
                    }
                    'H' | 'h' | '?' => print_help(),
                    _ => {}
                }
            }
            glib::ControlFlow::Continue
        });
    }

    // ── Ctrl-C ─────────────────────────────────────────────────────────────────
    {
        let pl_weak = pipeline.downgrade();
        ctrlc::set_handler(move || {
            if let Some(p) = pl_weak.upgrade() {
                p.send_event(gst::event::Eos::new());
            }
        })
        .expect("ctrlc handler");
    }

    // ── Bus watcher ────────────────────────────────────────────────────────────
    let ml2 = main_loop.clone();
    let _bus_watch = pipeline
        .bus()
        .unwrap()
        .add_watch(move |_, msg| {
            use gst::MessageView;
            match msg.view() {
                MessageView::Eos(..) => {
                    log!("[director-client] EOS");
                    ml2.quit();
                    return glib::ControlFlow::Break;
                }
                MessageView::Error(e) => {
                    log!("[director-client] ERROR: {} ({:?})", e.error(), e.debug());
                    ml2.quit();
                    return glib::ControlFlow::Break;
                }
                _ => {}
            }
            glib::ControlFlow::Continue
        })
        .unwrap();

    pipeline
        .set_state(gst::State::Playing)
        .expect("pipeline Playing");
    log!(
        "[director-client] Connected to {}:{} — Keys: 1–4 cut, T tally, Q quit, H help",
        SERVER_ADDR,
        SERVER_PORT
    );
    tally.lock().unwrap().print();

    main_loop.run();
    pipeline.set_state(gst::State::Null).unwrap();
    log!("[director-client] Stopped");
}

// ── Send TALLY_UPDATE + FluxControl{Routing} to server ───────────────────────

fn send_tally_and_routing(cam: u32, src_weak: &gst::glib::WeakRef<gstfluxsrc::FluxSrc>) {
    use flux_framing::{now_ns, FluxControl, TallyChannelState, TallyUpdate};
    use std::collections::HashMap;

    let src = match src_weak.upgrade() {
        Some(s) => s,
        None => return,
    };

    // Build TALLY_UPDATE (spec §8.1): mark new cam=program, others=idle.
    let mut channels: HashMap<String, TallyChannelState> = HashMap::new();
    for i in 0..N {
        channels.insert(
            i.to_string(),
            TallyChannelState {
                program: i as u32 == cam,
                preview: false,
                standby: false,
                iso_rec: false,
                streaming: false,
            },
        );
    }
    let tally_update = TallyUpdate {
        session_id: SESSION_ID.into(),
        ts_ns: now_ns(),
        channels,
        mixer_id: MIXER_ID.into(),
        transition: "cut".into(),
    };
    let tally_dg = tally_update.encode_datagram();
    src.send_datagram(tally_dg);

    // Build FluxControl{Routing} (spec §12 / FLUX-C): target_id = "cam-N"
    let routing_cmd = FluxControl::routing(SESSION_ID, &format!("cam-{}", cam + 1));
    let routing_dg = routing_cmd.encode_datagram(0);
    src.send_datagram(routing_dg);

    log!(
        "[director-client] Sent TALLY_UPDATE + Routing → cam-{}",
        cam + 1
    );
}

// ── Keyboard helpers ───────────────────────────────────────────────────────────

fn print_help() {
    log!(
        "\n[director-client] Keys:\n\
         1–4  cut to camera (sends TALLY_UPDATE + Routing upstream)\n\
         T    show tally state\n\
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
