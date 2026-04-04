//! `fluxsink` — GStreamer BaseSink element (server role).
//!
//! sink: application/x-flux
//! Sends FLUX frames over raw UDP datagrams (crypto_none LAN mode, spec §2.2).
//! A TCP thread on port+1 handles the SESSION handshake.

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
}

mod imp {
    use super::*;
    use flux_framing::{
        now_ns, BandwidthProbe, BwAction, BwGovernor, CdbcFeedback, FluxHeader, FrameType,
        SessionAccept, SessionRequest, DEFAULT_PORT, HEADER_SIZE,
    };
    use gst::subclass::prelude::*;
    use gst_base::subclass::prelude::*;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener, UdpSocket};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;

    struct Inner {
        bind_addr: String,
        port: u16,
        udp_sock: Option<Arc<UdpSocket>>,
        client_addr: Arc<Mutex<Option<SocketAddr>>>,
        /// Shared BW Governor; written by the CDBC reader thread.
        bw_gov: Arc<Mutex<BwGovernor>>,
        /// Monotonically increasing probe sequence number.
        probe_seq: Arc<AtomicU32>,
    }

    impl Default for Inner {
        fn default() -> Self {
            Inner {
                bind_addr: "0.0.0.0".into(),
                port: DEFAULT_PORT,
                udp_sock: None,
                client_addr: Arc::new(Mutex::new(None)),
                bw_gov: Arc::new(Mutex::new(BwGovernor::new())),
                probe_seq: Arc::new(AtomicU32::new(1)),
            }
        }
    }

    #[derive(Default)]
    pub struct FluxSink {
        inner: Mutex<Inner>,
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
                        .blurb("UDP bind address")
                        .default_value(Some("0.0.0.0"))
                        .build(),
                    glib::ParamSpecUInt::builder("port")
                        .nick("Port")
                        .blurb("FLUX media UDP port")
                        .default_value(DEFAULT_PORT as u32)
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
                    "Sends FLUX frames over UDP datagrams (crypto_none server)",
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
            let addr = format!("{}:{}", s.bind_addr, s.port);
            let sock = UdpSocket::bind(&addr).map_err(|e| {
                gst::error_msg!(gst::ResourceError::OpenWrite, ["bind UDP {}: {}", addr, e])
            })?;
            sock.set_nonblocking(false).ok();
            let sock = Arc::new(sock);
            s.udp_sock = Some(sock.clone());

            // Spawn TCP control thread for SESSION handshake
            let ctrl_addr = format!("{}:{}", s.bind_addr, s.port + 1);
            let client_ref = s.client_addr.clone();
            thread::spawn(move || {
                run_control_listener(&ctrl_addr, client_ref);
            });

            // Spawn CDBC feedback reader thread (reads from the same UDP socket)
            let cdbc_sock = sock.clone();
            let bw_ref = s.bw_gov.clone();
            let probe_seq_ref = s.probe_seq.clone();
            let client_addr_ref = s.client_addr.clone();
            thread::spawn(move || {
                run_cdbc_reader(cdbc_sock, bw_ref, probe_seq_ref, client_addr_ref);
            });

            eprintln!(
                "[fluxsink] UDP bound on {}  |  TCP control on :{}",
                addr,
                s.port + 1
            );
            Ok(())
        }

        fn stop(&self) -> Result<(), gst::ErrorMessage> {
            let mut s = self.inner.lock().unwrap();
            s.udp_sock = None;
            Ok(())
        }

        fn render(&self, buffer: &gst::Buffer) -> Result<gst::FlowSuccess, gst::FlowError> {
            use flux_framing::{fragment_encode, FluxHeader, HEADER_SIZE};

            let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
            let data = map.as_slice();

            // The buffer is a complete FLUX frame: 32-byte header + H.265 payload.
            // Parse the header so we can re-use its fields when fragmenting.
            let hdr = FluxHeader::decode(data).ok_or_else(|| {
                eprintln!("[fluxsink] malformed FLUX header");
                gst::FlowError::Error
            })?;
            let payload = &data[HEADER_SIZE..];

            let s = self.inner.lock().unwrap();
            let sock = s.udp_sock.as_ref().ok_or(gst::FlowError::Error)?.clone();
            let port = s.port;

            // Resolve client address: set by TCP handshake, fallback to loopback
            let dst: SocketAddr = s
                .client_addr
                .lock()
                .unwrap()
                .unwrap_or_else(|| format!("127.0.0.1:{}", port + 2).parse().unwrap());
            drop(s);

            for datagram in fragment_encode(&hdr, payload) {
                eprintln!(
                    "[fluxsink] sending datagram {} bytes to {}",
                    datagram.len(),
                    dst
                );
                sock.send_to(&datagram, dst).map_err(|e| {
                    eprintln!("[fluxsink] UDP send error: {}", e);
                    gst::FlowError::Error
                })?;
            }
            Ok(gst::FlowSuccess::Ok)
        }
    }

    /// Reads incoming UDP datagrams on the media port, dispatches CDBC_FEEDBACK
    /// frames to the BW Governor, and sends BANDWIDTH_PROBE when requested.
    fn run_cdbc_reader(
        sock: Arc<UdpSocket>,
        bw_gov: Arc<Mutex<BwGovernor>>,
        probe_seq: Arc<AtomicU32>,
        client_addr: Arc<Mutex<Option<SocketAddr>>>,
    ) {
        // Allow the socket to be shared: clone a reference for reading.
        // The socket is blocking; recv_from will wait for the next datagram.
        let mut buf = [0u8; 65536];
        loop {
            let (n, from) = match sock.recv_from(&mut buf) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[fluxsink/cdbc] recv error: {}", e);
                    break;
                }
            };
            let data = &buf[..n];

            if data.len() < HEADER_SIZE {
                continue;
            }

            let hdr = match FluxHeader::decode(data) {
                Some(h) => h,
                None => continue,
            };

            if hdr.frame_type != FrameType::CdbcFeedbackT {
                // Not a CDBC frame — ignore (media data also arrives here from
                // loopback tests, but the loopback sends to port+2, not port)
                continue;
            }

            let body = &data[HEADER_SIZE..];
            let fb: CdbcFeedback = match serde_json::from_slice(body) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("[fluxsink/cdbc] bad CDBC_FEEDBACK from {}: {}", from, e);
                    continue;
                }
            };

            eprintln!(
                "[fluxsink/cdbc] CDBC_FEEDBACK from {} — avail={}bps rx={}bps loss={:.1}% jitter={:.2}ms probe_result={}bps",
                from,
                fb.avail_bps,
                fb.rx_bps,
                fb.loss_pct,
                fb.jitter_ms,
                fb.probe_result_bps,
            );

            // Run the BW Governor state machine
            let action = {
                let mut gov = bw_gov.lock().unwrap();
                gov.ingest(&fb)
            };

            eprintln!("[fluxsink/cdbc] BwGovernor → {:?}", action);

            match action {
                BwAction::SendProbe => {
                    // Build and send a BANDWIDTH_PROBE datagram
                    let dst = match *client_addr.lock().unwrap() {
                        Some(a) => a,
                        None => {
                            eprintln!("[fluxsink/cdbc] no client addr for probe");
                            continue;
                        }
                    };
                    let seq = probe_seq.fetch_add(1, Ordering::Relaxed);
                    let probe = BandwidthProbe {
                        ts_ns: now_ns(),
                        probe_seq: seq,
                        probe_size: 1200, // ~1 MTU of probe payload
                    };
                    let payload = serde_json::to_vec(&probe).unwrap_or_default();
                    // Pad to probe_size so receiver can measure arrival bandwidth
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
                    let _ = sock.send_to(&pkt, dst);
                    eprintln!(
                        "[fluxsink/cdbc] BANDWIDTH_PROBE #{} → {} ({} bytes)",
                        seq,
                        dst,
                        pkt.len()
                    );
                }
                BwAction::AddLayer => {
                    eprintln!("[fluxsink/cdbc] ACTION: add enhancement layer (bitrate headroom available)");
                }
                BwAction::DropLayer => {
                    eprintln!("[fluxsink/cdbc] ACTION: drop top enhancement layer");
                }
                BwAction::EmergencyShed => {
                    eprintln!("[fluxsink/cdbc] ACTION: EMERGENCY — shed enhancement layers + disable monitor");
                }
                BwAction::EnableFec => {
                    eprintln!(
                        "[fluxsink/cdbc] ACTION: enable XOR Row FEC on base layer (loss > 5%)"
                    );
                }
                BwAction::EnableFecRS => {
                    eprintln!("[fluxsink/cdbc] ACTION: enable Reed-Solomon 2D FEC + IDR-only (loss > 15%)");
                }
                BwAction::RecoveryRampUp => {
                    eprintln!("[fluxsink/cdbc] ACTION: EMERGENCY recovery complete → RAMP_UP");
                }
                BwAction::Hold => {}
            }
        }
    }

    fn run_control_listener(addr: &str, client_store: Arc<Mutex<Option<SocketAddr>>>) {
        /// Monotonically increasing session counter, shared across all connections.
        static SESSION_COUNTER: AtomicU32 = AtomicU32::new(1);

        let listener = match TcpListener::bind(addr) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[fluxsink] TCP bind {}: {}", addr, e);
                return;
            }
        };
        for stream in listener.incoming().flatten() {
            let peer = stream.peer_addr().unwrap();
            eprintln!("[fluxsink] SESSION TCP connect from {}", peer);
            let mut tcp = stream;

            // ── Read SessionRequest ───────────────────────────────────────────
            let req: SessionRequest = {
                let mut len_buf = [0u8; 4];
                if tcp.read_exact(&mut len_buf).is_err() {
                    eprintln!(
                        "[fluxsink] failed to read SessionRequest length from {}",
                        peer
                    );
                    continue;
                }
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut body = vec![0u8; len];
                if tcp.read_exact(&mut body).is_err() {
                    eprintln!(
                        "[fluxsink] failed to read SessionRequest body from {}",
                        peer
                    );
                    continue;
                }
                match serde_json::from_slice(&body) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("[fluxsink] malformed SessionRequest from {}: {}", peer, e);
                        continue;
                    }
                }
            };

            eprintln!(
                "[fluxsink] SESSION_REQUEST from {} — client_id={} codec={:?} max_fps={} cdbc_interval_ms={} media_port={}",
                peer,
                req.client_id,
                req.codec_support,
                req.max_fps,
                req.cdbc_interval_ms,
                req.media_port,
            );

            // ── Build and send SessionAccept ──────────────────────────────────
            let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
            let session_id = format!("sess-{}-{}", now_ns() / 1_000_000, counter);

            let accept = SessionAccept {
                session_id: session_id.clone(),
                ..SessionAccept::default()
            };
            let json = serde_json::to_vec(&accept).unwrap_or_default();
            let _ = tcp.write_all(&(json.len() as u32).to_be_bytes());
            let _ = tcp.write_all(&json);

            // ── Register client media address from negotiated port ────────────
            let media_addr: SocketAddr =
                format!("{}:{}", peer.ip(), req.media_port).parse().unwrap();
            *client_store.lock().unwrap() = Some(media_addr);

            eprintln!(
                "[fluxsink] SESSION_ACCEPT sent — session_id={} client media addr={}",
                session_id, media_addr
            );
        }
    }
}
