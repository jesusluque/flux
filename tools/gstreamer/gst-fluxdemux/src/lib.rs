//! `fluxdemux` — GStreamer element that routes FLUX frames by TYPE nibble.
//!
//! sink: application/x-flux
//!
//! Dynamic src pads (created on first buffer of each type):
//!   `media_0`  — MEDIA_DATA (0x0), per channel_id (PoC: single channel)
//!   `control`  — SESSION_INFO (0x8), KEEPALIVE (0x9), STREAM_ANNOUNCE (0x5)
//!   `cdbc`     — CDBC_FEEDBACK (0x1)
//!   `misc`     — everything else (FEC, ANC, metadata, embeds…)

use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;

// ─── Plugin registration ──────────────────────────────────────────────────────

gst::plugin_define!(
    fluxdemux,
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
    FluxDemux::register(plugin)?;
    Ok(())
}

// ─── Public wrapper ───────────────────────────────────────────────────────────

glib::wrapper! {
    pub struct FluxDemux(ObjectSubclass<imp::FluxDemux>)
        @extends gst::Element, gst::Object;
}

impl FluxDemux {
    pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
        gst::Element::register(
            Some(plugin),
            "fluxdemux",
            gst::Rank::NONE,
            Self::static_type(),
        )
    }
}

// ─── Implementation submodule ─────────────────────────────────────────────────

mod imp {
    use flux_framing::{FluxHeader, FrameType, HEADER_SIZE};
    use gst::glib;
    use gst::FlowError;
    use gstreamer as gst;
    use gstreamer::prelude::*;
    use gstreamer::subclass::prelude::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    // ─── Route categories ──────────────────────────────────────────────────

