//! `fluxsink` — GStreamer BaseSink element (server role).
//!
//! sink: application/x-flux
//! Sends FLUX frames over QUIC datagrams (crypto_quic, spec §2.2).
//! SESSION handshake goes over QUIC Stream 0 (reliable bidi), replacing the
//! old TCP control connection.  All media frames and CDBC/KEEPALIVE/FLUX-C
//! datagrams go over QUIC RFC 9221 datagrams on the same connection.

use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_base as gst_base;
use gstreamer_video as gst_video;

gst::plugin_define!(
    fluxsink,
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
    FluxSink::register(plugin)?;
    Ok(())
}

glib::wrapper! {
    pub struct FluxSink(ObjectSubclass<imp::FluxSink>)
        @extends gst_base::BaseSink, gst::Element, gst::Object;
}

impl FluxSink {
    pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
        gst::Element::register(
            Some(plugin),
            "fluxsink",
            gst::Rank::NONE,
            Self::static_type(),
        )
    }

    /// Register a sync channel to receive FLUX-C `MetadataFrame` datagrams.
    /// Must be called before setting the pipeline to PLAYING.
    /// Returns the receiver end; the sender is stored inside the element.
    pub fn subscribe_flux_control(&self) -> std::sync::mpsc::Receiver<flux_framing::FluxControl> {
        use gstreamer::subclass::prelude::ObjectSubclassExt;
        let (tx, rx) = std::sync::mpsc::sync_channel(32);
        imp::FluxSink::from_obj(self).set_flux_control_tx(tx);
        rx
    }
}

mod imp {
    use super::*;
    use bytes::Bytes;
    use flux_framing::{
        now_ns, BandwidthProbe, BwAction, BwGovernor, CdbcFeedback, FluxControl, FluxHeader,
        FrameType, SessionAccept, SessionRequest, DEFAULT_PORT, HEADER_SIZE,
    };
    use gst::subclass::prelude::*;
    use gst_base::subclass::prelude::*;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    struct Inner {
        bind_addr: String,
        port: u16,
        /// Accepted QUIC connection (set by accept task, None until a client connects).
        connection: Arc<Mutex<Option<quinn::Connection>>>,
        /// Tokio runtime owned for the lifetime of the element's PLAYING state.
        rt: Option<tokio::runtime::Runtime>,
        /// Last accepted session_id.
        session_id_last: Arc<Mutex<String>>,
        /// BW Governor.
        bw_gov: Arc<Mutex<BwGovernor>>,
        /// Probe sequence counter.
        probe_seq: Arc<AtomicU32>,
        /// Number of CDBC_FEEDBACK datagrams received.
        cdbc_reports_received: Arc<AtomicU64>,
        /// Sender for FLUX-C control messages.
        flux_control_tx: Arc<Mutex<Option<std::sync::mpsc::SyncSender<FluxControl>>>>,
    }

    impl Default for Inner {
        fn default() -> Self {
            Inner {
                bind_addr: "0.0.0.0".into(),
                port: DEFAULT_PORT,
                connection: Arc::new(Mutex::new(None)),
                rt: None,
                session_id_last: Arc::new(Mutex::new(String::new())),
                bw_gov: Arc::new(Mutex::new(BwGovernor::new())),
                probe_seq: Arc::new(AtomicU32::new(0)),
                cdbc_reports_received: Arc::new(AtomicU64::new(0)),
                flux_control_tx: Arc::new(Mutex::new(None)),
            }
        }
    }

    #[derive(Default)]
    pub struct FluxSink {
        inner: Mutex<Inner>,
    }

