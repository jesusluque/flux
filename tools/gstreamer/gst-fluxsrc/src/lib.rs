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

    /// Send a pre-encoded FLUX datagram to the server over the live QUIC
    /// connection.  Returns `true` if the datagram was queued successfully,
    /// `false` if there is no active connection or the send failed.
    pub fn send_datagram(&self, data: Vec<u8>) -> bool {
        use gstreamer::subclass::prelude::ObjectSubclassExt;
        imp::FluxSrc::from_obj(self).send_datagram_inner(bytes::Bytes::from(data))
    }
}

// ─── Implementation submodule ─────────────────────────────────────────────────

mod imp {
    use flux_framing::{
        now_ns, FluxHeader, KeepalivePayload, SessionAccept, SessionRequest, StreamAnnounce,
        DEFAULT_PORT, HEADER_SIZE,
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
        /// False until the first datagram is received on this session.
        /// The session-dead timeout clock is not started until this is true,
        /// so the pipeline can stay in Paused for arbitrarily long without
        /// triggering a spurious EOS.
        first_rx_seen: bool,
        // NetSim token-bucket state
        tb_tokens: f64,
        tb_last: Instant,
        /// Fix 3: Set to true at the start of every new session so the very
        /// first downstream buffer carries BUFFER_FLAG_DISCONT, telling
        /// h265parse to flush its GOP state and vtdec_hw to reset.
        pending_discont: bool,
        /// When the pipeline is paused, record the moment we entered Paused so
        /// that on resume we can advance last_rx and last_ka_sent by the pause
        /// duration, keeping the session-dead and keepalive clocks frozen while
        /// the pipeline is not running.
        paused_at: Option<Instant>,
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
                first_rx_seen: false,
                tb_tokens: 0.0,
                tb_last: Instant::now(),
                pending_discont: false,
                paused_at: None,
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
        ///
        /// A2/A5: `raw_rx` is wrapped in `Arc<Mutex<Receiver>>`. `create()`
        /// clones the `Arc` (briefly locking `raw_rx_slot`) and then calls
        /// `recv_timeout()` while holding only the inner `Mutex<Receiver>` —
        /// not the outer slot lock.  On restart, `start()` creates a fresh
        /// channel pair and stores the new receiver by replacing the slot,
        /// which solves the A5 stale-channel problem that `OnceLock` could
        /// not handle (it cannot be reset after first set).
        raw_rx_slot: Mutex<Arc<Mutex<std::sync::mpsc::Receiver<Vec<u8>>>>>,
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
                raw_rx_slot: Mutex::new(Arc::new(Mutex::new(rrx))),
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

        fn change_state(
            &self,
            transition: gst::StateChange,
        ) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
            match transition {
                gst::StateChange::PlayingToPaused => {
                    // Freeze the session-dead and keepalive clocks while paused.
                    self.inner.lock().unwrap().paused_at = Some(Instant::now());
                }
                gst::StateChange::PausedToPlaying => {
                    // Advance last_rx and last_ka_sent by the time spent paused
                    // so the timers behave as if time did not pass.
                    let mut s = self.inner.lock().unwrap();
                    if let Some(at) = s.paused_at.take() {
                        let frozen = at.elapsed();
                        s.last_rx += frozen;
                        s.last_ka_sent += frozen;
                    }
                }
                _ => {}
            }
            self.parent_change_state(transition)
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
            transport.datagram_receive_buffer_size(Some(4 * 1024 * 1024));
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
                    "[fluxsrc] SESSION_ACCEPT — session_id={} ka_interval_ms={} ka_timeout_count={}",
                    a.session_id, a.keepalive_interval_ms, a.keepalive_timeout_count,
                );
                s.session_id = a.session_id.clone();
                s.ka_interval = Duration::from_millis(a.keepalive_interval_ms as u64);
                s.ka_timeout_count = a.keepalive_timeout_count;
            } else {
                gst::warning!(
                    gst::CAT_DEFAULT,
                    "FluxSrc: no SESSION_ACCEPT received (continuing without session)"
                );
            }

            s.last_rx = Instant::now();
            s.last_ka_sent = Instant::now();
            s.first_rx_seen = false;
            s.connection = Some(connection.clone());
            // Fix 3: Mark that the next buffer pushed downstream is the start
            // of a fresh session and must carry BUFFER_FLAG_DISCONT.
            s.pending_discont = true;

            // ── A5: Fresh raw channel on every start() ───────────────────────
            // Replace the raw_rx slot with a new receiver so that any stale
            // datagrams from a previous session are discarded and the old
            // sender (held by the previous recv task) is disconnected.
            let (rtx, rrx) = std::sync::mpsc::sync_channel::<Vec<u8>>(1024);
            *self.raw_rx_slot.lock().unwrap() = Arc::new(Mutex::new(rrx));
            *self.raw_tx.lock().unwrap() = Some(rtx);

            // ── Spawn QUIC datagram recv task ─────────────────────────────────
            // Receives server→client QUIC Datagrams (keepalive acks, bandwidth
            // probes) and forwards them to the `raw_tx` channel.  Media frames
            // arrive on uni-streams via `run_stream_announce_listener` instead.
            let raw_tx = self.raw_tx.lock().unwrap().clone();
            let stop_flag = self.stop_flag.clone();
            if let Some(raw_tx) = raw_tx {
                let conn_for_recv = connection.clone();
                rt.spawn(async move {
                    run_datagram_recv_task(conn_for_recv, raw_tx, stop_flag).await;
                });
            }

             // ── Spawn uni-stream listener task ────────────────────────────────
             // Receives all server-opened unidirectional streams: STREAM_ANNOUNCE
             // frames (M1) and, now, one-stream-per-AU media data frames.  The
             // media frames are forwarded to raw_tx so create() delivers them to
             // the GStreamer pipeline.
             {
                 let conn_for_uni = connection.clone();
                 let uni_raw_tx = self.raw_tx.lock().unwrap().clone();
                 rt.spawn(async move {
                     run_stream_announce_listener(conn_for_uni, uni_raw_tx).await;
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

                    // Only check for session death after we have received at
                    // least one datagram.  Before that, last_rx is set to the
                    // start() timestamp, so the timeout would fire spuriously
                    // while the pipeline is still in Paused (pre-roll).
                    if s.first_rx_seen && s.ka_timeout_count > 0 {
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
                            let ka_payload = KeepalivePayload {
                                ts_ns: now_ns(),
                                session_id: sid,
                                seq: s.keepalive_seq,
                            };
                            let ka_json = serde_json::to_vec(&ka_payload).unwrap();
                            // Build the header *after* serialising the body so
                            // payload_length is correct (spec §3.3).
                            let mut ka_hdr = FluxHeader::new_keepalive(0, s.keepalive_seq);
                            ka_hdr.payload_length = ka_json.len() as u32;
                            let mut pkt = Vec::with_capacity(HEADER_SIZE + ka_json.len());
                            pkt.extend_from_slice(&ka_hdr.encode());
                            pkt.extend_from_slice(&ka_json);
                            let bytes = bytes::Bytes::from(pkt);
                            let _ = conn.send_datagram(bytes);
                            s.keepalive_seq = s.keepalive_seq.wrapping_add(1);
                            s.keepalives_sent += 1;
                            s.last_ka_sent = Instant::now();
                            // The QUIC connection is alive as long as we can
                            // send.  Reset last_rx so the session-dead timer
                            // does not fire during windows where the server
                            // sends nothing (e.g. while awaiting an IDR).
                            s.last_rx = Instant::now();
                            s.first_rx_seen = true;
                        }
                    }
                }

                // ── Receive raw datagram (500 ms timeout) ─────────────────────
                // A2/A5: Clone the Arc<Mutex<Receiver>> while briefly locking
                // the slot, then release the slot lock before calling
                // recv_timeout — so property reads from other threads are never
                // blocked for up to 500 ms.
                let raw = {
                    let rx_arc = self.raw_rx_slot.lock().unwrap().clone();
                    let guard = rx_arc.lock().unwrap();
                    match guard.recv_timeout(Duration::from_millis(500)) {
                        Ok(d) => d,
                        Err(_) => {
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

                // Update last-received timestamp and arm the session-dead clock.
                {
                    let mut s = self.inner.lock().unwrap();
                    s.last_rx = Instant::now();
                    s.first_rx_seen = true;
                }

                // ── NetSim: random loss ───────────────────────────────────────
                let loss_x100 = self.netsim.loss_pct_x100.load(Ordering::Relaxed);
                if loss_x100 > 0 {
                    let r = lcg_rand() % 10000;
                    if r < loss_x100 {
                        continue;
                    }
                }

                // ── NetSim: token-bucket BW throttle ─────────────────────────
                // A3: When the token bucket is exhausted, we push the datagram
                // onto the delay heap with its computed release time rather than
                // calling std::thread::sleep() on the GStreamer source thread.
                // Sleeping here caused pipeline clock drift under congestion.
                let bw_kbps = self.netsim.bw_kbps.load(Ordering::Relaxed);
                if bw_kbps > 0 {
                    let byte_rate = (bw_kbps as f64) * 1000.0 / 8.0;
                    let datagram_bytes = raw.len() as f64;
                    let (enough_tokens, wait_secs) = {
                        let mut s = self.inner.lock().unwrap();
                        let e = s.tb_last.elapsed().as_secs_f64();
                        s.tb_tokens += e * byte_rate;
                        if s.tb_tokens > 65536.0 {
                            s.tb_tokens = 65536.0;
                        }
                        s.tb_last = Instant::now();
                        if s.tb_tokens >= datagram_bytes {
                            s.tb_tokens -= datagram_bytes;
                            (true, 0.0)
                        } else {
                            let deficit = datagram_bytes - s.tb_tokens;
                            (false, (deficit / byte_rate).min(0.5))
                        }
                    };
                    if !enough_tokens {
                        // Defer via delay heap — avoids blocking the source thread.
                        let release = Instant::now() + Duration::from_secs_f64(wait_secs);
                        let entry = DelayEntry { release, data: raw };
                        let (heap_lock, cvar) = &*self.delay_heap;
                        {
                            let mut heap = heap_lock.lock().unwrap();
                            heap.push(entry);
                        }
                        cvar.notify_one();
                        continue;
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

    // ─── STREAM_ANNOUNCE listener / media stream receiver ────────────────────

    /// Receives all incoming unidirectional QUIC streams from the server.
    ///
    /// Each stream carries exactly one FLUX frame (FLUX_HEADER + payload).
    /// Frame types handled:
    ///   - `MediaData`      → forwarded to `raw_tx` for the GStreamer pipeline
    ///   - `StreamAnnounce` → logged (M1 / spec §3.1)
    ///   - everything else  → ignored with a log line
    async fn run_stream_announce_listener(
        conn: quinn::Connection,
        raw_tx: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
    ) {
        loop {
            let mut recv = match conn.accept_uni().await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[fluxsrc/uni] accept_uni error: {}", e);
                    break;
                }
            };

            // Read all bytes from the unidirectional stream.
            let mut data = Vec::new();
            let mut chunk_buf = [0u8; 65536];
            loop {
                match recv.read(&mut chunk_buf).await {
                    Ok(Some(n)) => data.extend_from_slice(&chunk_buf[..n]),
                    Ok(None) => break, // stream finished (EOF)
                    Err(e) => {
                        eprintln!("[fluxsrc/uni] read error on uni stream: {}", e);
                        break;
                    }
                }
            }

            if data.len() < HEADER_SIZE {
                eprintln!(
                    "[fluxsrc/uni] uni stream too short ({} bytes) — skipping",
                    data.len()
                );
                continue;
            }

            let hdr = match FluxHeader::decode(&data) {
                Some(h) => h,
                None => {
                    eprintln!("[fluxsrc/uni] could not decode FLUX header — skipping");
                    continue;
                }
            };

            let payload = &data[HEADER_SIZE..];

            match hdr.frame_type {
                flux_framing::FrameType::MediaData => {
                    if hdr.is_keyframe() {
                        eprintln!(
                            "[fluxsrc/uni] MediaData keyframe=true group_ts_ns={} len={}",
                            hdr.group_timestamp_ns, data.len()
                        );
                    }
                    // Forward to the GStreamer pipeline via raw_tx.
                    if let Some(ref tx) = raw_tx {
                        if tx.try_send(data).is_err() {
                            eprintln!("[fluxsrc/uni] raw_tx full or closed — frame dropped");
                        }
                    }
                }
                flux_framing::FrameType::StreamAnnounce => {
                    match serde_json::from_slice::<StreamAnnounce>(payload) {
                        Ok(sa) => {
                            eprintln!(
                                "[fluxsrc] STREAM_ANNOUNCE — ch={} layer={} codec={} name={:?} rate={} res={}",
                                sa.channel_id, sa.layer_id, sa.codec, sa.name,
                                sa.frame_rate, sa.resolution,
                            );
                        }
                        Err(e) => {
                            eprintln!("[fluxsrc] malformed STREAM_ANNOUNCE payload: {}", e);
                        }
                    }
                }
                other => {
                    eprintln!(
                        "[fluxsrc/uni] unexpected frame type {:?} on uni stream — ignoring",
                        other
                    );
                }
            }
        }
    }

    // ─── QUIC recv task ───────────────────────────────────────────────────────

    /// Receives server→client QUIC Datagrams and forwards them to the raw_tx
    /// channel.  In the current PoC the server only uses QUIC datagrams for
    /// keepalive acknowledgements and bandwidth probes; media AUs arrive on
    /// unidirectional streams handled by `run_stream_announce_listener`.
    async fn run_datagram_recv_task(
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
        /// Send a pre-encoded datagram over the live QUIC connection.
        pub(super) fn send_datagram_inner(&self, bytes: bytes::Bytes) -> bool {
            let conn = self.inner.lock().unwrap().connection.clone();
            match conn {
                Some(c) => c.send_datagram(bytes).is_ok(),
                None => false,
            }
        }

        /// Wrap a raw QUIC datagram in a GStreamer buffer and pass it downstream.
        ///
        /// Fragment reassembly is intentionally NOT performed here.  The
        /// `fluxdeframer` element downstream owns reassembly; doing it in two
        /// places simultaneously (B6) caused race conditions and dead code.
        fn push_data(&self, data: Vec<u8>) -> Result<Option<gst::Buffer>, FlowError> {
            if FluxHeader::decode(&data).is_none() {
                return Ok(None); // discard undecodable datagrams silently
            }

            let len = data.len();
            let mut buf = gst::Buffer::with_size(len).map_err(|_| FlowError::Error)?;
            {
                let buf_ref = buf.get_mut().unwrap();

                // Fix 3: Stamp DISCONT on the very first buffer of each new
                // session so fluxdeframer → h265parse → vtdec_hw know the
                // reference picture chain has been reset.
                let discont = {
                    let mut s = self.inner.lock().unwrap();
                    let d = s.pending_discont;
                    s.pending_discont = false;
                    d
                };
                if discont {
                    buf_ref.set_flags(gst::BufferFlags::DISCONT);
                }

                let mut map = buf_ref.map_writable().map_err(|_| FlowError::Error)?;
                map[..len].copy_from_slice(&data);
            }
            Ok(Some(buf))
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
