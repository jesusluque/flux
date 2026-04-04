# FLUX Protocol Specification v0.4
## Fabric for Low-latency Unified eXchange

**Status:** Draft — LUCAB Internal
**Revision:** 2026-04-02
**Author:** LUCAB Media Technology
**Changelog v0.4:**
- QUIC Datagrams (RFC 9221) for all media — eliminates HoL retransmission latency
- Three crypto modes: `none` (LAN), `quic_tls` (default), `quic_tls+aes` (DRM)
- Software PTP as baseline sync; hardware PTP optional for LINE_SYNC
- High frame-rate support: up to 240 fps with adaptive CDBC intervals
- FLUX-E Delta system: incremental GLB nodes, Gaussian Splat delta frames, sequence streaming
- Dynamic CDBC interval (accelerates under congestion)
- Fixed BW Governor EMERGENCY sequence (layer drop first, FEC second)
- Metadata layout corrected: `meta_length` at head of payload for zero-copy parsing
- KEEPALIVE interval and timeout formally defined
- CAPTURE_TS_NS_LO wraparound reconstruction procedure
- EMBED priority modes: `background` / `burst` / `realtime`
- FLUX-C rate limiting
- Tally extended to 3 bits per channel (8 states)

---

## 1. Motivation and positioning

| Protocol | Transport | Media delivery | BW Control | Multi-stream sync | Discovery | Tally | Binary embed | Encryption | WAN |
|----------|-----------|----------------|------------|-------------------|-----------|-------|--------------|------------|-----|
| SRT | UDT/UDP | Reliable | Sender-side | No | No | No | No | AES-128/256 | Yes |
| RIST | RTP/UDP | Unreliable+ARQ | Passive RTCP | No | No | No | No | Profile-dependent | Yes |
| OMT | TCP | Reliable | Receiver hint | No | DNS-SD | No | No | No | No |
| NDI | TCP+UDP | Mixed | Receiver hint | No | DNS-SD | XML | No | Partial | Bridge |
| **FLUX** | **QUIC Datagram+Stream** | **Unreliable media, reliable control** | **CDBC** | **PTP sub-ms** | **DNS-SD + Registry** | **JSON** | **FLUX-E (any MIME, delta)** | **None / TLS 1.3 / TLS+AES** | **Yes** |

### The seven pillars

1. **CDBC** — Client-Driven Bandwidth Control (adaptive interval)
2. **MSS** — Multi-Stream Synchronization (software PTP baseline, hardware PTP optional)
3. **FLUX-D** — Discovery: DNS-SD + HTTP/JSON Registry
4. **FLUX-T** — Bidirectional tally (JSON, compact binary 3-bit)
5. **FLUX-M** — Automatic monitor stream (confidence sub-stream)
6. **FLUX-E** — Embedding of arbitrary data in-stream with delta/sequence support (GLB, USD, GS, CSV, FreeD, EXR…)
7. **FLUX-C** — Upstream control channel (PTZ, audio mix, routing) with rate limiting

---

## 2. Protocol stack

```
┌───────────────────────────────────────────────────────────────────────────┐
│                         FLUX Application Layer                            │
│                                                                           │
│  ┌─────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌──────────┐ ┌─────────┐  │
│  │  Media  │ │  CDBC  │ │  MSS   │ │FLUX-T  │ │ FLUX-E   │ │ FLUX-C  │  │
│  │  Video  │ │Feed-   │ │ Sync   │ │ Tally  │ │ Embed    │ │ Control │  │
│  │  Audio  │ │ back   │ │Barrier │ │ JSON   │ │ + Delta  │ │ Upstrm  │  │
│  │  ≤240fps│ │Adaptive│ │ swPTP  │ │ 3-bit  │ │ + Seq    │ │ RateLim │  │
│  └─────────┘ └────────┘ └────────┘ └────────┘ └──────────┘ └─────────┘  │
├───────────────────────────────────────────────────────────────────────────┤
│               FLUX Framing Layer — fixed 32-byte header                   │
│        Channel_ID | Layer_ID | Group_TS(PTP) | FEC_Group | Type           │
├──────────────────────────────┬────────────────────────────────────────────┤
│  QUIC Streams (RFC 9000)     │  QUIC Datagrams (RFC 9221)                │
│  Reliable: control, ARQ,     │  Unreliable: media, FEC, tally,           │
│  handshake, embed manifest   │  CDBC feedback, bandwidth probe           │
│  0-RTT reconnect │ Migration │  No HoL │ No retransmit │ BBR v3 pacing   │
├──────────────────────────────┴────────────────────────────────────────────┤
│                      UDP — IPv4 / IPv6                                    │
├───────────────────────────────────────────────────────────────────────────┤
│  Crypto mode: NONE (raw UDP) │ QUIC TLS 1.3 │ QUIC TLS 1.3 + AES-256-GCM│
└───────────────────────────────────────────────────────────────────────────┘

Parallel control plane (TCP/HTTP):
┌───────────────────────────────────────────────┐
│  FLUX Registry API (REST/JSON + WebSocket)    │
│  Discovery | Routing | Tally | Monitoring     │
└───────────────────────────────────────────────┘
```

### 2.1 QUIC transport modes — critical design decision

FLUX uses **two distinct QUIC delivery mechanisms** within the same connection:

| Data class | QUIC mechanism | Retransmit | Rationale |
|---|---|---|---|
| Media frames (video, audio) | **QUIC Datagram** (RFC 9221) | **No** | A late frame is worse than a lost frame. Zero HoL blocking. |
| FEC repair packets | QUIC Datagram | No | FEC is inherently loss-tolerant. |
| CDBC feedback, tally, probe | QUIC Datagram | No | Stale feedback is harmful; the next report supersedes. |
| Control (SESSION, ANNOUNCE, KEEPALIVE) | QUIC Stream 0 | Yes | Handshake must be reliable. |
| EMBED_MANIFEST | QUIC Stream | Yes | Manifest integrity is required before chunk reassembly. |
| EMBED_CHUNK (full assets) | QUIC Stream (low priority) | Yes | Binary assets must arrive complete. |
| EMBED_CHUNK (delta/realtime) | QUIC Datagram | No | Delta frames are time-critical; stale deltas are skipped. |
| Selective ARQ retransmit | QUIC Stream (dedicated) | Yes | Only for base-layer keyframes on explicit request. |

**Implementation note:** The QUIC connection MUST advertise `max_datagram_frame_size` ≥ 1350 bytes in transport parameters. FLUX frames larger than the QUIC datagram MTU are split at the FLUX framing layer (FRAG field) — NOT by QUIC.

### 2.2 Crypto modes

FLUX supports three encryption levels, negotiated in the handshake:

| Mode | Identifier | Transport | Payload | Use case |
|---|---|---|---|---|
| **None** | `crypto_none` | Raw UDP, no QUIC | None | Trusted LAN, maximum performance, lowest latency |
| **QUIC TLS** | `crypto_quic` | QUIC with TLS 1.3 | QUIC-encrypted | Default — WAN and general use |
| **QUIC TLS + AES** | `crypto_quic_aes` | QUIC with TLS 1.3 | AES-256-GCM over QUIC | DRM, classified content |

**`crypto_none` mode details:**

