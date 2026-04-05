//! `fluxdeframer` — GStreamer BaseTransform element.
//!
//! sink: application/x-flux  →  src: video/x-h265 (byte-stream, au)
//!
//! The server forces h265parse to output byte-stream (SPS/PPS inlined), so the
//! payload in every FLUX MediaData frame is a self-contained byte-stream AU.
//!
//! When QUIC path MTU forces fragmentation (frag field 0x1..0xF), this element
//! accumulates all fragments for one AU (same sequence_in_group) and pushes the
//! reassembled payload only when the last fragment (frag=0xF) arrives.
//! Unfragmented datagrams (frag=0x0) are pushed immediately.
//!
//! Implementation note: fragment reassembly requires allocating a buffer whose
//! size is only known after all fragments arrive.  We use the
//! `submit_input_buffer` + `generate_output` pattern (rather than `transform`)
//! so we control output buffer allocation.

use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_base as gst_base;

gst::plugin_define!(
    fluxdeframer,
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
    FluxDeframer::register(plugin)?;
    Ok(())
}

glib::wrapper! {
    pub struct FluxDeframer(ObjectSubclass<imp::FluxDeframer>)
        @extends gst_base::BaseTransform, gst::Element, gst::Object;
}

impl FluxDeframer {
    pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
        gst::Element::register(
            Some(plugin),
            "fluxdeframer",
            gst::Rank::NONE,
            Self::static_type(),
        )
    }
}

mod imp {
    use super::*;
    use flux_framing::{reconstruct_capture_ts, FluxHeader, FrameType, FLUX_VERSION, HEADER_SIZE};
    use gst::subclass::prelude::*;
    use gst_base::subclass::base_transform::GenerateOutputSuccess;
    use gst_base::subclass::prelude::*;
    use std::sync::Mutex;

    // ── Fragment reassembly state ─────────────────────────────────────────────

    struct FragState {
        /// sequence_in_group of the AU being assembled (None = idle).
        seq: Option<u32>,
        /// Accumulated payload bytes from all received fragments.
        payload: Vec<u8>,
        /// Header from the first fragment (carries timestamps, flags).
        hdr: Option<FluxHeader>,
        /// A fully reassembled AU ready to be pushed, or None.
        ready: Option<(Vec<u8>, FluxHeader)>,
    }

    impl Default for FragState {
        fn default() -> Self {
            FragState {
                seq: None,
                payload: Vec::new(),
                hdr: None,
                ready: None,
            }
        }
    }

    impl FragState {
        fn reset_assembly(&mut self) {
            self.seq = None;
            self.payload.clear();
            self.hdr = None;
        }
    }

    // ── Element ───────────────────────────────────────────────────────────────

    #[derive(Default)]
    pub struct FluxDeframer {
        state: Mutex<FragState>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FluxDeframer {
        const NAME: &'static str = "FluxDeframer";
        type Type = super::FluxDeframer;
        type ParentType = gst_base::BaseTransform;
    }

    impl ObjectImpl for FluxDeframer {}
    impl GstObjectImpl for FluxDeframer {}

    impl ElementImpl for FluxDeframer {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static META: std::sync::OnceLock<gst::subclass::ElementMetadata> =
                std::sync::OnceLock::new();
            Some(META.get_or_init(|| {
                gst::subclass::ElementMetadata::new(
                    "FLUX Deframer",
                    "Transform/Network/FLUX",
                    "Strips FLUX header, reassembles QUIC fragments, emits raw H.265",
                    "LUCAB Media Technology",
                )
            }))
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            static PADS: std::sync::OnceLock<Vec<gst::PadTemplate>> = std::sync::OnceLock::new();
            PADS.get_or_init(|| {
                vec![
                    gst::PadTemplate::new(
                        "sink",
                        gst::PadDirection::Sink,
                        gst::PadPresence::Always,
                        &gst::Caps::builder("application/x-flux").build(),
                    )
                    .unwrap(),
                    gst::PadTemplate::new(
                        "src",
                        gst::PadDirection::Src,
                        gst::PadPresence::Always,
                        &gst::Caps::builder("video/x-h265")
                            .field("stream-format", "byte-stream")
                            .field("alignment", "au")
                            .build(),
                    )
                    .unwrap(),
                ]
            })
        }
    }

    impl BaseTransformImpl for FluxDeframer {
        const MODE: gst_base::subclass::BaseTransformMode =
            gst_base::subclass::BaseTransformMode::NeverInPlace;
        const PASSTHROUGH_ON_SAME_CAPS: bool = false;
        const TRANSFORM_IP_ON_PASSTHROUGH: bool = false;

