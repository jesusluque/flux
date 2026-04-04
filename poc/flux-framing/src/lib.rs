//! FLUX wire-format: 32-byte header, frame types, JSON structs.
//!
//! Spec reference: FLUX_Protocol_Spec_v0_4_EN.md §4

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

// ─── Constants ───────────────────────────────────────────────────────────────

pub const FLUX_VERSION: u8 = 4;
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
}

impl FrameType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v & 0x0F {
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
    pub fn new_media(
        channel_id: u16,
        group_id: u16,
        layer: u8,
        is_keyframe: bool,
        payload_len: u32,
        seq: u32,
    ) -> Self {
        let now_ns = now_ns();
        let pts_90k = (now_ns / (1_000_000_000 / PTS_CLOCK_HZ)) as u32;
        let mut f: u8 = 0; // no metadata in the PoC
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
            group_timestamp_ns: now_ns,
            presentation_ts: pts_90k,
            capture_ts_ns_lo: (now_ns & 0xFFFF_FFFF) as u32,
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
pub fn reconstruct_capture_ts(group_ts_ns: u64, capture_ts_lo: u32) -> u64 {
    let c = capture_ts_lo as u64;
    let g_lo = group_ts_ns & 0xFFFF_FFFF;
    let candidate_hi = group_ts_ns & 0xFFFF_FFFF_0000_0000;

    if c > g_lo.wrapping_add(1u64 << 31) {
        // C is from the previous wrap epoch
        candidate_hi.wrapping_sub(1u64 << 32) | c
    } else if g_lo > c.wrapping_add(1u64 << 31) {
        // C is from the next wrap epoch
        candidate_hi.wrapping_add(1u64 << 32) | c
    } else {
        candidate_hi | c
    }
}

// ─── JSON message structs ─────────────────────────────────────────────────────

/// SESSION_REQUEST from client → server (spec §3.1 / §3.2)
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionRequest {
    pub flux_version: String,
    pub client_id: String,
    pub crypto_mode: String,
    pub codec_support: Vec<String>,
    pub max_channels: u8,
    pub max_layers: u8,
    pub max_fps: u16,
    pub sync_mode: String,
    pub ptp_mode: String,
    pub cdbc_interval_ms: u32,
    /// UDP port the client is listening on for incoming media datagrams.
    /// The server uses this to direct media instead of assuming a fixed port.
    pub media_port: u16,
}

impl Default for SessionRequest {
    fn default() -> Self {
        SessionRequest {
            flux_version: "0.4".into(),
            client_id: "FLUX-POC-CLIENT-01".into(),
            crypto_mode: "crypto_none".into(),
            codec_support: vec!["h265".into()],
            max_channels: 4,
            max_layers: 1,
            max_fps: 60,
            sync_mode: "frame_sync".into(),
            ptp_mode: "software".into(),
            cdbc_interval_ms: 50,
            media_port: 7402,
        }
    }
}

/// SESSION_ACCEPT from server → client (spec §3.1)
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionAccept {
    pub flux_version: String,
    pub session_id: String,
    pub crypto_mode_ack: String,
    pub keepalive_interval_ms: u32,
    pub keepalive_timeout: u32,
    pub max_datagram_size: u32,
}

impl Default for SessionAccept {
    fn default() -> Self {
        SessionAccept {
            flux_version: "0.4".into(),
            session_id: "poc-session-001".into(),
            crypto_mode_ack: "crypto_none".into(),
            keepalive_interval_ms: 1000,
            keepalive_timeout: 3,
            max_datagram_size: 65000,
        }
    }
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
}

/// KEEPALIVE payload (spec §3.3)
#[derive(Debug, Serialize, Deserialize)]
pub struct KeepalivePayload {
    pub ts_ns: u64,
    pub session_id: String,
    pub seq: u32,
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
            flux_version: "0.4".into(),
            client_id: "test-client".into(),
            crypto_mode: "crypto_none".into(),
            codec_support: vec!["h265".into(), "av1".into()],
            max_channels: 8,
            max_layers: 2,
            max_fps: 120,
            sync_mode: "frame_sync".into(),
            ptp_mode: "software".into(),
            cdbc_interval_ms: 25,
            media_port: 9000,
        };
        let json = serde_json::to_vec(&req).unwrap();
        let back: SessionRequest = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.flux_version, req.flux_version);
        assert_eq!(back.client_id, req.client_id);
        assert_eq!(back.codec_support, req.codec_support);
        assert_eq!(back.max_fps, req.max_fps);
        assert_eq!(back.media_port, req.media_port);
        assert_eq!(back.cdbc_interval_ms, req.cdbc_interval_ms);
    }

    #[test]
    fn session_accept_roundtrip() {
        let accept = SessionAccept {
            flux_version: "0.4".into(),
            session_id: "sess-1234567890-1".into(),
            crypto_mode_ack: "crypto_none".into(),
            keepalive_interval_ms: 500,
            keepalive_timeout: 5,
            max_datagram_size: 9000,
        };
        let json = serde_json::to_vec(&accept).unwrap();
        let back: SessionAccept = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.session_id, accept.session_id);
        assert_eq!(back.keepalive_interval_ms, accept.keepalive_interval_ms);
        assert_eq!(back.keepalive_timeout, accept.keepalive_timeout);
        assert_eq!(back.max_datagram_size, accept.max_datagram_size);
    }
}
