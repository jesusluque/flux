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
    use flux_framing::{FluxHeader, FrameType, FLUX_VERSION, HEADER_SIZE};
    use gst::subclass::prelude::*;
    use gst_base::subclass::base_transform::GenerateOutputSuccess;
    use gst_base::subclass::prelude::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    // ── Fragment reassembly state ─────────────────────────────────────────────
    //
    // Spec §4.1 FRAG field encoding:
    //   0x0       = unfragmented (complete AU in one datagram)
    //   0x1–0xD   = fragment index (1-based); more fragments follow
    //   0xE       = last fragment (its 1-based index tells us the total count)
    //   0xF       = reserved
    //
    // QUIC datagrams are delivered best-effort and may arrive out of order.
    // We store every received fragment in a BTreeMap keyed by its 1-based index
    // and only emit the reassembled AU once all expected fragments are present.

    struct FragState {
        /// sequence_in_group of the AU being assembled (None = idle).
        seq: Option<u32>,
        /// Received fragments: frag_index (1-based) → payload chunk.
        frags: BTreeMap<u8, Vec<u8>>,
        /// Header from the first fragment received (carries timestamps, flags).
        hdr: Option<FluxHeader>,
        /// Total expected fragment count, set when the 0xE (last) frag arrives.
        /// None means the last fragment has not arrived yet.
        total: Option<u8>,
        /// A fully reassembled AU ready to be pushed, or None.
        /// The bool indicates whether DISCONT should be set on the output buffer.
        ready: Option<(Vec<u8>, FluxHeader, bool)>,
        /// Carry the DISCONT flag for the AU currently being assembled.
        pending_discont: bool,
    }

    impl Default for FragState {
        fn default() -> Self {
            FragState {
                seq: None,
                frags: BTreeMap::new(),
                hdr: None,
                total: None,
                ready: None,
                pending_discont: false,
            }
        }
    }

    impl FragState {
        fn reset_assembly(&mut self) {
            self.seq = None;
            self.frags.clear();
            self.hdr = None;
            self.total = None;
            self.pending_discont = false;
        }

        /// Returns true if every expected fragment has arrived.
        fn is_complete(&self) -> bool {
            if let Some(total) = self.total {
                self.frags.len() == total as usize
            } else {
                false
            }
        }

        /// Concatenate all fragments in index order into a single payload.
        fn assemble(&mut self) -> Vec<u8> {
            let mut out = Vec::new();
            for (_, chunk) in &self.frags {
                out.extend_from_slice(chunk);
            }
            out
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
            let map = inbuf.map_readable().map_err(|_| gst::FlowError::Error)?;
            let data = map.as_slice();

            if data.len() < HEADER_SIZE {
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

            // Fix 2: A DISCONT input means the upstream reference chain is
            // broken (new session / reconnect).  Discard any in-progress
            // fragment assembly and propagate the flag to the next output buffer.
            if is_discont {
                st.reset_assembly();
                st.pending_discont = true;
            }

            let seq = hdr.sequence_in_group;

            match hdr.frag {
                // ── Unfragmented AU ───────────────────────────────────────────
                0x0 => {
                    if st.seq.is_some() {
                        eprintln!(
                            "[fluxdeframer] unfragmented seq={} arrived while seq={:?} in progress — discarding old",
                            seq, st.seq
                        );
                        st.reset_assembly();
                    }
                    let discont = st.pending_discont;
                    st.pending_discont = false;
                    st.ready = Some((chunk.to_vec(), hdr.clone(), discont));
                }

                // ── Last fragment (spec 0xE) ───────────────────────────────────
                0xE => {
                    // If this is for a different seq, discard any old assembly.
                    if st.seq.is_some() && st.seq != Some(seq) {
                        eprintln!(
                            "[fluxdeframer] last-frag seq={} arrived while assembling seq={:?} — discarding old",
                            seq, st.seq
                        );
                        st.reset_assembly();
                    }
                    if st.seq.is_none() {
                        st.seq = Some(seq);
                    }
                    // Store frag 0xE under its sentinel key.  We compute total as
                    // (highest non-last index + 1) once both the last-frag and at
                    // least one mid-frag are present.  If 0xE is the only frag so
                    // far (out-of-order: last arrived first), total stays None
                    // until the mid-frags fill in.
                    if st.hdr.is_none() {
                        st.hdr = Some(hdr.clone());
                    }
                    st.frags.insert(0xE, chunk.to_vec());
                    let highest_mid = st.frags.keys().filter(|&&k| k != 0xE).max().copied();
                    let total = highest_mid.map(|h| h + 1).unwrap_or(1);
                    st.total = Some(total);
                }

                // ── Mid / first fragment (0x1–0xD) ───────────────────────────
                n @ 0x1..=0xD => {
                    // If this is for a different seq, discard old assembly.
                    if st.seq.is_some() && st.seq != Some(seq) {
                        eprintln!(
                            "[fluxdeframer] frag={} seq={} arrived while assembling seq={:?} — discarding old",
                            n, seq, st.seq
                        );
                        st.reset_assembly();
                    }
                    if st.seq.is_none() {
                        st.seq = Some(seq);
                    }
                    // Keep the header from fragment index 1 (the canonical first
                    // fragment) because it carries the correct timestamps.
                    // If frag=1 hasn't arrived yet, store any header as a fallback
                    // and replace it when frag=1 does arrive.
                    if st.hdr.is_none() || n == 1 {
                        st.hdr = Some(hdr.clone());
                    }
                    st.frags.insert(n, chunk.to_vec());

                    // If the last fragment (0xE) already arrived, recompute total.
                    if st.frags.contains_key(&0xE) {
                        let highest_mid = st.frags.keys().filter(|&&k| k != 0xE).max().copied();
                        let total = highest_mid.map(|h| h + 1).unwrap_or(1);
                        st.total = Some(total);
                    }
                }

                _ => {
                    // frag=0xF is reserved — discard
                    return Ok(gst_base::BASE_TRANSFORM_FLOW_DROPPED);
                }
            }

            // Check if the AU is fully reassembled.
            if hdr.frag == 0x0 {
                // Already placed in ready above.
                return Ok(gst::FlowSuccess::Ok);
            }

            if !st.is_complete() {
                return Ok(gst_base::BASE_TRANSFORM_FLOW_DROPPED);
            }

            // All fragments received — assemble and move to ready.
            let payload = st.assemble();
            let first_hdr = st.hdr.take().unwrap();
            let discont = st.pending_discont;
            st.reset_assembly();
            st.ready = Some((payload, first_hdr, discont));

            Ok(gst::FlowSuccess::Ok)
        }

        /// Called after `submit_input_buffer` returned `Buffer`.  At this point
        /// `state.ready` holds the fully reassembled AU; allocate a new buffer
        /// of exactly the right size and fill it.
        fn generate_output(&self) -> Result<GenerateOutputSuccess, gst::FlowError> {
            let (payload, hdr, discont) = {
                let mut st = self.state.lock().unwrap();
                match st.ready.take() {
                    Some(triple) => triple,
                    None => {
                        return Ok(GenerateOutputSuccess::NoOutput);
                    }
                }
            };

            // group_timestamp_ns carries the full DTS (== PTS since the server
            // uses allow-frame-reordering=false).  Use it directly for both PTS
            // and DTS.  presentation_ts is a u32 90kHz field that overflows after
            // ~13 hours of uptime — unusable on a machine with ~1000 h uptime.
            let ts = gst::ClockTime::from_nseconds(hdr.group_timestamp_ns);
            if hdr.is_keyframe() {
                eprintln!(
                    "[fluxdeframer] output buf: keyframe=true group_timestamp_ns={} pts_gst={}",
                    hdr.group_timestamp_ns, ts,
                );
            }

            let mut outbuf =
                gst::Buffer::with_size(payload.len()).map_err(|_| gst::FlowError::Error)?;
            {
                let outbuf_ref = outbuf.get_mut().ok_or(gst::FlowError::Error)?;
                outbuf_ref.set_pts(ts);
                outbuf_ref.set_dts(ts);
                if !hdr.is_keyframe() {
                    outbuf_ref.set_flags(gst::BufferFlags::DELTA_UNIT);
                }
                // Fix 2: Forward DISCONT from the upstream source buffer so that
                // h265parse and vtdec_hw know the reference picture chain is
                // broken and reset their internal state accordingly.
                if discont {
                    outbuf_ref.set_flags(gst::BufferFlags::DISCONT);
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