        /// Called for every incoming buffer.  We accumulate fragment state here.
        /// Returns `BASE_TRANSFORM_FLOW_DROPPED` while still accumulating, or
        /// `FlowSuccess::Ok` when a complete AU has been placed in `state.ready`
        /// (which causes GStreamer to call `generate_output`).
        fn submit_input_buffer(
            &self,
            is_discont: bool,
            inbuf: gst::Buffer,
        ) -> Result<gst::FlowSuccess, gst::FlowError> {
            let _ = is_discont;

            let map = inbuf.map_readable().map_err(|_| gst::FlowError::Error)?;
            let data = map.as_slice();

            if data.len() < HEADER_SIZE {
                // Too short — discard silently
                return Ok(gst_base::BASE_TRANSFORM_FLOW_DROPPED);
            }

            let hdr = match FluxHeader::decode(data) {
                Some(h) if h.version == FLUX_VERSION => h,
                _ => return Ok(gst_base::BASE_TRANSFORM_FLOW_DROPPED),
            };

            // Non-media frames are discarded (keepalive etc. handled upstream)
            if hdr.frame_type != FrameType::MediaData {
                return Ok(gst_base::BASE_TRANSFORM_FLOW_DROPPED);
            }

            // Skip optional metadata block (spec §4.4)
            let payload_start = if hdr.has_metadata() && data.len() >= HEADER_SIZE + 2 {
                let meta_len =
                    u16::from_be_bytes([data[HEADER_SIZE], data[HEADER_SIZE + 1]]) as usize;
                HEADER_SIZE + 2 + meta_len
            } else {
                HEADER_SIZE
            };

            let chunk = &data[payload_start..];

            let mut st = self.state.lock().unwrap();

            match hdr.frag {
                0x0 => {
                    // Unfragmented — emit immediately
                    if st.seq.is_some() {
                        eprintln!(
                            "[fluxdeframer] unfragmented seq={} arrived while seq={:?} in progress — discarding old",
                            hdr.sequence_in_group, st.seq
                        );
                        st.reset_assembly();
                    }
                    st.ready = Some((chunk.to_vec(), hdr));
                }
                0xF => {
                    // Last fragment
                    if st.seq != Some(hdr.sequence_in_group) {
                        eprintln!(
                            "[fluxdeframer] last-frag seq={} but assembling={:?} — discarding",
                            hdr.sequence_in_group, st.seq
                        );
                        st.reset_assembly();
                        return Ok(gst_base::BASE_TRANSFORM_FLOW_DROPPED);
                    }
                    st.payload.extend_from_slice(chunk);
                    let payload = std::mem::take(&mut st.payload);
                    let first_hdr = st.hdr.take().unwrap();
                    st.reset_assembly();
                    st.ready = Some((payload, first_hdr));
                }
                n => {
                    // Middle / first fragment (0x1..0xE)
                    if n == 1 || st.seq.is_none() {
                        if st.seq.is_some() && st.seq != Some(hdr.sequence_in_group) {
                            eprintln!(
                                "[fluxdeframer] new seq={} while seq={:?} incomplete — discarding old",
                                hdr.sequence_in_group, st.seq
                            );
                        }
                        st.reset_assembly();
                        st.seq = Some(hdr.sequence_in_group);
                        st.hdr = Some(hdr.clone());
                    } else if st.seq != Some(hdr.sequence_in_group) {
                        eprintln!(
                            "[fluxdeframer] mid-frag seq={} != assembling={:?} — discarding",
                            hdr.sequence_in_group, st.seq
                        );
                        st.reset_assembly();
                        return Ok(gst_base::BASE_TRANSFORM_FLOW_DROPPED);
                    }
                    st.payload.extend_from_slice(chunk);
                    // Not ready yet — signal no output needed
                    return Ok(gst_base::BASE_TRANSFORM_FLOW_DROPPED);
                }
            }

            // AU is ready — tell GStreamer to call generate_output()
            Ok(gst::FlowSuccess::Ok)
        }

        /// Called after `submit_input_buffer` returned `Buffer`.  At this point
        /// `state.ready` holds the fully reassembled AU; allocate a new buffer
        /// of exactly the right size and fill it.
        fn generate_output(&self) -> Result<GenerateOutputSuccess, gst::FlowError> {
            let (payload, hdr) = {
                let mut st = self.state.lock().unwrap();
                match st.ready.take() {
                    Some(pair) => pair,
                    None => {
                        return Ok(GenerateOutputSuccess::NoOutput);
                    }
                }
            };

            let full_capture_ns =
                reconstruct_capture_ts(hdr.group_timestamp_ns, hdr.capture_ts_ns_lo);

            let pts = gst::ClockTime::from_nseconds(
                (hdr.presentation_ts as u64) * (1_000_000_000 / 90_000),
            );

            let mut outbuf =
                gst::Buffer::with_size(payload.len()).map_err(|_| gst::FlowError::Error)?;
            {
                let outbuf_ref = outbuf.get_mut().ok_or(gst::FlowError::Error)?;
                outbuf_ref.set_pts(pts);
                outbuf_ref.set_dts(gst::ClockTime::from_nseconds(full_capture_ns));
                if !hdr.is_keyframe() {
                    outbuf_ref.set_flags(gst::BufferFlags::DELTA_UNIT);
                }
                let mut map = outbuf_ref
                    .map_writable()
                    .map_err(|_| gst::FlowError::Error)?;
                map.copy_from_slice(&payload);
            }

            Ok(GenerateOutputSuccess::Buffer(outbuf))
        }

        fn transform_caps(
            &self,
            direction: gst::PadDirection,
            _caps: &gst::Caps,
            filter: Option<&gst::Caps>,
        ) -> Option<gst::Caps> {
            let out = match direction {
                gst::PadDirection::Sink => gst::Caps::builder("video/x-h265")
                    .field("stream-format", "byte-stream")
                    .field("alignment", "au")
                    .build(),
                gst::PadDirection::Src => gst::Caps::builder("application/x-flux").build(),
                _ => return None,
            };
            if let Some(f) = filter {
                Some(out.intersect(f))
            } else {
                Some(out)
            }
        }
    }
}