    impl FluxSink {
        pub(super) fn set_flux_control_tx(
            &self,
            tx: std::sync::mpsc::SyncSender<flux_framing::FluxControl>,
        ) {
            *self.inner.lock().unwrap().flux_control_tx.lock().unwrap() = Some(tx);
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FluxSink {
        const NAME: &'static str = "FluxSink";
        type Type = super::FluxSink;
        type ParentType = gst_base::BaseSink;
    }

    impl ObjectImpl for FluxSink {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPS: std::sync::OnceLock<Vec<glib::ParamSpec>> = std::sync::OnceLock::new();
            PROPS.get_or_init(|| {
                vec![
                    glib::ParamSpecString::builder("bind-address")
                        .nick("Bind address")
                        .blurb("QUIC bind address")
                        .default_value(Some("0.0.0.0"))
                        .build(),
                    glib::ParamSpecUInt::builder("port")
                        .nick("Port")
                        .blurb("FLUX QUIC port")
                        .default_value(DEFAULT_PORT as u32)
                        .build(),
                    glib::ParamSpecString::builder("session-id-last")
                        .nick("Last session ID")
                        .blurb("Session ID from the most recently accepted SESSION_REQUEST")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt64::builder("cdbc-reports-received")
                        .nick("CDBC reports received")
                        .blurb("Number of CDBC_FEEDBACK datagrams received from the client")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt::builder("bw-probes-sent")
                        .nick("BW probes sent")
                        .blurb("Number of BANDWIDTH_PROBE datagrams sent to the client")
                        .read_only()
                        .build(),
                    glib::ParamSpecString::builder("bw-governor-state")
                        .nick("BW Governor state")
                        .blurb("Current BW Governor state (Probe/Stable/RampUp/RampDown/Emergency)")
                        .read_only()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let mut s = self.inner.lock().unwrap();
            match pspec.name() {
                "bind-address" => s.bind_addr = value.get::<String>().unwrap(),
                "port" => s.port = value.get::<u32>().unwrap() as u16,
                _ => {}
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let s = self.inner.lock().unwrap();
            match pspec.name() {
                "bind-address" => s.bind_addr.to_value(),
                "port" => (s.port as u32).to_value(),
                "session-id-last" => s.session_id_last.lock().unwrap().clone().to_value(),
                "cdbc-reports-received" => {
                    s.cdbc_reports_received.load(Ordering::Relaxed).to_value()
                }
                "bw-probes-sent" => s.probe_seq.load(Ordering::Relaxed).to_value(),
                "bw-governor-state" => format!("{:?}", s.bw_gov.lock().unwrap().state).to_value(),
                _ => unimplemented!(),
            }
        }
    }

    impl GstObjectImpl for FluxSink {}

    impl ElementImpl for FluxSink {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static META: std::sync::OnceLock<gst::subclass::ElementMetadata> =
                std::sync::OnceLock::new();
            Some(META.get_or_init(|| {
                gst::subclass::ElementMetadata::new(
                    "FLUX Sink",
                    "Sink/Network/FLUX",
                    "Sends FLUX frames over QUIC datagrams (crypto_quic server)",
                    "LUCAB Media Technology",
                )
            }))
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            static PADS: std::sync::OnceLock<Vec<gst::PadTemplate>> = std::sync::OnceLock::new();
            PADS.get_or_init(|| {
                vec![gst::PadTemplate::new(
                    "sink",
                    gst::PadDirection::Sink,
                    gst::PadPresence::Always,
                    &gst::Caps::builder("application/x-flux").build(),
                )
                .unwrap()]
            })
        }
    }

    impl BaseSinkImpl for FluxSink {
        fn start(&self) -> Result<(), gst::ErrorMessage> {
            let mut s = self.inner.lock().unwrap();

            // ── Tokio runtime ────────────────────────────────────────────────
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .map_err(|e| {
                    gst::error_msg!(gst::ResourceError::Failed, ["tokio runtime: {}", e])
                })?;

            // ── TLS: ephemeral self-signed cert (rcgen) ───────────────────────
            let rcgen::CertifiedKey { cert, key_pair } =
                rcgen::generate_simple_self_signed(vec!["localhost".into()]).map_err(|e| {
                    gst::error_msg!(gst::ResourceError::Failed, ["rcgen: {}", e])
                })?;

            let cert_der = rustls::pki_types::CertificateDer::from(cert);
            let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(
                rustls::pki_types::PrivatePkcs8KeyDer::from(key_pair.serialize_der()),
            );

            // ── quinn ServerConfig (with_single_cert handles TLS internally) ──
            let mut transport = quinn::TransportConfig::default();
            transport.datagram_receive_buffer_size(Some(2 * 1024 * 1024));
            transport.max_concurrent_bidi_streams(64u32.into());
            transport.max_concurrent_uni_streams(128u32.into());
            transport.keep_alive_interval(Some(std::time::Duration::from_secs(5)));

            let mut server_cfg =
                quinn::ServerConfig::with_single_cert(vec![cert_der], key_der).map_err(|e| {
                    gst::error_msg!(gst::ResourceError::Failed, ["server cert: {}", e])
                })?;
            server_cfg.transport_config(Arc::new(transport));

            // ── Bind QUIC endpoint ────────────────────────────────────────────
            let bind_addr: std::net::SocketAddr =
                format!("{}:{}", s.bind_addr, s.port).parse().map_err(|e| {
                    gst::error_msg!(gst::ResourceError::Failed, ["parse bind addr: {}", e])
                })?;

            // quinn::Endpoint::server() requires an active Tokio context on the
            // calling thread.  Enter the runtime before calling it.
            let _rt_guard = rt.enter();

            let endpoint =
                quinn::Endpoint::server(server_cfg, bind_addr).map_err(|e| {
                    gst::error_msg!(
                        gst::ResourceError::OpenWrite,
                        ["QUIC bind {}: {}", bind_addr, e]
                    )
                })?;

            eprintln!("[fluxsink] QUIC endpoint bound on {} (crypto_quic)", bind_addr);

            // ── Spawn connection-accept + handshake task ──────────────────────
            let conn_slot = s.connection.clone();
            let sid_ref = s.session_id_last.clone();
            let bw_ref = s.bw_gov.clone();
            let probe_seq_ref = s.probe_seq.clone();
            let cdbc_rx_ref = s.cdbc_reports_received.clone();
            let flux_ctrl_tx = s.flux_control_tx.clone();
            // Weak reference to self so the accept task can send ForceKeyUnitEvent
            let element_weak = self.obj().downgrade();

            rt.spawn(async move {
                accept_loop(
                    endpoint,
                    conn_slot,
                    sid_ref,
                    bw_ref,
                    probe_seq_ref,
                    cdbc_rx_ref,
                    flux_ctrl_tx,
                    element_weak,
                )
                .await;
            });

            // Reset connection slot for this session
            *s.connection.lock().unwrap() = None;
            s.rt = Some(rt);

            Ok(())
        }

        fn stop(&self) -> Result<(), gst::ErrorMessage> {
            let mut s = self.inner.lock().unwrap();
            if let Some(conn) = s.connection.lock().unwrap().take() {
                conn.close(quinn::VarInt::from_u32(0), b"stopped");
            }
            // Dropping the runtime shuts down the tokio executor and all tasks
            s.rt = None;
            Ok(())
        }

        fn render(&self, buffer: &gst::Buffer) -> Result<gst::FlowSuccess, gst::FlowError> {
            use flux_framing::{FluxHeader, HEADER_SIZE};

            let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
            let data = map.as_slice();

            let hdr = FluxHeader::decode(data).ok_or_else(|| {
                eprintln!("[fluxsink] malformed FLUX header");
                gst::FlowError::Error
            })?;
            let payload = &data[HEADER_SIZE..];

            let s = self.inner.lock().unwrap();

            let conn = {
                let guard = s.connection.lock().unwrap();
                guard.clone()
            };

            let conn = match conn {
                Some(c) => c,
                None => {
                    // No client yet — drop the frame silently.
                    return Ok(gst::FlowSuccess::Ok);
                }
            };

            // ── QUIC-aware fragmentation ──────────────────────────────────────
            // QUIC datagram size is bounded by the negotiated path MTU (typically
            // ~1200 bytes on loopback). Use conn.max_datagram_size() and leave
            // 32 bytes for the FLUX header. Hard-floor at 512 bytes in case the
            // runtime returns an unexpectedly small value.
            let max_dg = conn.max_datagram_size().unwrap_or(1200);
            let chunk_size = max_dg.saturating_sub(HEADER_SIZE).max(512);

            let datagrams: Vec<Vec<u8>> = if payload.len() <= chunk_size {
                // Single unfragmented datagram, frag=0
                let mut h = hdr.clone();
                h.frag = 0;
                h.payload_length = payload.len() as u32;
                let mut dg = Vec::with_capacity(HEADER_SIZE + payload.len());
                dg.extend_from_slice(&h.encode());
                dg.extend_from_slice(payload);
                vec![dg]
            } else {
                let chunks: Vec<&[u8]> = payload.chunks(chunk_size).collect();
                let n = chunks.len();
                chunks
                    .iter()
                    .enumerate()
                    .map(|(i, chunk)| {
                        let mut h = hdr.clone();
                        // frag: 1-based index for non-last; 0xF for last fragment
                        h.frag = if i == n - 1 { 0xF } else { (i + 1) as u8 };
                        h.payload_length = chunk.len() as u32;
                        let mut dg = Vec::with_capacity(HEADER_SIZE + chunk.len());
                        dg.extend_from_slice(&h.encode());
                        dg.extend_from_slice(chunk);
                        dg
                    })
                    .collect()
            };

            for datagram in datagrams {
                let bytes = Bytes::from(datagram);
                if let Err(e) = conn.send_datagram(bytes) {
                    eprintln!("[fluxsink] send_datagram error: {}", e);
                    // Clear connection so we wait for a new client.
                    *s.connection.lock().unwrap() = None;
                    return Ok(gst::FlowSuccess::Ok);
                }
            }
            Ok(gst::FlowSuccess::Ok)
        }
    }

    // ── Accept loop ───────────────────────────────────────────────────────────

    async fn accept_loop(
        endpoint: quinn::Endpoint,
        conn_slot: Arc<Mutex<Option<quinn::Connection>>>,
        session_id_store: Arc<Mutex<String>>,
        bw_gov: Arc<Mutex<BwGovernor>>,
        probe_seq: Arc<AtomicU32>,
        cdbc_reports_received: Arc<AtomicU64>,
        flux_control_tx: Arc<Mutex<Option<std::sync::mpsc::SyncSender<FluxControl>>>>,
        element_weak: gst::glib::WeakRef<super::FluxSink>,
    ) {
        static SESSION_COUNTER: std::sync::atomic::AtomicU32 =
            std::sync::atomic::AtomicU32::new(1);

        while let Some(incoming) = endpoint.accept().await {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[fluxsink] QUIC accept error: {}", e);
                    continue;
                }
            };

            eprintln!("[fluxsink] QUIC connection from {}", conn.remote_address());

            // ── SESSION handshake on Stream 0 ─────────────────────────────
            let (mut send, mut recv) = match conn.accept_bi().await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[fluxsink] accept bidi stream error: {}", e);
                    continue;
                }
            };

            // Read [u32 BE length][JSON body]
            let req: SessionRequest = {
                let mut len_buf = [0u8; 4];
                if recv_exact(&mut recv, &mut len_buf).await.is_err() {
                    eprintln!("[fluxsink] failed to read SessionRequest length");
                    continue;
                }
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut body = vec![0u8; len];
                if recv_exact(&mut recv, &mut body).await.is_err() {
                    eprintln!("[fluxsink] failed to read SessionRequest body");
                    continue;
                }
                match serde_json::from_slice(&body) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("[fluxsink] malformed SessionRequest: {}", e);
                        continue;
                    }
                }
            };

