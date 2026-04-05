//! FLUX wire-format: 32-byte header, frame types, JSON structs.
//!
//! Spec reference: FLUX_Protocol_Spec_v0_6_2_EN.md §4

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

// ─── Constants ───────────────────────────────────────────────────────────────

pub const FLUX_VERSION: u8 = 6;
pub const HEADER_SIZE: usize = 32;

/// Default media port (spec §7.1)
pub const DEFAULT_PORT: u16 = 7400;
/// Default monitor port
pub const DEFAULT_MONITOR_PORT: u16 = 7401;
/// Default registry port
pub const DEFAULT_REGISTRY_PORT: u16 = 7500;

/// 90 kHz presentation timestamp clock (spec §4.1)
pub const PTS_CLOCK_HZ: u64 = 90_000;

// ─── Frame types (spec §4.3) ──────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    MediaData = 0x0,
    CdbcFeedbackT = 0x1,
    SyncAnchor = 0x2,
    LayerStatus = 0x3,
    QualityRequest = 0x4,
    StreamAnnounce = 0x5,
    StreamEnd = 0x6,
    FecRepair = 0x7,
    SessionInfo = 0x8,
    Keepalive = 0x9,
    TallyUpdate = 0xA,
    AncData = 0xB,
    MetadataFrame = 0xC,
    BandwidthProbe = 0xD,
    EmbedManifest = 0xE,
    EmbedChunk = 0xF,
    // v0.6 additions (spec §4.3)
    FluxmKeyEpoch = 0x10,
    FluxmNack = 0x11,
    FluxmStat = 0x12,
}

impl FrameType {
    /// Decode a raw frame-type byte.
    ///
    /// The `& 0x0F` mask that existed in v0.4 is intentionally removed: types
    /// 0x10–0x12 were added in v0.6 and would be silently corrupted by it.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x0 => Some(Self::MediaData),
            0x1 => Some(Self::CdbcFeedbackT),
            0x2 => Some(Self::SyncAnchor),
            0x3 => Some(Self::LayerStatus),
            0x4 => Some(Self::QualityRequest),
            0x5 => Some(Self::StreamAnnounce),
            0x6 => Some(Self::StreamEnd),
            0x7 => Some(Self::FecRepair),
            0x8 => Some(Self::SessionInfo),
            0x9 => Some(Self::Keepalive),
            0xA => Some(Self::TallyUpdate),
            0xB => Some(Self::AncData),
            0xC => Some(Self::MetadataFrame),
            0xD => Some(Self::BandwidthProbe),
            0xE => Some(Self::EmbedManifest),
            0xF => Some(Self::EmbedChunk),
            0x10 => Some(Self::FluxmKeyEpoch),
            0x11 => Some(Self::FluxmNack),
            0x12 => Some(Self::FluxmStat),
            _ => None,
        }
    }
}

// ─── FLAGS bits (spec §4.2) ──────────────────────────────────────────────────

pub mod flags {
    pub const KEYFRAME: u8 = 1 << 7;
    pub const ENCRYPTED: u8 = 1 << 6;
    pub const DROP_ELIGIBLE: u8 = 1 << 5;
    pub const EMBED_ASSOC: u8 = 1 << 4;
    pub const MONITOR_COPY: u8 = 1 << 3;
    pub const SYNC_MASTER: u8 = 1 << 2;
    pub const LAST_IN_GOP: u8 = 1 << 1;
    pub const HAS_METADATA: u8 = 1 << 0;
}

// ─── 32-byte FLUX header (spec §4.1) ─────────────────────────────────────────
//
//  Byte  0: VER(4) | TYPE(4)
//  Byte  1: FLAGS(8)
//  Bytes 2–3: CHANNEL_ID(16)
//  Byte  4: LAYER(4) | FRAG(4)
//  Bytes 5–6: GROUP_ID(16)
//  Byte  7: RSVD(8)
//  Bytes 8–11:  GROUP_TIMESTAMP_NS bits 63..32
//  Bytes 12–15: GROUP_TIMESTAMP_NS bits 31..0
//  Bytes 16–19: PRESENTATION_TS (32-bit, 90 kHz)
//  Bytes 20–23: CAPTURE_TS_NS_LO (low 32 bits)
//  Bytes 24–26: PAYLOAD_LENGTH(24)
//  Byte  27:    FEC_GROUP(8)
//  Bytes 28–31: SEQUENCE_IN_GROUP(32)

#[derive(Debug, Clone)]
pub struct FluxHeader {
    pub version: u8, // 4-bit, should be FLUX_VERSION
    pub frame_type: FrameType,
    pub flags: u8,
    pub channel_id: u16,
    pub layer: u8, // 4-bit
    pub frag: u8,  // 4-bit  (0 = not fragmented)
    pub group_id: u16,
    pub group_timestamp_ns: u64, // full PTP timestamp
    pub presentation_ts: u32,    // 90 kHz clock
    pub capture_ts_ns_lo: u32,   // low 32 bits of capture ns
    pub payload_length: u32,     // 24-bit on wire
    pub fec_group: u8,
    pub sequence_in_group: u32,
}

