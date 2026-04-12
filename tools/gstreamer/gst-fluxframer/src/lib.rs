//! `fluxframer` — GStreamer BaseTransform element.
//!
//! sink: video/x-h265  →  src: application/x-flux
//!
//! Prepends the 32-byte FLUX header (TYPE=MEDIA_DATA) to each incoming buffer.

use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_base as gst_base;

// ─── Plugin registration ──────────────────────────────────────────────────────

gst::plugin_define!(
    fluxframer,
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
    FluxFramer::register(plugin)?;
    Ok(())
}

// ─── Public GObject wrapper ───────────────────────────────────────────────────

glib::wrapper! {
    pub struct FluxFramer(ObjectSubclass<imp::FluxFramer>)
        @extends gst_base::BaseTransform, gst::Element, gst::Object;
}

impl FluxFramer {
    pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
        gst::Element::register(
            Some(plugin),
            "fluxframer",
            gst::Rank::NONE,
            Self::static_type(),
        )
    }
}

// ─── Implementation ───────────────────────────────────────────────────────────

mod imp {
    use super::*;
    use flux_framing::{FluxHeader, HEADER_SIZE};
    use gst::subclass::prelude::*;
    use gst_base::subclass::prelude::*;
    use std::sync::Mutex;

    fn cat() -> &'static gst::DebugCategory {
        static CAT: std::sync::OnceLock<gst::DebugCategory> = std::sync::OnceLock::new();
        CAT.get_or_init(|| {
            gst::DebugCategory::new(
                "fluxframer",
                gst::DebugColorFlags::empty(),
                Some("FLUX framer"),
            )
        })
    }

    #[derive(Default)]
    struct State {
        seq: u32,
        channel_id: u16,
        group_id: u16,
        layer: u8,
    }

    #[derive(Default)]
    pub struct FluxFramer {
        state: Mutex<State>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FluxFramer {
        const NAME: &'static str = "FluxFramer";
        type Type = super::FluxFramer;
        type ParentType = gst_base::BaseTransform;
    }

    impl ObjectImpl for FluxFramer {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPS: std::sync::OnceLock<Vec<glib::ParamSpec>> = std::sync::OnceLock::new();
            PROPS.get_or_init(|| {
                vec![
                    glib::ParamSpecUInt::builder("channel-id")
                        .nick("Channel ID")
                        .blurb("FLUX channel identifier (0–65535)")
                        .default_value(0)
                        .build(),
                    glib::ParamSpecUInt::builder("group-id")
                        .nick("Group ID")
                        .blurb("FLUX sync group identifier")
                        .default_value(1)
                        .build(),
                    glib::ParamSpecUInt::builder("layer")
                        .nick("Layer")
                        .blurb("Quality layer (0 = base)")
                        .maximum(15)
                        .default_value(0)
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let mut s = self.state.lock().unwrap();
            match pspec.name() {
                "channel-id" => s.channel_id = value.get::<u32>().unwrap() as u16,
                "group-id" => s.group_id = value.get::<u32>().unwrap() as u16,
                "layer" => s.layer = value.get::<u32>().unwrap() as u8,
                _ => {}
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let s = self.state.lock().unwrap();
            match pspec.name() {
                "channel-id" => (s.channel_id as u32).to_value(),
                "group-id" => (s.group_id as u32).to_value(),
                "layer" => (s.layer as u32).to_value(),
                _ => unimplemented!(),
            }
        }
    }

    impl GstObjectImpl for FluxFramer {}

    impl ElementImpl for FluxFramer {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static META: std::sync::OnceLock<gst::subclass::ElementMetadata> =
                std::sync::OnceLock::new();
            Some(META.get_or_init(|| {
                gst::subclass::ElementMetadata::new(
                    "FLUX Framer",
                    "Transform/Network/FLUX",
                    "Wraps H.265 buffers in a 32-byte FLUX header (MEDIA_DATA)",
                    "Jesus Luque",
                )
            }))
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            static PADS: std::sync::OnceLock<Vec<gst::PadTemplate>> = std::sync::OnceLock::new();
            PADS.get_or_init(|| {
                let sink_caps = gst::Caps::builder("video/x-h265").build();
                let src_caps = gst::Caps::builder("application/x-flux").build();
                vec![
                    gst::PadTemplate::new(
                        "sink",
                        gst::PadDirection::Sink,
                        gst::PadPresence::Always,
                        &sink_caps,
                    )
                    .unwrap(),
                    gst::PadTemplate::new(
                        "src",
                        gst::PadDirection::Src,
                        gst::PadPresence::Always,
                        &src_caps,
                    )
                    .unwrap(),
                ]
            })
        }
    }

    impl BaseTransformImpl for FluxFramer {
        const MODE: gst_base::subclass::BaseTransformMode =
            gst_base::subclass::BaseTransformMode::NeverInPlace;
        const PASSTHROUGH_ON_SAME_CAPS: bool = false;
        const TRANSFORM_IP_ON_PASSTHROUGH: bool = false;

        /// Explicitly pass upstream events (e.g. ForceKeyUnit) from our src pad
        /// through to our sink pad so they reach vtenc_h265.  BaseTransform
        /// default does forward them, but this makes it explicit and lets us log.
        fn src_event(&self, event: gst::Event) -> bool {
            gst::debug!(
                cat(),
                "[fluxframer] src_event: type={:?} — forwarding upstream",
                event.type_()
            );
            self.parent_src_event(event)
        }

        fn transform(
            &self,
            inbuf: &gst::Buffer,
            outbuf: &mut gst::BufferRef,
        ) -> Result<gst::FlowSuccess, gst::FlowError> {
            let map_in = inbuf.map_readable().map_err(|_| gst::FlowError::Error)?;
            let payload = map_in.as_slice();

            let is_keyframe = !inbuf.flags().contains(gst::BufferFlags::DELTA_UNIT);

            // Use the buffer's GStreamer PTS/DTS (nanoseconds from pipeline
            // clock origin — starts near 0 at pipeline start).  Falling back
            // to 0 if either is NONE is safe: the client will just show a
            // PTS of 0 for that frame, which GStreamer handles gracefully.
            let pts_ns = inbuf.pts().map(|t| t.nseconds()).unwrap_or(0);
            let dts_ns = inbuf
                .dts()
                .or_else(|| inbuf.pts())
                .map(|t| t.nseconds())
                .unwrap_or(0);

            // group_timestamp_ns must be identical for the same frame across all
            // independent server pipelines so that fluxsync can align them into
            // the same slot.
            //
            // We derive group_ts from the buffer's DTS (pipeline clock, starts
            // near 0 at pipeline start) rather than wall-clock now_ns().  All 4
            // pipelines start at the same time with the same 30fps videotestsrc,
            // so their DTS values are frame_number × 33_333_333 ns and stay in
            // lockstep regardless of vtenc encoding latency variations.
            //
            // Wall-clock now_ns() at transform() time varies by vtenc latency
            // (which can differ by >16ms between pipelines on the same host),
            // causing different pipelines to snap to adjacent 33ms grid cells
            // for the same logical frame — the misalignment we observed.
            //
            // Snapping dts_ns to the nearest 33ms boundary eliminates the
            // ±jitter from minor scheduling differences between pipelines.
            const FRAME_NS: u64 = 1_000_000_000 / 30; // 33_333_333 ns
                                                      // Use DTS (falls back to PTS, then 0) — pipeline-clock based, so
                                                      // identical across all streams for the same logical frame.
            let group_ts = (dts_ns + FRAME_NS / 2) / FRAME_NS * FRAME_NS;

            let mut st = self.state.lock().unwrap();
            st.seq = st.seq.wrapping_add(1);
            let hdr = FluxHeader::new_media(
                st.channel_id,
                st.group_id,
                st.layer,
                is_keyframe,
                payload.len() as u32,
                st.seq,
                pts_ns,
                dts_ns,
                group_ts,
            );
            drop(st);

            let hdr_bytes = hdr.encode();
            let mut map_out = outbuf.map_writable().map_err(|_| gst::FlowError::Error)?;
            map_out[..HEADER_SIZE].copy_from_slice(&hdr_bytes);
            map_out[HEADER_SIZE..].copy_from_slice(payload);

            Ok(gst::FlowSuccess::Ok)
        }

        fn transform_size(
            &self,
            _direction: gst::PadDirection,
            _caps: &gst::Caps,
            size: usize,
            _othercaps: &gst::Caps,
        ) -> Option<usize> {
            Some(size + HEADER_SIZE)
        }

        fn transform_caps(
            &self,
            direction: gst::PadDirection,
            _caps: &gst::Caps,
            filter: Option<&gst::Caps>,
        ) -> Option<gst::Caps> {
            let out = match direction {
                gst::PadDirection::Sink => gst::Caps::builder("application/x-flux").build(),
                gst::PadDirection::Src => gst::Caps::builder("video/x-h265").build(),
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