            eprintln!(
                "[fluxsink] SESSION_REQUEST — client_id={} codec={:?} max_fps={} cdbc_interval_ms={}",
                req.client_id, req.codec_support, req.max_fps, req.cdbc_interval_ms,
            );

            // Build SessionAccept
            let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
            let session_id = format!("sess-{}-{}", now_ns() / 1_000_000, counter);

            let accept = SessionAccept {
                session_id: session_id.clone(),
                ..SessionAccept::default()
            };
            let json = serde_json::to_vec(&accept).unwrap_or_default();
            let mut msg = Vec::with_capacity(4 + json.len());
            msg.extend_from_slice(&(json.len() as u32).to_be_bytes());
            msg.extend_from_slice(&json);
            if let Err(e) = send.write_all(&msg).await {
                eprintln!("[fluxsink] write SessionAccept: {}", e);
                continue;
            }
            let _ = send.finish();

            *session_id_store.lock().unwrap() = session_id.clone();
            eprintln!(
                "[fluxsink] SESSION_ACCEPT sent — session_id={} peer={}",
                session_id,
                conn.remote_address()
            );

            // Publish connection so render() can send datagrams
            *conn_slot.lock().unwrap() = Some(conn.clone());

            // Force an IDR keyframe so the new client doesn't receive P-frames
            // before ever seeing an IDR (which h265parse/vtdec_hw would flag as
            // "broken/invalid nal").  The UpstreamForceKeyUnitEvent travels up
            // from fluxsink's sink pad toward vtenc_h265, telling it to emit an
            // IDR on its very next output buffer.
            if let Some(element) = element_weak.upgrade() {
                let fku = gst_video::UpstreamForceKeyUnitEvent::builder()
                    .all_headers(true)
                    .build();
                if let Some(sink_pad) = element.static_pad("sink") {
                    sink_pad.send_event(fku);
                    eprintln!("[fluxsink] ForceKeyUnitEvent sent upstream for new client");
                }
            }

