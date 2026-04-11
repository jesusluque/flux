//! `fluxcdbc` — GStreamer BaseTransform (passthrough) element.
//!
//! sink/src: application/x-flux  (passthrough — does NOT modify media)
//!
//! Observes incoming FLUX frames to:
//!   1. Measure inter-arrival jitter and datagram loss (sequence gap detection)
//!   2. Send CDBC_FEEDBACK JSON datagrams to the server at an adaptive interval
//!      (50 ms normal, 10 ms under loss — spec §5.1)
//!
//! CDBC_FEEDBACK is delivered as a QUIC Datagram (spec §4.4 table).  The caller
//! must inject a send callback via [`FluxCdbc::set_send_callback`] before the
//! element transitions to PLAYING.  The callback receives the fully-encoded
//! datagram bytes and is responsible for forwarding them over the QUIC connection
//! (e.g. `fluxsrc.send_datagram(pkt)`).  If no callback is set, feedback is
//! silently discarded (element still measures and exposes stats).
//!
//! Properties:
//!   cdbc-interval     — normal interval ms (default: 50)
//!   cdbc-min-interval — fast interval ms (default: 10)
//!
//! Exposes read-only stats properties: loss-pct, jitter-ms, rx-bps

use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;

// ─── Plugin registration ──────────────────────────────────────────────────────

gst::plugin_define!(
    fluxcdbc,
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
    FluxCdbc::register(plugin)?;
    Ok(())
}

// ─── Public wrapper ───────────────────────────────────────────────────────────

glib::wrapper! {
    pub struct FluxCdbc(ObjectSubclass<imp::FluxCdbc>)
        @extends gstreamer_base::BaseTransform, gst::Element, gst::Object;
}

impl FluxCdbc {
    pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
        gst::Element::register(
            Some(plugin),
            "fluxcdbc",
            gst::Rank::NONE,
            Self::static_type(),
        )
    }

    /// Register a callback that will be called with each encoded CDBC_FEEDBACK
    /// datagram.  Wire this to `fluxsrc.send_datagram()` so feedback is
    /// delivered over QUIC (spec §4.4 — QUIC Datagram delivery).
    ///
    /// # Example
    /// ```ignore
    /// let src = fluxsrc.clone();
    /// fluxcdbc.set_send_callback(move |pkt| { src.send_datagram(pkt); });
    /// ```
    pub fn set_send_callback<F>(&self, f: F)
    where
        F: Fn(Vec<u8>) + Send + Sync + 'static,
    {
        use gstreamer::subclass::prelude::ObjectSubclassExt;
        imp::FluxCdbc::from_obj(self).set_send_fn(Box::new(f));
    }
}

// ─── Implementation submodule ─────────────────────────────────────────────────

mod imp {
    use flux_framing::{now_ns, CdbcFeedback, FluxHeader, FrameType, HEADER_SIZE};
    use gst::glib;
    use gst::{Buffer, FlowError};
    use gstreamer as gst;
    use gstreamer::prelude::*;
    use gstreamer::subclass::prelude::*;
    use gstreamer_base as gst_base;
    use gstreamer_base::prelude::*;
    use gstreamer_base::subclass::prelude::*;
    use serde_json;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    // Type alias for the send callback (QUIC datagram delivery).
    type SendFn = Box<dyn Fn(Vec<u8>) + Send + Sync + 'static>;

    // ─── Measurement state ────────────────────────────────────────────────

    struct Measurement {
        // Configuration
        interval_ms: u64,
        min_interval_ms: u64,

        // Running stats
        last_send: Instant,
        last_arrival: Option<Instant>,
        jitter_ms: f64,
        last_seq: Option<u32>,
        lost_count: u64,
        recv_count: u64,
        byte_count: u64,
        bw_window_start: Instant,

        // Cumulative stats (survive window resets)
        reports_sent: u64,
        lost_total: u64,

        // Monotonic sequence counter for CDBC_FEEDBACK datagrams
        feedback_seq: u32,

        // Last computed feedback (for property reads)
        last_fb: CdbcFeedback,
    }

    impl Default for Measurement {
        fn default() -> Self {
            Measurement {
                interval_ms: 50,
                min_interval_ms: 10,
                last_send: Instant::now(),
                last_arrival: None,
                jitter_ms: 0.0,
                last_seq: None,
                lost_count: 0,
                recv_count: 0,
                byte_count: 0,
                bw_window_start: Instant::now(),
                reports_sent: 0,
                lost_total: 0,
                feedback_seq: 0,
                last_fb: CdbcFeedback::default(),
            }
        }
    }

    // ─── GObject subclass ─────────────────────────────────────────────────

    pub struct FluxCdbc {
        meas: Mutex<Measurement>,
        /// QUIC-datagram send callback; None until set_send_callback() is called.
        send_fn: Arc<Mutex<Option<SendFn>>>,
    }

    impl Default for FluxCdbc {
        fn default() -> Self {
            FluxCdbc {
                meas: Mutex::new(Measurement::default()),
                send_fn: Arc::new(Mutex::new(None)),
            }
        }
    }