When `crypto_none` is selected, FLUX operates over **raw UDP** without the QUIC layer. In this mode:
- The FLUX 32-byte framing header is sent directly inside UDP datagrams.
- All QUIC features (stream multiplexing, congestion control, 0-RTT) are unavailable.
- The application MUST implement its own congestion avoidance (CDBC still operates, but pacing is done at the FLUX layer).
- Session management uses a parallel TCP connection for reliable messages (SESSION_INFO, EMBED_MANIFEST, STREAM_ANNOUNCE).
- **This mode is intended exclusively for controlled LAN environments** where switches provide QoS and there is no untrusted traffic.
- Discovery, Registry, and authentication (JWT/API key over the TCP control plane) still operate normally.

**Latency advantage of `crypto_none`:** Eliminates ~2–5 µs per packet of TLS encrypt/decrypt overhead. At 240 fps with 16 channels, this saves ~10–80 µs per frame cycle on typical hardware. At multi-gigabit rates (JPEG-XS), the CPU saving is significant — up to 1–2 CPU cores freed.

---

## 3. Session model

### 3.1 Handshake

```
Client                                          Server
  |                                               |
  |-- QUIC Initial (TLS 1.3 ClientHello) -------> |   (or TCP SYN if crypto_none)
  |<---------- Handshake Complete --------------- |
  |                                               |
  |-- FLUX_SESSION_REQUEST (reliable) ----------> |
  |   { version, capabilities, sync_mode,        |
  |     channel_mask, max_layers, codec_caps,     |
  |     embed_support, cdbc_interval_ms,          |
  |     crypto_mode, max_fps, ptp_mode }          |
  |                                               |
  |<-- FLUX_SESSION_ACCEPT (reliable) ----------- |
  |   { session_id, streams[], group_ids[],       |
  |     fec_schema, ptp_anchor_ns,                |
  |     embed_catalog[], monitor_stream_id,       |
  |     crypto_mode_ack, max_datagram_size,       |
  |     keepalive_interval_ms, keepalive_timeout } |
  |                                               |
  |<-- STREAM_ANNOUNCE × N (reliable) ---------- |
  |   (one frame per channel × layer)            |
  |                                               |
  |<── Media datagrams begin ───────────────────  |
  |                                               |
  |── CDBC_FEEDBACK (datagram, adaptive) ──────> |
  |── TALLY_UPDATE (datagram, on change) ──────> |
  |<── EMBED_MANIFEST (reliable, when asset) ──  |
  |<── EMBED_CHUNK × N ────────────────────────  |
```

### 3.2 Capabilities JSON

```json
{
  "flux_version": "0.4",
  "client_id": "LUCAB-RECEIVER-01",
  "crypto_mode": "crypto_quic",
  "codec_support": ["h265", "av1", "jpegxs", "ullc"],
  "max_channels": 16,
  "max_layers": 4,
  "max_fps": 240,
  "sync_mode": "frame_sync",
  "ptp_mode": "software",
  "sync_tolerance_ns": 500000,
  "fec_support": ["xor", "rs_2d"],
  "cdbc_interval_ms": 50,
  "cdbc_interval_min_ms": 10,
  "hdr_support": ["sdr", "hlg", "pq"],
  "audio_formats": ["pcm_f32", "aes67"],
  "embed_support": {
    "max_concurrent_assets": 8,
    "max_asset_size_mb": 512,
    "delta_support": true,
    "sequence_support": true,
    "mime_types": [
      "model/gltf-binary",
      "model/vnd.usd",
      "model/vnd.gaussian-splat",
      "application/json",
      "text/csv",
      "image/x-exr",
      "application/vnd.flux.tracking",
      "application/vnd.flux.gs-delta",
      "application/vnd.flux.glb-delta",
      "application/octet-stream"
    ]
  },
  "tally_support": true,
  "monitor_stream": true,
  "upstream_control": {
    "capabilities": ["ptz", "audio_mix", "routing"],
    "max_commands_per_second": 60
  },
  "embed_cache": [
    { "asset_id": "scene-glb-take-001", "sha256": "a3f2c1..." },
    { "asset_id": "tracking-cal-v2",    "sha256": "b7e9d2..." }
  ]
}
```

### 3.3 KEEPALIVE specification

| Parameter | Value | Negotiation |
|---|---|---|
| `keepalive_interval_ms` | Default: 1000 ms | Server sets in SESSION_ACCEPT; client may request in SESSION_REQUEST |
| `keepalive_timeout_count` | Default: 3 | Number of missed keepalives before declaring session dead |
| Effective timeout | `interval × count` = 3000 ms default | — |

Both sides MUST send KEEPALIVE frames at the negotiated interval. A KEEPALIVE carries:

```json
{
  "ts_ns": 1743580812345678901,
  "session_id": "sess-001",
  "seq": 12345
}
```

If a peer misses `keepalive_timeout_count` consecutive keepalives, it MUST:
1. Attempt 0-RTT reconnect (if QUIC mode).
2. If reconnect fails within 2 × timeout, declare session dead and emit `SESSION_LOST` event.

---

## 4. FLUX frame format (wire format)

### 4.1 Header — fixed 32 bytes

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|VER(4)|TYPE(4)|    FLAGS(8)   |        CHANNEL_ID (16)        |  [0–3]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|LAYER(4)|FRAG(4)|             GROUP_ID (16)         |RSVD (8) |  [4–7]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|              GROUP_TIMESTAMP_NS — bits 63..32                 |  [8–11]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|              GROUP_TIMESTAMP_NS — bits 31..0                  |  [12–15]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|           PRESENTATION_TS (32 bits, 90 kHz clock)             |  [16–19]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|           CAPTURE_TS_NS_LO (32 bits, ns mod 2³²)              |  [20–23]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|          PAYLOAD_LENGTH (24 bits)             | FEC_GROUP(8)  |  [24–27]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|              SEQUENCE_IN_GROUP (32 bits)                      |  [28–31]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### 4.2 CAPTURE_TS_NS_LO wraparound reconstruction

`CAPTURE_TS_NS_LO` stores only the low 32 bits of the capture timestamp in nanoseconds. The wrap period is ~4.295 seconds. Receivers MUST reconstruct the full 64-bit capture timestamp using the following procedure:

```
Let G = GROUP_TIMESTAMP_NS (full 64-bit)
Let C = CAPTURE_TS_NS_LO (low 32 bits)
Let G_lo = G & 0xFFFFFFFF

candidate_hi = G & 0xFFFFFFFF00000000

if C > G_lo + 2^31:
    full_capture_ts = (candidate_hi - 2^32) | C    // C is from previous wrap
elif G_lo > C + 2^31:
    full_capture_ts = (candidate_hi + 2^32) | C    // C is from next wrap
else:
    full_capture_ts = candidate_hi | C              // same epoch
```

This guarantees correct reconstruction as long as `|capture_ts - group_ts| < 2.147 seconds`, which is always true for live media.

**FLAGS (8 bits):**
```
Bit 7: KEYFRAME       — independent frame (IDR/keyframe)
Bit 6: ENCRYPTED      — payload has additional AES-256-GCM (only in crypto_quic_aes mode)
Bit 5: DROP_ELIGIBLE  — may be dropped under congestion (B-frames, enhancement layers)
Bit 4: EMBED_ASSOC    — this frame has an associated embedded asset (see ASSET_ID in metadata)
Bit 3: MONITOR_COPY   — this frame also feeds the monitor stream
Bit 2: SYNC_MASTER    — this stream is master of the sync group
Bit 1: LAST_IN_GOP    — last frame of the group of pictures
Bit 0: HAS_METADATA   — payload begins with a metadata length + JSON block
```