impl FluxHeader {
    /// Create a MEDIA_DATA header for an H.265 buffer.
    ///
    /// `pts_ns` and `dts_ns` should come from the upstream GStreamer buffer's
    /// PTS and DTS (nanoseconds from the pipeline clock origin, i.e. small
    /// values starting near 0 at pipeline start).  Using wall-clock `now_ns()`
    /// here caused u32 overflow in `presentation_ts` (90 kHz ticks of a wall-
    /// clock timestamp exceed 2³² after ~13 hours, but wrap randomly on a
    /// freshly started pipeline because the Unix epoch offset is already huge).
    pub fn new_media(
        channel_id: u16,
        group_id: u16,
        layer: u8,
        is_keyframe: bool,
        payload_len: u32,
        seq: u32,
        pts_ns: u64,
        dts_ns: u64,
    ) -> Self {
        // presentation_ts: 90 kHz ticks.  pts_ns is small (pipeline clock),
        // so the conversion and the u32 truncation are safe for sessions up to
        // ~13 hours (2^32 / 90000 ≈ 47721 s).
        let pts_90k = (pts_ns / (1_000_000_000 / PTS_CLOCK_HZ)) as u32;
        let mut f: u8 = 0;
        if is_keyframe {
            f |= flags::KEYFRAME;
        }

        FluxHeader {
            version: FLUX_VERSION,
            frame_type: FrameType::MediaData,
            flags: f,
            channel_id,
            layer,
            frag: 0,
            group_id,
            // group_timestamp_ns carries the full DTS (nanoseconds from
            // pipeline clock origin) so the receiver can use it as DTS.
            group_timestamp_ns: dts_ns,
            presentation_ts: pts_90k,
            // capture_ts_ns_lo: low 32 bits of DTS for reconstruct_capture_ts.
            capture_ts_ns_lo: (dts_ns & 0xFFFF_FFFF) as u32,
            payload_length: payload_len,
            fec_group: 0,
            sequence_in_group: seq,
        }
    }

    /// Create a KEEPALIVE header.
    pub fn new_keepalive(channel_id: u16, seq: u32) -> Self {
        let now_ns = now_ns();
        FluxHeader {
            version: FLUX_VERSION,
            frame_type: FrameType::Keepalive,
            flags: 0,
            channel_id,
            layer: 0,
            frag: 0,
            group_id: 0,
            group_timestamp_ns: now_ns,
            presentation_ts: 0,
            capture_ts_ns_lo: 0,
            payload_length: 0,
            fec_group: 0,
            sequence_in_group: seq,
        }
    }

    /// Serialize header into 32-byte array (big-endian).
    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut b = [0u8; HEADER_SIZE];

        // byte 0: VER(4) | TYPE(4)
        b[0] = ((self.version & 0x0F) << 4) | (self.frame_type as u8 & 0x0F);
        // byte 1: FLAGS
        b[1] = self.flags;
        // bytes 2-3: CHANNEL_ID
        let ch = self.channel_id.to_be_bytes();
        b[2] = ch[0];
        b[3] = ch[1];
        // byte 4: LAYER(4) | FRAG(4)
        b[4] = ((self.layer & 0x0F) << 4) | (self.frag & 0x0F);
        // bytes 5-6: GROUP_ID
        let gid = self.group_id.to_be_bytes();
        b[5] = gid[0];
        b[6] = gid[1];
        // byte 7: RSVD = 0
        b[7] = 0;
        // bytes 8-11: GROUP_TIMESTAMP_NS high 32
        let gts_hi = ((self.group_timestamp_ns >> 32) as u32).to_be_bytes();
        b[8..12].copy_from_slice(&gts_hi);
        // bytes 12-15: GROUP_TIMESTAMP_NS low 32
        let gts_lo = (self.group_timestamp_ns as u32).to_be_bytes();
        b[12..16].copy_from_slice(&gts_lo);
        // bytes 16-19: PRESENTATION_TS
        b[16..20].copy_from_slice(&self.presentation_ts.to_be_bytes());
        // bytes 20-23: CAPTURE_TS_NS_LO
        b[20..24].copy_from_slice(&self.capture_ts_ns_lo.to_be_bytes());
        // bytes 24-26: PAYLOAD_LENGTH (24-bit)
        let pl = self.payload_length;
        b[24] = ((pl >> 16) & 0xFF) as u8;
        b[25] = ((pl >> 8) & 0xFF) as u8;
        b[26] = (pl & 0xFF) as u8;
        // byte 27: FEC_GROUP
        b[27] = self.fec_group;
        // bytes 28-31: SEQUENCE_IN_GROUP
        b[28..32].copy_from_slice(&self.sequence_in_group.to_be_bytes());