            // Spawn datagram reader task
            let conn_for_reader = conn.clone();
            let conn_slot_reader = conn_slot.clone();
            let bw_ref = bw_gov.clone();
            let probe_ref = probe_seq.clone();
            let cdbc_ref = cdbc_reports_received.clone();
            let ctrl_tx = flux_control_tx.clone();

            tokio::spawn(async move {
                run_datagram_reader(
                    conn_for_reader,
                    conn_slot_reader,
                    bw_ref,
                    probe_ref,
                    cdbc_ref,
                    ctrl_tx,
                )
                .await;
            });
        }
    }

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

    // ── Datagram reader ───────────────────────────────────────────────────────

    async fn run_datagram_reader(
        conn: quinn::Connection,
        conn_slot: Arc<Mutex<Option<quinn::Connection>>>,
        bw_gov: Arc<Mutex<BwGovernor>>,
        probe_seq: Arc<AtomicU32>,
        cdbc_reports_received: Arc<AtomicU64>,
        flux_control_tx: Arc<Mutex<Option<std::sync::mpsc::SyncSender<FluxControl>>>>,
    ) {
        loop {
            let datagram = match conn.read_datagram().await {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("[fluxsink/dgram] read_datagram error: {}", e);
                    *conn_slot.lock().unwrap() = None;
                    break;
                }
            };

            let data = datagram.as_ref();
            if data.len() < HEADER_SIZE {
                continue;
            }

            let hdr = match FluxHeader::decode(data) {
                Some(h) => h,
                None => continue,
            };

            match hdr.frame_type {
                FrameType::MetadataFrame => {
                    let body = &data[HEADER_SIZE..];
                    match FluxControl::decode_body(body) {
                        Some(ctrl) => {
                            eprintln!(
                                "[fluxsink/ctrl] FLUX-C {:?} from {}",
                                ctrl.control_type,
                                conn.remote_address()
                            );
                            if let Some(tx) = flux_control_tx.lock().unwrap().as_ref() {
                                let _ = tx.try_send(ctrl);
                            }
                        }
                        None => {
                            eprintln!(
                                "[fluxsink/ctrl] unreadable MetadataFrame from {}",
                                conn.remote_address()
                            );
                        }
                    }
                }

                FrameType::CdbcFeedbackT => {
                    let body = &data[HEADER_SIZE..];
                    let fb: CdbcFeedback = match serde_json::from_slice(body) {
                        Ok(f) => f,
                        Err(e) => {
                            eprintln!("[fluxsink/cdbc] bad CDBC_FEEDBACK: {}", e);
                            continue;
                        }
                    };

                    eprintln!(
                        "[fluxsink/cdbc] CDBC_FEEDBACK — avail={}bps rx={}bps loss={:.1}% jitter={:.2}ms probe_result={}bps",
                        fb.avail_bps, fb.rx_bps, fb.loss_pct, fb.jitter_ms, fb.probe_result_bps,
                    );

                    cdbc_reports_received.fetch_add(1, Ordering::Relaxed);

                    let action = {
                        let mut gov = bw_gov.lock().unwrap();
                        gov.ingest(&fb)
                    };

                    eprintln!("[fluxsink/cdbc] BwGovernor → {:?}", action);

                    match action {
                        BwAction::SendProbe => {
                            let seq = probe_seq.fetch_add(1, Ordering::Relaxed);
                            let probe = BandwidthProbe {
                                ts_ns: now_ns(),
                                probe_seq: seq,
                                probe_size: 1200,
                            };
                            let payload = serde_json::to_vec(&probe).unwrap_or_default();
                            let mut padded = payload.clone();
                            while padded.len() < probe.probe_size as usize {
                                padded.push(0u8);
                            }
                            let probe_hdr = FluxHeader {
                                version: flux_framing::FLUX_VERSION,
                                frame_type: FrameType::BandwidthProbe,
                                flags: 0,
                                channel_id: 0,
                                layer: 0,
                                frag: 0,
                                group_id: 0,
                                group_timestamp_ns: now_ns(),
                                presentation_ts: 0,
                                capture_ts_ns_lo: 0,
                                payload_length: padded.len() as u32,
                                fec_group: 0,
                                sequence_in_group: seq,
                            };
                            let mut pkt = Vec::with_capacity(HEADER_SIZE + padded.len());
                            pkt.extend_from_slice(&probe_hdr.encode());
                            pkt.extend_from_slice(&padded);
                            let bytes = Bytes::from(pkt);
                            let _ = conn.send_datagram(bytes);
                            eprintln!(
                                "[fluxsink/cdbc] BANDWIDTH_PROBE #{} sent ({} bytes)",
                                seq, probe.probe_size
                            );
                        }
                        BwAction::AddLayer => {
                            eprintln!("[fluxsink/cdbc] ACTION: add enhancement layer");
                        }
                        BwAction::DropLayer => {
                            eprintln!("[fluxsink/cdbc] ACTION: drop top enhancement layer");
                        }
                        BwAction::EmergencyShed => {
                            eprintln!("[fluxsink/cdbc] ACTION: EMERGENCY — shed layers");
                        }
                        BwAction::EnableFec => {
                            eprintln!("[fluxsink/cdbc] ACTION: enable XOR Row FEC (loss > 5%)");
                        }
                        BwAction::EnableFecRS => {
                            eprintln!("[fluxsink/cdbc] ACTION: enable RS 2D FEC (loss > 15%)");
                        }
                        BwAction::RecoveryRampUp => {
                            eprintln!("[fluxsink/cdbc] ACTION: EMERGENCY recovery → RAMP_UP");
                        }
                        BwAction::Hold => {}
                    }
                }

                FrameType::Keepalive => {
                    eprintln!("[fluxsink] KEEPALIVE from {}", conn.remote_address());
                }

                _ => {}
            }
        }
    }
}