    fn pad_name_for(ft: FrameType) -> &'static str {
        match ft {
            FrameType::MediaData => "media_0",
            FrameType::CdbcFeedbackT => "cdbc",
            FrameType::SessionInfo
            | FrameType::Keepalive
            | FrameType::StreamAnnounce
            | FrameType::StreamEnd => "control",
            _ => "misc",
        }
    }

    // ─── State ────────────────────────────────────────────────────────────

    struct Inner {
        src_pads: HashMap<String, gst::Pad>,
        // Sticky events cached from sink pad, replayed on newly-created src pads
        stream_start: Option<gst::Event>,
        caps_ev: Option<gst::Event>,
        segment: Option<gst::Event>,
    }

    impl Default for Inner {
        fn default() -> Self {
            Inner {
                src_pads: HashMap::new(),
                stream_start: None,
                caps_ev: None,
                segment: None,
            }
        }
    }

    // ─── GObject subclass ─────────────────────────────────────────────────

    #[derive(Default)]
    pub struct FluxDemux {
        inner: Mutex<Inner>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FluxDemux {
        const NAME: &'static str = "FluxDemux";
        type Type = super::FluxDemux;
        type ParentType = gst::Element;
    }

    impl ObjectImpl for FluxDemux {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            let sink_tmpl = obj.pad_template("sink").expect("sink template missing");
            let sink_pad = gst::Pad::builder_from_template(&sink_tmpl)
                .chain_function(|pad, parent, buf| {
                    FluxDemux::from_obj(parent.unwrap().downcast_ref::<super::FluxDemux>().unwrap())
                        .chain(pad, buf)
                })
                .event_function(|pad, parent, event| {
                    FluxDemux::from_obj(parent.unwrap().downcast_ref::<super::FluxDemux>().unwrap())
                        .sink_event(pad, event)
                })
                .build();
            obj.add_pad(&sink_pad).expect("add sink pad");
        }
    }
    impl GstObjectImpl for FluxDemux {}

    impl ElementImpl for FluxDemux {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static META: std::sync::OnceLock<gst::subclass::ElementMetadata> =
                std::sync::OnceLock::new();
            Some(META.get_or_init(|| {
                gst::subclass::ElementMetadata::new(
                    "FLUX Demux",
                    "Demux/Network/FLUX",
                    "Routes FLUX frames to typed src pads by frame TYPE nibble",
                    "LUCAB Media Technology",
                )
            }))
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            static PADS: std::sync::OnceLock<Vec<gst::PadTemplate>> = std::sync::OnceLock::new();
            PADS.get_or_init(|| {
                let sink_caps = gst::Caps::builder("application/x-flux").build();
                let any_caps = gst::Caps::new_any();
                vec![
                    gst::PadTemplate::new(
                        "sink",
                        gst::PadDirection::Sink,
                        gst::PadPresence::Always,
                        &sink_caps,
                    )
                    .unwrap(),
                    gst::PadTemplate::new(
                        "media_0",
                        gst::PadDirection::Src,
                        gst::PadPresence::Sometimes,
                        &gst::Caps::builder("application/x-flux").build(),
                    )
                    .unwrap(),
                    gst::PadTemplate::new(
                        "control",
                        gst::PadDirection::Src,
                        gst::PadPresence::Sometimes,
                        &any_caps,
                    )
                    .unwrap(),
                    gst::PadTemplate::new(
                        "cdbc",
                        gst::PadDirection::Src,
                        gst::PadPresence::Sometimes,
                        &any_caps,
                    )
                    .unwrap(),
                    gst::PadTemplate::new(
                        "misc",
                        gst::PadDirection::Src,
                        gst::PadPresence::Sometimes,
                        &any_caps,
                    )
                    .unwrap(),
                ]
            })
        }
    }

    impl FluxDemux {
        fn sink_event(&self, _pad: &gst::Pad, event: gst::Event) -> bool {
            use gst::EventView;
            // Cache sticky events so we can replay them on newly-created src pads
            {
                let mut inner = self.inner.lock().unwrap();
                match event.view() {
                    EventView::StreamStart(_) => {
                        inner.stream_start = Some(event.clone());
                    }
                    EventView::Caps(_) => {
                        inner.caps_ev = Some(event.clone());
                    }
                    EventView::Segment(_) => {
                        inner.segment = Some(event.clone());
                    }
                    _ => {}
                }
            }
            // Forward to all existing src pads
            let pads: Vec<gst::Pad> = self
                .inner
                .lock()
                .unwrap()
                .src_pads
                .values()
                .cloned()
                .collect();
            for pad in &pads {
                pad.push_event(event.clone());
            }
            true
        }

        fn chain(&self, _pad: &gst::Pad, buf: gst::Buffer) -> Result<gst::FlowSuccess, FlowError> {
            if buf.size() < HEADER_SIZE {
                return Ok(gst::FlowSuccess::Ok);
            }

            let map = buf.map_readable().map_err(|_| FlowError::Error)?;
            let hdr = match FluxHeader::decode(map.as_slice()) {
                Some(h) => h,
                None => return Ok(gst::FlowSuccess::Ok),
            };
            drop(map);

            let pad_name = pad_name_for(hdr.frame_type);
            let obj = self.obj();

            // Check if the src pad already exists (fast path, no pad creation).
            let existing = self.inner.lock().unwrap().src_pads.get(pad_name).cloned();

            let src_pad = if existing.is_some() {
                existing
            } else {
                // Build the new pad outside the mutex to avoid deadlock:
                // add_pad() fires pad-added synchronously, and push_event() can
                // trigger upstream caps queries — both must not hold inner's mutex.
                let tmpl = obj
                    .pad_template(pad_name)
                    .or_else(|| obj.pad_template("misc"));
                if let Some(tmpl) = tmpl {
                    let new_pad = gst::Pad::builder_from_template(&tmpl)
                        .name(pad_name)
                        .build();
                    new_pad.set_active(true).ok();

                    // add_pad fires pad-added → downstream links the pad
                    obj.add_pad(&new_pad).ok();

                    // Replay cached sticky events so downstream elements get
                    // STREAM_START, CAPS (application/x-flux) and SEGMENT.
                    // Must be done outside the mutex.
                    let (ss, caps, seg) = {
                        let inner = self.inner.lock().unwrap();
                        (
                            inner.stream_start.clone(),
                            inner.caps_ev.clone(),
                            inner.segment.clone(),
                        )
                    };
                    if let Some(ev) = ss {
                        let _ = new_pad.push_event(ev);
                    }
                    if let Some(ev) = caps {
                        let _ = new_pad.push_event(ev);
                    }
                    if let Some(ev) = seg {
                        let _ = new_pad.push_event(ev);
                    }

                    gst::info!(
                        gst::CAT_DEFAULT,
                        "FluxDemux: created src pad '{}'",
                        pad_name
                    );

                    // Store and return the new pad
                    let mut inner = self.inner.lock().unwrap();
                    // Another thread may have inserted while we were unlocked
                    inner
                        .src_pads
                        .entry(pad_name.to_string())
                        .or_insert(new_pad)
                        .clone()
                        .into()
                } else {
                    None
                }
            };

            if let Some(pad) = src_pad {
                match pad.push(buf) {
                    Ok(_) => {}
                    // not-linked means nothing is consuming this pad right now —
                    // that is normal for misc/control pads that have no downstream
                    // element. Continue processing other buffers.
                    Err(FlowError::NotLinked) => {}
                    Err(FlowError::Flushing) => {}
                    Err(e) => {
                        gst::warning!(
                            gst::CAT_DEFAULT,
                            "FluxDemux: push on '{}' returned {:?}",
                            pad_name,
                            e
                        );
                        return Err(e);
                    }
                }
            }

            Ok(gst::FlowSuccess::Ok)
        }
    }
}