        b
    }

    /// Decode a 32-byte slice into a FluxHeader. Returns None if version or
    /// type are invalid.
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < HEADER_SIZE {
            return None;
        }

        let ver = (b[0] >> 4) & 0x0F;
        let typ = b[0] & 0x0F;
        let ft = FrameType::from_u8(typ)?;

        let ch_id = u16::from_be_bytes([b[2], b[3]]);
        let layer = (b[4] >> 4) & 0x0F;
        let frag = b[4] & 0x0F;
        let grp = u16::from_be_bytes([b[5], b[6]]);

        let gts_hi = u32::from_be_bytes([b[8], b[9], b[10], b[11]]) as u64;
        let gts_lo = u32::from_be_bytes([b[12], b[13], b[14], b[15]]) as u64;
        let gts = (gts_hi << 32) | gts_lo;

        let pts = u32::from_be_bytes([b[16], b[17], b[18], b[19]]);
        let cts_lo = u32::from_be_bytes([b[20], b[21], b[22], b[23]]);
        let pl = ((b[24] as u32) << 16) | ((b[25] as u32) << 8) | (b[26] as u32);
        let fec = b[27];
        let seq = u32::from_be_bytes([b[28], b[29], b[30], b[31]]);

        Some(FluxHeader {
            version: ver,
            frame_type: ft,
            flags: b[1],
            channel_id: ch_id,
            layer,
            frag,
            group_id: grp,
            group_timestamp_ns: gts,
            presentation_ts: pts,
            capture_ts_ns_lo: cts_lo,
            payload_length: pl,
            fec_group: fec,
            sequence_in_group: seq,
        })
    }

    pub fn is_keyframe(&self) -> bool {
        self.flags & flags::KEYFRAME != 0
    }
    pub fn has_metadata(&self) -> bool {
        self.flags & flags::HAS_METADATA != 0
    }
    pub fn is_drop_eligible(&self) -> bool {
        self.flags & flags::DROP_ELIGIBLE != 0
    }
}

// ─── CAPTURE_TS_NS_LO wraparound reconstruction (spec §4.2) ─────────────────

/// Reconstruct full 64-bit capture timestamp from the low-32 field and the
/// known GROUP_TIMESTAMP_NS. Valid when |capture_ts – group_ts| < 2.147 s.
///
/// Comparisons are performed on 32-bit values (wrapping) to correctly handle
/// modular arithmetic at the 2³² boundary.  Using 64-bit arithmetic caused
/// off-by-one errors at the epoch boundary.
pub fn reconstruct_capture_ts(group_ts_ns: u64, capture_ts_lo: u32) -> u64 {
    let c = capture_ts_lo;
    let g_lo = group_ts_ns as u32; // low 32 bits — intentional truncation
    let candidate_hi = group_ts_ns & 0xFFFF_FFFF_0000_0000;

    // All comparisons are done on u32, so wrapping at 2³² is automatic.
    if c > g_lo.wrapping_add(1u32 << 31) {
        // C is from the previous wrap epoch
        candidate_hi.wrapping_sub(1u64 << 32) | (c as u64)
    } else if g_lo > c.wrapping_add(1u32 << 31) {
        // C is from the next wrap epoch
        candidate_hi.wrapping_add(1u64 << 32) | (c as u64)
    } else {
        candidate_hi | (c as u64)
    }
}

// ─── JSON message structs ─────────────────────────────────────────────────────

/// Embed capabilities declared by the client in SESSION_REQUEST (spec §3.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedSupport {
    #[serde(default)]
    pub max_concurrent_assets: u32,
    #[serde(default)]
    pub max_asset_size_mb: u32,
    #[serde(default)]
    pub delta_support: bool,
    #[serde(default)]
    pub sequence_support: bool,
    #[serde(default)]
    pub video_texture_binding_support: bool,
    /// GS residual codec identifiers (spec §3.2 / §10.9): "raw-attr", "queen-v1"
    #[serde(default)]
    pub gs_codecs: Vec<String>,
    #[serde(default)]
    pub mime_types: Vec<String>,
}

impl Default for EmbedSupport {
    fn default() -> Self {
        EmbedSupport {
            max_concurrent_assets: 8,
            max_asset_size_mb: 512,
            delta_support: true,
            sequence_support: true,
            video_texture_binding_support: true,
            gs_codecs: vec!["raw-attr".into()],
            mime_types: vec![
                "model/gltf-binary".into(),
                "model/vnd.gaussian-splat".into(),
                "application/vnd.flux.gs-residual".into(),
                "application/vnd.flux.gs-delta".into(),
                "application/json".into(),
                "application/octet-stream".into(),
            ],
        }
    }
}

/// Upstream-control capabilities (spec §12).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UpstreamControl {
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub max_commands_per_second: u32,
}

/// Cached embed asset entry declared by the client (spec §3.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedCacheEntry {
    pub asset_id: String,
    pub sha256: String,
}

/// SESSION_REQUEST from client → server (spec §3.1 / §3.2)
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionRequest {
    pub flux_version: String,
    /// Informative profile field (spec §3.2 v0.6 note). Optional — servers treat
    /// its absence as `"flux_quic"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flux_profile: Option<String>,
    pub client_id: String,
    pub crypto_mode: String,
    pub codec_support: Vec<String>,
    pub max_channels: u8,
    pub max_layers: u8,
    pub max_fps: u16,
    pub sync_mode: String,
    pub ptp_mode: String,
    #[serde(default)]
    pub sync_tolerance_ns: u64,
    #[serde(default)]
    pub fec_support: Vec<String>,
    pub cdbc_interval_ms: u32,
    #[serde(default)]
    pub cdbc_interval_min_ms: u32,
    #[serde(default)]
    pub hdr_support: Vec<String>,
    #[serde(default)]
    pub audio_formats: Vec<String>,
    #[serde(default)]
    pub embed_support: EmbedSupport,
    #[serde(default)]
    pub tally_support: bool,
    #[serde(default)]
    pub monitor_stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_control: Option<UpstreamControl>,
    /// Previously-cached embed assets (spec §3.2).
    #[serde(default)]
    pub embed_cache: Vec<EmbedCacheEntry>,
}

