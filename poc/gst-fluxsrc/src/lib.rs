//! `fluxsrc` — GStreamer PushSrc element (client role).
//!
//! src: application/x-flux
//!
//! Connects to fluxsink via QUIC (crypto_quic, spec §2.2):
//!   - Opens a QUIC connection to server_addr:port
//!   - SESSION handshake on QUIC Stream 0 (replaces old TCP :port+1)
//!   - Media datagrams received via `connection.read_datagram()`
//!   - KEEPALIVE and CDBC_FEEDBACK sent back via `connection.send_datagram()`
//!
//! Network simulation (NetSim)
//! ───────────────────────────
//! Three independently-adjustable impairments are applied to every incoming
//! datagram *before* it is handed to the fragment-reassembly path:
//!
//!   sim-loss-pct   (f64, 0.0–100.0)  — random packet drop probability
//!   sim-delay-ms   (u32, 0–500)      — artificial one-way latency (ms)
//!   sim-bw-kbps    (u32, 0=off)      — token-bucket bandwidth cap (kbps)

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
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    // ─── NetSim ───────────────────────────────────────────────────────────────

    #[derive(Default)]
    struct NetSim {
        loss_pct_x100: AtomicU32, // 0–10000
        delay_ms: AtomicU32,      // 0–500
        bw_kbps: AtomicU32,       // 0 = off
    }

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
            other.release.cmp(&self.release) // min-heap
        }
    }

    // ─── Fragment reassembly ──────────────────────────────────────────────────

    struct FragAssembly {
        seq: u32,
        data: Vec<u8>,
    }

    // ─── QUIC connection handle (shared between start() and the recv task) ────

    struct Inner {
        server_addr: String,
        port: u16,
        /// QUIC connection, set after successful connect+handshake.
        connection: Option<quinn::Connection>,
        /// Tokio runtime, owned for the element's PLAYING lifetime.
        rt: Option<tokio::runtime::Runtime>,
        session_id: String,
        keepalive_seq: u32,
        keepalives_sent: u64,
        last_ka_sent: Instant,
        ka_interval: Duration,
        ka_timeout_count: u32,
        last_rx: Instant,
        frag_assembly: Option<FragAssembly>,
        // NetSim token-bucket state
        tb_tokens: f64,
        tb_last: Instant,
    }

    impl Default for Inner {
        fn default() -> Self {
            Inner {
                server_addr: "127.0.0.1".into(),
                port: DEFAULT_PORT,
                connection: None,
                rt: None,
                session_id: String::new(),
                keepalive_seq: 0,
                keepalives_sent: 0,
                last_ka_sent: Instant::now(),
                ka_interval: Duration::from_millis(1000),
                ka_timeout_count: 0,
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
        netsim: Arc<NetSim>,
        /// Datagrams received from QUIC, after BW throttle + loss, pushed here.
        delayed_tx: Mutex<Option<std::sync::mpsc::SyncSender<Vec<u8>>>>,
        delayed_rx: Mutex<Option<std::sync::mpsc::Receiver<Vec<u8>>>>,
        /// Shared delay heap + condvar for the delay thread.
        delay_heap: Arc<(Mutex<BinaryHeap<DelayEntry>>, Condvar)>,
        /// Stop flag for the delay thread (and recv task).
        stop_flag: Arc<AtomicU32>,
        /// Raw datagrams from the QUIC recv task → NetSim pipeline.
        /// The recv task pushes here; create() reads and applies NetSim.
        raw_rx: Mutex<Option<std::sync::mpsc::Receiver<Vec<u8>>>>,
        raw_tx: Mutex<Option<std::sync::mpsc::SyncSender<Vec<u8>>>>,
    }

    impl Default for FluxSrc {
        fn default() -> Self {
            let (dtx, drx) = std::sync::mpsc::sync_channel(1024);
            let (rtx, rrx) = std::sync::mpsc::sync_channel(1024);
            FluxSrc {
                inner: Mutex::new(Inner::default()),
                netsim: Arc::new(NetSim::default()),
                delayed_tx: Mutex::new(Some(dtx)),
                delayed_rx: Mutex::new(Some(drx)),
                delay_heap: Arc::new((Mutex::new(BinaryHeap::new()), Condvar::new())),
                stop_flag: Arc::new(AtomicU32::new(0)),
                raw_rx: Mutex::new(Some(rrx)),
                raw_tx: Mutex::new(Some(rtx)),
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
                        .blurb("FLUX server QUIC port (default 7400)")
                        .default_value(DEFAULT_PORT as u32)
                        .build(),
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
                    "Receives FLUX frames over QUIC datagrams (crypto_quic client)",
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
            self.stop_flag.store(0, Ordering::Relaxed);

            let mut s = self.inner.lock().unwrap();

            // ── Tokio runtime ────────────────────────────────────────────────
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .map_err(|e| {
                    gst::error_msg!(gst::ResourceError::Failed, ["tokio runtime: {}", e])
                })?;

            // ── Build QUIC client endpoint with skip-verify TLS ──────────────
            // PoC trust model: equivalent to the old crypto_none.
            // We accept any server cert (no PKI, ephemeral self-signed).
            let tls_cfg = rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(SkipVerify::new()))
                .with_no_client_auth();

            let quic_client_crypto =
                quinn::crypto::rustls::QuicClientConfig::try_from(tls_cfg).map_err(|e| {
                    gst::error_msg!(
                        gst::ResourceError::Failed,
                        ["quinn client crypto: {}", e]
                    )
                })?;

            let mut transport = quinn::TransportConfig::default();
            transport.datagram_receive_buffer_size(Some(2 * 1024 * 1024));
            transport.keep_alive_interval(Some(Duration::from_secs(5)));

            let mut client_cfg = quinn::ClientConfig::new(Arc::new(quic_client_crypto));
            client_cfg.transport_config(Arc::new(transport));

            // quinn::Endpoint::client() requires an active Tokio context.
            let _rt_guard = rt.enter();

            let mut endpoint =
                quinn::Endpoint::client("0.0.0.0:0".parse().unwrap()).map_err(|e| {
                    gst::error_msg!(
                        gst::ResourceError::OpenRead,
                        ["QUIC client endpoint: {}", e]
                    )
                })?;
            endpoint.set_default_client_config(client_cfg);

            // ── Connect to server ────────────────────────────────────────────
            let server_addr: std::net::SocketAddr =
                format!("{}:{}", s.server_addr, s.port)
                    .parse()
                    .map_err(|e| {
                        gst::error_msg!(
                            gst::ResourceError::Failed,
                            ["invalid server address: {}", e]
                        )
                    })?;

            let connection = rt
                .block_on(async {
                    let connecting = endpoint
                        .connect(server_addr, "localhost")
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
                    connecting.await.map_err(|e| std::io::Error::new(std::io::ErrorKind::ConnectionRefused, e.to_string()))
                })
                .map_err(|e| {
                    gst::error_msg!(
                        gst::ResourceError::OpenRead,
                        ["QUIC connect to {}: {}", server_addr, e]
                    )
                })?;

            eprintln!(
                "[fluxsrc] QUIC connected to {} (crypto_quic)",
                connection.remote_address()
            );

            // ── SESSION handshake on Stream 0 ────────────────────────────────
            let (mut send, mut recv) = rt
                .block_on(async { connection.open_bi().await })
                .map_err(|e| {
                    gst::error_msg!(
                        gst::ResourceError::OpenRead,
                        ["open bidi stream: {}", e]
                    )
                })?;

            let req = SessionRequest::default();
            let json: Vec<u8> = serde_json::to_vec(&req).unwrap();
            let mut msg: Vec<u8> = Vec::with_capacity(4 + json.len());
            msg.extend_from_slice(&(json.len() as u32).to_be_bytes());
            msg.extend_from_slice(&json);
            rt.block_on(async { send.write_all(&msg).await })
                .map_err(|e| {
                    gst::error_msg!(
                        gst::ResourceError::OpenRead,
                        ["write SessionRequest: {}", e]
                    )
                })?;

            // Read SessionAccept
            let accept: Option<SessionAccept> = rt.block_on(async {
                let mut len_buf = [0u8; 4];
                if recv_exact(&mut recv, &mut len_buf).await.is_err() {
                    return None;
                }
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut body = vec![0u8; len];
                if recv_exact(&mut recv, &mut body).await.is_err() {
                    return None;
                }
                serde_json::from_slice::<SessionAccept>(&body).ok()
            });

            if let Some(ref a) = accept {
                eprintln!(
                    "[fluxsrc] SESSION_ACCEPT — session_id={} ka_interval_ms={} ka_timeout={}",
                    a.session_id, a.keepalive_interval_ms, a.keepalive_timeout,
                );
                s.session_id = a.session_id.clone();
                s.ka_interval = Duration::from_millis(a.keepalive_interval_ms as u64);
                s.ka_timeout_count = a.keepalive_timeout;
            } else {
                gst::warning!(
                    gst::CAT_DEFAULT,
                    "FluxSrc: no SESSION_ACCEPT received (continuing without session)"
                );
            }

            s.last_rx = Instant::now();
            s.last_ka_sent = Instant::now();
            s.connection = Some(connection.clone());

            // ── Spawn QUIC datagram recv task ─────────────────────────────────
            // Pulls raw datagrams and forwards them to `raw_tx` channel.
            // create() reads from `raw_rx`, applies NetSim, then pushes downstream.
            let raw_tx = self.raw_tx.lock().unwrap().clone();
            let stop_flag = self.stop_flag.clone();
            if let Some(raw_tx) = raw_tx {
                let conn_for_recv = connection.clone();
                rt.spawn(async move {
                    run_recv_task(conn_for_recv, raw_tx, stop_flag).await;
                });
            }

            // ── Spawn delay thread ────────────────────────────────────────────
            drop(s); // release mutex before spawning threads

            let heap_pair = Arc::clone(&self.delay_heap);
            let stop_flag2 = Arc::clone(&self.stop_flag);
            let dtx = self.delayed_tx.lock().unwrap().clone();
            if let Some(dtx) = dtx {
                std::thread::Builder::new()
                    .name("fluxsrc-delay".into())
                    .spawn(move || run_delay_thread(heap_pair, dtx, stop_flag2))
                    .ok();
            }

            self.inner.lock().unwrap().rt = Some(rt);

            Ok(())
        }

        fn stop(&self) -> Result<(), gst::ErrorMessage> {
            self.stop_flag.store(1, Ordering::Relaxed);
            let (_heap, cvar) = &*self.delay_heap;
            cvar.notify_all();

            let mut s = self.inner.lock().unwrap();
            if let Some(conn) = s.connection.take() {
                conn.close(quinn::VarInt::from_u32(0), b"stopped");
            }
            s.rt = None;
            Ok(())
        }
    }

    impl PushSrcImpl for FluxSrc {
        fn create(
            &self,
            _buf: Option<&mut gst::BufferRef>,
        ) -> Result<gst_base::subclass::base_src::CreateSuccess, FlowError> {
            loop {
                // ── Drain delayed channel (non-blocking) ──────────────────────
                {
                    if let Some(rx) = self.delayed_rx.lock().unwrap().as_ref() {
                        while let Ok(data) = rx.try_recv() {
                            if let Some(buf) = self.push_data(data)? {
                                return Ok(gst_base::subclass::base_src::CreateSuccess::NewBuffer(buf));
                            }
                        }
                    }
                }

                // ── Keepalive + session-dead check ────────────────────────────
                {
                    let mut s = self.inner.lock().unwrap();

                    if s.ka_timeout_count > 0 {
                        let deadline = s.ka_interval.saturating_mul(s.ka_timeout_count);
                        if s.last_rx.elapsed() > deadline {
                            eprintln!(
                                "[fluxsrc] session '{}' dead — no datagrams for {:?}",
                                s.session_id,
                                s.last_rx.elapsed()
                            );
                            return Err(FlowError::Eos);
                        }
                    }

                    if s.last_ka_sent.elapsed() >= s.ka_interval {
                        if let Some(ref conn) = s.connection.clone() {
                            let sid = s.session_id.clone();
                            let ka_hdr = FluxHeader::new_keepalive(0, s.keepalive_seq);
                            let ka_payload = KeepalivePayload {
                                ts_ns: now_ns(),
                                session_id: sid,
                                seq: s.keepalive_seq,
                            };
                            let ka_json = serde_json::to_vec(&ka_payload).unwrap();
                            let mut pkt = Vec::with_capacity(HEADER_SIZE + ka_json.len());
                            pkt.extend_from_slice(&ka_hdr.encode());
                            pkt.extend_from_slice(&ka_json);
                            let bytes = bytes::Bytes::from(pkt);
                            let _ = conn.send_datagram(bytes);
                            s.keepalive_seq = s.keepalive_seq.wrapping_add(1);
                            s.keepalives_sent += 1;
                            s.last_ka_sent = Instant::now();
                        }
                    }
                }

                // ── Receive raw datagram (500 ms timeout) ─────────────────────
                let raw = {
                    match self
                        .raw_rx
                        .lock()
                        .unwrap()
                        .as_ref()
                        .and_then(|rx| rx.recv_timeout(Duration::from_millis(500)).ok())
                    {
                        Some(d) => d,
                        None => {
                            // Timeout — loop back to check keepalive / session dead.
                            // Also drain delayed channel.
                            if let Some(rx) = self.delayed_rx.lock().unwrap().as_ref() {
                                while let Ok(data) = rx.try_recv() {
                                    if let Some(buf) = self.push_data(data)? {
                                        return Ok(gst_base::subclass::base_src::CreateSuccess::NewBuffer(buf));
                                    }
                                }
                            }
                            continue;
                        }
                    }
                };

                // Update last-received timestamp.
                self.inner.lock().unwrap().last_rx = Instant::now();

                // ── NetSim: random loss ───────────────────────────────────────
                let loss_x100 = self.netsim.loss_pct_x100.load(Ordering::Relaxed);
                if loss_x100 > 0 {
                    let r = lcg_rand() % 10000;
                    if r < loss_x100 {
                        continue;
                    }
                }

                // ── NetSim: token-bucket BW throttle ─────────────────────────
                let bw_kbps = self.netsim.bw_kbps.load(Ordering::Relaxed);
                if bw_kbps > 0 {
                    let byte_rate = (bw_kbps as f64) * 1000.0 / 8.0;
                    let datagram_bytes = raw.len() as f64;
                    loop {
                        let tokens = {
                            let mut s = self.inner.lock().unwrap();
                            let e = s.tb_last.elapsed().as_secs_f64();
                            s.tb_tokens += e * byte_rate;
                            if s.tb_tokens > 65536.0 {
                                s.tb_tokens = 65536.0;
                            }
                            s.tb_last = Instant::now();
                            s.tb_tokens
                        };
                        if tokens >= datagram_bytes {
                            self.inner.lock().unwrap().tb_tokens -= datagram_bytes;
                            break;
                        }
                        let deficit = datagram_bytes - tokens;
                        let wait_secs = deficit / byte_rate;
                        std::thread::sleep(Duration::from_secs_f64(wait_secs.min(0.1)));
                    }
                }

                // ── NetSim: artificial delay ──────────────────────────────────
                let delay_ms = self.netsim.delay_ms.load(Ordering::Relaxed);
                if delay_ms > 0 {
                    let release = Instant::now() + Duration::from_millis(delay_ms as u64);
                    let entry = DelayEntry { release, data: raw };
                    let (heap_lock, cvar) = &*self.delay_heap;
                    {
                        let mut heap = heap_lock.lock().unwrap();
                        heap.push(entry);
                    }
                    cvar.notify_one();
                    if let Some(rx) = self.delayed_rx.lock().unwrap().as_ref() {
                        while let Ok(data) = rx.try_recv() {
                            if let Some(buf) = self.push_data(data)? {
                                return Ok(gst_base::subclass::base_src::CreateSuccess::NewBuffer(buf));
                            }
                        }
                    }
                    continue;
                }

                // ── No delay: process immediately ─────────────────────────────
                if let Some(buf) = self.push_data(raw)? {
                    return Ok(gst_base::subclass::base_src::CreateSuccess::NewBuffer(buf));
                }
            }
        }
    }

    // ─── QUIC recv task ───────────────────────────────────────────────────────

    async fn run_recv_task(
        conn: quinn::Connection,
        tx: std::sync::mpsc::SyncSender<Vec<u8>>,
        stop_flag: Arc<AtomicU32>,
    ) {
        loop {
            if stop_flag.load(Ordering::Relaxed) != 0 {
                break;
            }
            match conn.read_datagram().await {
                Ok(dg) => {
                    if tx.try_send(dg.to_vec()).is_err() {
                        break; // receiver gone
                    }
                }
                Err(e) => {
                    eprintln!("[fluxsrc/recv] read_datagram error: {}", e);
                    break;
                }
            }
        }
    }

    // ─── Fragment reassembly ──────────────────────────────────────────────────

    impl FluxSrc {
        fn push_data(&self, data: Vec<u8>) -> Result<Option<gst::Buffer>, FlowError> {
            let n = data.len();
            let hdr = match FluxHeader::decode(&data) {
                Some(h) => h,
                None => return Ok(None),
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
                        Duration::ZERO
                    } else {
                        top.release - now
                    }
                } else {
                    Duration::from_millis(100)
                }
            };

            if wait_dur > Duration::ZERO {
                let heap = heap_lock.lock().unwrap();
                let _guard = cvar.wait_timeout(heap, wait_dur).unwrap();
                continue;
            }

            let mut heap = heap_lock.lock().unwrap();
            let now = Instant::now();
            while let Some(top) = heap.peek() {
                if top.release <= now {
                    let entry = heap.pop().unwrap();
                    if tx.try_send(entry.data).is_err() {
                        return;
                    }
                } else {
                    break;
                }
            }
        }
    }

    // ─── Skip-verify TLS (PoC) ────────────────────────────────────────────────

    #[derive(Debug)]
    struct SkipVerify(Arc<rustls::crypto::CryptoProvider>);

    impl SkipVerify {
        fn new() -> Self {
            Self(Arc::new(rustls::crypto::ring::default_provider()))
        }
    }

    impl rustls::client::danger::ServerCertVerifier for SkipVerify {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls::pki_types::CertificateDer<'_>,
            _intermediates: &[rustls::pki_types::CertificateDer<'_>],
            _server_name: &rustls::pki_types::ServerName<'_>,
            _ocsp_response: &[u8],
            _now: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &rustls::pki_types::CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &rustls::pki_types::CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }

    // ─── recv_exact helper ────────────────────────────────────────────────────

    async fn recv_exact(
        recv: &mut quinn::RecvStream,
        buf: &mut [u8],
    ) -> Result<(), std::io::Error> {
        let mut filled = 0;
        while filled < buf.len() {
            match recv.read(&mut buf[filled..]).await {
                Ok(Some(n)) => filled += n,
                Ok(None) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "stream closed",
                    ))
                }
                Err(e) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    ))
                }
            }
        }
        Ok(())
    }

    // ─── LCG pseudo-random ────────────────────────────────────────────────────

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