### 4.3 Frame types

| Code | Name | Dir | Delivery | Description |
|------|------|-----|----------|-------------|
| `0x0` | `MEDIA_DATA` | S→C | Datagram | Media payload (video, audio, compressed ANC) |
| `0x1` | `CDBC_FEEDBACK` | C→S | Datagram | Receiver bandwidth report |
| `0x2` | `SYNC_ANCHOR` | S→C | Datagram | PTP anchor for the sync group |
| `0x3` | `LAYER_STATUS` | S→C | Stream | Available layer change notification |
| `0x4` | `QUALITY_REQUEST` | C→S | Stream | Receiver requests quality/layer change |
| `0x5` | `STREAM_ANNOUNCE` | S→C | Stream | New stream declaration |
| `0x6` | `STREAM_END` | Both | Stream | Stream termination |
| `0x7` | `FEC_REPAIR` | S→C | Datagram | FEC repair packet |
| `0x8` | `SESSION_INFO` | Both | Stream | Handshake and capabilities (JSON) |
| `0x9` | `KEEPALIVE` | Both | Datagram | Heartbeat with timestamp (interval negotiated) |
| `0xA` | `TALLY_UPDATE` | C→S | Datagram | Per-channel tally state (compact 3-bit or JSON) |
| `0xB` | `ANC_DATA` | S→C | Datagram | Broadcast ancillary data (VANC/HANC) |
| `0xC` | `METADATA_FRAME` | Both | Datagram | Per-frame or out-of-band JSON metadata |
| `0xD` | `BANDWIDTH_PROBE` | S→C | Datagram | Probe packet for receiver-side BW measurement |
| `0xE` | `EMBED_MANIFEST` | S→C | Stream | Declares an asset to be embedded in the stream |
| `0xF` | `EMBED_CHUNK` | S→C | Stream or Datagram | Fragment of an embedded asset (mode-dependent) |

### 4.4 Metadata payload layout (corrected)

When `HAS_METADATA=1` in FLAGS, the `MEDIA_DATA` payload has the following layout:

```
[ meta_length: uint16 ]                              — FIRST: metadata length (0 = no metadata block, payload is all media)
[ meta_json: meta_length bytes, UTF-8 JSON ]         — metadata block
[ media_bytes: PAYLOAD_LENGTH - meta_length - 2 ]    — media data
```

**Rationale:** Placing `meta_length` at the head of the payload allows the receiver to split metadata from media in a single read without buffering the entire payload. This enables zero-copy forwarding of the media portion to the decoder while the metadata is parsed independently.

---

## 5. CDBC — Client-Driven Bandwidth Control

The receiver measures continuously and reports at an **adaptive** interval:

### 5.1 Adaptive CDBC interval

| BW Governor state | CDBC interval | Rationale |
|---|---|---|
| PROBE | `cdbc_interval_ms` (default 50 ms) | Normal probing rate |
| STABLE | `cdbc_interval_ms` (default 50 ms) | Low overhead when link is healthy |
| RAMP_UP | `cdbc_interval_ms` (default 50 ms) | Conservative ramp requires stable readings |
| RAMP_DOWN | `cdbc_interval_min_ms` (default 10 ms) | Fast convergence under degradation |
| EMERGENCY | `cdbc_interval_min_ms` (default 10 ms) | Maximum responsiveness during crisis |

At high frame rates (>120 fps), the minimum practical CDBC interval is 1 frame period:
- 120 fps → min 8.33 ms
- 200 fps → min 5 ms
- 240 fps → min 4.17 ms

The receiver SHOULD NOT send CDBC reports more frequently than once per frame period.

### 5.2 CDBC_FEEDBACK frame

```json
{
  "ts_ns": 1743580812345678901,
  "rx_bps": 48500000,
  "avail_bps": 62000000,
  "rtt_ms": 12,
  "loss_pct": 0.1,
  "jitter_ms": 0.8,
  "preferred_max_layer": 2,
  "fps_actual": 200,
  "per_channel": {
    "0": { "rx_bps": 45000000, "loss_pct": 0.0 },
    "1": { "rx_bps": 3500000,  "loss_pct": 0.3 }
  },
  "probe_result_bps": 65000000,
  "datagram_drop_count": 12
}
```

### 5.3 BW Governor on the server

The server maintains a **BW Governor** state machine:

```
States: PROBE → STABLE → RAMP_UP / RAMP_DOWN → EMERGENCY

PROBE:       sends BANDWIDTH_PROBE, waits for result
STABLE:      holds current layers
RAMP_UP:     if avail_bps > 1.15 × current_bps for 3 reports → add enhancement layer
RAMP_DOWN:   if avail_bps < 0.85 × current_bps → drop top enhancement layer
EMERGENCY:   sequence matters — see §5.4
```

### 5.4 EMERGENCY state — corrected priority order

The EMERGENCY state follows a strict **shed-then-protect** sequence to avoid congestion death spirals:

```
STEP 1 — Shed load (immediate, no extra bandwidth cost):
  a) Drop all enhancement layers (layer > 0) → DROP_ELIGIBLE frames stop
  b) Disable monitor streams
  c) Pause EMBED_CHUNK transfers (non-delta)
  d) Reduce frame rate if negotiated (see §5.6)

STEP 2 — Evaluate remaining loss on base layer only:
  Wait 2 × cdbc_interval_min_ms for updated CDBC report

STEP 3 — Protect base layer (only if loss persists after shedding):
  if loss_pct > 5%  → enable XOR Row FEC on base layer only
  if loss_pct > 15% → switch to Reed-Solomon 2D on base layer + force IDR-only mode

STEP 4 — Recovery:
  if loss_pct < 1% for 5 consecutive reports → transition to RAMP_UP
```

**Rationale:** v0.3 activated maximum FEC *during* congestion, injecting ~50% additional traffic into an already saturated link. v0.4 first removes all non-essential traffic, then applies FEC only to the surviving base layer.

### 5.5 Per-layer QUIC priority

QUIC supports priority hints (RFC 9218 Extensible Prioritization). When using `crypto_quic` or `crypto_quic_aes`:

```
Layer 0 (base):         urgency = 0 (highest), incremental = true
Layer 1 (enhancement):  urgency = 2, incremental = true
Layer 2 (enhancement):  urgency = 3, incremental = true
Layer 3 (enhancement):  urgency = 4, incremental = true
Monitor stream:         urgency = 5, incremental = true
Embed chunks (bulk):    urgency = 6, incremental = true
Embed delta (realtime): urgency = 1, incremental = true   ← elevated priority
```

Note: RFC 9218 urgency is 0–7 (0 = highest). This replaces the v0.3 arbitrary 0–255 scale.

### 5.6 High frame-rate considerations (120–240 fps)

At frame rates above 120 fps, the frame period drops below typical network jitter:

| FPS | Frame period | Notes |
|---|---|---|
| 120 | 8.33 ms | Comfortable with standard CDBC |
| 144 | 6.94 ms | Common gaming/LED wall refresh |
| 200 | 5.00 ms | High-speed capture, LED sync |
| 240 | 4.17 ms | Maximum specified; sport/scientific |

**Requirements at high fps:**
- Jitter buffer depth MUST be at least 3 frame periods at the target fps.
- The sender SHOULD use BBR v3 pacing to spread datagrams evenly across each frame interval.
- At ≥200 fps, `FRAME_SYNC` tolerance is automatically clamped to ±1 frame period (±5 ms at 200 fps, ±4.17 ms at 240 fps).
- GOP structure: at ≥120 fps, IDR interval SHOULD be at least every 1 second (120–240 frames). Frequent IDRs at these rates waste significant bandwidth.
- CDBC reports are rate-limited to maximum once per frame period.
- `LAST_IN_GOP` flag importance increases — the receiver uses it to discard partial GOPs when joining mid-stream.

---

## 6. MSS — Multi-Stream Synchronization

### 6.1 PTP modes

FLUX defines two PTP modes, negotiated per-session:

| Mode | Identifier | Precision | Requirements | Use case |
|---|---|---|---|---|
| **Software PTP** | `ptp_software` | ±50–500 µs | NTP/PTP daemon, `clock_gettime(CLOCK_REALTIME)` | Default. Sufficient for FRAME_SYNC and SAMPLE_SYNC. |
| **Hardware PTP** | `ptp_hardware` | ±10–100 ns | IEEE 1588 PTP grandmaster, NIC with hardware timestamping (`SO_TIMESTAMPING`) | Required for LINE_SYNC. |

**Software PTP baseline:**

All FLUX implementations MUST support software PTP. The implementation:
1. Uses the system clock synchronized via `linuxptp` (ptp4l/phc2sys) or equivalent.
2. Reads timestamps via `clock_gettime(CLOCK_REALTIME)` or `clock_gettime(CLOCK_TAI)`.
3. Achieves typical precision of ±100 µs on a well-configured LAN.
4. Is sufficient for FRAME_SYNC (±20 ms at 50 fps, ±4.17 ms at 240 fps) and SAMPLE_SYNC (±20.8 µs at 48 kHz).

**Hardware PTP (optional):**

For LINE_SYNC precision (±18 µs at 1080p50), hardware PTP is required:
1. NIC must support `SOF_TIMESTAMPING_TX_HARDWARE` and `SOF_TIMESTAMPING_RX_HARDWARE`.
2. The PTP grandmaster must be on the same L2 domain.
3. Timestamps are read via `SO_TIMESTAMPING` cmsg on the socket.
4. The `ptp_mode` field in the handshake declares the mode; both sides must agree.

If a client requests `ptp_hardware` but the server only supports `ptp_software`, the server downgrades to `ptp_software` and reports this in `SESSION_ACCEPT`. The client MUST NOT attempt LINE_SYNC without confirmed hardware PTP from both peers.

### 6.2 Sync groups

Each stream is declared a member of a `GROUP_ID`. All streams in the same group share the same `GROUP_TIMESTAMP_NS` (64-bit PTP, nanoseconds since epoch).

```json
{
  "channel_id": 0,
  "layer_id": 0,
  "name": "CAM_A_VIDEO",
  "content_type": "video",
  "codec": "h265",
  "group_id": 1,
  "sync_role": "master",
  "frame_rate": "240/1",
  "resolution": "1920x1080",
  "hdr": "pq",
  "colorspace": "bt2100"
}
```

```json
{
  "channel_id": 1,
  "name": "CAM_A_AUDIO",
  "content_type": "audio",
  "codec": "pcm_f32",
  "group_id": 1,
  "sync_role": "slave",
  "sample_rate": 48000,
  "channels": 16
}
```

### 6.3 Sync Barrier on the receiver

The receiver implements a configurable **sync barrier**:

```
FRAME_SYNC:   tolerance = ±1 frame period (auto-scales with fps: ±20 ms at 50, ±4.17 ms at 240)
SAMPLE_SYNC:  tolerance = ±1 audio sample (±20.8 µs at 48 kHz)
LINE_SYNC:    tolerance = ±1 scan line (≈ ±18 µs at 1080p50) — requires ptp_hardware
```

The receiver maintains a separate jitter buffer per stream and aligns them on `GROUP_TIMESTAMP_NS` before delivering to the application. If a stream in the group exceeds the barrier timeout, it emits a `SYNC_LOST` event and continues with the available streams.

### 6.4 SYNC_ANCHOR frame

The group master emits a `SYNC_ANCHOR` at an adaptive interval:

| PTP mode | Default interval | Rationale |
|---|---|---|
| `ptp_software` | 500 ms | Compensates for higher clock drift |
| `ptp_hardware` | 1000 ms | Hardware clock is stable |

```json
{
  "group_id": 1,
  "ptp_mode": "software",
  "ptp_anchor_ns": 1743580812000000000,
  "estimated_drift_ppb": 150,
  "members": [0, 1, 2],
  "frame_index": 50000,
  "sync_tolerance_ns": 500000,
  "fps": 240
}
```

The `estimated_drift_ppb` field allows the receiver to extrapolate clock correction between anchors, enabling tighter sync with software PTP.

---

## 7. FLUX-D — Discovery

Inspired by NDI Find, but with a JSON API and no hard dependency on Bonjour.

### 7.1 Layer 1: DNS-SD / mDNS

Senders announce themselves as `_flux._udp.local` services:

```
Name: "CAM_A (LUCAB Studio)"
TXT records:
  flux_version=0.4
  channels=2
  groups=1
  port=7400
  crypto=crypto_quic
  max_fps=240
  registry=http://192.168.1.100:7500
```

### 7.2 Layer 2: FLUX Registry Server

An optional server (LAN or WAN) centralises senders. REST/JSON API:

```
GET  /api/sources            → list all senders
GET  /api/sources/{id}       → sender detail
POST /api/sources/{id}/route → redirect to another sender (like NDI routing)
GET  /api/groups             → active sync groups
WS   /api/events             → WebSocket push events (new source, tally changes)
```

```json
{
  "sources": [
    {
      "id": "cam-a-lucab-studio",
      "name": "CAM_A (LUCAB Studio)",
      "host": "192.168.1.50",
      "port": 7400,
      "channels": 2,
      "groups": [1],
      "codecs": ["h265", "jpegxs"],
      "max_fps": 240,
      "hdr": true,
      "crypto_modes": ["crypto_none", "crypto_quic"],
      "embed_catalog": ["scene_glb", "tracking_freed"],
      "delta_support": true,
      "tally": { "program": false, "preview": true },
      "uptime_s": 3600,
      "monitor_url": "flux://192.168.1.50:7401/monitor"
    }
  ]
}
```

### 7.3 Dynamic routing

As with NDI Routing: a receiver connected to a virtual source can be redirected to a different real source without breaking the session:

```json
// POST /api/sources/virtual-main/route
{ "target_id": "cam-b-lucab-studio" }
```

---

## 8. FLUX-T — Bidirectional tally

### 8.1 Upstream from receiver (C→S) — JSON mode

The receiver sends `TALLY_UPDATE` whenever programme/preview state changes:

```json
{
  "session_id": "sess-001",
  "ts_ns": 1743580812345678901,
  "channels": {
    "0": { "program": true,  "preview": false, "standby": false, "iso_rec": true,  "streaming": false },
    "1": { "program": false, "preview": true,  "standby": false, "iso_rec": false, "streaming": true  },
    "2": { "program": false, "preview": false, "standby": true,  "iso_rec": false, "streaming": false }
  },
  "mixer_id": "LUCAB-MIXER-01",
  "transition": "cut"
}
```

### 8.2 Compact binary mode (ultra-low latency)

For tally latency below 1 ms, the receiver may use **compact binary mode** — extended to 3 bits per channel (8 states):

```
[ ts_ns_lo: uint32 ]        — low 32 bits of timestamp
[ flags: uint8 ]            — bit 0: compact mode, bit 1: group_tally
[ channel_count: uint8 ]
[ tally_bitmap: N bytes ]   — 3 bits per channel, packed big-endian
[ mixer_id: uint16 ]
```

**3-bit tally states:**

| Value | State | Typical colour |
|---|---|---|
| `000` | Idle | Off |
| `001` | Preview | Green |
| `010` | Program | Red |
| `011` | Standby | Yellow |
| `100` | ISO Recording | Blue |
| `101` | Streaming | Purple |
| `110` | Clean Feed | White |
| `111` | Reserved | — |

Packing: for `channel_count` channels, the bitmap requires `ceil(channel_count × 3 / 8)` bytes.

### 8.3 Downstream from server (S→C)

The server can send tally back to synchronise camera tally lights and displays:

```json
{
  "type": "tally_confirm",
  "channel": 0,
  "state": "program",
  "color": "#FF0000",
  "label": "PGM"
}
```

---

## 9. FLUX-M — Monitor Stream

Inspired by NDI|HX dual-quality. The server automatically generates a low-quality sub-stream per channel.

### 9.1 Configuration

```json
{
  "monitor_streams": [
    {
      "source_channel_id": 0,
      "monitor_channel_id": 100,
      "codec": "h265",
      "bitrate_kbps": 500,
      "resolution": "640x360",
      "frame_rate": "25/1",
      "latency_mode": "ultralow"
    }
  ]
}
```

### 9.2 Monitor stream use cases

- Confidence monitoring in production without consuming main stream bandwidth
- Thumbnail preview in the FLUX Registry UI
- Parallel safety recording
- AI feed (content analysis, scene detection, automated direction)

### 9.3 Monitor stream at high fps

When the source operates at ≥120 fps, the monitor stream SHOULD be decimated to a standard rate (25/30/50/60 fps). The decimation happens server-side. The `frame_rate` field in monitor configuration explicitly sets the target rate.

---

## 10. FLUX-E — Embedding arbitrary data in-stream

**This is the most differentiating extension of FLUX relative to all existing protocols.**

FLUX allows any MIME-typed binary payload to be multiplexed into the media stream, with optional temporal association to specific frames. This enables transmission of 3D scenes, camera data, point clouds, data sheets or any asset synchronised with video.

### 10.1 Supported MIME types (non-exhaustive)

| MIME Type | Broadcast / VP use case |
|-----------|------------------------|
| `model/gltf-binary` | 3D scene (.glb) for VP — the LED volume environment |
| `model/vnd.usd` / `model/vnd.usdz` | OpenUSD scene for nDisplay |
| `model/vnd.gaussian-splat` | 3D GS asset (SPZ, PLY, HAC format) — full keyframe |
| `application/vnd.flux.glb-delta` | **Incremental GLB update — transforms, animations, node changes only** |
| `application/vnd.flux.gs-delta` | **Gaussian Splat delta frame — splat position/color/opacity changes** |
| `application/vnd.flux.gs-sequence` | **Gaussian Splat sequence header — declares a temporal GS sequence** |
| `application/json` | Telemetry, production data, cue sheets |
| `text/csv` | Time series, scores, sports statistics |
| `image/x-exr` | HDR reference frame, environment HDRI |
| `image/png` / `image/webp` | Thumbnails, textures, overlays |
| `application/vnd.flux.tracking` | Camera tracking data (extended FreeD) |
| `application/vnd.flux.anc` | Broadcast ancillary (SMPTE 291M over IP) |
| `application/vnd.flux.mocap` | Per-frame motion capture data |
| `application/vnd.flux.led-config` | LED processor configuration (Brompton/Tessera) |
| `application/octet-stream` | Generic binary payload |

### 10.2 Embed priority modes

FLUX-E assets can be transferred at three priority levels:

| Mode | Delivery | QUIC mechanism | Urgency | Use case |
|---|---|---|---|---|
| **`background`** | Best-effort, no impact on media | Stream (reliable), urgency 6 | Lowest | Large GLB/USD initial load |
| **`burst`** | Temporarily reduces media quality to make room | Stream (reliable), urgency 3 | Medium | Scene change — new GLB needed before next take |
| **`realtime`** | Per-frame, synchronized with video | Datagram (unreliable), urgency 1 | High | Delta GLB, GS delta, tracking data |

The priority is declared in `EMBED_MANIFEST.priority`. When `burst` mode is active, the BW Governor temporarily reduces the top enhancement layer to create headroom for the asset transfer. The server MUST restore full media quality within 2 seconds after the asset transfer completes.

### 10.3 Embedding flow (full asset)

```
Server                                       Client
   |                                            |
   |-- EMBED_MANIFEST (reliable) ------------> |
   |   { asset_id, mime_type, total_bytes,     |
   |     chunk_count, sha256, priority,        |
   |     frame_assoc, delta_base, ttl_s }      |
   |                                            |
   |-- EMBED_CHUNK #0 (reliable) ------------> |
   |-- EMBED_CHUNK #1 -----------------------> |
   |   (interleaved with media, priority-based) |
   |-- EMBED_CHUNK #N -----------------------> |
   |                                            |
   |   (client reassembles, verifies SHA-256)  |
   |                                            |
   |<-- EMBED_ACK (reliable) ------------------|
   |   { asset_id, status: "ready" }           |
```

### 10.4 EMBED_MANIFEST frame (JSON)

```json
{
  "asset_id": "scene-glb-take-003",
  "mime_type": "model/gltf-binary",
  "name": "VP_Scene_Take_003.glb",
  "total_bytes": 48234567,
  "chunk_size": 65536,
  "chunk_count": 736,
  "sha256": "a3f2c1...e8d4b9",
  "compression": "zstd",
  "priority": "background",
  "ttl_s": 3600,
  "frame_assoc": {
    "mode": "from_frame",
    "group_id": 1,
    "group_ts_ns": 1743580900000000000,
    "description": "Active scene from frame 5000"
  },
  "delta_base": null,
  "metadata": {
    "scene_name": "VP_Scene_001",
    "software": "Unreal Engine 5.6",
    "units": "centimeters",
    "up_axis": "Z"
  }
}
```

### 10.5 EMBED_CHUNK frame (wire)

Uses the standard FLUX header with `TYPE=0xF`. The payload is:

```
[ asset_id_hash: uint32 ]     — hash of asset_id (fast lookup)
[ chunk_index: uint32 ]       — fragment index (0-based)
[ chunk_length: uint16 ]      — data length in this chunk
[ data: chunk_length bytes ]  — binary fragment data
```