impl Default for SessionRequest {
    fn default() -> Self {
        SessionRequest {
            flux_version: "0.6.2".into(),
            flux_profile: Some("flux_quic".into()),
            client_id: "FLUX-POC-CLIENT-01".into(),
            crypto_mode: "crypto_quic".into(),
            codec_support: vec!["h265".into()],
            max_channels: 4,
            max_layers: 1,
            max_fps: 60,
            sync_mode: "frame_sync".into(),
            ptp_mode: "software".into(),
            sync_tolerance_ns: 500_000,
            fec_support: vec!["xor".into()],
            cdbc_interval_ms: 50,
            cdbc_interval_min_ms: 10,
            hdr_support: vec!["sdr".into(), "hlg".into(), "pq".into()],
            audio_formats: vec!["pcm_f32".into()],
            embed_support: EmbedSupport::default(),
            tally_support: false,
            monitor_stream: false,
            upstream_control: None,
            embed_cache: vec![],
        }
    }
}

/// Stream descriptor returned in SESSION_ACCEPT (spec §3.1).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StreamDescriptor {
    pub channel_id: u16,
    pub layer: u8,
    pub codec: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub fps_num: u32,
    #[serde(default)]
    pub fps_den: u32,
    #[serde(default)]
    pub bit_depth: u8,
    #[serde(default)]
    pub hdr_mode: String,
    #[serde(default)]
    pub audio_channels: u8,
    #[serde(default)]
    pub audio_sample_rate: u32,
    #[serde(default)]
    pub audio_format: String,
}

/// SESSION_ACCEPT from server → client (spec §3.1)
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionAccept {
    pub flux_version: String,
    pub session_id: String,
    /// Active streams (one entry per channel × layer) — may be empty in PoC
    /// until STREAM_ANNOUNCE is implemented.
    #[serde(default)]
    pub streams: Vec<StreamDescriptor>,
    /// Group IDs announced by this server.
    #[serde(default)]
    pub group_ids: Vec<u16>,
    /// FEC schema in use: "none", "xor", "rs_2d".
    #[serde(default = "default_fec_schema")]
    pub fec_schema: String,
    /// PTP anchor timestamp (ns) for clock synchronisation.
    #[serde(default)]
    pub ptp_anchor_ns: u64,
    /// Embed catalog entries available on this server.
    #[serde(default)]
    pub embed_catalog: Vec<String>,
    /// Channel/stream ID of the monitor copy, if any.
    #[serde(default)]
    pub monitor_stream_id: Option<u16>,
    pub crypto_mode_ack: String,
    pub max_datagram_size: u32,
    pub keepalive_interval_ms: u32,
    /// Number of missed keepalive intervals before the session is declared dead (spec §3.3).
    pub keepalive_timeout_count: u32,
}

fn default_fec_schema() -> String {
    "none".into()
}

impl Default for SessionAccept {
    fn default() -> Self {
        SessionAccept {
            flux_version: "0.6.2".into(),
            session_id: "poc-session-001".into(),
            streams: vec![],
            group_ids: vec![],
            fec_schema: "none".into(),
            ptp_anchor_ns: 0,
            embed_catalog: vec![],
            monitor_stream_id: None,
            crypto_mode_ack: "crypto_quic".into(),
            keepalive_interval_ms: 1000,
            keepalive_timeout_count: 3,
            max_datagram_size: 65000,
        }
    }
}

/// STREAM_ANNOUNCE payload (spec §6.2).
///
/// Sent by the server on a reliable QUIC uni-directional stream immediately
/// after SESSION_ACCEPT — one frame per channel × layer.  The JSON body is
/// wrapped with the standard 32-byte FLUX header (TYPE=0x5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamAnnounce {
    pub channel_id: u16,
    pub layer_id: u8,
    pub name: String,
    pub content_type: String,
    pub codec: String,
    #[serde(default)]
    pub group_id: u16,
    /// Sync role: "master" | "slave" | "independent"
    #[serde(default = "default_sync_role")]
    pub sync_role: String,
    /// Frame rate as "num/den" string, e.g. "60/1".
    #[serde(default)]
    pub frame_rate: String,
    /// Resolution as "WxH" string, e.g. "1920x1080".
    #[serde(default)]
    pub resolution: String,
    #[serde(default)]
    pub hdr: String,
    #[serde(default)]
    pub colorspace: String,
    /// Optional GLB video-texture binding (spec §6.2 v0.5+).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glb_texture_role: Option<GlbTextureRole>,
}

fn default_sync_role() -> String {
    "master".into()
}

/// GLB video-texture binding declared in STREAM_ANNOUNCE (spec §6.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlbTextureRole {
    pub asset_id: String,
    pub material_path: String,
    pub slot: String,
    #[serde(default)]
    pub hint_resolution: String,
    #[serde(default)]
    pub hint_format: String,
}

/// CDBC_FEEDBACK datagram (spec §5.2)
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct CdbcFeedback {
    pub ts_ns: u64,
    pub rx_bps: u64,
    pub avail_bps: u64,
    pub rtt_ms: f64,
    pub loss_pct: f64,
    pub jitter_ms: f64,
    pub fps_actual: f64,
    pub datagram_drop_count: u64,
    /// Measured receive bandwidth from the most recent BANDWIDTH_PROBE (bps).
    /// Zero if no probe result is available.
    #[serde(default)]
    pub probe_result_bps: u64,
}

/// KEEPALIVE payload (spec §3.3)
#[derive(Debug, Serialize, Deserialize)]
pub struct KeepalivePayload {
    pub ts_ns: u64,
    pub session_id: String,
    pub seq: u32,
}

