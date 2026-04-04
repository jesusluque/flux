//! `fluxsrc` — GStreamer PushSrc element (client role).
//!
//! src: application/x-flux
//!
//! Connects to fluxsink via:
//!   - TCP to server_addr:port+1 for SESSION handshake (crypto_none)
//!   - UDP bind on :7402 to receive media datagrams
//!   - Sends KEEPALIVE and CDBC_FEEDBACK back to server via UDP
//!
//! Each received UDP datagram is pushed as a GstBuffer downstream.
//!
//! Network simulation (NetSim)
//! ───────────────────────────
//! Three independently-adjustable impairments are applied to every incoming
//! datagram *before* it is handed to the fragment-reassembly path:
//!
//!   sim-loss-pct   (f64, 0.0–100.0)  — random packet drop probability
//!   sim-delay-ms   (u32, 0–500)      — artificial one-way latency (ms)
//!   sim-bw-kbps    (u32, 0=off)      — token-bucket bandwidth cap (kbps)
//!
//! All three are writable GObject properties, so the client keyboard thread
//! can adjust them at runtime via `element.set_property(...)`.

use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;

// ─── Plugin registration ──────────────────────────────────────────────────────

gst::plugin_define!(
    fluxsrc,
    env!("CARGO_PKG_DESCRIPTION"),
    plugin_init,
    concat!(env!("CARGO_PKG_VERSION")),
    "MPL-2.0",
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_REPOSITORY"),
    "2026-04-03"
);

fn plugin_init(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    FluxSrc::register(plugin)?;
    Ok(())
}

// ─── Public wrapper ───────────────────────────────────────────────────────────

glib::wrapper! {
    pub struct FluxSrc(ObjectSubclass<imp::FluxSrc>)
        @extends gstreamer_base::PushSrc, gstreamer_base::BaseSrc, gst::Element, gst::Object;
}

impl FluxSrc {
    pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
        gst::Element::register(
            Some(plugin),
            "fluxsrc",
            gst::Rank::NONE,
            Self::static_type(),
        )
    }
}

// ─── Implementation submodule ─────────────────────────────────────────────────

