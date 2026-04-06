//! `fluxsync` — MSS sync-barrier GStreamer element (spec §6.3).
//!
//! Operates on `application/x-flux` buffers **before** the decoder, i.e.
//! between `fluxdemux`/`fluxcdbc` and `h265parse`.
//!
//! Pipeline usage (per spec §16, one instance per stream):
//!   fluxsrc ! fluxdemux ! fluxcdbc ! fluxsync group=1 stream=0 nstreams=4 ! h265parse ! vtdec_hw
//!   fluxsrc ! fluxdemux ! fluxcdbc ! fluxsync group=1 stream=1 nstreams=4 ! h265parse ! vtdec_hw
//!   ...
//!
//! All `fluxsync` instances sharing the same `group` value coordinate via a
//! global `Arc<Mutex<SyncGroup>>`.  When all `nstreams` instances have
//! deposited a buffer with matching `GROUP_TIMESTAMP_NS` (within ±tolerance),
//! the barrier releases all of them simultaneously.
//!
//! Properties:
//!   group    (u32)   — sync-group identifier (matches server-side group_id)
//!   stream   (u32)   — 0-based index of this stream within the group
//!   nstreams (u32)   — total number of streams in the group
//!   latency  (u64)   — jitter-buffer depth in ms (default 200)
//!
//! Read-only stats:
//!   frames-synced (u64), frames-dropped (u64), max-skew-ns (u64)

use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;

// ─── Plugin registration ──────────────────────────────────────────────────────

gst::plugin_define!(
    fluxsync,
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
    FluxSync::register(plugin)?;
    Ok(())
}

// ─── Public wrapper ───────────────────────────────────────────────────────────

glib::wrapper! {
    pub struct FluxSync(ObjectSubclass<imp::FluxSync>)
        @extends gstreamer_base::Aggregator, gst::Element, gst::Object;
}

impl FluxSync {
    pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
        gst::Element::register(
            Some(plugin),
            "fluxsync",
            gst::Rank::NONE,
            Self::static_type(),
        )
    }
}

// ─── Shared barrier state (one per group) ────────────────────────────────────

pub mod sync_group {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// One slot per stream in a sync group.
    #[derive(Default)]
    pub struct Slot {
        /// Pending buffer waiting to be released.
        pub buffer: Option<gst::Buffer>,
        pub group_ts: u64,
    }

    pub struct SyncGroup {
        pub nstreams: u32,
        pub slots: Vec<Slot>,
        /// Condvar-style notification: bump to wake blocked aggregate() calls.
        pub generation: u64,
    }

    impl SyncGroup {
        pub fn new(nstreams: u32) -> Self {
            let mut slots = Vec::with_capacity(nstreams as usize);
            for _ in 0..nstreams {
                slots.push(Slot::default());
            }
            SyncGroup {
                nstreams,
                slots,
                generation: 0,
            }
        }
    }

    use gstreamer as gst;

    lazy_static::lazy_static! {
        static ref REGISTRY: Mutex<HashMap<u32, Arc<Mutex<SyncGroup>>>> =
            Mutex::new(HashMap::new());
    }

    /// Get or create the shared SyncGroup for the given group_id.
    pub fn get_or_create(group_id: u32, nstreams: u32) -> Arc<Mutex<SyncGroup>> {
        let mut reg = REGISTRY.lock().unwrap();
        reg.entry(group_id)
            .or_insert_with(|| Arc::new(Mutex::new(SyncGroup::new(nstreams))))
            .clone()
    }

    pub fn remove(group_id: u32) {
        let mut reg = REGISTRY.lock().unwrap();
        reg.remove(&group_id);
    }
}

// ─── Implementation ───────────────────────────────────────────────────────────

mod imp {
    use super::sync_group;
    use flux_framing::{FluxHeader, HEADER_SIZE};
    use gst::glib;
    use gstreamer as gst;
    use gstreamer_base::prelude::*;
    use gstreamer_base::subclass::prelude::*;
    use gstreamer_base::AggregatorPad;
    use std::sync::{Arc, Mutex};

