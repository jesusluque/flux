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

    /// Send an arbitrary datagram to the connected client (S→C direction).
    ///
    /// Used by poc004 to deliver `tally_confirm` datagrams back to the director
    /// client without going through the GStreamer media pipeline.
    /// Returns `true` if the datagram was queued, `false` if no client is
    /// connected or the send queue is full.
    pub fn send_datagram(&self, data: Vec<u8>) -> bool {
        use gstreamer::subclass::prelude::ObjectSubclassExt;
        imp::FluxSink::from_obj(self).send_datagram_inner(bytes::Bytes::from(data))
    }
}

mod imp {
    use super::*;
    use bytes::Bytes;
    use flux_framing::{
        now_ns, BandwidthProbe, BwAction, BwGovernor, CdbcFeedback, FluxControl, FluxHeader,
        FrameType, SessionAccept, SessionRequest, StreamAnnounce, DEFAULT_PORT, HEADER_SIZE,
    };
    use gst::subclass::prelude::*;
    use gst_base::subclass::prelude::*;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    fn cat() -> &'static gst::DebugCategory {
        static CAT: std::sync::OnceLock<gst::DebugCategory> = std::sync::OnceLock::new();
        CAT.get_or_init(|| {
            gst::DebugCategory::new(
                "fluxsink",
                gst::DebugColorFlags::empty(),
                Some("FLUX sink"),
            )
        })
    }

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
        /// A4: Generation counter incremented each time a new client connects.
        /// The datagram-reader task receives a copy of its generation at spawn
        /// time and exits as soon as the global counter advances past it.
        reader_generation: Arc<AtomicU32>,
        /// PTS of the last frame successfully sent to the client (nanoseconds,
        /// pipeline clock).  render() drops any frame whose PTS is strictly
        /// less than this value, enforcing monotonically-increasing PTS on the
        /// wire.  Reset to 0 on each new client connection.
        /// 0 means "nothing sent yet this session" (pass all frames through).
        last_sent_pts_ns: Arc<AtomicU64>,
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
                reader_generation: Arc::new(AtomicU32::new(0)),
                last_sent_pts_ns: Arc::new(AtomicU64::new(0)),
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

        /// Send a raw datagram to the currently-connected client (S→C).
        /// Returns `true` if the datagram was queued, `false` if no client is
        /// connected or the QUIC send queue is full.
        pub(super) fn send_datagram_inner(&self, bytes: Bytes) -> bool {
            // Clone the Arc<Mutex<Option<Connection>>> while holding inner lock,
            // then release inner lock before acquiring the connection lock.
            let conn_arc = self.inner.lock().unwrap().connection.clone();
            let guard = conn_arc.lock().unwrap();
            match guard.as_ref() {
                Some(c) => c.send_datagram(bytes).is_ok(),
                None => false,
            }
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
                    "Jesus Luque",
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
            transport.datagram_receive_buffer_size(Some(4 * 1024 * 1024));
            transport.max_concurrent_bidi_streams(64u32.into());
            transport.max_concurrent_uni_streams(512u32.into());
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

            gst::info!(cat(), "[fluxsink] QUIC endpoint bound on {} (crypto_quic)", bind_addr);

            // ── Spawn connection-accept + handshake task ──────────────────────
            let conn_slot = s.connection.clone();
            let sid_ref = s.session_id_last.clone();
            let bw_ref = s.bw_gov.clone();
            let probe_seq_ref = s.probe_seq.clone();
            let cdbc_rx_ref = s.cdbc_reports_received.clone();
            let flux_ctrl_tx = s.flux_control_tx.clone();
            let reader_gen_ref = s.reader_generation.clone();
            let last_sent_pts_ref = s.last_sent_pts_ns.clone();

            rt.spawn(async move {
                accept_loop(
                    endpoint,
                    conn_slot,
                    sid_ref,
                    bw_ref,
                    probe_seq_ref,
                    cdbc_rx_ref,
                    flux_ctrl_tx,
                    reader_gen_ref,
                    last_sent_pts_ref,
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
                gst::warning!(cat(), "[fluxsink] malformed FLUX header");
                gst::FlowError::Error
            })?;
            let payload = &data[HEADER_SIZE..];

            // A1: Extract the connection Arc without holding `inner` while we
            // later acquire `connection`'s own lock.  Holding both simultaneously
            // creates a fragile nested-lock ordering.
            let (conn_arc, last_sent_pts) = {
                let s = self.inner.lock().unwrap();
                (s.connection.clone(), s.last_sent_pts_ns.clone())
            };

            // Wait for the first IDR before sending to the client.
            // vtenc_h265 does not honour ForceKeyUnit immediately; it takes
            // several frames.  We gate on is_keyframe() which is set by
            // h265parse based on actual NAL unit type.
            // last_sent_pts==0 means "no IDR sent yet this session".
            if last_sent_pts.load(Ordering::Acquire) == 0 && !hdr.is_keyframe() {
                return Ok(gst::FlowSuccess::Ok);
            }

            // Monotonicity enforcement: drop any frame whose PTS is strictly
            // less than the last frame we sent.  This discards stale frames
            // that vtenc_h265 flushes from its hardware pipeline after a
            // ForceKeyUnit — they arrive out of PTS order relative to the IDR.
            {
                let frame_pts = hdr.group_timestamp_ns;
                let prev = last_sent_pts.load(Ordering::Acquire);
                if hdr.is_keyframe() {
                    gst::debug!(
                        cat(),
                        "[fluxsink] render: keyframe=true group_ts_ns={} last_sent={}",
                        frame_pts, prev
                    );
                }
                if prev > 0 && frame_pts < prev {
                    gst::debug!(
                        cat(),
                        "[fluxsink] drop stale frame pts_ns={} < last_sent_pts_ns={}",
                        frame_pts, prev
                    );
                    return Ok(gst::FlowSuccess::Ok);
                }
                // Will update last_sent_pts after the send succeeds (below).
            }

            let conn = {
                let guard = conn_arc.lock().unwrap();
                guard.clone()
            };

            let conn = match conn {
                Some(c) => c,
                None => {
                    // No client yet — drop the frame silently.
                    return Ok(gst::FlowSuccess::Ok);
                }
            };

             // ── Stream-per-AU media delivery ─────────────────────────────────
             // QUIC datagram frames are bounded by the path MTU (~1200 bytes)
             // and cannot carry a full H.265 AU (up to ~200 KB for an IDR).
             // Instead we open a short-lived unidirectional QUIC stream for each
             // AU and write [FLUX_HEADER (frag=0) | PAYLOAD] to it.  The stream
             // is immediately finished after the write, so the receiver sees EOF
             // and knows the AU is complete.  This is reliable (QUIC streams are
             // retransmitted) and unbounded in size.
             //
             // The stream open + write is async, so we fire it as a tokio task
             // via the runtime stored in Inner.  render() is called on the GST
             // streaming thread, not inside an async executor.
             let rt_handle = {
                 let s = self.inner.lock().unwrap();
                 s.rt.as_ref().map(|r| r.handle().clone())
             };
             let rt_handle = match rt_handle {
                 Some(h) => h,
                 None => return Ok(gst::FlowSuccess::Ok),
             };

             // Build the unfragmented FLUX frame (frag=0).
             let is_kf = hdr.is_keyframe();
             let mut send_hdr = hdr.clone();
             send_hdr.frag = 0;
             send_hdr.payload_length = payload.len() as u32;
             let mut frame = Vec::with_capacity(HEADER_SIZE + payload.len());
             frame.extend_from_slice(&send_hdr.encode());
             frame.extend_from_slice(payload);
             let frame_bytes = bytes::Bytes::from(frame);

             // Record this frame's PTS as the new high-water mark before
             // dispatching the async send.  render() is called sequentially
             // on the GStreamer streaming thread so no extra locking is needed.
             let frame_pts = hdr.group_timestamp_ns;
             let prev = last_sent_pts.load(Ordering::Acquire);
             if frame_pts > prev {
                 last_sent_pts.store(frame_pts, Ordering::Release);
             }

             rt_handle.spawn(async move {
                 match conn.open_uni().await {
                     Ok(mut uni) => {
                         if is_kf { gst::debug!(cat(), "[fluxsink] open_uni OK keyframe=true, writing {} bytes", frame_bytes.len()); }
                         if let Err(e) = uni.write_all(&frame_bytes).await {
                             gst::warning!(cat(), "[fluxsink] media stream write error: {}", e);
                         } else if let Err(e) = uni.finish() {
                             gst::warning!(cat(), "[fluxsink] media stream finish error: {}", e);
                         } else if is_kf {
                             gst::debug!(cat(), "[fluxsink] media stream finished OK keyframe=true");
                         }
                     }
                     Err(e) => {
                         gst::warning!(cat(), "[fluxsink] open_uni for media error: {}", e);
                     }
                 }
             });

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
        reader_generation: Arc<AtomicU32>,
        last_sent_pts: Arc<AtomicU64>,
    ) {
        static SESSION_COUNTER: std::sync::atomic::AtomicU32 =
            std::sync::atomic::AtomicU32::new(1);

        while let Some(incoming) = endpoint.accept().await {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(e) => {
                    gst::warning!(cat(), "[fluxsink] QUIC accept error: {}", e);
                    continue;
                }
            };

            gst::info!(cat(), "[fluxsink] QUIC connection from {}", conn.remote_address());

            // ── SESSION handshake on Stream 0 ─────────────────────────────
            let (mut send, mut recv) = match conn.accept_bi().await {
                Ok(s) => s,
                Err(e) => {
                    gst::warning!(cat(), "[fluxsink] accept bidi stream error: {}", e);
                    continue;
                }
            };

            // Read [u32 BE length][JSON body]
            let req: SessionRequest = {
                let mut len_buf = [0u8; 4];
                if recv_exact(&mut recv, &mut len_buf).await.is_err() {
                    gst::warning!(cat(), "[fluxsink] failed to read SessionRequest length");
                    continue;
                }
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut body = vec![0u8; len];
                if recv_exact(&mut recv, &mut body).await.is_err() {
                    gst::warning!(cat(), "[fluxsink] failed to read SessionRequest body");
                    continue;
                }
                match serde_json::from_slice(&body) {
                    Ok(r) => r,
                    Err(e) => {
                        gst::warning!(cat(), "[fluxsink] malformed SessionRequest: {}", e);
                        continue;
                    }
                }
            };

            gst::info!(
                cat(),
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
                gst::warning!(cat(), "[fluxsink] write SessionAccept: {}", e);
                continue;
            }
            if let Err(e) = send.finish() {
                gst::warning!(cat(), "[fluxsink] finish SessionAccept stream: {}", e);
                continue;
            }

            *session_id_store.lock().unwrap() = session_id.clone();
            gst::info!(
                cat(),
                "[fluxsink] SESSION_ACCEPT sent — session_id={} peer={}",
                session_id,
                conn.remote_address()
            );

            // M1: Send STREAM_ANNOUNCE on a reliable unidirectional QUIC stream
            // (spec §3.1 / §6.2).  One frame per channel × layer; PoC sends
            // a single H.265 channel 0, layer 0.
            {
                let announce = StreamAnnounce {
                    channel_id: 0,
                    layer_id: 0,
                    name: "FLUX_POC_VIDEO".into(),
                    content_type: "video".into(),
                    codec: "h265".into(),
                    group_id: 1,
                    sync_role: "master".into(),
                    frame_rate: "60/1".into(),
                    resolution: "1920x1080".into(),
                    hdr: "sdr".into(),
                    colorspace: "bt709".into(),
                    glb_texture_role: None,
                };
                let json = serde_json::to_vec(&announce).unwrap_or_default();
                // FLUX header for this stream frame
                let hdr = FluxHeader {
                    version: flux_framing::FLUX_VERSION,
                    frame_type: FrameType::StreamAnnounce,
                    flags: 0,
                    channel_id: 0,
                    layer: 0,
                    frag: 0,
                    group_id: 1,
                    group_timestamp_ns: now_ns(),
                    presentation_ts: 0,
                    capture_ts_ns_lo: 0,
                    payload_length: json.len() as u32,
                    fec_group: 0,
                    sequence_in_group: 0,
                };
                let mut msg = Vec::with_capacity(HEADER_SIZE + json.len());
                msg.extend_from_slice(&hdr.encode());
                msg.extend_from_slice(&json);

                match conn.open_uni().await {
                    Ok(mut uni_send) => {
                        if let Err(e) = uni_send.write_all(&msg).await {
                            gst::warning!(cat(), "[fluxsink] write STREAM_ANNOUNCE: {}", e);
                        } else if let Err(e) = uni_send.finish() {
                            gst::warning!(cat(), "[fluxsink] finish STREAM_ANNOUNCE stream: {}", e);
                        } else {
                            gst::info!(
                                cat(),
                                "[fluxsink] STREAM_ANNOUNCE sent — ch=0 layer=0 codec=h265 peer={}",
                                conn.remote_address()
                            );
                        }
                    }
                    Err(e) => {
                        gst::warning!(cat(), "[fluxsink] open_uni for STREAM_ANNOUNCE failed: {}", e);
                    }
                }
            }

            // A4: Increment generation counter before publishing the new
            // connection.  Any still-running datagram reader from the previous
            // client will see the changed generation on its next loop iteration
            // and exit, preventing two readers racing on conn_slot.
            let my_gen = reader_generation.fetch_add(1, Ordering::Release) + 1;

            // Reset the PTS high-water mark so the new session starts fresh.
            // render() uses last_sent_pts==0 as a proxy for "no IDR seen yet":
            // it drops all non-keyframe buffers until the first IDR arrives.
            // vtenc_h265 emits IDRs naturally every ~2 s; no FKU push needed.
            last_sent_pts.store(0, Ordering::Release);

            // Publish connection so render() can (conditionally) send frames.
            *conn_slot.lock().unwrap() = Some(conn.clone());

            // Spawn datagram reader task — passes its expected generation so it
            // self-terminates when a newer client connects.
            let conn_for_reader = conn.clone();
            let conn_slot_reader = conn_slot.clone();
            let bw_ref = bw_gov.clone();
            let probe_ref = probe_seq.clone();
            let cdbc_ref = cdbc_reports_received.clone();
            let ctrl_tx = flux_control_tx.clone();
            let gen_ref = reader_generation.clone();

            tokio::spawn(async move {
                run_datagram_reader(
                    conn_for_reader,
                    conn_slot_reader,
                    bw_ref,
                    probe_ref,
                    cdbc_ref,
                    ctrl_tx,
                    gen_ref,
                    my_gen,
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
        gen_ref: Arc<AtomicU32>,
        my_gen: u32,
    ) {
        loop {
            // A4: Exit immediately if a newer client has connected.
            if gen_ref.load(Ordering::Acquire) != my_gen {
                gst::debug!(
                    cat(),
                    "[fluxsink/dgram] reader gen={} superseded — exiting",
                    my_gen
                );
                break;
            }

            let datagram = match conn.read_datagram().await {
                Ok(d) => d,
                Err(e) => {
                    gst::warning!(cat(), "[fluxsink/dgram] read_datagram error: {}", e);
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
                            gst::debug!(
                                cat(),
                                "[fluxsink/ctrl] FLUX-C {:?} from {}",
                                ctrl.control_type,
                                conn.remote_address()
                            );
                            if let Some(tx) = flux_control_tx.lock().unwrap().as_ref() {
                                let _ = tx.try_send(ctrl);
                            }
                        }
                        None => {
                            gst::warning!(
                                cat(),
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
                            gst::warning!(cat(), "[fluxsink/cdbc] bad CDBC_FEEDBACK: {}", e);
                            continue;
                        }
                    };

                    gst::debug!(
                        cat(),
                        "[fluxsink/cdbc] CDBC_FEEDBACK — avail={}bps rx={}bps loss={:.1}% jitter={:.2}ms probe_result={}bps",
                        fb.avail_bps, fb.rx_bps, fb.loss_pct, fb.jitter_ms, fb.probe_result_bps,
                    );

                    cdbc_reports_received.fetch_add(1, Ordering::Relaxed);

                    let action = {
                        let mut gov = bw_gov.lock().unwrap();
                        gov.ingest(&fb)
                    };

                    gst::debug!(cat(), "[fluxsink/cdbc] BwGovernor → {:?}", action);

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
                            gst::debug!(
                                cat(),
                                "[fluxsink/cdbc] BANDWIDTH_PROBE #{} sent ({} bytes)",
                                seq, probe.probe_size
                            );
                        }
                        BwAction::AddLayer => {
                            gst::debug!(cat(), "[fluxsink/cdbc] ACTION: add enhancement layer");
                        }
                        BwAction::DropLayer => {
                            gst::debug!(cat(), "[fluxsink/cdbc] ACTION: drop top enhancement layer");
                        }
                        BwAction::EmergencyShed => {
                            gst::warning!(cat(), "[fluxsink/cdbc] ACTION: EMERGENCY — shed layers");
                        }
                        BwAction::EnableFec => {
                            gst::debug!(cat(), "[fluxsink/cdbc] ACTION: enable XOR Row FEC (loss > 5%)");
                        }
                        BwAction::EnableFecRS => {
                            gst::debug!(cat(), "[fluxsink/cdbc] ACTION: enable RS 2D FEC (loss > 15%)");
                        }
                        BwAction::RecoveryRampUp => {
                            gst::warning!(cat(), "[fluxsink/cdbc] ACTION: EMERGENCY recovery → RAMP_UP");
                        }
                        BwAction::Hold => {}
                    }
                }

                FrameType::Keepalive => {
                    gst::debug!(cat(), "[fluxsink] KEEPALIVE from {}", conn.remote_address());
                }

                _ => {}
            }
        }
    }
}