### 10.6 Temporal asset↔frame association

An asset can be associated with frames in four ways:

**a) Exact association** — the asset is valid for a specific frame (per-frame tracking, per-frame GS delta):
```json
{ "mode": "exact_frame", "group_ts_ns": 1743580812345678901 }
```

**b) Range association** — the asset is valid between two timestamps (3D scene active during a take):
```json
{ "mode": "range", "start_ns": 1743580900000000000, "end_ns": 1743581200000000000 }
```

**c) Persistent asset** — available for the whole session or until `ttl_s`:
```json
{ "mode": "session", "ttl_s": 3600 }
```

**d) Sequence step** — part of an ordered temporal sequence (GS sequence, animation timeline):
```json
{ "mode": "sequence", "sequence_id": "gs-seq-001", "step_index": 42, "step_ts_ns": 1743580812345678901 }
```

### 10.7 Hash-based deduplication and cache invalidation

The server maintains a catalogue of sent assets. The client can declare its cache in the handshake (by `sha256`). The server skips sending assets the client already holds:

```json
{
  "embed_cache": [
    { "asset_id": "scene-glb-take-001", "sha256": "a3f2c1..." },
    { "asset_id": "tracking-cal-v2",    "sha256": "b7e9d2..." }
  ]
}
```

**Cache invalidation:** If the server has a newer version of a cached asset (same `asset_id`, different `sha256`), it MUST send an `EMBED_MANIFEST` with the new `sha256`. The client MUST invalidate its cached copy and accept the new transfer. The manifest includes `"invalidates_sha256": "a3f2c1..."` to make the invalidation explicit.

---

## 11. FLUX-E Delta — Incremental asset updates

### 11.1 Concept

For live virtual production, full scene transfers (GLB files of 50–500 MB) are impractical for real-time updates. FLUX-E Delta enables sending **only what changed** relative to a known base asset.

The delta system works for any structured asset, but has specific optimisations for:
- **GLB (glTF-Binary):** node transforms, morph targets, animation keyframes, texture swaps
- **Gaussian Splats:** per-splat position, color, opacity, SH coefficient changes
- **USD:** layer overrides, transform edits

### 11.2 Delta reference model

```
           Full keyframe (GLB/GS/USD)
           ┌─────────────────────────┐
           │   asset_id: "scene-001" │
           │   sha256: "abc123..."   │
           │   48 MB                 │
           └──────────┬──────────────┘
                      │ base
        ┌─────────────┼──────────────┬──────────────┐
        ▼             ▼              ▼              ▼
   Delta frame 1  Delta frame 2  Delta frame 3  Delta frame N
   2 KB           800 B          3 KB           1.5 KB
   (node moved)   (light color)  (anim key)     (splat update)
```

Each delta references its `delta_base` — the `asset_id + sha256` of the full keyframe it modifies. The client applies deltas on top of the base in memory.

### 11.3 GLB Delta format (`application/vnd.flux.glb-delta`)

A GLB delta is a compact binary + JSON hybrid that describes changes to specific nodes in the glTF scene graph:

```json
{
  "delta_base": {
    "asset_id": "scene-glb-take-003",
    "sha256": "a3f2c1...e8d4b9"
  },
  "delta_seq": 42,
  "ts_ns": 1743580812345678901,
  "operations": [
    {
      "op": "transform",
      "node": "/nodes/15",
      "translation": [1.5, 0.0, -2.3],
      "rotation": [0.0, 0.707, 0.0, 0.707],
      "scale": [1.0, 1.0, 1.0]
    },
    {
      "op": "morph_weights",
      "node": "/nodes/22",
      "weights": [0.0, 0.8, 0.3]
    },
    {
      "op": "material_property",
      "material": "/materials/3",
      "property": "baseColorFactor",
      "value": [1.0, 0.2, 0.2, 1.0]
    },
    {
      "op": "animation_sample",
      "animation": "/animations/0",
      "time_s": 2.5,
      "channels": [
        { "target": "/nodes/10", "path": "translation", "value": [0.0, 1.2, 0.0] },
        { "target": "/nodes/10", "path": "rotation",    "value": [0.0, 0.0, 0.0, 1.0] }
      ]
    },
    {
      "op": "visibility",
      "node": "/nodes/5",
      "visible": false
    },
    {
      "op": "texture_swap",
      "material": "/materials/1",
      "slot": "baseColorTexture",
      "texture_sha256": "d4e5f6...",
      "data_offset": 0,
      "data_length": 32768
    }
  ],
  "binary_payloads_length": 32768
}
```

The JSON is followed by concatenated binary payloads (referenced by `data_offset` + `data_length` in operations like `texture_swap`).

**Supported GLB delta operations:**

| Operation | Description | Typical size |
|---|---|---|
| `transform` | Node translation/rotation/scale | 60 bytes |
| `morph_weights` | Blend shape weights | 4 bytes per weight |
| `material_property` | Change material parameter | 20–40 bytes |
| `animation_sample` | Provide animation values at a specific time | Variable |
| `visibility` | Show/hide node | 12 bytes |
| `texture_swap` | Replace texture data (binary payload appended) | Header + texture bytes |
| `node_add` | Add a new node (references existing mesh/material) | ~200 bytes |
| `node_remove` | Remove a node by path | 20 bytes |
| `light_property` | Change light intensity, color, range | 30 bytes |

**Delta keyframe insertion:** Every N seconds (configurable, default 10 s), or when the accumulated delta size exceeds 10% of the base asset, the server SHOULD send a new full keyframe and reset the delta sequence. This prevents drift accumulation and allows late-joining clients to sync.

### 11.4 Gaussian Splat Delta format (`application/vnd.flux.gs-delta`)

Gaussian Splats are represented as arrays of splats, each with position (xyz), rotation (quaternion), scale (xyz), opacity, and spherical harmonic (SH) coefficients. A GS delta encodes **only the changed splats**.

```json
{
  "delta_base": {
    "asset_id": "gs-scene-take-001",
    "sha256": "f1a2b3..."
  },
  "delta_seq": 100,
  "ts_ns": 1743580812345678901,
  "total_splat_count": 500000,
  "changed_splat_count": 1200,
  "encoding": "packed_f16",
  "fields_mask": "0x3F",
  "compression": "zstd",
  "data_offset": 0,
  "data_length": 28800
}
```

**`fields_mask` bitfield — which splat attributes are included:**

```
Bit 0: position (xyz, 3×f16 = 6 bytes)
Bit 1: rotation (quaternion, 4×f16 = 8 bytes)
Bit 2: scale (xyz, 3×f16 = 6 bytes)
Bit 3: opacity (1×f16 = 2 bytes)
Bit 4: SH coefficients (variable, depends on SH degree)
Bit 5: splat_id index (uint32 = 4 bytes) — which splat changed
```

**Binary layout per changed splat (when all fields present):**

```
[ splat_id: uint32 ]           — index into the base splat array
[ position: 3 × float16 ]     — xyz
[ rotation: 4 × float16 ]     — quaternion (xyzw)
[ scale: 3 × float16 ]        — xyz
[ opacity: float16 ]           — alpha
[ sh_coeffs: N × float16 ]    — SH (N depends on degree: 1→1, 2→4, 3→9 coefficients × 3 RGB)
```

