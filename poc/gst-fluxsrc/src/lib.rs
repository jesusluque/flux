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
        now_ns, FluxHeader, KeepalivePayload, SessionRequest, DEFAULT_PORT, HEADER_SIZE,
    };
    use gst::glib;
    use gst::FlowError;
    use gstreamer as gst;
    use gstreamer::prelude::*;
    use gstreamer::subclass::prelude::*;
    use gstreamer_base as gst_base;
    use gstreamer_base::subclass::prelude::*;
    use serde_json;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream, UdpSocket};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    // ─── State ────────────────────────────────────────────────────────────────

    struct Inner {
        server_addr: String,
        port: u16,
        udp_sock: Option<Arc<UdpSocket>>,
        server_udp: Option<SocketAddr>,
        session_id: String,
        keepalive_seq: u32,
        last_ka: Instant,
        ka_interval: Duration,
    }

    impl Default for Inner {
        fn default() -> Self {
            Inner {
                server_addr: "127.0.0.1".into(),
                port: DEFAULT_PORT,
                udp_sock: None,
                server_udp: None,
                session_id: "poc-session-001".into(),
                keepalive_seq: 0,
                last_ka: Instant::now(),
                ka_interval: Duration::from_millis(1000),
            }
        }
    }

    // ─── GObject subclass ─────────────────────────────────────────────────────

    #[derive(Default)]
    pub struct FluxSrc {
        inner: Mutex<Inner>,
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
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let mut s = self.inner.lock().unwrap();
            match pspec.name() {
                "address" => s.server_addr = value.get::<String>().unwrap(),
                "port" => s.port = value.get::<u32>().unwrap() as u16,
                _ => {}
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let s = self.inner.lock().unwrap();
            match pspec.name() {
                "address" => s.server_addr.to_value(),
                "port" => (s.port as u32).to_value(),
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

            // Bind local UDP socket for receiving media datagrams
            let local_addr = "0.0.0.0:7402";
            let sock = UdpSocket::bind(local_addr).map_err(|e| {
                gst::error_msg!(
                    gst::ResourceError::OpenRead,
                    ["bind UDP {}: {}", local_addr, e]
                )
            })?;
            sock.set_read_timeout(Some(Duration::from_millis(500))).ok();

            // Store server UDP address (media port = port)
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

            // TCP SESSION handshake on server control port (port + 1)
            let ctrl_addr = format!("{}:{}", s.server_addr, s.port + 1);
            match TcpStream::connect_timeout(&ctrl_addr.parse().unwrap(), Duration::from_secs(5)) {
                Ok(mut tcp) => {
                    let req = SessionRequest::default();
                    let json = serde_json::to_vec(&req).unwrap();
                    let _ = tcp.write_all(&(json.len() as u32).to_be_bytes());
                    let _ = tcp.write_all(&json);

                    let mut len_buf = [0u8; 4];
                    if tcp.read_exact(&mut len_buf).is_ok() {
                        let len = u32::from_be_bytes(len_buf) as usize;
                        let mut body = vec![0u8; len];
                        let _ = tcp.read_exact(&mut body);
                        gst::info!(
                            gst::CAT_DEFAULT,
                            "FluxSrc: SESSION_ACCEPT received ({} bytes)",
                            len
                        );
                    }
                }
                Err(e) => {
                    gst::warning!(
                        gst::CAT_DEFAULT,
                        "FluxSrc: TCP handshake to {} failed: {} (continuing)",
                        ctrl_addr,
                        e
                    );
                }
            }

            gst::info!(
                gst::CAT_DEFAULT,
                "FluxSrc: listening on {} for datagrams from {}",
                local_addr,
                server_udp
            );
            Ok(())
        }

        fn stop(&self) -> Result<(), gst::ErrorMessage> {
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

            let (sock_clone, _server_udp, _session_id) = {
                let mut s = self.inner.lock().unwrap();
                let sock = s.udp_sock.as_ref().ok_or(FlowError::Error)?.clone();
                let server_udp = s.server_udp;
                let sid = s.session_id.clone();

                // Send KEEPALIVE if interval elapsed
                if s.last_ka.elapsed() >= s.ka_interval {
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
                        s.last_ka = Instant::now();
                    }
                }

                (sock.clone(), server_udp, sid)
            };

            // Blocking receive (with 500 ms timeout set in start())
            match sock_clone.recv_from(&mut recv_buf) {
                Ok((n, _from)) => {
                    let data = &recv_buf[..n];
                    let mut buf = gst::Buffer::with_size(n).map_err(|_| FlowError::Error)?;
                    {
                        let buf_ref = buf.get_mut().unwrap();
                        let mut map = buf_ref.map_writable().map_err(|_| FlowError::Error)?;
                        map[..n].copy_from_slice(data);
                    }
                    Ok(gst_base::subclass::base_src::CreateSuccess::NewBuffer(buf))
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    let buf = gst::Buffer::with_size(0).map_err(|_| FlowError::Error)?;
                    Ok(gst_base::subclass::base_src::CreateSuccess::NewBuffer(buf))
                }
                Err(e) => {
                    gst::warning!(gst::CAT_DEFAULT, "UDP recv error: {}", e);
                    Err(FlowError::Error)
                }
            }
        }
    }
}