// ─── FLUX-C upstream control (spec §12) ──────────────────────────────────────

/// The `type` values carried inside a `FluxControl` JSON body (spec §12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlType {
    /// PTZ move/zoom/focus command (spec §12.2).
    Ptz,
    /// Audio gain/mute per-channel (spec §12.3).
    AudioMix,
    /// Source routing redirect request (spec §7.3).
    Routing,
    /// PoC extension — request the server source to switch videotestsrc pattern.
    /// `pattern_id` maps directly to GStreamer `videotestsrc pattern` enum values.
    TestPattern,
}

/// FLUX-C upstream control datagram (spec §12).
///
/// Sent client → server as a `MetadataFrame (0xC)` datagram.
/// The server is not required to acknowledge in the PoC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FluxControl {
    /// Discriminant — which command this is.
    #[serde(rename = "type")]
    pub control_type: ControlType,
    /// Wall-clock send time.
    pub ts_ns: u64,
    /// Session this command belongs to.
    pub session_id: String,

    // ── PTZ fields (all optional) ─────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pan_deg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tilt_deg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom_pos: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_pos: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,

    // ── AudioMix fields ───────────────────────────────────────────────────
    /// Per-channel mute state  (index = channel id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mute: Option<Vec<bool>>,
    /// Per-channel gain in dB  (index = channel id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<Vec<f64>>,

    // ── Routing fields ────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,

    // ── TestPattern fields ────────────────────────────────────────────────
    /// GStreamer `videotestsrc` pattern enum value (0=smpte, 1=snow, 2=black,
    /// 3=white, 18=ball, 19=smpte75, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern_id: Option<u32>,
}

impl FluxControl {
    /// Build a PTZ command targeting `channel_id`.
    pub fn ptz(
        session_id: &str,
        channel_id: u16,
        pan_deg: f64,
        tilt_deg: f64,
        zoom_pos: f64,
        focus_pos: f64,
        speed: f64,
    ) -> Self {
        FluxControl {
            control_type: ControlType::Ptz,
            ts_ns: now_ns(),
            session_id: session_id.into(),
            channel_id: Some(channel_id),
            pan_deg: Some(pan_deg),
            tilt_deg: Some(tilt_deg),
            zoom_pos: Some(zoom_pos),
            focus_pos: Some(focus_pos),
            speed: Some(speed),
            mute: None,
            gain_db: None,
            target_id: None,
            pattern_id: None,
        }
    }

    /// Build an audio-mix command (mute/unmute per channel).
    pub fn audio_mix(session_id: &str, mute: Vec<bool>, gain_db: Vec<f64>) -> Self {
        FluxControl {
            control_type: ControlType::AudioMix,
            ts_ns: now_ns(),
            session_id: session_id.into(),
            channel_id: None,
            pan_deg: None,
            tilt_deg: None,
            zoom_pos: None,
            focus_pos: None,
            speed: None,
            mute: Some(mute),
            gain_db: Some(gain_db),
            target_id: None,
            pattern_id: None,
        }
    }

    /// Build a routing redirect request.
    pub fn routing(session_id: &str, target_id: &str) -> Self {
        FluxControl {
            control_type: ControlType::Routing,
            ts_ns: now_ns(),
            session_id: session_id.into(),
            channel_id: None,
            pan_deg: None,
            tilt_deg: None,
            zoom_pos: None,
            focus_pos: None,
            speed: None,
            mute: None,
            gain_db: None,
            target_id: Some(target_id.into()),
            pattern_id: None,
        }
    }

    /// Build a test-pattern switch command (PoC extension).
    ///
    /// `pattern_id` maps to GStreamer `videotestsrc` pattern enum:
    ///   0=smpte  1=snow  2=black  3=white  4=red  5=green  6=blue
    ///   7=checkers-1  18=ball  19=smpte75  24=circular
    pub fn test_pattern(session_id: &str, pattern_id: u32) -> Self {
        FluxControl {
            control_type: ControlType::TestPattern,
            ts_ns: now_ns(),
            session_id: session_id.into(),
            channel_id: None,
            pan_deg: None,
            tilt_deg: None,
            zoom_pos: None,
            focus_pos: None,
            speed: None,
            mute: None,
            gain_db: None,
            target_id: None,
            pattern_id: Some(pattern_id),
        }
    }

    /// Encode this command into a complete FLUX datagram (header + JSON body).
    ///
    /// Uses `MetadataFrame (0xC)` as the frame type — the `type` field in the
    /// JSON body distinguishes it from per-frame media metadata (spec §14).
    pub fn encode_datagram(&self, seq: u32) -> Vec<u8> {
        let body = serde_json::to_vec(self).unwrap_or_default();
        let hdr = FluxHeader {
            version: FLUX_VERSION,
            frame_type: FrameType::MetadataFrame,
            flags: 0,
            channel_id: self.channel_id.unwrap_or(0),
            layer: 0,
            frag: 0,
            group_id: 0,
            group_timestamp_ns: self.ts_ns,
            presentation_ts: 0,
            capture_ts_ns_lo: 0,
            payload_length: body.len() as u32,
            fec_group: 0,
            sequence_in_group: seq,
        };
        let mut dg = Vec::with_capacity(HEADER_SIZE + body.len());
        dg.extend_from_slice(&hdr.encode());
        dg.extend_from_slice(&body);
        dg
    }

