//! `fluxdeframer` — GStreamer BaseTransform element.
//!
//! sink: application/x-flux  →  src: video/x-h265 (byte-stream, au)
//!
//! The server forces h265parse to output byte-stream (SPS/PPS inlined), so the
//! payload in every FLUX MediaData frame is a self-contained byte-stream AU.

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
    use gst_base::subclass::prelude::*;

    #[derive(Default)]
    pub struct FluxDeframer;

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
                    "Strips the 32-byte FLUX header and emits raw H.265",
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

        fn transform(
            &self,
            inbuf: &gst::Buffer,
            outbuf: &mut gst::BufferRef,
        ) -> Result<gst::FlowSuccess, gst::FlowError> {
            let map_in = inbuf.map_readable().map_err(|_| gst::FlowError::Error)?;
            let data = map_in.as_slice();

            if data.len() < HEADER_SIZE {
                return Err(gst::FlowError::Error);
            }

            let hdr = FluxHeader::decode(data).ok_or(gst::FlowError::Error)?;

            if hdr.version != FLUX_VERSION {
                return Err(gst::FlowError::Error);
            }

            match hdr.frame_type {
                FrameType::MediaData => {}
                _ => return Ok(gst::FlowSuccess::Ok),
            }

            let full_capture_ns =
                reconstruct_capture_ts(hdr.group_timestamp_ns, hdr.capture_ts_ns_lo);

            // Skip optional metadata block (spec §4.4)
            let payload_start = if hdr.has_metadata() && data.len() >= HEADER_SIZE + 2 {
                let meta_len =
                    u16::from_be_bytes([data[HEADER_SIZE], data[HEADER_SIZE + 1]]) as usize;
                HEADER_SIZE + 2 + meta_len
            } else {
                HEADER_SIZE
            };

            let h265_data = &data[payload_start..];

            // Restore timestamps
            let pts = gst::ClockTime::from_nseconds(
                (hdr.presentation_ts as u64) * (1_000_000_000 / 90_000),
            );
            outbuf.set_pts(pts);
            outbuf.set_dts(gst::ClockTime::from_nseconds(full_capture_ns));

            if !hdr.is_keyframe() {
                outbuf.set_flags(gst::BufferFlags::DELTA_UNIT);
            }

            let mut map_out = outbuf.map_writable().map_err(|_| gst::FlowError::Error)?;
            map_out[..h265_data.len()].copy_from_slice(h265_data);
            drop(map_out);
            // Resize to actual payload (may be smaller than transform_size allocated)
            outbuf.set_size(h265_data.len());
            Ok(gst::FlowSuccess::Ok)
        }

        fn transform_size(
            &self,
            direction: gst::PadDirection,
            _caps: &gst::Caps,
            size: usize,
            _othercaps: &gst::Caps,
        ) -> Option<usize> {
            match direction {
                gst::PadDirection::Sink => Some(size.saturating_sub(HEADER_SIZE)),
                _ => Some(size + HEADER_SIZE),
            }
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