Total per splat (SH degree 1, all fields): 4 + 6 + 8 + 6 + 2 + 6 = **32 bytes**.
For 1,200 changed splats: 38.4 KB uncompressed, ~15–20 KB with zstd — easily fits in a single datagram burst.

**Delivery:** GS deltas use `priority: "realtime"` and are sent as QUIC Datagrams (unreliable). If a delta is lost, the next delta (or the next keyframe) supersedes it. Deltas are cumulative-relative-to-keyframe: each delta contains the full current state of the changed splats (not a diff-of-diff), so any single lost delta only causes a brief visual glitch until the next delta or keyframe arrives.

### 11.5 Gaussian Splat Sequences (`application/vnd.flux.gs-sequence`)

For pre-rendered or captured 4D Gaussian Splat sequences (e.g., volumetric video), FLUX supports streaming a temporal sequence of GS frames synchronised with the video timeline:

```json
{
  "asset_id": "gs-seq-volumetric-actor-a",
  "mime_type": "application/vnd.flux.gs-sequence",
  "sequence_id": "gs-seq-001",
  "total_steps": 6000,
  "step_fps": 30,
  "step_duration_ns": 33333333,
  "total_splat_count": 200000,
  "sh_degree": 2,
  "encoding": "packed_f16",
  "keyframe_interval": 30,
  "compression": "zstd",
  "loop": false,
  "frame_assoc": {
    "mode": "range",
    "start_ns": 1743580900000000000,
    "end_ns": 1743581100000000000
  }
}
```

**Sequence delivery model:**

```
                  Keyframe (all splats)
                  ┌───────────────────┐
Step 0:           │ 200K splats, full │  ~6 MB compressed
                  └────────┬──────────┘
                           │
        ┌──────────────────┼──────────────────┐──── ...
        ▼                  ▼                  ▼
Step 1: GS delta      Step 2: GS delta   Step 3: GS delta
        ~20 KB              ~18 KB             ~22 KB
        (datagram)          (datagram)         (datagram)
        ...
Step 29: GS delta
        ~19 KB
                  ┌───────────────────┐
Step 30:          │ Keyframe (full)   │  ~6 MB compressed
                  └───────────────────┘
```

- **Keyframes** are sent as full `model/vnd.gaussian-splat` assets via reliable QUIC Streams.
- **Delta steps** are sent as `application/vnd.flux.gs-delta` via QUIC Datagrams (realtime priority).
- Each delta step includes `step_index` in the frame association (`mode: "sequence"`).
- The receiver pre-buffers `keyframe_interval` steps ahead when bandwidth allows.
- If a delta step is lost (datagram delivery), the receiver holds the last known state until the next delta or keyframe.
- The receiver can request a re-keyframe via `QUALITY_REQUEST` with `"force_gs_keyframe": true`.

### 11.6 Use case: per-frame camera tracking

```json
{
  "frame_ts_ns": 1743580812345678901,
  "tracking": {
    "type": "freed_extended",
    "camera_id": "CAM_A",
    "pos_mm": { "x": 1234.5, "y": -567.8, "z": 890.1 },
    "rot_deg": { "pan": 12.345, "tilt": -5.678, "roll": 0.001 },
    "zoom_mm": 35.0,
    "focus_mm": 2500.0,
    "fov_deg": 42.5,
    "lens_model": "Angenieux_EZ-1",
    "confidence": 0.998
  }
}
```

For high-frequency tracking (≥120 fps), the `application/vnd.flux.tracking` asset contains an array of zstd-compressed tracking samples sent as realtime-priority datagrams, associated frame-by-frame via `group_ts_ns`.

---

## 12. FLUX-C — Upstream control channel

The receiver can send control commands to the server (PTZ, audio mix, routing).

### 12.1 Rate limiting

All upstream control commands are subject to rate limiting:

| Parameter | Default | Negotiation |
|---|---|---|
| `max_commands_per_second` | 60 | Client declares in capabilities; server may reduce in SESSION_ACCEPT |
| `burst_allowance` | 10 | Maximum commands queued in a burst |

If the receiver exceeds the rate limit, the server MUST discard excess commands and MAY send a `CONTROL_THROTTLE` warning:

```json
{
  "type": "control_throttle",
  "ts_ns": 1743580812345678901,
  "current_rate": 85,
  "max_rate": 60,
  "dropped_count": 12
}
```

### 12.2 PTZ control

```json
{
  "type": "ptz",
  "channel_id": 0,
  "ts_ns": 1743580812345678901,
  "pan_deg": 12.5,
  "tilt_deg": -3.2,
  "zoom_pos": 0.65,
  "focus_pos": 0.42,
  "iris_fstop": 4.0,
  "speed": 0.8,
  "protocol_hint": "visca_over_flux"
}
```

### 12.3 Audio mix control

```json
{
  "type": "audio_mix",
  "ts_ns": 1743580812345678901,
  "channels": {
    "0": { "gain_db": -6.0, "mute": false, "pan": 0.0 },
    "1": { "gain_db": 0.0,  "mute": true,  "pan": 0.0 },
    "2": { "gain_db": -12.0,"mute": false, "pan": -0.5 }
  }
}
```

---

## 13. FEC and error recovery

FLUX combines mechanisms activated dynamically by the BW Governor **after** load shedding (see §5.4):

| Mechanism | Activation | Overhead | Latency cost | Case |
|-----------|-----------|---------|------|------|
| **None** | loss < 0.5% | 0% | 0 | Healthy link |
| **XOR Row FEC** | loss > 0.5% (base layer) | ~25% | +1 FEC row period | Short bursts |
| **Reed-Solomon 2D** | loss > 2% (base layer) | ~50% | +1 block period | Long bursts |
| **Selective ARQ** | Base layer keyframes only | Variable | +1 RTT | Critical IDRs (via dedicated reliable QUIC stream) |
| **Layer drop** | Insufficient BW (EMERGENCY step 1) | 0% | 0 | Severe congestion |

The FEC ratio is negotiated per channel and adjusted in real time via `LAYER_STATUS`.

**Important:** FEC overhead is calculated **only against the surviving base layer traffic**, not against the full multi-layer bitrate. This prevents the EMERGENCY death spiral described in v0.3.

At high frame rates (≥120 fps), XOR Row FEC groups SHOULD span at most 4 frames to keep the repair latency below 1 frame period at the target fps.

---

## 14. Per-frame metadata — recommended JSON schema

All frames with `HAS_METADATA=1` carry a prepended JSON block (see §4.4 for layout). Extensible base schema:

```json
{
  "ts_ns": 1743580812345678901,
  "frame_index": 5000,
  "fps": 240,
  "scene": "VP_Take_003",
  "take": 3,
  "production": "LUCAB_Production_01",
  "colorimetry": {
    "primaries": "bt2020",
    "transfer": "pq",
    "matrix": "bt2020nc",
    "max_cll_nits": 1000,
    "max_fall_nits": 400
  },
  "embed_refs": ["scene-glb-take-003", "tracking-cam-a"],
  "delta_refs": ["glb-delta-seq-42", "gs-delta-seq-100"],
  "tally": { "program": true, "preview": false },
  "custom": {}
}
```

The `custom` field allows proprietary extensions without breaking the base schema.

---

## 15. Security

