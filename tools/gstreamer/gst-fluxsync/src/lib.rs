//! `fluxsync` — MSS timestamp-aligned stream synchroniser (spec §6.3).
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
//! global `Arc<GroupEntry>` registry.
//!
//! Alignment design — timestamp-keyed slot buffer:
//!
//!   The shared GroupEntry holds a BTreeMap<group_timestamp_ns, Slot>.
//!   Each Slot has one Option<gst::Buffer> per stream and a `ready` flag.
//!
//!   On each buffer arrival (stream index S, timestamp T):
//!     1. Lock the group.
//!     2. Store buffer into slot[T].buffers[S].
//!     3. If slot[T] now has all nstreams buffers → mark ready, notify_all.
//!     4. Evict slots older than `latency_ms` relative to the newest T seen.
//!        Evicted slots are marked `evicted` and notified so waiters wake.
//!     5. Unlock, then wait on the condvar until slot[T].ready || slot[T].evicted.
//!     6. Take own buffer back from slot[T] (or pass original through if evicted).
//!     7. Clean up slot if all buffers have been taken.
//!
//! This guarantees that frames with matching group_timestamp_ns exit fluxsync
//! simultaneously, regardless of per-stream artificial delay.  If one stream
//! is so late that its slot gets evicted, that stream's frame passes through
//! immediately (the other streams already passed theirs through on eviction).
//!
//! Properties:
//!   group    (u32)   — sync-group identifier (matches server-side group_id)
//!   stream   (u32)   — 0-based index of this stream within the group
//!   nstreams (u32)   — total number of streams in the group
//!   latency  (u64)   — alignment window / eviction timeout in ms (default 200)
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
        @extends gstreamer_base::BaseTransform, gst::Element, gst::Object;
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

// ─── Shared alignment state (one per group) ───────────────────────────────────

pub mod sync_group {
    use gstreamer as gst;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::{Arc, Condvar, Mutex};

    /// One slot per unique group_timestamp_ns.
    pub struct Slot {
        /// Buffers deposited by each stream (indexed by stream_id).
        /// None = stream hasn't arrived yet for this timestamp.
        pub buffers: Vec<Option<gst::Buffer>>,
        /// How many streams have deposited a buffer.
        pub count: u32,
        /// Set when count == nstreams → all streams release.
        pub ready: bool,
        /// Set when the slot is evicted (latency window expired) before all
        /// streams arrived.  Waiters pass their original buffer through.
        pub evicted: bool,
    }

    impl Slot {
        pub fn new(nstreams: usize) -> Self {
            Slot {
                buffers: vec![None; nstreams],
                count: 0,
                ready: false,
                evicted: false,
            }
        }
    }

    pub struct GroupState {
        pub nstreams: u32,
        /// The largest group_timestamp_ns seen so far (used for eviction window).
        pub newest_ts: u64,
        /// Alignment window: slots with ts < newest_ts - latency_ns are evicted.
        pub latency_ns: u64,
        /// Per-timestamp slots.
        pub slots: BTreeMap<u64, Slot>,
        // Stats
        pub frames_synced: u64,
        pub frames_dropped: u64,
        pub max_skew_ns: u64,
    }

    impl GroupState {
        pub fn new(nstreams: u32, latency_ns: u64) -> Self {
            GroupState {
                nstreams,
                newest_ts: 0,
                latency_ns,
                slots: BTreeMap::new(),
                frames_synced: 0,
                frames_dropped: 0,
                max_skew_ns: 0,
            }
        }
    }

    pub struct GroupEntry {
        pub state: Mutex<GroupState>,
        pub condvar: Condvar,
    }

    impl GroupEntry {
        pub fn new(nstreams: u32, latency_ns: u64) -> Self {
            GroupEntry {
                state: Mutex::new(GroupState::new(nstreams, latency_ns)),
                condvar: Condvar::new(),
            }
        }
    }

    lazy_static::lazy_static! {
        static ref REGISTRY: Mutex<HashMap<u32, Arc<GroupEntry>>> =
            Mutex::new(HashMap::new());
    }

