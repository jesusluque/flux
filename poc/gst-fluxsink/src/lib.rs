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
    use flux_framing::{SessionAccept, DEFAULT_PORT};
    use gst::subclass::prelude::*;
    use gst_base::subclass::prelude::*;
    use std::io::Write;
    use std::net::{SocketAddr, TcpListener, UdpSocket};
    use std::sync::{Arc, Mutex};
    use std::thread;

    struct Inner {
        bind_addr: String,
        port: u16,
        udp_sock: Option<Arc<UdpSocket>>,
        client_addr: Arc<Mutex<Option<SocketAddr>>>,
    }

    impl Default for Inner {
        fn default() -> Self {
            Inner {
                bind_addr: "0.0.0.0".into(),
                port: DEFAULT_PORT,
                udp_sock: None,
                client_addr: Arc::new(Mutex::new(None)),
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
            let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
            let s = self.inner.lock().unwrap();
            let sock = s.udp_sock.as_ref().ok_or(gst::FlowError::Error)?.clone();

            // Resolve client address: set by TCP handshake, fallback to loopback
            let dst: SocketAddr = s
                .client_addr
                .lock()
                .unwrap()
                .unwrap_or_else(|| "127.0.0.1:7402".parse().unwrap());
            drop(s);

            sock.send_to(map.as_slice(), dst).map_err(|e| {
                eprintln!("[fluxsink] UDP send error: {}", e);
                gst::FlowError::Error
            })?;
            Ok(gst::FlowSuccess::Ok)
        }
    }

    fn run_control_listener(addr: &str, client_store: Arc<Mutex<Option<SocketAddr>>>) {
        let listener = match TcpListener::bind(addr) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[fluxsink] TCP bind {}: {}", addr, e);
                return;
            }
        };
        for stream in listener.incoming().flatten() {
            let peer = stream.peer_addr().unwrap();
            eprintln!("[fluxsink] SESSION from {}", peer);
            let accept = SessionAccept::default();
            let json = serde_json::to_vec(&accept).unwrap_or_default();
            let mut tcp = stream;
            let _ = tcp.write_all(&(json.len() as u32).to_be_bytes());
            let _ = tcp.write_all(&json);
            // Register the media return address (client sends datagrams from port 7402)
            let media_addr: SocketAddr = format!("{}:7402", peer.ip()).parse().unwrap();
            *client_store.lock().unwrap() = Some(media_addr);
            eprintln!("[fluxsink] client media addr = {}", media_addr);
        }
    }
}