    /// Try to parse a `FluxControl` from the body (everything after the 32-byte header).
    pub fn decode_body(body: &[u8]) -> Option<Self> {
        serde_json::from_slice(body).ok()
    }
}

/// BANDWIDTH_PROBE payload (spec §5.3 / frame type 0xD)
///
/// Sent server → client as a padded datagram; the client measures the
/// arrival bandwidth and returns it in the next CDBC_FEEDBACK.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BandwidthProbe {
    /// Timestamp when the probe was sent (ns since UNIX epoch).
    pub ts_ns: u64,
    /// Probe sequence number (monotonically increasing per session).
    pub probe_seq: u32,
    /// Nominal probe size in bytes (so the receiver can verify it).
    pub probe_size: u32,
}

// ─── BW Governor (spec §5.3) ──────────────────────────────────────────────────

/// States of the server-side BW Governor state machine (spec §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BwState {
    /// Sending BANDWIDTH_PROBE datagrams; waiting for probe_result_bps.
    Probe,
    /// Link is healthy; holding current layer set.
    Stable,
    /// Available bandwidth is growing; ramping up to add an enhancement layer.
    RampUp,
    /// Available bandwidth is shrinking; dropping top enhancement layer.
    RampDown,
    /// Severe congestion; executing shed-then-protect sequence (spec §5.4).
    Emergency,
}

/// Server-side BW Governor.
///
/// Call [`BwGovernor::ingest`] each time a `CdbcFeedback` arrives.
/// The returned [`BwAction`] tells the caller what to do next.
pub struct BwGovernor {
    pub state: BwState,
    /// Current baseline bitrate in bps (set from last accepted report).
    pub current_bps: u64,
    /// Number of consecutive RAMP_UP-qualifying reports.
    ramp_up_count: u32,
    /// Number of consecutive loss-free reports during EMERGENCY recovery.
    recovery_count: u32,
    /// Timestamp of the last state transition (for probe timeout logic).
    pub state_entered: std::time::Instant,
}

/// Action the server should take after processing a CDBC_FEEDBACK report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BwAction {
    /// Nothing to do; continue as-is.
    Hold,
    /// Send a BANDWIDTH_PROBE datagram now.
    SendProbe,
    /// Add an enhancement layer (increase quality / bitrate).
    AddLayer,
    /// Drop the top enhancement layer.
    DropLayer,
    /// Execute EMERGENCY shed sequence: drop all enhancement layers, disable
    /// monitor streams, pause EMBED_CHUNK, reduce fps if negotiated.
    EmergencyShed,
    /// EMERGENCY step 3: enable XOR Row FEC on base layer (loss_pct > 5 %).
    EnableFec,
    /// EMERGENCY step 3: switch to Reed-Solomon 2D + IDR-only mode (loss > 15 %).
    EnableFecRS,
    /// Transition from EMERGENCY back to RAMP_UP after 5 clean reports.
    RecoveryRampUp,
}

impl BwGovernor {
    pub fn new() -> Self {
        BwGovernor {
            state: BwState::Probe,
            current_bps: 0,
            ramp_up_count: 0,
            recovery_count: 0,
            state_entered: std::time::Instant::now(),
        }
    }

    /// Process one CDBC_FEEDBACK report and return the recommended action.
    ///
    /// Implements the state machine from spec §5.3 + §5.4.
    pub fn ingest(&mut self, fb: &CdbcFeedback) -> BwAction {
        let avail = fb.avail_bps;
        let loss = fb.loss_pct;

        match self.state {
            BwState::Probe => {
                // If we got a probe result, adopt it and move to STABLE
                if fb.probe_result_bps > 0 {
                    self.current_bps = fb.probe_result_bps;
                } else if avail > 0 {
                    self.current_bps = avail;
                }
                self.transition(BwState::Stable);
                BwAction::Hold
            }

            BwState::Stable => {
                if loss > 5.0 {
                    self.transition(BwState::Emergency);
                    return BwAction::EmergencyShed;
                }
                if self.current_bps > 0 && avail < (self.current_bps * 85 / 100) {
                    self.transition(BwState::RampDown);
                    return BwAction::DropLayer;
                }
                if self.current_bps > 0 && avail > (self.current_bps * 115 / 100) {
                    self.ramp_up_count += 1;
                    if self.ramp_up_count >= 3 {
                        self.ramp_up_count = 0;
                        self.transition(BwState::RampUp);
                        return BwAction::AddLayer;
                    }
                } else {
                    self.ramp_up_count = 0;
                }
                // Periodically re-probe (every ~5 s) to refresh baseline
                if self.state_entered.elapsed().as_secs() >= 5 {
                    self.transition(BwState::Probe);
                    return BwAction::SendProbe;
                }
                BwAction::Hold
            }

            BwState::RampUp => {
                // After adding a layer, update baseline and return to STABLE
                self.current_bps = avail;
                self.transition(BwState::Stable);
                BwAction::Hold
            }

            BwState::RampDown => {
                self.current_bps = avail;
                if loss > 5.0 {
                    self.transition(BwState::Emergency);
                    return BwAction::EmergencyShed;
                }
                self.transition(BwState::Stable);
                BwAction::Hold
            }

            BwState::Emergency => {
                // §5.4 STEP 3: check if FEC is needed after shedding
                if loss > 15.0 {
                    return BwAction::EnableFecRS;
                }
                if loss > 5.0 {
                    return BwAction::EnableFec;
                }
                // §5.4 STEP 4: 5 clean reports → RAMP_UP
                if loss < 1.0 {
                    self.recovery_count += 1;
                    if self.recovery_count >= 5 {
                        self.recovery_count = 0;
                        self.transition(BwState::RampUp);
                        return BwAction::RecoveryRampUp;
                    }
                } else {
                    self.recovery_count = 0;
                }
                BwAction::Hold
            }
        }
    }