    pub fn get_or_create(group_id: u32, nstreams: u32, latency_ns: u64) -> Arc<GroupEntry> {
        let mut reg = REGISTRY.lock().unwrap();
        reg.entry(group_id)
            .or_insert_with(|| Arc::new(GroupEntry::new(nstreams, latency_ns)))
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
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const DEFAULT_LATENCY_MS: u64 = 200;
    /// Short polling interval for the condvar wait loop (ms).
    /// Bounds per-frame stall when a stream is artificially delayed.
    const CHECK_INTERVAL_MS: u64 = 10;

    fn cat() -> &'static gst::DebugCategory {
        static CAT: std::sync::OnceLock<gst::DebugCategory> = std::sync::OnceLock::new();
        CAT.get_or_init(|| {
            gst::DebugCategory::new(
                "fluxsync",
                gst::DebugColorFlags::empty(),
                Some("FLUX MSS sync"),
            )
        })
    }

    // ── GObject struct ────────────────────────────────────────────────────────

    pub struct FluxSync {
        group: std::sync::atomic::AtomicU32,
        stream: std::sync::atomic::AtomicU32,
        nstreams: std::sync::atomic::AtomicU32,
        latency_ms: std::sync::atomic::AtomicU64,
        /// Shared alignment state (initialised in `start()`).
        group_entry: Mutex<Option<Arc<sync_group::GroupEntry>>>,
    }