    const DEFAULT_LATENCY_MS: u64 = 200;
    // FRAME_SYNC tolerance: ±1 frame period @ 60fps ≈ 16.7ms.  Use 50ms for PoC.
    const TOLERANCE_NS: u64 = 50_000_000;

    // ── Per-instance mutable state ────────────────────────────────────────────

    #[derive(Default)]
    pub struct Stats {
        pub frames_synced: u64,
        pub frames_dropped: u64,
        pub max_skew_ns: u64,
    }

    // ── GObject struct ────────────────────────────────────────────────────────

    pub struct FluxSync {
        stats: Mutex<Stats>,
        group: std::sync::atomic::AtomicU32,
        stream: std::sync::atomic::AtomicU32,
        nstreams: std::sync::atomic::AtomicU32,
        latency_ms: std::sync::atomic::AtomicU64,
        /// Shared barrier state (initialised in `start()`).
        sync_group: Mutex<Option<Arc<Mutex<sync_group::SyncGroup>>>>,
    }

    impl Default for FluxSync {
        fn default() -> Self {
            FluxSync {
                stats: Mutex::new(Stats::default()),
                group: std::sync::atomic::AtomicU32::new(0),
                stream: std::sync::atomic::AtomicU32::new(0),
                nstreams: std::sync::atomic::AtomicU32::new(1),
                latency_ms: std::sync::atomic::AtomicU64::new(DEFAULT_LATENCY_MS),
                sync_group: Mutex::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FluxSync {
        const NAME: &'static str = "GstFluxSync";
        type Type = super::FluxSync;
        type ParentType = gstreamer_base::Aggregator;
    }

    // ── GObject properties ────────────────────────────────────────────────────

    impl ObjectImpl for FluxSync {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPS: std::sync::OnceLock<Vec<glib::ParamSpec>> = std::sync::OnceLock::new();
            PROPS.get_or_init(|| {
                vec![
                    glib::ParamSpecUInt::builder("group")
                        .nick("Sync group")
                        .blurb("GROUP_ID value (matches server-side group_id)")
                        .default_value(0)
                        .build(),
                    glib::ParamSpecUInt::builder("stream")
                        .nick("Stream index")
                        .blurb("0-based index of this stream within the sync group")
                        .default_value(0)
                        .build(),
                    glib::ParamSpecUInt::builder("nstreams")
                        .nick("Number of streams")
                        .blurb("Total number of streams in the sync group")
                        .default_value(1)
                        .build(),
                    glib::ParamSpecUInt64::builder("latency")
                        .nick("Latency (ms)")
                        .blurb("Jitter-buffer depth in ms before timeout fires")
                        .default_value(DEFAULT_LATENCY_MS)
                        .build(),
                    glib::ParamSpecUInt64::builder("frames-synced")
                        .nick("Frames synced")
                        .blurb("Cohorts released with all streams in sync")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt64::builder("frames-dropped")
                        .nick("Frames dropped")
                        .blurb("Frames dropped due to timeout or skew")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt64::builder("max-skew-ns")
                        .nick("Max skew (ns)")
                        .blurb("Maximum observed GROUP_TIMESTAMP_NS skew")
                        .read_only()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "group" => self.group.store(
                    value.get::<u32>().unwrap(),
                    std::sync::atomic::Ordering::Relaxed,
                ),
                "stream" => self.stream.store(
                    value.get::<u32>().unwrap(),
                    std::sync::atomic::Ordering::Relaxed,
                ),
                "nstreams" => self.nstreams.store(
                    value.get::<u32>().unwrap(),
                    std::sync::atomic::Ordering::Relaxed,
                ),
                "latency" => {
                    let v: u64 = value.get().unwrap();
                    self.latency_ms
                        .store(v, std::sync::atomic::Ordering::Relaxed);
                    self.obj()
                        .set_latency(gst::ClockTime::from_nseconds(v * 1_000_000), None);
                }
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            use std::sync::atomic::Ordering::Relaxed;
            match pspec.name() {
                "group" => self.group.load(Relaxed).to_value(),
                "stream" => self.stream.load(Relaxed).to_value(),
                "nstreams" => self.nstreams.load(Relaxed).to_value(),
                "latency" => self.latency_ms.load(Relaxed).to_value(),
                "frames-synced" => self.stats.lock().unwrap().frames_synced.to_value(),
                "frames-dropped" => self.stats.lock().unwrap().frames_dropped.to_value(),
                "max-skew-ns" => self.stats.lock().unwrap().max_skew_ns.to_value(),
                _ => unimplemented!(),
            }
        }
    }

    // ── GstElement boilerplate ────────────────────────────────────────────────

    impl GstObjectImpl for FluxSync {}

    impl ElementImpl for FluxSync {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static META: std::sync::OnceLock<gst::subclass::ElementMetadata> =
                std::sync::OnceLock::new();
            Some(META.get_or_init(|| {
                gst::subclass::ElementMetadata::new(
                    "FLUX MSS Sync Barrier",
                    "Filter/FLUX",
                    "Multi-stream sync barrier on application/x-flux buffers (spec §6.3)",
                    "LUCAB Media Technology",
                )
            }))
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            static TEMPLATES: std::sync::OnceLock<Vec<gst::PadTemplate>> =
                std::sync::OnceLock::new();
            TEMPLATES.get_or_init(|| {
                let flux_caps = gst::Caps::new_empty_simple("application/x-flux");
                vec![
                    gst::PadTemplate::with_gtype(
                        "sink",
                        gst::PadDirection::Sink,
                        gst::PadPresence::Always,
                        &flux_caps,
                        gstreamer_base::AggregatorPad::static_type(),
                    )
                    .unwrap(),
                    gst::PadTemplate::new(
                        "src",
                        gst::PadDirection::Src,
                        gst::PadPresence::Always,
                        &flux_caps,
                    )
                    .unwrap(),
                ]
            })
        }
    }

    // ── GstAggregator implementation ──────────────────────────────────────────

    impl AggregatorImpl for FluxSync {
        fn start(&self) -> Result<(), gst::ErrorMessage> {
            use std::sync::atomic::Ordering::Relaxed;
            let latency_ns = self.latency_ms.load(Relaxed) * 1_000_000;
            self.obj()
                .set_latency(gst::ClockTime::from_nseconds(latency_ns), None);

            let group_id = self.group.load(Relaxed);
            let nstreams = self.nstreams.load(Relaxed);
            let sg = sync_group::get_or_create(group_id, nstreams);
            *self.sync_group.lock().unwrap() = Some(sg);
            Ok(())
        }

        fn stop(&self) -> Result<(), gst::ErrorMessage> {
            *self.sync_group.lock().unwrap() = None;
            Ok(())
        }

        /// Core barrier logic.
        ///
        /// Called by GstAggregator when all sink pads have at least one buffer
        /// queued, or when `timeout=true` (latency deadline expired).
        fn aggregate(&self, timeout: bool) -> Result<gst::FlowSuccess, gst::FlowError> {
            use gstreamer_base::prelude::AggregatorPadExt;
            use std::sync::atomic::Ordering::Relaxed;

            let agg = self.obj();
            let stream_idx = self.stream.load(Relaxed) as usize;

            // --- Get the sink pad and peek the next buffer ------------------

            let sink_pads: Vec<_> = agg
                .sink_pads()
                .into_iter()
                .filter_map(|p| p.downcast::<AggregatorPad>().ok())
                .collect();

            let sink_pad = match sink_pads.first() {
                Some(p) => p.clone(),
                None => return Err(gst::FlowError::Eos),
            };

            let buf = match sink_pad.peek_buffer() {
                Some(b) => b,
                None => {
                    if timeout {
                        // No data at all after timeout — skip this cycle.
                        return Ok(gst::FlowSuccess::Ok);
                    }
                    return Err(gstreamer_base::AGGREGATOR_FLOW_NEED_DATA);
                }
            };

            let my_ts = match read_group_ts(&buf) {
                Some(ts) => ts,
                None => {
                    // Malformed buffer — drop it.
                    sink_pad.pop_buffer();
                    self.stats.lock().unwrap().frames_dropped += 1;
                    return Ok(gst::FlowSuccess::Ok);
                }
            };

            // --- Deposit our buffer into the shared slot --------------------

            let sg_arc = match self.sync_group.lock().unwrap().as_ref().cloned() {
                Some(a) => a,
                None => return Err(gst::FlowError::Eos),
            };

            {
                let mut sg = sg_arc.lock().unwrap();
                let nstreams = sg.nstreams as usize;
                if stream_idx >= nstreams {
                    log::error!("fluxsync: stream {} >= nstreams {}", stream_idx, nstreams);
                    return Err(gst::FlowError::Error);
                }
                // Deposit.
                sg.slots[stream_idx].buffer = Some(buf.clone());
                sg.slots[stream_idx].group_ts = my_ts;
            }

            // --- Check if barrier is met (all slots filled + in sync) -------

            let can_release = {
                let sg = sg_arc.lock().unwrap();
                let all_filled = sg.slots.iter().all(|s| s.buffer.is_some());
                if !all_filled && !timeout {
                    false
                } else {
                    // Check timestamp spread.
                    let filled: Vec<u64> = sg
                        .slots
                        .iter()
                        .filter(|s| s.buffer.is_some())
                        .map(|s| s.group_ts)
                        .collect();
                    if filled.is_empty() {
                        false
                    } else {
                        let min_ts = *filled.iter().min().unwrap();
                        let max_ts = *filled.iter().max().unwrap();
                        let skew = max_ts - min_ts;
                        skew <= TOLERANCE_NS || timeout
                    }
                }
            };

            if !can_release {
                // Barrier not met yet — signal need-data to come back.
                return Err(gstreamer_base::AGGREGATOR_FLOW_NEED_DATA);
            }

            // --- Release: pop our buffer and push downstream ----------------

            // Check we're still the designated releaser (stream 0 updates
            // stats; all streams pop and push their own buffer).
            let my_buf = {
                let mut sg = sg_arc.lock().unwrap();
                // Compute final skew for stats.
                let filled_ts: Vec<u64> = sg
                    .slots
                    .iter()
                    .filter(|s| s.buffer.is_some())
                    .map(|s| s.group_ts)
                    .collect();
                let max_ts = filled_ts.iter().copied().max().unwrap_or(my_ts);
                let skew = if my_ts > max_ts {
                    my_ts - max_ts
                } else {
                    max_ts - my_ts
                };
                {
                    let mut stats = self.stats.lock().unwrap();
                    if skew > stats.max_skew_ns {
                        stats.max_skew_ns = skew;
                    }
                    stats.frames_synced += 1;
                }
                sg.slots[stream_idx].buffer.take()
            };

            // Consume from the sink pad queue.
            sink_pad.pop_buffer();

            if let Some(buf) = my_buf {
                agg.finish_buffer(buf)?;
            }

            Ok(gst::FlowSuccess::Ok)
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Read GROUP_TIMESTAMP_NS from bytes 8–15 of a FLUX buffer.
    fn read_group_ts(buf: &gst::Buffer) -> Option<u64> {
        let map = buf.map_readable().ok()?;
        let data = map.as_slice();
        if data.len() < HEADER_SIZE {
            return None;
        }
        let hdr = FluxHeader::decode(data)?;
        Some(hdr.group_timestamp_ns)
    }
}