    fn transition(&mut self, next: BwState) {
        self.state = next;
        self.state_entered = std::time::Instant::now();
    }
}

// ─── Utility ─────────────────────────────────────────────────────────────────

/// Current time in nanoseconds since UNIX epoch (software PTP baseline).
pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

// ─── Fragmentation helpers (PoC §4.1 FRAG field) ─────────────────────────────
//
// FRAG encoding used in this PoC:
//   0x0          → single unfragmented datagram
//   0x1 .. 0xE   → fragment index (1-based), more fragments follow
//   0xF          → last fragment of a multi-fragment AU
//
// All fragments of one AU share the same SEQUENCE_IN_GROUP value.
// Maximum payload chunk per datagram: FRAG_MTU bytes.

/// Maximum payload bytes carried in a single UDP datagram.
/// macOS limits UDP datagrams to net.inet.udp.maxdgram = 9216 bytes by default.
/// We use 8192 bytes of payload + 32-byte FLUX header = 8224 bytes total, safely
/// under the 9216-byte OS limit.
pub const FRAG_MTU: usize = 8_192;

/// Encode `payload` as one or more (header_bytes, chunk) pairs ready to be
/// sent as UDP datagrams.  `hdr` is the template header (frag=0, correct flags,
/// seq, payload_length = full payload length).
///
/// Returns a `Vec` of fully-serialized datagrams (header ++ chunk).
pub fn fragment_encode(hdr: &FluxHeader, payload: &[u8]) -> Vec<Vec<u8>> {
    if payload.len() <= FRAG_MTU {
        // Single unfragmented datagram, frag=0
        let mut h = hdr.clone();
        h.frag = 0;
        h.payload_length = payload.len() as u32;
        let header_bytes = h.encode();
        let mut dg = Vec::with_capacity(HEADER_SIZE + payload.len());
        dg.extend_from_slice(&header_bytes);
        dg.extend_from_slice(payload);
        return vec![dg];
    }

    let chunks: Vec<&[u8]> = payload.chunks(FRAG_MTU).collect();
    let n = chunks.len();
    let mut datagrams = Vec::with_capacity(n);

    for (i, chunk) in chunks.iter().enumerate() {
        let mut h = hdr.clone();
        // frag index: first fragment = 1, last fragment = 0xF
        h.frag = if i == n - 1 { 0xF } else { (i + 1) as u8 };
        // payload_length in each fragment = size of this chunk
        h.payload_length = chunk.len() as u32;
        let header_bytes = h.encode();
        let mut dg = Vec::with_capacity(HEADER_SIZE + chunk.len());
        dg.extend_from_slice(&header_bytes);
        dg.extend_from_slice(chunk);
        datagrams.push(dg);
    }
    datagrams
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let hdr = FluxHeader::new_media(0, 1, 0, true, 1024, 42);
        let enc = hdr.encode();
        let dec = FluxHeader::decode(&enc).unwrap();
        assert_eq!(dec.version, FLUX_VERSION);
        assert_eq!(dec.channel_id, 0);
        assert_eq!(dec.group_id, 1);
        assert_eq!(dec.payload_length, 1024);
        assert_eq!(dec.sequence_in_group, 42);
        assert!(dec.is_keyframe());
    }

    #[test]
    fn capture_ts_same_epoch() {
        let gts: u64 = 0x0000_0001_8000_0000;
        let cts_lo: u32 = 0x8000_1000;
        let full = reconstruct_capture_ts(gts, cts_lo);
        assert_eq!(full & 0xFFFF_FFFF, cts_lo as u64);
    }

    #[test]
    fn frame_type_roundtrip() {
        for t in 0x0u8..=0xFu8 {
            let ft = FrameType::from_u8(t).unwrap();
            assert_eq!(ft as u8, t);
        }
    }

    #[test]
    fn session_request_roundtrip() {
        let req = SessionRequest {
            flux_version: "0.6.2".into(),
            flux_profile: Some("flux_quic".into()),
            client_id: "test-client".into(),
            crypto_mode: "crypto_quic".into(),
            codec_support: vec!["h265".into(), "av1".into()],
            max_channels: 8,
            max_layers: 2,
            max_fps: 120,
            sync_mode: "frame_sync".into(),
            ptp_mode: "software".into(),
            sync_tolerance_ns: 500_000,
            fec_support: vec!["xor".into()],
            cdbc_interval_ms: 25,
            cdbc_interval_min_ms: 10,
            hdr_support: vec!["sdr".into(), "hlg".into()],
            audio_formats: vec!["pcm_f32".into()],
            embed_support: EmbedSupport::default(),
            tally_support: true,
            monitor_stream: false,
            upstream_control: None,
            embed_cache: vec![],
        };
        let json = serde_json::to_vec(&req).unwrap();
        let back: SessionRequest = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.flux_version, req.flux_version);
        assert_eq!(back.flux_profile, req.flux_profile);
        assert_eq!(back.client_id, req.client_id);
        assert_eq!(back.codec_support, req.codec_support);
        assert_eq!(back.max_fps, req.max_fps);
        assert_eq!(back.cdbc_interval_ms, req.cdbc_interval_ms);
        assert_eq!(back.sync_tolerance_ns, req.sync_tolerance_ns);
        assert_eq!(back.fec_support, req.fec_support);
        assert_eq!(back.tally_support, req.tally_support);
    }

    #[test]
    fn session_accept_roundtrip() {
        let accept = SessionAccept {
            flux_version: "0.6.2".into(),
            session_id: "sess-1234567890-1".into(),
            streams: vec![StreamDescriptor {
                channel_id: 0,
                layer: 0,
                codec: "h265".into(),
                width: 1920,
                height: 1080,
                fps_num: 60,
                fps_den: 1,
                bit_depth: 8,
                hdr_mode: "sdr".into(),
                ..StreamDescriptor::default()
            }],
            group_ids: vec![1],
            fec_schema: "none".into(),
            ptp_anchor_ns: 12345678,
            embed_catalog: vec![],
            monitor_stream_id: None,
            crypto_mode_ack: "crypto_quic".into(),
            keepalive_interval_ms: 500,
            keepalive_timeout_count: 5,
            max_datagram_size: 9000,
        };
        let json = serde_json::to_vec(&accept).unwrap();
        let back: SessionAccept = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.session_id, accept.session_id);
        assert_eq!(back.keepalive_interval_ms, accept.keepalive_interval_ms);
        assert_eq!(back.keepalive_timeout_count, accept.keepalive_timeout_count);
        assert_eq!(back.max_datagram_size, accept.max_datagram_size);
        assert_eq!(back.fec_schema, accept.fec_schema);
        assert_eq!(back.ptp_anchor_ns, accept.ptp_anchor_ns);
        assert_eq!(back.streams.len(), 1);
        assert_eq!(back.streams[0].codec, "h265");
        assert_eq!(back.group_ids, vec![1]);
    }

    #[test]
    fn bw_governor_stable_to_ramp_up() {
        let mut gov = BwGovernor::new();

        // First: absorb initial PROBE report to set current_bps
        let probe_report = CdbcFeedback {
            ts_ns: 0,
            rx_bps: 50_000_000,
            avail_bps: 50_000_000,
            rtt_ms: 5.0,
            loss_pct: 0.0,
            jitter_ms: 0.5,
            fps_actual: 60.0,
            datagram_drop_count: 0,
            probe_result_bps: 55_000_000,
        };
        gov.ingest(&probe_report); // PROBE → STABLE, current_bps = 55 Mbps

        assert_eq!(gov.state, BwState::Stable);
        assert_eq!(gov.current_bps, 55_000_000);

        // Send 3 × avail > 1.15 × current → RAMP_UP on the 3rd
        let ramp_report = CdbcFeedback {
            avail_bps: 65_000_000, // 118% of 55 Mbps
            loss_pct: 0.0,
            ..probe_report.clone()
        };
        assert_eq!(gov.ingest(&ramp_report), BwAction::Hold); // count=1
        assert_eq!(gov.ingest(&ramp_report), BwAction::Hold); // count=2
        assert_eq!(gov.ingest(&ramp_report), BwAction::AddLayer); // count=3 → RAMP_UP
        assert_eq!(gov.state, BwState::RampUp);
    }

    #[test]
    fn bw_governor_stable_to_ramp_down() {
        let mut gov = BwGovernor::new();
        // Seed PROBE → STABLE at 50 Mbps
        gov.ingest(&CdbcFeedback {
            probe_result_bps: 50_000_000,
            avail_bps: 50_000_000,
            loss_pct: 0.0,
            ..CdbcFeedback::default()
        });
        assert_eq!(gov.state, BwState::Stable);

        // avail < 85% of 50 Mbps → RAMP_DOWN
        let action = gov.ingest(&CdbcFeedback {
            avail_bps: 40_000_000, // 80%
            loss_pct: 0.0,
            ..CdbcFeedback::default()
        });
        assert_eq!(action, BwAction::DropLayer);
        assert_eq!(gov.state, BwState::RampDown);
    }

    #[test]
    fn bw_governor_emergency_recovery() {
        let mut gov = BwGovernor::new();
        gov.ingest(&CdbcFeedback {
            probe_result_bps: 50_000_000,
            avail_bps: 50_000_000,
            loss_pct: 0.0,
            ..CdbcFeedback::default()
        });

        // High loss → EMERGENCY
        let action = gov.ingest(&CdbcFeedback {
            avail_bps: 50_000_000,
            loss_pct: 8.0,
            ..CdbcFeedback::default()
        });
        assert_eq!(action, BwAction::EmergencyShed);
        assert_eq!(gov.state, BwState::Emergency);

        // 5 × clean reports → RecoveryRampUp
        let clean = CdbcFeedback {
            avail_bps: 50_000_000,
            loss_pct: 0.0,
            ..CdbcFeedback::default()
        };
        for _ in 0..4 {
            assert_eq!(gov.ingest(&clean), BwAction::Hold);
        }
        assert_eq!(gov.ingest(&clean), BwAction::RecoveryRampUp);
        assert_eq!(gov.state, BwState::RampUp);
    }
}