mod imp {
    use flux_framing::{
        now_ns, FluxHeader, KeepalivePayload, SessionAccept, SessionRequest, DEFAULT_PORT,
        HEADER_SIZE,
    };
    use gst::glib;
    use gst::FlowError;
    use gstreamer as gst;
    use gstreamer::prelude::*;
    use gstreamer::subclass::prelude::*;
    use gstreamer_base as gst_base;
    use gstreamer_base::subclass::prelude::*;
    use serde_json;
    use std::collections::BinaryHeap;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream, UdpSocket};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    // ─── NetSim ───────────────────────────────────────────────────────────────

    /// Network-impairment simulation parameters.
    ///
    /// All fields are `AtomicU32` so the keyboard thread can write them without
    /// taking `inner`'s mutex.
    ///
    /// Units:
    ///   loss_pct_x100  — integer hundredths of a percent (0–10000; 500 = 5.00%)
    ///   delay_ms       — milliseconds (0–500)
    ///   bw_kbps        — kilobits/s (0 = unlimited)
    #[derive(Default)]
    struct NetSim {
        loss_pct_x100: AtomicU32, // 0–10000
        delay_ms: AtomicU32,      // 0–500
        bw_kbps: AtomicU32,       // 0 = off
    }

    /// One entry in the delay heap.
    /// `Reverse` makes `BinaryHeap` a min-heap on release time.
    struct DelayEntry {
        release: Instant,
        data: Vec<u8>,
    }
    impl PartialEq for DelayEntry {
        fn eq(&self, other: &Self) -> bool {
            self.release == other.release
        }
    }
    impl Eq for DelayEntry {}
    impl PartialOrd for DelayEntry {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for DelayEntry {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            // min-heap: earlier release time = higher priority
            other.release.cmp(&self.release)
        }
    }

    // ─── State ────────────────────────────────────────────────────────────────

    /// In-progress fragment reassembly buffer for one AU.
    struct FragAssembly {
        seq: u32,
        data: Vec<u8>,
    }

    struct Inner {
        server_addr: String,
        port: u16,
        udp_sock: Option<Arc<UdpSocket>>,
        server_udp: Option<SocketAddr>,
        session_id: String,
        keepalive_seq: u32,
        keepalives_sent: u64,
        last_ka_sent: Instant,
        ka_interval: Duration,
        /// Number of consecutive missed server keepalives before session is dead.
        /// 0 means timeout detection is disabled (server never sends keepalives yet).
        ka_timeout_count: u32,
        /// Last time we received *any* valid datagram from the server.
        /// Used to detect session death when ka_timeout_count > 0.
        last_rx: Instant,
        /// Fragment reassembly state (None = no in-progress AU)
        frag_assembly: Option<FragAssembly>,

        // ── NetSim token-bucket state ─────────────────────────────────────
        /// Fractional byte tokens accumulated by the token bucket.
        tb_tokens: f64,
        /// When tokens were last updated.
        tb_last: Instant,
    }

    impl Default for Inner {
        fn default() -> Self {
            Inner {
                server_addr: "127.0.0.1".into(),
                port: DEFAULT_PORT,
                udp_sock: None,
                server_udp: None,
                session_id: String::new(),
                keepalive_seq: 0,
                keepalives_sent: 0,
                last_ka_sent: Instant::now(),
                ka_interval: Duration::from_millis(1000),
                ka_timeout_count: 0, // disabled until SESSION_ACCEPT received
                last_rx: Instant::now(),
                frag_assembly: None,
                tb_tokens: 0.0,
                tb_last: Instant::now(),
            }
        }
    }

    // ─── GObject subclass ─────────────────────────────────────────────────────

    pub struct FluxSrc {
        inner: Mutex<Inner>,
        /// Shared network-simulation parameters (written by keyboard thread,
        /// read by create() and delay thread).
        netsim: Arc<NetSim>,
        /// Datagrams that have passed BW throttle and loss check are pushed
        /// here (possibly after a delay).  `create()` reads from the receiver.
        delayed_tx: Mutex<Option<std::sync::mpsc::SyncSender<Vec<u8>>>>,
        delayed_rx: Mutex<Option<std::sync::mpsc::Receiver<Vec<u8>>>>,
        /// Shared delay heap + condvar for the delay thread.
        delay_heap: Arc<(Mutex<BinaryHeap<DelayEntry>>, Condvar)>,
        /// Set to true when stop() is called so the delay thread exits.
        stop_flag: Arc<AtomicU32>,
    }

    impl Default for FluxSrc {
        fn default() -> Self {
            let (tx, rx) = std::sync::mpsc::sync_channel(1024);
            FluxSrc {
                inner: Mutex::new(Inner::default()),
                netsim: Arc::new(NetSim::default()),
                delayed_tx: Mutex::new(Some(tx)),
                delayed_rx: Mutex::new(Some(rx)),
                delay_heap: Arc::new((Mutex::new(BinaryHeap::new()), Condvar::new())),
                stop_flag: Arc::new(AtomicU32::new(0)),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FluxSrc {
        const NAME: &'static str = "FluxSrc";
        type Type = super::FluxSrc;
        type ParentType = gst_base::PushSrc;
    }

    impl ObjectImpl for FluxSrc {
        fn constructed(&self) {
            self.parent_constructed();
            // Mark as a live source so the pipeline transitions to PLAYING without
            // waiting for a preroll buffer (live sources don't block in PAUSED).
            use gstreamer_base::prelude::BaseSrcExt;
            self.obj().set_live(true);
        }

        fn properties() -> &'static [glib::ParamSpec] {
            static PROPS: std::sync::OnceLock<Vec<glib::ParamSpec>> = std::sync::OnceLock::new();
            PROPS.get_or_init(|| {
                vec![
                    glib::ParamSpecString::builder("address")
                        .nick("Server address")
                        .blurb("IP address of the FLUX server")
                        .default_value(Some("127.0.0.1"))
                        .build(),
                    glib::ParamSpecUInt::builder("port")
                        .nick("Port")
                        .blurb("FLUX server media port (default 7400)")
                        .default_value(DEFAULT_PORT as u32)
                        .build(),
                    // ── Read-only session stats ───────────────────────────────
                    glib::ParamSpecString::builder("session-id")
                        .nick("Session ID")
                        .blurb("Negotiated session ID (set after SESSION_ACCEPT)")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt::builder("keepalive-interval-ms")
                        .nick("KA interval ms")
                        .blurb("Keepalive interval negotiated in SESSION_ACCEPT (ms)")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt::builder("keepalive-timeout-count")
                        .nick("KA timeout count")
                        .blurb("Number of missed keepalives before session is declared dead")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt64::builder("keepalives-sent")
                        .nick("Keepalives sent")
                        .blurb("Total KEEPALIVE datagrams sent to the server")
                        .read_only()
                        .build(),
                    // ── NetSim writable properties ────────────────────────────
                    glib::ParamSpecDouble::builder("sim-loss-pct")
                        .nick("Sim loss %")
                        .blurb("Random packet loss probability (0.0–100.0 %)")
                        .minimum(0.0)
                        .maximum(100.0)
                        .default_value(0.0)
                        .build(),
                    glib::ParamSpecUInt::builder("sim-delay-ms")
                        .nick("Sim delay ms")
                        .blurb("Artificial one-way latency added to every datagram (0–500 ms)")
                        .minimum(0)
                        .maximum(500)
                        .default_value(0)
                        .build(),
                    glib::ParamSpecUInt::builder("sim-bw-kbps")
                        .nick("Sim BW kbps")
                        .blurb("Token-bucket bandwidth cap in kbps (0 = unlimited)")
                        .default_value(0)
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "address" => {
                    self.inner.lock().unwrap().server_addr = value.get::<String>().unwrap()
                }
                "port" => self.inner.lock().unwrap().port = value.get::<u32>().unwrap() as u16,
                "sim-loss-pct" => {
                    let pct: f64 = value.get().unwrap();
                    let x100 = (pct * 100.0).round().clamp(0.0, 10000.0) as u32;
                    self.netsim.loss_pct_x100.store(x100, Ordering::Relaxed);
                }
                "sim-delay-ms" => {
                    let ms: u32 = value.get().unwrap();
                    self.netsim.delay_ms.store(ms.min(500), Ordering::Relaxed);
                }
                "sim-bw-kbps" => {
                    let kbps: u32 = value.get().unwrap();
                    self.netsim.bw_kbps.store(kbps, Ordering::Relaxed);
                    // Reset token bucket so a sudden cap doesn't cause a
                    // huge burst of accumulated tokens.
                    self.inner.lock().unwrap().tb_tokens = 0.0;
                }
                _ => {}
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "address" => self.inner.lock().unwrap().server_addr.to_value(),
                "port" => (self.inner.lock().unwrap().port as u32).to_value(),
                "session-id" => self.inner.lock().unwrap().session_id.to_value(),
                "keepalive-interval-ms" => {
                    (self.inner.lock().unwrap().ka_interval.as_millis() as u32).to_value()
                }
                "keepalive-timeout-count" => self.inner.lock().unwrap().ka_timeout_count.to_value(),
                "keepalives-sent" => self.inner.lock().unwrap().keepalives_sent.to_value(),
                "sim-loss-pct" => {
                    let x100 = self.netsim.loss_pct_x100.load(Ordering::Relaxed);
                    (x100 as f64 / 100.0).to_value()
                }
                "sim-delay-ms" => self.netsim.delay_ms.load(Ordering::Relaxed).to_value(),
                "sim-bw-kbps" => self.netsim.bw_kbps.load(Ordering::Relaxed).to_value(),
                _ => unimplemented!(),
            }
        }
    }

    impl GstObjectImpl for FluxSrc {}

    impl ElementImpl for FluxSrc {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static META: std::sync::OnceLock<gst::subclass::ElementMetadata> =
                std::sync::OnceLock::new();
            Some(META.get_or_init(|| {
                gst::subclass::ElementMetadata::new(
                    "FLUX Source",
                    "Source/Network/FLUX",
                    "Receives FLUX frames over UDP (crypto_none client)",
                    "LUCAB Media Technology",
                )
            }))
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            static PADS: std::sync::OnceLock<Vec<gst::PadTemplate>> = std::sync::OnceLock::new();
            PADS.get_or_init(|| {
                let src_caps = gst::Caps::builder("application/x-flux").build();
                vec![gst::PadTemplate::new(
                    "src",
                    gst::PadDirection::Src,
                    gst::PadPresence::Always,
                    &src_caps,
                )
                .unwrap()]
            })
        }
    }

    impl BaseSrcImpl for FluxSrc {
        fn is_seekable(&self) -> bool {
            false
        }

        fn caps(&self, _filter: Option<&gst::Caps>) -> Option<gst::Caps> {
            Some(gst::Caps::builder("application/x-flux").build())
        }

        fn start(&self) -> Result<(), gst::ErrorMessage> {
            let mut s = self.inner.lock().unwrap();

            // ── Bind local UDP socket ─────────────────────────────────────────
            // Use port advertised in SessionRequest (default 7402).
            let req_defaults = SessionRequest::default();
            let local_addr = format!("0.0.0.0:{}", req_defaults.media_port);
            let sock = UdpSocket::bind(&local_addr).map_err(|e| {
                gst::error_msg!(
                    gst::ResourceError::OpenRead,
                    ["bind UDP {}: {}", local_addr, e]
                )
            })?;
            sock.set_read_timeout(Some(Duration::from_millis(500))).ok();

            let server_udp: SocketAddr =
                format!("{}:{}", s.server_addr, s.port)
                    .parse()
                    .map_err(|e| {
                        gst::error_msg!(
                            gst::ResourceError::Failed,
                            ["invalid server address: {}", e]
                        )
                    })?;

            s.server_udp = Some(server_udp);
            s.udp_sock = Some(Arc::new(sock));
            s.last_rx = Instant::now();

            // ── TCP SESSION handshake ─────────────────────────────────────────
            let ctrl_addr = format!("{}:{}", s.server_addr, s.port + 1);
            match TcpStream::connect_timeout(&ctrl_addr.parse().unwrap(), Duration::from_secs(5)) {
                Ok(mut tcp) => {
                    // Send SessionRequest with our media_port
                    let req = SessionRequest::default();
                    let json = serde_json::to_vec(&req).unwrap();
                    let _ = tcp.write_all(&(json.len() as u32).to_be_bytes());
                    let _ = tcp.write_all(&json);

                    // Read SessionAccept
                    let mut len_buf = [0u8; 4];
                    if tcp.read_exact(&mut len_buf).is_ok() {
                        let len = u32::from_be_bytes(len_buf) as usize;
                        let mut body = vec![0u8; len];
                        if tcp.read_exact(&mut body).is_ok() {
                            match serde_json::from_slice::<SessionAccept>(&body) {
                                Ok(accept) => {
                                    eprintln!(
                                        "[fluxsrc] SESSION_ACCEPT — session_id={} ka_interval_ms={} ka_timeout={}",
                                        accept.session_id,
                                        accept.keepalive_interval_ms,
                                        accept.keepalive_timeout,
                                    );
                                    s.session_id = accept.session_id;
                                    s.ka_interval =
                                        Duration::from_millis(accept.keepalive_interval_ms as u64);
                                    s.ka_timeout_count = accept.keepalive_timeout;
                                }
                                Err(e) => {
                                    gst::warning!(
                                        gst::CAT_DEFAULT,
                                        "FluxSrc: malformed SESSION_ACCEPT: {}",
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    gst::warning!(
                        gst::CAT_DEFAULT,
                        "FluxSrc: TCP handshake to {} failed: {} (continuing without session)",
                        ctrl_addr,
                        e
                    );
                }
            }

            gst::info!(
                gst::CAT_DEFAULT,
                "FluxSrc: listening on {} for datagrams from {} (session_id='{}')",
                local_addr,
                server_udp,
                s.session_id,
            );

            // ── Spawn delay thread ────────────────────────────────────────────
            // The delay thread owns a copy of the delay heap and the mpsc sender.
            // It wakes whenever a new entry is pushed or an existing one is due.
            drop(s); // release the mutex before spawning

            let heap_pair = Arc::clone(&self.delay_heap);
            let stop_flag = Arc::clone(&self.stop_flag);
            let tx = self.delayed_tx.lock().unwrap().clone();

            if let Some(tx) = tx {
                std::thread::Builder::new()
                    .name("fluxsrc-delay".into())
                    .spawn(move || {
                        run_delay_thread(heap_pair, tx, stop_flag);
                    })
                    .ok();
            }

            Ok(())
        }

        fn stop(&self) -> Result<(), gst::ErrorMessage> {
            // Signal the delay thread to exit.
            self.stop_flag.store(1, Ordering::Relaxed);
            // Wake the delay thread if it is sleeping on the condvar.
            let (_heap, cvar) = &*self.delay_heap;
            cvar.notify_all();

            let mut s = self.inner.lock().unwrap();
            s.udp_sock = None;
            s.server_udp = None;
            Ok(())
        }
    }

    impl PushSrcImpl for FluxSrc {
        fn create(
            &self,
            _buf: Option<&mut gst::BufferRef>,
        ) -> Result<gst_base::subclass::base_src::CreateSuccess, FlowError> {
            let mut recv_buf = vec![0u8; 65536];

            loop {
                // ── Pull from delayed channel (non-blocking first) ─────────────
                //
                // If a datagram was previously enqueued in the delay heap and is
                // now due, the delay thread will have placed it in `delayed_rx`.
                // We try to drain it here before going back to the socket.
                {
                    if let Some(rx) = self.delayed_rx.lock().unwrap().as_ref() {
                        // Non-blocking try_recv.
                        while let Ok(data) = rx.try_recv() {
                            if let Some(buf) = self.push_data(data)? {
                                return Ok(gst_base::subclass::base_src::CreateSuccess::NewBuffer(
                                    buf,
                                ));
                            }
                        }
                    }
                }

                // ── Keepalive + session-dead check ────────────────────────────
                let sock_clone = {
                    let mut s = self.inner.lock().unwrap();
                    let sock = s.udp_sock.as_ref().ok_or(FlowError::Error)?.clone();
                    let server_udp = s.server_udp;
                    let sid = s.session_id.clone();

                    if s.ka_timeout_count > 0 {
                        let deadline = s.ka_interval.saturating_mul(s.ka_timeout_count);
                        if s.last_rx.elapsed() > deadline {
                            eprintln!(
                                "[fluxsrc] session '{}' dead — no datagrams for {:?} (deadline {:?})",
                                sid,
                                s.last_rx.elapsed(),
                                deadline,
                            );
                            return Err(FlowError::Eos);
                        }
                    }

                    if s.last_ka_sent.elapsed() >= s.ka_interval {
                        if let Some(dst) = server_udp {
                            let ka_hdr = FluxHeader::new_keepalive(0, s.keepalive_seq);
                            let ka_payload = KeepalivePayload {
                                ts_ns: now_ns(),
                                session_id: sid.clone(),
                                seq: s.keepalive_seq,
                            };
                            let ka_json = serde_json::to_vec(&ka_payload).unwrap();
                            let mut pkt = Vec::with_capacity(HEADER_SIZE + ka_json.len());
                            pkt.extend_from_slice(&ka_hdr.encode());
                            pkt.extend_from_slice(&ka_json);
                            let _ = sock.send_to(&pkt, dst);
                            s.keepalive_seq = s.keepalive_seq.wrapping_add(1);
                            s.keepalives_sent += 1;
                            s.last_ka_sent = Instant::now();
                        }
                    }

                    sock.clone()
                };

                // ── Blocking receive (500 ms timeout) ─────────────────────────
                let (n, _from) = match sock_clone.recv_from(&mut recv_buf) {
                    Ok(v) => v,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        // Timeout — loop back to check keepalive / session dead.
                        // Also drain delayed channel on timeout.
                        if let Some(rx) = self.delayed_rx.lock().unwrap().as_ref() {
                            while let Ok(data) = rx.try_recv() {
                                if let Some(buf) = self.push_data(data)? {
                                    return Ok(
                                        gst_base::subclass::base_src::CreateSuccess::NewBuffer(buf),
                                    );
                                }
                            }
                        }
                        continue;
                    }
                    Err(e) => {
                        gst::warning!(gst::CAT_DEFAULT, "UDP recv error: {}", e);
                        return Err(FlowError::Error);
                    }
                };

                // Update last-received timestamp on every valid datagram.
                self.inner.lock().unwrap().last_rx = Instant::now();

                let raw = recv_buf[..n].to_vec();

                // ── NetSim: random loss ───────────────────────────────────────
                let loss_x100 = self.netsim.loss_pct_x100.load(Ordering::Relaxed);
                if loss_x100 > 0 {
                    // LCG-based pseudo-random — no rand crate needed.
                    // Range 0–9999 inclusive.
                    let r = lcg_rand() % 10000;
                    if r < loss_x100 {
                        // Drop this datagram.
                        continue;
                    }
                }

                // ── NetSim: token-bucket BW throttle ─────────────────────────
                let bw_kbps = self.netsim.bw_kbps.load(Ordering::Relaxed);
                if bw_kbps > 0 {
                    let byte_rate = (bw_kbps as f64) * 1000.0 / 8.0; // bytes/sec
                    let datagram_bytes = raw.len() as f64;
                    loop {
                        let elapsed = {
                            let mut s = self.inner.lock().unwrap();
                            let e = s.tb_last.elapsed().as_secs_f64();
                            s.tb_tokens += e * byte_rate;
                            // Cap at one datagram-max worth of burst (65536 bytes)
                            if s.tb_tokens > 65536.0 {
                                s.tb_tokens = 65536.0;
                            }
                            s.tb_last = Instant::now();
                            s.tb_tokens
                        };
                        if elapsed >= datagram_bytes {
                            self.inner.lock().unwrap().tb_tokens -= datagram_bytes;
                            break;
                        }
                        // Not enough tokens — sleep for the time it takes to
                        // accumulate enough tokens for this datagram.
                        let deficit = datagram_bytes - elapsed;
                        let wait_secs = deficit / byte_rate;
                        std::thread::sleep(Duration::from_secs_f64(wait_secs.min(0.1)));
                    }
                }

                // ── NetSim: artificial delay ──────────────────────────────────
                let delay_ms = self.netsim.delay_ms.load(Ordering::Relaxed);
                if delay_ms > 0 {
                    // Push into delay heap; the delay thread will release it.
                    let release = Instant::now() + Duration::from_millis(delay_ms as u64);
                    let entry = DelayEntry { release, data: raw };
                    let (heap_lock, cvar) = &*self.delay_heap;
                    {
                        let mut heap = heap_lock.lock().unwrap();
                        heap.push(entry);
                    }
                    cvar.notify_one();
                    // Try to pull any already-due packets before going back to recv.
                    if let Some(rx) = self.delayed_rx.lock().unwrap().as_ref() {
                        while let Ok(data) = rx.try_recv() {
                            if let Some(buf) = self.push_data(data)? {
                                return Ok(gst_base::subclass::base_src::CreateSuccess::NewBuffer(
                                    buf,
                                ));
                            }
                        }
                    }
                    continue;
                }

                // ── No delay: process immediately ─────────────────────────────
                if let Some(buf) = self.push_data(raw)? {
                    return Ok(gst_base::subclass::base_src::CreateSuccess::NewBuffer(buf));
                }
                // else: fragment not yet complete — keep looping
            }
        }
    }

    // ─── Fragment reassembly (shared between direct path and delayed path) ───

    impl FluxSrc {
        /// Parse `data` as a FLUX datagram and run it through fragment reassembly.
        /// Returns `Some(GstBuffer)` when a complete AU is ready, `None` otherwise.
        fn push_data(&self, data: Vec<u8>) -> Result<Option<gst::Buffer>, FlowError> {
            let n = data.len();

            let hdr = match FluxHeader::decode(&data) {
                Some(h) => h,
                None => return Ok(None), // malformed, skip
            };

            let payload = &data[HEADER_SIZE..];
            let frag = hdr.frag;
            let seq = hdr.sequence_in_group;
            gst::trace!(
                gst::CAT_DEFAULT,
                "rx {} bytes frag=0x{:X} seq={}",
                n,
                frag,
                seq
            );

            let complete: Option<Vec<u8>> = {
                let mut s = self.inner.lock().unwrap();

                if frag == 0x0 {
                    s.frag_assembly = None;
                    Some(data)
                } else if frag == 0xF {
                    if let Some(ref mut asm) = s.frag_assembly {
                        if asm.seq == seq {
                            asm.data.extend_from_slice(payload);
                            let mut full = Vec::with_capacity(HEADER_SIZE + asm.data.len());
                            let mut emit_hdr = hdr.clone();
                            emit_hdr.frag = 0;
                            emit_hdr.payload_length = asm.data.len() as u32;
                            full.extend_from_slice(&emit_hdr.encode());
                            full.extend_from_slice(&asm.data);
                            s.frag_assembly = None;
                            Some(full)
                        } else {
                            s.frag_assembly = None;
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    match s.frag_assembly {
                        Some(ref mut asm) if asm.seq == seq => {
                            asm.data.extend_from_slice(payload);
                        }
                        _ => {
                            s.frag_assembly = Some(FragAssembly {
                                seq,
                                data: payload.to_vec(),
                            });
                        }
                    }
                    None
                }
            };

            if let Some(full_data) = complete {
                let len = full_data.len();
                let mut buf = gst::Buffer::with_size(len).map_err(|_| FlowError::Error)?;
                {
                    let buf_ref = buf.get_mut().unwrap();
                    let mut map = buf_ref.map_writable().map_err(|_| FlowError::Error)?;
                    map[..len].copy_from_slice(&full_data);
                }
                return Ok(Some(buf));
            }

            Ok(None)
        }
    }

    // ─── Delay thread ─────────────────────────────────────────────────────────

    /// Background thread: monitors the delay heap and forwards entries to the
    /// mpsc channel once their release time has arrived.
    fn run_delay_thread(
        heap_pair: Arc<(Mutex<BinaryHeap<DelayEntry>>, Condvar)>,
        tx: std::sync::mpsc::SyncSender<Vec<u8>>,
        stop_flag: Arc<AtomicU32>,
    ) {
        let (heap_lock, cvar) = &*heap_pair;
        loop {
            if stop_flag.load(Ordering::Relaxed) != 0 {
                break;
            }

            let wait_dur = {
                let heap = heap_lock.lock().unwrap();
                if let Some(top) = heap.peek() {
                    let now = Instant::now();
                    if top.release <= now {
                        // Ready immediately — don't wait.
                        Duration::ZERO
                    } else {
                        top.release - now
                    }
                } else {
                    // No entries; wait up to 100 ms for a push.
                    Duration::from_millis(100)
                }
            };

            if wait_dur > Duration::ZERO {
                // Sleep until the next entry is due (or woken by a push).
                let heap = heap_lock.lock().unwrap();
                let _guard = cvar.wait_timeout(heap, wait_dur).unwrap();
                // (We re-check the heap each iteration regardless of why we woke.)
                continue;
            }

            // Pop all entries that are now due.
            let mut heap = heap_lock.lock().unwrap();
            let now = Instant::now();
            while let Some(top) = heap.peek() {
                if top.release <= now {
                    let entry = heap.pop().unwrap();
                    // Best-effort send; if the receiver is gone, exit.
                    if tx.try_send(entry.data).is_err() {
                        return;
                    }
                } else {
                    break;
                }
            }
        }
    }

    // ─── Simple LCG pseudo-random (no rand crate) ────────────────────────────

    /// Thread-local LCG state for fast, dependency-free random u32 generation.
    fn lcg_rand() -> u32 {
        use std::cell::Cell;
        thread_local! {
            static STATE: Cell<u64> = Cell::new(0x123456789ABCDEF0);
        }
        STATE.with(|s| {
            let v = s
                .get()
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            s.set(v);
            (v >> 33) as u32
        })
    }
}