    impl FluxCdbc {
        /// Called from the public wrapper to install the QUIC send callback.
        pub(super) fn set_send_fn(&self, f: SendFn) {
            *self.send_fn.lock().unwrap() = Some(f);
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FluxCdbc {
        const NAME: &'static str = "FluxCdbc";
        type Type = super::FluxCdbc;
        type ParentType = gst_base::BaseTransform;
    }

    impl ObjectImpl for FluxCdbc {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPS: std::sync::OnceLock<Vec<glib::ParamSpec>> = std::sync::OnceLock::new();
            PROPS.get_or_init(|| {
                vec![
                    glib::ParamSpecUInt64::builder("cdbc-interval")
                        .nick("CDBC interval ms")
                        .blurb("Normal feedback interval in milliseconds (STABLE/PROBE/RAMP_UP)")
                        .default_value(50)
                        .build(),
                    glib::ParamSpecUInt64::builder("cdbc-min-interval")
                        .nick("CDBC min interval ms")
                        .blurb("Fast feedback interval in milliseconds (RAMP_DOWN/EMERGENCY)")
                        .default_value(10)
                        .build(),
                    // Read-only stats
                    glib::ParamSpecDouble::builder("loss-pct")
                        .nick("Loss %")
                        .blurb("Measured datagram loss percentage")
                        .read_only()
                        .build(),
                    glib::ParamSpecDouble::builder("jitter-ms")
                        .nick("Jitter ms")
                        .blurb("Measured inter-arrival jitter in milliseconds")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt64::builder("rx-bps")
                        .nick("Rx bps")
                        .blurb("Measured receive bitrate in bits per second")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt64::builder("reports-sent")
                        .nick("Reports sent")
                        .blurb("Total CDBC_FEEDBACK datagrams sent to the server")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt64::builder("datagrams-lost-total")
                        .nick("Datagrams lost total")
                        .blurb("Cumulative datagram loss count across all measurement windows")
                        .read_only()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let mut m = self.meas.lock().unwrap();
            match pspec.name() {
                "cdbc-interval" => m.interval_ms = value.get::<u64>().unwrap(),
                "cdbc-min-interval" => m.min_interval_ms = value.get::<u64>().unwrap(),
                _ => {}
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let m = self.meas.lock().unwrap();
            match pspec.name() {
                "cdbc-interval" => m.interval_ms.to_value(),
                "cdbc-min-interval" => m.min_interval_ms.to_value(),
                "loss-pct" => m.last_fb.loss_pct.to_value(),
                "jitter-ms" => m.last_fb.jitter_ms.to_value(),
                "rx-bps" => m.last_fb.rx_bps.to_value(),
                "reports-sent" => m.reports_sent.to_value(),
                "datagrams-lost-total" => (m.lost_total + m.lost_count).to_value(),
                _ => unimplemented!(),
            }
        }
    }

    impl GstObjectImpl for FluxCdbc {}

    impl ElementImpl for FluxCdbc {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static META: std::sync::OnceLock<gst::subclass::ElementMetadata> =
                std::sync::OnceLock::new();
            Some(META.get_or_init(|| {
                gst::subclass::ElementMetadata::new(
                    "FLUX CDBC",
                    "Network/FLUX",
                    "Client-Driven Bandwidth Control — measures loss/jitter, sends adaptive CDBC_FEEDBACK",
                    "LUCAB Media Technology",
                )
            }))
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            static PADS: std::sync::OnceLock<Vec<gst::PadTemplate>> = std::sync::OnceLock::new();
            PADS.get_or_init(|| {
                let caps = gst::Caps::builder("application/x-flux").build();
                vec![
                    gst::PadTemplate::new(
                        "sink",
                        gst::PadDirection::Sink,
                        gst::PadPresence::Always,
                        &caps,
                    )
                    .unwrap(),
                    gst::PadTemplate::new(
                        "src",
                        gst::PadDirection::Src,
                        gst::PadPresence::Always,
                        &caps,
                    )
                    .unwrap(),
                ]
            })
        }
    }

    impl BaseTransformImpl for FluxCdbc {
        const MODE: gst_base::subclass::BaseTransformMode =
            gst_base::subclass::BaseTransformMode::Both;
        const PASSTHROUGH_ON_SAME_CAPS: bool = true;
        const TRANSFORM_IP_ON_PASSTHROUGH: bool = true;

        /// Answer LATENCY queries locally so that the compositor does not log
        /// "Latency query failed" during startup, before fluxdemux has created
        /// its dynamic `media_0` pad and linked it to fluxcdbc.sink.
        ///
        /// Without this override, BaseTransform's default query handler forwards
        /// the query upstream through the sink pad, which has no peer at startup
        /// → the query fails → compositor logs a warning and uses latency=0.
        fn query(&self, direction: gst::PadDirection, query: &mut gst::QueryRef) -> bool {
            if direction == gst::PadDirection::Src {
                if let gst::QueryViewMut::Latency(q) = query.view_mut() {
                    q.set(
                        true,
                        gst::ClockTime::from_mseconds(60),
                        gst::ClockTime::NONE,
                    );
                    return true;
                }
            }
            gstreamer_base::subclass::base_transform::BaseTransformImplExt::parent_query(
                self, direction, query,
            )
        }

        fn start(&self) -> Result<(), gst::ErrorMessage> {
            // Enable passthrough mode: we only observe, never modify buffers.
            self.obj().set_passthrough(true);
            Ok(())
        }

        fn stop(&self) -> Result<(), gst::ErrorMessage> {
            Ok(())
        }

        fn transform_ip_passthrough(&self, buf: &Buffer) -> Result<gst::FlowSuccess, FlowError> {
            let map = match buf.map_readable() {
                Ok(m) => m,
                Err(_) => return Ok(gst::FlowSuccess::Ok),
            };
            let data = map.as_slice();

            if data.len() < HEADER_SIZE {
                return Ok(gst::FlowSuccess::Ok);
            }

            let hdr = match FluxHeader::decode(data) {
                Some(h) => h,
                None => return Ok(gst::FlowSuccess::Ok),
            };

            let now = Instant::now();
            let mut m = self.meas.lock().unwrap();

            // Only account MEDIA_DATA frames in stats
            if hdr.frame_type == FrameType::MediaData {
                // ── Jitter (RFC 3550 §A.8) ──────────────────────────────────
                if let Some(prev) = m.last_arrival {
                    let d = now.duration_since(prev).as_secs_f64() * 1000.0;
                    m.jitter_ms += (d - m.jitter_ms) / 16.0;
                }
                m.last_arrival = Some(now);

                // ── Sequence gap loss detection ──────────────────────────────
                if let Some(last_seq) = m.last_seq {
                    let expected = last_seq.wrapping_add(1);
                    if hdr.sequence_in_group != expected {
                        let gap = hdr.sequence_in_group.wrapping_sub(expected);
                        if gap < 1000 {
                            m.lost_count += gap as u64;
                        }
                    }
                }
                m.last_seq = Some(hdr.sequence_in_group);
                m.recv_count += 1;
                m.byte_count += data.len() as u64;
            }

            // ── Compute loss_pct ─────────────────────────────────────────────
            let total = m.recv_count + m.lost_count;
            let loss_pct = if total > 0 {
                (m.lost_count as f64 / total as f64) * 100.0
            } else {
                0.0
            };

            // ── Choose adaptive interval (spec §5.1) ─────────────────────────
            let interval_ms = if loss_pct > 0.5 {
                m.min_interval_ms
            } else {
                m.interval_ms
            };

            // ── Compute rx_bps over the measurement window ──────────────────
            let window_s = m.bw_window_start.elapsed().as_secs_f64();
            let rx_bps = if window_s > 0.1 {
                ((m.byte_count * 8) as f64 / window_s) as u64
            } else {
                0u64
            };

            // ── Send CDBC_FEEDBACK datagram ──────────────────────────────────
            if m.last_send.elapsed().as_millis() as u64 >= interval_ms {
                let fb = CdbcFeedback {
                    ts_ns: now_ns(),
                    rx_bps,
                    avail_bps: rx_bps,
                    rtt_ms: 0.0,
                    loss_pct,
                    jitter_ms: m.jitter_ms,
                    fps_actual: 0.0,
                    datagram_drop_count: m.lost_count,
                    probe_result_bps: 0,
                    preferred_max_layer: None,
                    per_channel: None,
                };

                m.last_fb = fb.clone();

                // Grab the send callback (if installed) under a brief lock.
                let send_fn_guard = self.send_fn.lock().unwrap();
                if let Some(ref send_fn) = *send_fn_guard {
                    let json = serde_json::to_vec(&fb).unwrap_or_default();
                    let seq = m.feedback_seq;
                    m.feedback_seq = m.feedback_seq.wrapping_add(1);
                    let fb_hdr = FluxHeader {
                        version: flux_framing::FLUX_VERSION,
                        frame_type: FrameType::CdbcFeedbackT,
                        flags: 0,
                        channel_id: 0,
                        layer: 0,
                        frag: 0,
                        group_id: 0,
                        group_timestamp_ns: now_ns(),
                        presentation_ts: 0,
                        capture_ts_ns_lo: 0,
                        payload_length: json.len() as u32,
                        fec_group: 0,
                        sequence_in_group: seq,
                    };
                    let mut pkt = Vec::with_capacity(HEADER_SIZE + json.len());
                    pkt.extend_from_slice(&fb_hdr.encode());
                    pkt.extend_from_slice(&json);

                    send_fn(pkt);
                    m.reports_sent += 1;

                    gst::debug!(
                        gst::CAT_DEFAULT,
                        "CDBC_FEEDBACK seq={} | loss={:.1}% jitter={:.2}ms rx={}bps",
                        seq,
                        loss_pct,
                        m.jitter_ms,
                        rx_bps
                    );
                }

                m.last_send = now;
                m.lost_total += m.lost_count;
                m.byte_count = 0;
                m.recv_count = 0;
                m.lost_count = 0;
                m.bw_window_start = now;
            }

            Ok(gst::FlowSuccess::Ok)
        }
    }
}