| Feature | `crypto_none` | `crypto_quic` | `crypto_quic_aes` |
|---|---|---|---|
| Transport encryption | None | TLS 1.3 (QUIC native) | TLS 1.3 (QUIC native) |
| Payload encryption | None | None (QUIC-encrypted) | AES-256-GCM per-frame |
| Authentication | JWT/API key over TCP control | mTLS or JWT in handshake | mTLS or JWT in handshake |
| Asset integrity | SHA-256 in EMBED_MANIFEST | SHA-256 in EMBED_MANIFEST | SHA-256 in EMBED_MANIFEST |
| Access control | FLUX Registry (OAuth 2.0 / API keys) | Same | Same |

**`crypto_quic_aes` latency warning:** The additional AES-256-GCM encryption adds ~2–5 µs per frame on hardware with AES-NI. Without AES-NI, latency can increase by 0.5–1 ms per frame. This mode is intended only for DRM or classified content. Implementations SHOULD warn operators when this mode is active.

---

## 16. Implementation — notes for GStreamer / Rust

### Suggested GStreamer elements

```
fluxsrc          — QUIC/UDP receiver, emits pads per channel/layer
fluxsink         — QUIC/UDP transmitter
fluxdemux        — splits media / embed / delta / metadata into pads
fluxsync         — MSS barrier (multi-stream jitter buffer, software or hardware PTP)
fluxembedsrc     — injects FLUX-E assets into the pipeline
fluxembeddec     — receives and reassembles assets, emits on downstream pad
fluxdeltadec     — applies GLB/GS deltas to base assets in memory
fluxcdbc         — measures BW, generates adaptive CDBC_FEEDBACK
fluxtally        — manages bidirectional tally (JSON + compact binary)
fluxcrypto       — handles crypto mode selection (none/quic/quic+aes)
```

### GStreamer pipeline example (receiver, high fps)

```
fluxsrc uri=flux://192.168.1.50:7400 crypto=crypto_none ! fluxdemux name=d
d.video_0_0 ! fluxsync group=1 ptp-mode=software ! h265parse ! nvh265dec ! videoconvert ! autovideosink
d.audio_0   ! fluxsync group=1 ptp-mode=software ! audio/x-raw,format=F32LE ! autoaudiosink
d.embed_glb ! fluxembeddec mime=model/gltf-binary ! fluxdeltadec ! appsink name=glb_sink
d.embed_gs  ! fluxembeddec mime=model/vnd.gaussian-splat ! fluxdeltadec ! appsink name=gs_sink
d.metadata  ! appsink name=meta_sink
```

### GStreamer pipeline example (VP with delta GS)

```
fluxsrc uri=flux://192.168.1.50:7400 crypto=crypto_quic ! fluxdemux name=d
d.video_0_0 ! fluxsync group=1 ! queue max-size-time=20000000 ! nvh265dec ! glimagesink

# GLB scene with live delta updates
d.embed_glb       ! fluxembeddec ! fluxdeltadec name=scene_dec
d.delta_glb       ! scene_dec.delta_sink
scene_dec.src     ! appsink name=scene_sink emit-signals=true

# Gaussian Splat sequence with live delta
d.embed_gs        ! fluxembeddec ! fluxdeltadec name=gs_dec
d.delta_gs        ! gs_dec.delta_sink
gs_dec.src        ! appsink name=gs_sink emit-signals=true

d.tracking        ! appsink name=tracking_sink
```

### Relevant Rust crates

```toml
[dependencies]
quinn          = "0.11"   # QUIC (backed by rustls), with datagram support
s2n-quic       = "1"      # Alternative: AWS QUIC with datagram support
serde_json     = "1"      # JSON metadata
zstd           = "0.13"   # asset/delta compression
sha2           = "0.10"   # EMBED integrity
mdns-sd        = "0.10"   # DNS-SD discovery
tokio          = { version = "1", features = ["full"] }
bytes          = "1"      # buffer management
gstreamer      = "0.22"   # GStreamer-rs integration
half           = "2"      # float16 for GS delta encoding
glam           = "0.27"   # vector/quaternion math for delta transforms
```

### Implementation note: quinn datagram configuration

```rust
// Enable QUIC Datagrams (RFC 9221) in quinn
let mut transport_config = quinn::TransportConfig::default();
transport_config.datagram_receive_buffer_size(Some(2 * 1024 * 1024)); // 2 MB
transport_config.max_concurrent_bidi_streams(64u32.into());
transport_config.max_concurrent_uni_streams(128u32.into());

// For crypto_none mode, use raw UDP sockets directly:
let socket = tokio::net::UdpSocket::bind("0.0.0.0:7400").await?;
// FLUX framing is applied directly to UDP payloads
```

---

## 17. QUIC transport summary per session

### When `crypto_quic` or `crypto_quic_aes`:

| QUIC mechanism | Content | Direction | Urgency (RFC 9218) |
|---|---|---|---|
| Stream 0 (bidi) | Control (SESSION, ANNOUNCE, KEEPALIVE) | Bidirectional | 0 (critical) |
| Stream 2 (uni) | Selective ARQ retransmits (base layer keyframes) | S→C | 0 |
| Datagram | CDBC_FEEDBACK + TALLY | C→S | — (unreliable) |
| Datagram | SYNC_ANCHOR | S→C | — (unreliable) |
| Datagram | Media: all channels, all layers | S→C | — (unreliable, app-level priority via FLUX header) |
| Datagram | FEC_REPAIR | S→C | — (unreliable) |
| Datagram | EMBED_CHUNK (realtime delta) | S→C | — (unreliable) |
| Stream 4..N (uni) | EMBED_MANIFEST | S→C | 0 |
| Stream 4..N (uni) | EMBED_CHUNK (background/burst full assets) | S→C | 3 (burst) or 6 (background) |
| Datagram | BANDWIDTH_PROBE | S→C | — (unreliable) |
| Datagram | KEEPALIVE | Both | — (unreliable) |
| Datagram | UPSTREAM_CONTROL (FLUX-C) | C→S | — (unreliable) |

### When `crypto_none`:

| Transport | Content | Direction | Notes |
|---|---|---|---|
| UDP (main port) | All datagram-class frames | Both | FLUX framing directly over UDP |
| TCP (control port) | SESSION, ANNOUNCE, STREAM_END, EMBED_MANIFEST, EMBED_ACK | Both | Reliable control channel |
| TCP (control port) | EMBED_CHUNK (background/burst) | S→C | Reliable asset transfer |
| UDP (main port) | EMBED_CHUNK (realtime delta) | S→C | Unreliable, time-critical |

---

## 18. Version negotiation and backwards compatibility

FLUX v0.4 clients connecting to v0.3 servers:
- The server will reject `crypto_none` — the client MUST fall back to `crypto_quic`.
- Delta embed types will not be in the server's `embed_catalog` — the client falls back to full asset transfers.
- QUIC Datagram support may not be available — the client falls back to QUIC Streams for media (with the latency implications documented in v0.3 analysis). The client detects this when `max_datagram_frame_size` is absent from the server's QUIC transport parameters.
- High fps negotiation: if the server's `max_fps` is lower than the client's request, the session operates at the server's maximum.

---

*FLUX v0.4 — LUCAB Media Technology — draft for internal review*