    impl Default for FluxSync {
        fn default() -> Self {
            FluxSync {
                group: std::sync::atomic::AtomicU32::new(0),
                stream: std::sync::atomic::AtomicU32::new(0),
                nstreams: std::sync::atomic::AtomicU32::new(1),
                latency_ms: std::sync::atomic::AtomicU64::new(DEFAULT_LATENCY_MS),
                group_entry: Mutex::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FluxSync {
        const NAME: &'static str = "GstFluxSync";
        type Type = super::FluxSync;
        type ParentType = gstreamer_base::BaseTransform;
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
                        .blurb("Alignment window / eviction timeout in ms")
                        .default_value(DEFAULT_LATENCY_MS)
                        .build(),
                    glib::ParamSpecUInt64::builder("frames-synced")
                        .nick("Frames synced")
                        .blurb("Slots released with all streams aligned")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt64::builder("frames-dropped")
                        .nick("Frames dropped")
                        .blurb("Frames passed through due to eviction (slot timeout)")
                        .read_only()
                        .build(),
                    glib::ParamSpecUInt64::builder("max-skew-ns")
                        .nick("Max skew (ns)")
                        .blurb("Maximum observed GROUP_TIMESTAMP_NS skew within a slot")
                        .read_only()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            use std::sync::atomic::Ordering::Relaxed;
            match pspec.name() {
                "group" => self.group.store(value.get::<u32>().unwrap(), Relaxed),
                "stream" => self.stream.store(value.get::<u32>().unwrap(), Relaxed),
                "nstreams" => self.nstreams.store(value.get::<u32>().unwrap(), Relaxed),
                "latency" => self.latency_ms.store(value.get::<u64>().unwrap(), Relaxed),
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
                "frames-synced" => self
                    .group_entry
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|e| e.state.lock().unwrap().frames_synced)
                    .unwrap_or(0)
                    .to_value(),
                "frames-dropped" => self
                    .group_entry
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|e| e.state.lock().unwrap().frames_dropped)
                    .unwrap_or(0)
                    .to_value(),
                "max-skew-ns" => self
                    .group_entry
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|e| e.state.lock().unwrap().max_skew_ns)
                    .unwrap_or(0)
                    .to_value(),
                _ => unimplemented!(),
            }
        }
    }

    impl GstObjectImpl for FluxSync {}

    impl ElementImpl for FluxSync {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static META: std::sync::OnceLock<gst::subclass::ElementMetadata> =
                std::sync::OnceLock::new();
            Some(META.get_or_init(|| {
                gst::subclass::ElementMetadata::new(
                    "FLUX MSS Sync Aligner",
                    "Filter/FLUX",
                    "Timestamp-aligned multi-stream synchroniser on application/x-flux buffers (spec §6.3)",
                    "Jesus Luque",
                )
            }))
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            static TEMPLATES: std::sync::OnceLock<Vec<gst::PadTemplate>> =
                std::sync::OnceLock::new();
            TEMPLATES.get_or_init(|| {
                let flux_caps = gst::Caps::new_empty_simple("application/x-flux");
                vec![
                    gst::PadTemplate::new(
                        "sink",
                        gst::PadDirection::Sink,
                        gst::PadPresence::Always,
                        &flux_caps,
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

        fn change_state(
            &self,
            transition: gst::StateChange,
        ) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
            match transition {
                // Going into pause or stopping: evict all pending slots so any
                // thread blocked on the condvar wakes up and passes through.
                // We do NOT return FlowError::Flushing — that kills the pad and
                // prevents resume.  Instead, eviction causes transform_ip to
                // take the pass-through path (slot.evicted=true) and return Ok.
                gst::StateChange::PlayingToPaused | gst::StateChange::PausedToReady => {
                    if let Some(entry) = self.group_entry.lock().unwrap().as_ref() {
                        {
                            let mut gs = entry.state.lock().unwrap();
                            for slot in gs.slots.values_mut() {
                                slot.evicted = true;
                            }
                            // Reset newest_ts so the eviction window starts
                            // fresh on resume — avoids all post-resume slots
                            // being immediately evicted as "too old".
                            gs.newest_ts = 0;
                            gs.slots.clear();
                        }
                        entry.condvar.notify_all();
                    }
                }
                _ => {}
            }
            self.parent_change_state(transition)
        }
    }

    // ── BaseTransform implementation ──────────────────────────────────────────

    impl BaseTransformImpl for FluxSync {
        const MODE: gstreamer_base::subclass::BaseTransformMode =
            gstreamer_base::subclass::BaseTransformMode::AlwaysInPlace;
        const PASSTHROUGH_ON_SAME_CAPS: bool = false;
        const TRANSFORM_IP_ON_PASSTHROUGH: bool = true;

        /// Answer LATENCY queries locally so the compositor does not fail when
        /// the upstream dynamic-pad chain (fluxdemux → fluxcdbc → sync_queue →
        /// fluxsync) is not yet fully linked at startup or post-resume.
        ///
        /// We report live=true, min=latency_ms (our own alignment window),
        /// max=unlimited.  This matches the values reported by fluxsrc and the
        /// fluxdemux src-pad query function.
        fn query(&self, direction: gst::PadDirection, query: &mut gst::QueryRef) -> bool {
            use std::sync::atomic::Ordering::Relaxed;
            if direction == gst::PadDirection::Src {
                if let gst::QueryViewMut::Latency(q) = query.view_mut() {
                    let latency_ms = self.latency_ms.load(Relaxed);
                    q.set(
                        true,
                        gst::ClockTime::from_mseconds(latency_ms),
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
            use std::sync::atomic::Ordering::Relaxed;
            let group_id = self.group.load(Relaxed);
            let nstreams = self.nstreams.load(Relaxed);
            let latency_ns = self.latency_ms.load(Relaxed) * 1_000_000;
            let entry = sync_group::get_or_create(group_id, nstreams, latency_ns);
            *self.group_entry.lock().unwrap() = Some(entry);
            Ok(())
        }

        fn stop(&self) -> Result<(), gst::ErrorMessage> {
            // Wake any waiters so they don't hang on pipeline teardown.
            if let Some(entry) = self.group_entry.lock().unwrap().take() {
                {
                    let mut gs = entry.state.lock().unwrap();
                    for slot in gs.slots.values_mut() {
                        slot.evicted = true;
                    }
                }
                entry.condvar.notify_all();
            }
            Ok(())
        }

        /// Timestamp-aligned synchronisation.
        ///
        /// Each buffer is deposited into a shared slot keyed by its
        /// GROUP_TIMESTAMP_NS.  The calling thread then waits until either:
        ///   a) all nstreams have deposited into that slot (aligned release), or
        ///   b) the slot is evicted (latency window expired) — pass-through.
        ///
        /// The buffer is modified in-place (AlwaysInPlace mode): we swap the
        /// aligned buffer back into *buf before returning.
        ///
        /// Delay-freeze avoidance: instead of waiting the full latency_ms in a
        /// single condvar wait, we loop in short CHECK_INTERVAL_MS increments.
        /// On each wake we re-evaluate newest_ts.  If newest_ts has advanced
        /// past ts + latency_ns (meaning other streams are clearly ahead and the
        /// current slot is genuinely stale), we evict immediately rather than
        /// waiting for the remainder of the full timeout.  This bounds the
        /// per-frame stall to CHECK_INTERVAL_MS when a stream is artificially
        /// delayed, while still honouring the full latency window when all
        /// streams are close together.
        fn transform_ip(
            &self,
            buf: &mut gst::BufferRef,
        ) -> Result<gst::FlowSuccess, gst::FlowError> {
            use std::sync::atomic::Ordering::Relaxed;

            let stream_id = self.stream.load(Relaxed) as usize;

            let ts = match read_group_ts(buf) {
                Some(ts) => ts,
                None => {
                    // Malformed: pass through immediately.
                    return Ok(gst::FlowSuccess::Ok);
                }
            };

            let entry = match self.group_entry.lock().unwrap().as_ref().cloned() {
                Some(e) => e,
                None => return Ok(gst::FlowSuccess::Ok),
            };

            // ── Deposit buffer into slot ──────────────────────────────────────
            //
            // We need to give the buffer to the shared slot.  BaseTransform
            // gives us a `&mut BufferRef` — we can't move it.  Instead we
            // replace its contents with the aligned buffer when we wake up.
            //
            // Strategy: copy the buffer's bytes out, store a new GstBuffer in
            // the slot.  On wake we copy the (possibly different) aligned bytes
            // back.  For in-order same-timestamp slots the bytes are identical,
            // but this keeps the code simple and correct.
            //
            // For a 640×360 H.265 stream the per-frame copy is at most a few
            // KB (compressed), so the overhead is negligible.
            let buf_copy: gst::Buffer = {
                let map = buf.map_readable().map_err(|_| gst::FlowError::Error)?;
                let mut copy =
                    gst::Buffer::with_size(map.len()).map_err(|_| gst::FlowError::Error)?;
                {
                    let cr = copy.get_mut().unwrap();
                    cr.map_writable()
                        .map_err(|_| gst::FlowError::Error)?
                        .copy_from_slice(map.as_slice());
                    // Preserve flags/timestamps from original.
                    cr.set_pts(buf.pts());
                    cr.set_dts(buf.dts());
                    cr.set_flags(buf.flags());
                }
                copy
            };

            // Deposit into slot and determine role.
            let (slot_complete, slot_evicted) = {
                let mut gs = entry.state.lock().unwrap();

                // Update newest timestamp and latency window.
                if ts > gs.newest_ts {
                    gs.newest_ts = ts;
                }
                let latency_ns = gs.latency_ns;
                let evict_before = gs.newest_ts.saturating_sub(latency_ns);

                // Evict stale slots (older than the window).
                let to_evict: Vec<u64> = gs
                    .slots
                    .keys()
                    .copied()
                    .filter(|&k| k < evict_before)
                    .collect();
                let mut any_evicted = false;
                for k in to_evict {
                    if let Some(slot) = gs.slots.get_mut(&k) {
                        if !slot.ready {
                            slot.evicted = true;
                            any_evicted = true;
                            gs.frames_dropped += 1;
                        }
                    }
                }
                if any_evicted {
                    entry.condvar.notify_all();
                }

                // Get or create the slot for our timestamp.
                let nstreams = gs.nstreams as usize;
                let slot = gs
                    .slots
                    .entry(ts)
                    .or_insert_with(|| sync_group::Slot::new(nstreams));

                // Deposit our buffer (overwrite if we already deposited — shouldn't
                // happen in normal operation but guards against duplicate delivery).
                if slot.buffers[stream_id].is_none() {
                    slot.count += 1;
                }
                slot.buffers[stream_id] = Some(buf_copy);

                // Extract slot fields before releasing the mutable borrow of gs
                // so we can update gs.frames_synced without a double-borrow error.
                let complete = slot.count == nstreams as u32;
                let already_ready = slot.ready;
                let slot_evicted_flag = slot.evicted;
                if complete && !already_ready {
                    slot.ready = true;
                }
                let slot_ready_now = slot.ready;
                let _ = slot; // release mutable borrow of gs (drop reference)

                if complete && !already_ready {
                    gs.frames_synced += 1;
                    entry.condvar.notify_all();
                }

                gst::debug!(
                    cat(),
                    "[fluxsync] stream={} deposited ts={} slot_count={}/{} complete={} evicted={}",
                    stream_id,
                    ts,
                    gs.slots.get(&ts).map(|s| s.count).unwrap_or(0),
                    gs.nstreams,
                    slot_ready_now,
                    slot_evicted_flag,
                );

                (slot_ready_now, slot_evicted_flag)
            };

            // ── Wait for slot to complete or be evicted ───────────────────────
            if !slot_complete && !slot_evicted {
                let latency_ns = self.latency_ms.load(Relaxed) * 1_000_000;
                let deadline = std::time::Instant::now()
                    + Duration::from_millis(self.latency_ms.load(Relaxed));

                loop {
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    let check = remaining.min(Duration::from_millis(CHECK_INTERVAL_MS));

                    let gs = entry.state.lock().unwrap();
                    let (guard, _timed_out) = entry
                        .condvar
                        .wait_timeout_while(gs, check, |gs| {
                            if let Some(slot) = gs.slots.get(&ts) {
                                !slot.ready && !slot.evicted
                            } else {
                                false // slot removed — wake up
                            }
                        })
                        .unwrap();

                    // Check if slot is now done.
                    if let Some(slot) = guard.slots.get(&ts) {
                        if slot.ready || slot.evicted {
                            break;
                        }
                        // Early eviction: if newest_ts has advanced past ts +
                        // latency_ns, this slot is genuinely stale (delayed
                        // stream case). Evict now rather than waiting for the
                        // full timeout to expire.
                        if guard.newest_ts > ts.saturating_add(latency_ns) {
                            break; // will be evicted in post-wait block below
                        }
                    } else {
                        break; // slot removed
                    }
                    drop(guard);
                }

                // After the loop, evict if the slot is still incomplete.
                {
                    let mut gs = entry.state.lock().unwrap();
                    if let Some(slot) = gs.slots.get_mut(&ts) {
                        if !slot.ready && !slot.evicted {
                            slot.evicted = true;
                            gs.frames_dropped += 1;
                            entry.condvar.notify_all();
                        }
                    }
                }
            }

            // ── Retrieve aligned buffer from slot ─────────────────────────────
            //
            // Two cases:
            //
            //   slot.ready  — All streams deposited.  We take back our own
            //                 buf_copy (bytes == original buf bytes) and copy
            //                 timestamps back.  The byte copy is a no-op in
            //                 practice (same data) but keeps the code uniform.
            //
            //   slot.evicted / slot removed — Only some streams deposited;
            //                 the slot was evicted (timeout or state change).
            //                 Our own buf_copy is still in the slot.  We take
            //                 it, copy timestamps, but deliberately SKIP the
            //                 byte copy: buf already contains the correct
            //                 content (our original frame bytes) and copying
            //                 is unnecessary.  More importantly we must NOT
            //                 accidentally copy a different stream's buffer if
            //                 the slot index was somehow wrong.
            {
                let mut gs = entry.state.lock().unwrap();
                let is_ready = gs.slots.get(&ts).map(|s| s.ready).unwrap_or(false);

                if let Some(slot) = gs.slots.get_mut(&ts) {
                    if let Some(aligned_buf) = slot.buffers[stream_id].take() {
                        if is_ready {
                            // Aligned release: copy bytes + timestamps.
                            let src_map = aligned_buf
                                .map_readable()
                                .map_err(|_| gst::FlowError::Error)?;
                            if src_map.len() == buf.size() {
                                let mut dst_map =
                                    buf.map_writable().map_err(|_| gst::FlowError::Error)?;
                                dst_map.copy_from_slice(src_map.as_slice());
                            }
                            drop(src_map);
                        }
                        // Always update timestamps from our own copy (they are
                        // identical to buf's timestamps but make it explicit).
                        buf.set_pts(aligned_buf.pts());
                        buf.set_dts(aligned_buf.dts());
                        buf.set_flags(aligned_buf.flags());
                    }
                    // Remove slot when all streams have taken their buffer.
                    let all_taken = slot
                        .buffers
                        .iter()
                        .all(|b: &Option<gst::Buffer>| b.is_none());
                    if all_taken {
                        gs.slots.remove(&ts);
                    }
                }
                // else: slot was already removed (e.g. cleared on PlayingToPaused)
                //       — pass through as-is (buf already has the correct content).
            }

            gst::debug!(
                cat(),
                "[fluxsync] stream={} ts={} → released",
                stream_id,
                ts,
            );

            Ok(gst::FlowSuccess::Ok)
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn read_group_ts(buf: &gst::BufferRef) -> Option<u64> {
        let map = buf.map_readable().ok()?;
        let data = map.as_slice();
        if data.len() < HEADER_SIZE {
            return None;
        }
        let hdr = FluxHeader::decode(data)?;
        Some(hdr.group_timestamp_ns)
    }
}
