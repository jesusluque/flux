# FLUX Protocol PoC — Technical Documentation

**Spec reference:** FLUX Protocol Spec v0.6.3  
**Implementation language:** Rust (GStreamer plugins), C/C++ (Filament renderer)  
**Platform:** macOS (Apple Silicon / x86_64)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Repository Layout](#2-repository-layout)
3. [Protocol Fundamentals](#3-protocol-fundamentals)
   - 3.1 [Wire Format — 32-byte FLUX Header (§4.1)](#31-wire-format--32-byte-flux-header-41)
   - 3.2 [FLAGS Bits (§4.2–§4.3)](#32-flags-bits-4243)
   - 3.3 [Frame Types (§4.4)](#33-frame-types-44)
   - 3.4 [Session Model (§3)](#34-session-model-3)
   - 3.5 [Transport — QUIC/TLS (§2.5)](#35-transport--quictls-25)
4. [GStreamer Plugin Inventory](#4-gstreamer-plugin-inventory)
   - 4.1 [flux-framing — shared wire-format library](#41-flux-framing--shared-wire-format-library)
   - 4.2 [gst-fluxframer — server-side FLUX framer](#42-gst-fluxframer--server-side-flux-framer)
   - 4.3 [gst-fluxdeframer — client-side FLUX deframer](#43-gst-fluxdeframer--client-side-flux-deframer)
   - 4.4 [gst-fluxsink — server-side QUIC sender](#44-gst-fluxsink--server-side-quic-sender)
   - 4.5 [gst-fluxsrc — client-side QUIC receiver](#45-gst-fluxsrc--client-side-quic-receiver)
   - 4.6 [gst-fluxdemux — FLUX frame router](#46-gst-fluxdemux--flux-frame-router)
   - 4.7 [gst-fluxcdbc — Client-Driven Bandwidth Control (§5)](#47-gst-fluxcdbc--client-driven-bandwidth-control-5)
   - 4.8 [gst-fluxsync — Multi-Stream Synchronisation barrier (§6.3)](#48-gst-fluxsync--multi-stream-synchronisation-barrier-63)
5. [poc001 — Single-Stream Unicast](#5-poc001--single-stream-unicast)
6. [poc002 — Four-Stream Mosaic with MSS](#6-poc002--four-stream-mosaic-with-mss)
7. [poc003 — fluxvideotex Live Video Texture on Filament 3D Cube](#7-poc003--fluxvideotex-live-video-texture-on-filament-3d-cube)
8. [Key Engineering Problems Solved](#8-key-engineering-problems-solved)
9. [Known Limitations and Future Work](#9-known-limitations-and-future-work)

---

## 1. Overview

This repository is a proof-of-concept implementation of the **FLUX Protocol** — a low-latency, multi-stream media-transport protocol carried over QUIC/TLS. The PoC covers three concrete demonstrations:

| PoC | Description | Spec coverage |
|-----|-------------|---------------|
| **poc001** | Single-stream unicast: H.265 video from server to client over QUIC, with CDBC feedback and FLUX-C upstream control | §3, §4, §5.1–§5.2, §12 |
| **poc002** | Four-stream mosaic: four independent H.265 streams synchronised at the client into a 2×2 mosaic using the Multi-Stream Synchronisation (MSS) barrier | §4, §5, §6.3 |
| **poc003** | `fluxvideotex`: RGBA video frames applied as a live GPU texture onto a Filament-rendered 3D cube, using the `flux://` URI scheme for texture binding | §10.8, §10.10, §16 |

All three PoCs share a common Rust workspace of seven GStreamer plugins.

---

## 2. Repository Layout

```
flux/
├── spec/
│   └── FLUX_Protocol_Spec_v0_6_3_EN.md   # Authoritative spec
│
├── tools/
│   ├── gstreamer/                         # Rust workspace — 7 GStreamer plugins + 1 lib
│   │   ├── Cargo.toml                     # Workspace manifest
│   │   ├── flux-framing/                  # Shared wire-format library (no GStreamer dep)
│   │   ├── gst-fluxframer/                # Server-side FLUX framer (BaseTransform)
│   │   ├── gst-fluxdeframer/              # Client-side FLUX deframer (BaseTransform)
│   │   ├── gst-fluxsink/                  # Server-side QUIC sender (BaseSink)
│   │   ├── gst-fluxsrc/                   # Client-side QUIC receiver (PushSrc)
│   │   ├── gst-fluxdemux/                 # FLUX frame router (Element)
│   │   ├── gst-fluxcdbc/                  # CDBC observer (BaseTransform passthrough)
│   │   └── gst-fluxsync/                  # MSS sync barrier (BaseTransform)
│   │
│   └── filament/                          # C/C++ Filament renderer for poc003
│       ├── CMakeLists.txt
│       ├── cmake/FetchFilament.cmake
│       └── gst-fluxvideotex/
│           ├── CMakeLists.txt
│           ├── filament_scene.h / .cpp    # Offscreen Filament renderer
│           ├── fluxvideotex.h / .c        # GStreamer BaseTransform (C)
│           └── assets/
│               ├── gen_cube.py            # GLB cube generator
│               └── cube.glb               # Pre-generated cube asset
│
├── poc001/                                # Single-stream unicast
│   ├── Cargo.toml
│   ├── server/src/main.rs
│   └── client/src/main.rs
│
├── poc002/                                # Four-stream mosaic
│   ├── Cargo.toml
│   ├── multi-server/src/main.rs
│   └── mosaic-client/src/main.rs
│
├── poc003/                                # fluxvideotex 3D cube
│   ├── CMakeLists.txt
│   └── src/main.cpp
│
└── docs/
    └── DOCUMENTATION.md                   # This file
```

---

## 3. Protocol Fundamentals

### 3.1 Wire Format — 32-byte FLUX Header (§4.1)

Every FLUX frame begins with a fixed 32-byte big-endian header defined in `flux-framing/src/lib.rs`:

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|  VERSION(4) |  TYPE(4)  |  FLAGS(8)   | CHANNEL_ID(16)        |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| LAYER(4) | FRAG(4) |         GROUP_ID(24)                     |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                    GROUP_TIMESTAMP_NS (64)                     |
|                                                                |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                   PRESENTATION_TS (32)                         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|               CAPTURE_TS_NS_LO (32)                            |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                   PAYLOAD_LENGTH (32)                          |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|   FEC_GROUP(16)   |       SEQUENCE_IN_GROUP (16)               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

The `GROUP_TIMESTAMP_NS` field is critical for MSS (§6.3): all four server pipelines snap the buffer DTS to the nearest 33 ms grid boundary:

```rust
const FRAME_NS: u64 = 1_000_000_000 / 30; // = 33_333_333
group_timestamp_ns = (dts_ns + FRAME_NS / 2) / FRAME_NS * FRAME_NS;
```

This guarantees that all four streams assign the same `GROUP_TIMESTAMP_NS` to the same logical frame, which is the key used by `gst-fluxsync` for slot-based alignment.

### 3.2 FLAGS Bits (§4.2–§4.3)

| Bit | Mask | Name | Meaning |
|-----|------|------|---------|
| 0 | `0x01` | `FLAG_KEYFRAME` | Buffer contains an IDR/keyframe |
| 1 | `0x02` | `FLAG_END_OF_STREAM` | Last frame of the stream |
| 2 | `0x04` | `FLAG_HAS_METADATA` | An optional 2-byte length-prefixed metadata block follows the header |
| 3 | `0x08` | `FLAG_ENCRYPTED` | Payload is encrypted (not used in PoC) |
| 4 | `0x10` | `FLAG_FEC_BLOCK` | FEC block (not used in PoC) |

### 3.3 Frame Types (§4.4)

| Value | Name | Direction | Description |
|-------|------|-----------|-------------|
| `0x0` | `MediaData` | server→client | Compressed video/audio AU |
| `0x1` | `CdbcFeedbackT` | client→server | CDBC bandwidth feedback |
| `0x2` | `BandwidthProbe` | server→client | BW probe packet |
| `0x3` | `FluxControl` | client→server | FLUX-C upstream control (PTZ, mute, etc.) |
| `0x4` | `MetadataFrame` | either | Sidecar metadata |
| `0x5` | `StreamAnnounce` | server→client | Stream capability announcement (M1) |
| `0x6` | `StreamEnd` | server→client | Stream graceful termination |
| `0x7` | `GlbTextureRole` | either | GLB texture role assignment (§10.10) |
| `0x8` | `SessionInfo` | server→client | Session state update |
| `0x9` | `Keepalive` | client→server | Heartbeat (§3.3) |
| `0xA` | `SessionRequest` | client→server | SESSION handshake request |
| `0xB` | `SessionAccept` | server→client | SESSION handshake response |
| `0xC` | `MetadataFrame` | either | (alias for FLUX-C metadata) |
| `0x12` | `KeepaliveAck` | server→client | Keepalive acknowledgement |

### 3.4 Session Model (§3)

The session lifecycle is:

1. **SESSION_REQUEST** (§3.1): client opens a bidirectional QUIC stream (Stream 0) and sends a length-prefixed JSON `SessionRequest`:
   ```json
   { "version": "0.6.3", "codec": "h265", ... }
   ```
   Wire format: `[u32 BE length][JSON body]`

2. **SESSION_ACCEPT** (§3.2): server responds with a length-prefixed JSON `SessionAccept`:
   ```json
   {
     "session_id": "uuid",
     "keepalive_interval_ms": 1000,
     "keepalive_timeout_count": 3
   }
   ```

3. **STREAM_ANNOUNCE** (§3.1 M1): server sends a `StreamAnnounce` frame on a unidirectional QUIC stream, advertising codec, resolution, frame rate, channel ID, and layer ID.

4. **Media delivery**: each Access Unit is sent as a single unidirectional QUIC stream (stream-per-AU). Fragmented AUs use the `FRAG` nibble: `0x0` = unfragmented, `0x1`–`0xD` = mid-fragments, `0xE` = last fragment.

5. **KEEPALIVE** (§3.3): client sends a `KEEPALIVE` datagram every `keepalive_interval_ms` (default 1000 ms). Server declares session dead after `keepalive_timeout_count` (default 3) consecutive missed keepalives.

6. **Session termination**: client sends `STREAM_END`, or connection is closed with QUIC error code 0.

### 3.5 Transport — QUIC/TLS (§2.5)

All PoCs use `quinn` 0.11 + `rustls` with a **skip-verify** TLS policy (`SkipVerify` struct in `gst-fluxsrc`). This is equivalent to the spec's `crypto_none` mode and is appropriate for a PoC on a trusted local network. The QUIC transport is configured with:

- Datagram receive buffer: 4 MiB
- QUIC keep-alive interval: 5 s (independent of FLUX keepalives)
- Media AUs: unidirectional QUIC streams (one per AU)
- Control frames (KEEPALIVE, CDBC_FEEDBACK, FLUX-C): QUIC datagrams

---

## 4. GStreamer Plugin Inventory

All plugins live in the Rust workspace at `tools/gstreamer/`. They are linked statically into each PoC binary via `plugin_register_static()`.

### 4.1 flux-framing — shared wire-format library

**Crate:** `flux-framing`  
**File:** `tools/gstreamer/flux-framing/src/lib.rs`

Pure-Rust library with no GStreamer dependency. Provides:

| Type | Description |
|------|-------------|
| `FluxHeader` | 32-byte header struct with `encode()`/`decode()` methods |
| `FrameType` | Enum of all frame type values (§4.4) |
| `SessionRequest` / `SessionAccept` | JSON-serialisable session handshake structs |
| `CdbcFeedback` | CDBC feedback payload (§5.2) |
| `BwGovernor` | State machine: `PROBE → STABLE → RAMP_UP / RAMP_DOWN → EMERGENCY` (§5.3–§5.4) |
| `FluxControl` | FLUX-C upstream control command (§12) |
| `KeepalivePayload` | Keepalive datagram body (§3.3) |
| `BandwidthProbe` | BW probe payload (§5.2) |
| `StreamAnnounce` | M1 stream announcement payload (§3.1) |
| `GlbTextureRole` | GLB texture role assignment (§10.10) |
| `EmbedSupport` | Embed capabilities struct |

Constants:
- `FLUX_VERSION = 1` — header version nibble
- `HEADER_SIZE = 32` — fixed header size in bytes

### 4.2 gst-fluxframer — server-side FLUX framer

**Crate:** `gst-fluxframer`  
**File:** `tools/gstreamer/gst-fluxframer/src/lib.rs`  
**GStreamer type:** `BaseTransform` (NeverInPlace)

| Pad | Caps |
|-----|------|
| sink | `video/x-h265, stream-format=byte-stream, alignment=au` |
| src  | `application/x-flux` |

**Function:** Prepends a 32-byte FLUX header to each incoming H.265 Access Unit buffer. The `GROUP_TIMESTAMP_NS` is computed by snapping the buffer's DTS to a 33 ms grid (30 fps boundary), ensuring that all server pipelines in a multi-stream scenario assign the same timestamp to the same logical frame.

**Properties:**

| Property | Type | Default | Description |
|----------|------|---------|-------------|
| `channel-id` | u32 | 0 | FLUX channel identifier |
| `group-id` | u32 | 1 | Sync group identifier (must match fluxsync on client) |
| `layer` | u32 | 0 | Scalable layer index (0–15) |

**Timestamp snapping formula** (`gst-fluxframer/src/lib.rs`):
```rust
const FRAME_NS: u64 = 1_000_000_000 / 30; // 33_333_333 ns
let group_timestamp_ns = (dts_ns + FRAME_NS / 2) / FRAME_NS * FRAME_NS;
```

### 4.3 gst-fluxdeframer — client-side FLUX deframer

**Crate:** `gst-fluxdeframer`  
**File:** `tools/gstreamer/gst-fluxdeframer/src/lib.rs`  
**GStreamer type:** `BaseTransform` (NeverInPlace), uses `submit_input_buffer` + `generate_output` for variable-size reassembly

| Pad | Caps |
|-----|------|
| sink | `application/x-flux` |
| src  | `video/x-h265, stream-format=byte-stream, alignment=au` |

**Function:** Strips the 32-byte FLUX header, reassembles fragmented AUs using a `BTreeMap<frag_index, chunk>`, and assigns GStreamer PTS using a wall-clock anchor formula.

**Fragment reassembly:**
- `frag = 0x0`: unfragmented AU — placed in `ready` immediately
- `frag = 0x1`–`0xD`: mid-fragments — stored in map; `total` is updated when `0xE` arrives
- `frag = 0xE`: last fragment — sentinel stored under key `0xE`; `total = highest_mid_index + 1`
- Assembly complete when `frags.len() == total`

**PTS formula** (implemented in `generate_output`):

```
On first frame:
    gts_epoch  = hdr.group_timestamp_ns   (wall-clock origin)
    rt_anchor  = current_running_time()   (pipeline clock origin)

Every frame:
    delta_ns   = hdr.group_timestamp_ns - gts_epoch
    pts        = rt_anchor + delta_ns + TOTAL_LATENCY_NS
```

Where `TOTAL_LATENCY_NS = 400_000_000` (400 ms). This gives the compositor sufficient headroom for decode + render while ensuring cross-stream PTS alignment (all streams share the same `group_timestamp_ns` for the same logical frame).

**Pause/resume handling:** On `PausedToPlaying`, both `rt_anchor` and `gts_epoch` are reset. Without resetting `gts_epoch`, `delta_ns` would include the entire pre-pause running time plus pause duration, producing PTS values far in the future that the compositor would silently drop.

**DISCONT propagation:** `FluxSrc` stamps `BUFFER_FLAG_DISCONT` on the first buffer of each new session. `FluxDeframer` propagates this flag to `h265parse` and `vtdec_hw` so they flush GOP state and wait for the next IDR.

### 4.4 gst-fluxsink — server-side QUIC sender

**Crate:** `gst-fluxsink`  
**File:** `tools/gstreamer/gst-fluxsink/src/lib.rs`  
**GStreamer type:** `BaseSink`

| Pad | Caps |
|-----|------|
| sink | `application/x-flux` |

**Function:** Binds a QUIC endpoint, accepts client connections, performs the SESSION handshake (§3.1–§3.2) on Stream 0, then sends each incoming FLUX-framed AU on a short-lived unidirectional QUIC stream. Sends `STREAM_ANNOUNCE` once per channel. Receives `CDBC_FEEDBACK` and `FLUX-C` datagrams from clients.

**Properties:**

| Property | Type | Default | Description |
|----------|------|---------|-------------|
| `bind-address` | String | `"0.0.0.0"` | Local bind address |
| `port` | u32 | 7400 | QUIC listen port |
| `session-id-last` | String | — | Read-only: last negotiated session ID |
| `cdbc-reports-received` | u64 | — | Read-only: total CDBC_FEEDBACK datagrams received |
| `bw-probes-sent` | u64 | — | Read-only: total BW probe datagrams sent |
| `bw-governor-state` | String | — | Read-only: current BwGovernor state name |

**IDR gate:** Non-keyframe buffers are dropped until the first IDR is received by the client. This prevents the decoder from receiving mid-GOP data on initial connection.

**Monotonicity enforcement:** `last_sent_pts_ns` tracks the last-sent PTS; frames with PTS ≤ last value are discarded to prevent downstream decoder stalls.

**Generation counter:** Incremented on each new client connection. Used to discard stale buffers from a previous session in case of rapid client reconnect.

**Public API:**
```rust
pub fn subscribe_flux_control(&self) -> mpsc::Receiver<FluxControl>
```

### 4.5 gst-fluxsrc — client-side QUIC receiver

**Crate:** `gst-fluxsrc`  
**File:** `tools/gstreamer/gst-fluxsrc/src/lib.rs`  
**GStreamer type:** `PushSrc` (live source)

| Pad | Caps |
|-----|------|
| src | `application/x-flux` |

**Function:** Connects to a FLUX server over QUIC/TLS, performs the SESSION handshake, receives media AUs on unidirectional streams and control datagrams, passes them as `gst::Buffer` objects downstream. Sends KEEPALIVE datagrams at negotiated interval. Implements a three-stage NetSim pipeline.

**Connection lifecycle:**
1. `start()`: creates Tokio runtime, builds QUIC endpoint with `SkipVerify` TLS, connects, performs SESSION handshake, spawns datagram-recv task and uni-stream listener task, spawns delay thread.
2. `create()` loop: drains delayed channel, checks keepalive/session-dead timer, blocks on `raw_rx` with 500 ms timeout, applies NetSim, wraps datagram in `gst::Buffer`.
3. `stop()`: sets `stop_flag`, closes QUIC connection, drops Tokio runtime.

**NetSim pipeline** (applied in order inside `create()`):

1. **Random loss** (`sim-loss-pct`): LCG pseudo-random drop, stored as `loss_pct_x100` to avoid floating-point atomics.
2. **Token-bucket bandwidth throttle** (`sim-bw-kbps`): bytes-per-second rate limiting. When tokens are exhausted, the datagram is pushed onto the delay heap (non-blocking) rather than sleeping the source thread.
3. **Artificial delay** (`sim-delay-ms`): datagram is pushed onto a binary-heap priority queue, ordered by `release = Instant::now() + delay`. A dedicated delay thread drains the heap and forwards to `delayed_tx`.

**Properties:**

| Property | Type | Default | Description |
|----------|------|---------|-------------|
| `address` | String | `"127.0.0.1"` | FLUX server IP address |
| `port` | u32 | 7400 | FLUX server QUIC port |
| `session-id` | String | — | Read-only: negotiated session ID |
| `keepalive-interval-ms` | u32 | — | Read-only: interval from SESSION_ACCEPT |
| `keepalive-timeout-count` | u32 | — | Read-only: timeout count from SESSION_ACCEPT |
| `keepalives-sent` | u64 | — | Read-only: total KEEPALIVE datagrams sent |
| `sim-loss-pct` | f64 | 0.0 | NetSim random loss probability (0–100%) |
| `sim-delay-ms` | u32 | 0 | NetSim artificial one-way latency (0–500 ms) |
| `sim-bw-kbps` | u32 | 0 | NetSim token-bucket BW cap (0 = unlimited) |

**Latency query:** Reports `live=true, min=200ms` in response to GST_QUERY_LATENCY, preventing the compositor from logging "Latency query failed" during startup.

**Public API:**
```rust
pub fn send_datagram(&self, bytes: Vec<u8>) -> bool
```

Used by poc001 client to send FLUX-C commands and CDBC feedback upstream.

### 4.6 gst-fluxdemux — FLUX frame router

**Crate:** `gst-fluxdemux`  
**File:** `tools/gstreamer/gst-fluxdemux/src/lib.rs`  
**GStreamer type:** `Element` (plain, with dynamic src pads)

| Pad | Caps | Presence |
|-----|------|----------|
| sink | `application/x-flux` | Always |
| `media_0` | `application/x-flux` | Sometimes (dynamic) |
| `control` | any | Sometimes (dynamic) |
| `cdbc` | any | Sometimes (dynamic) |
| `misc` | any | Sometimes (dynamic) |

**Function:** Routes incoming FLUX frames to typed output pads based on the `frame_type` nibble:

| Frame type | Output pad |
|------------|-----------|
| `MediaData (0x0)` | `media_0` |
| `SessionInfo (0x8)`, `Keepalive (0x9)`, `StreamAnnounce (0x5)`, `StreamEnd (0x6)` | `control` |
| `CdbcFeedbackT (0x1)` | `cdbc` |
| All others | `misc` |

Src pads are created on demand (first buffer of each type). Sticky events (`STREAM_START`, `CAPS`, `SEGMENT`) are cached and replayed on newly-created pads to ensure downstream elements receive all required upstream events.

**Latency query:** Both the element itself and each dynamically-created src pad answer `GST_QUERY_LATENCY` with `live=true, min=200ms` to prevent compositor startup warnings.

### 4.7 gst-fluxcdbc — Client-Driven Bandwidth Control (§5)

**Crate:** `gst-fluxcdbc`  
**File:** `tools/gstreamer/gst-fluxcdbc/src/lib.rs`  
**GStreamer type:** `BaseTransform` (passthrough — both pads `application/x-flux`)

**Function:** Observes `MediaData` frames in passthrough mode, measuring inter-arrival jitter (RFC 3550 §A.8 EWMA), sequence-gap-based loss, and receive bitrate. Periodically sends `CDBC_FEEDBACK` datagrams (§5.2) upstream via an installed send callback.

**Adaptive interval** (§5.1):
- Normal: `cdbc-interval` (default 50 ms) — used in `STABLE/PROBE/RAMP_UP` states
- Fast: `cdbc-min-interval` (default 10 ms) — used when `loss_pct > 0.5%`

**Jitter measurement** (RFC 3550 §A.8):
```rust
jitter_ms += (inter_arrival_ms - jitter_ms) / 16.0;  // EWMA with α=1/16
```

**Loss measurement:** Sequence gap detection using `sequence_in_group`. Gaps > 1000 are ignored (assumed to be sequence number wrap or out-of-order).

**CDBC_FEEDBACK payload** (`CdbcFeedback` struct):
```json
{
  "ts_ns": 1234567890,
  "rx_bps": 4000000,
  "avail_bps": 4000000,
  "rtt_ms": 0.0,
  "loss_pct": 0.0,
  "jitter_ms": 1.2,
  "fps_actual": 0.0,
  "datagram_drop_count": 0,
  "probe_result_bps": 0
}
```

**Public API:**
```rust
pub fn set_send_callback(&self, f: impl Fn(Vec<u8>) + Send + Sync + 'static)
```

In poc001/poc002, this callback calls `fluxsrc.send_datagram()` to route the CDBC datagram over the existing QUIC connection.

**Read-only properties:** `loss-pct`, `jitter-ms`, `rx-bps`, `reports-sent`, `datagrams-lost-total`

**Configurable properties:** `cdbc-interval` (u64, ms), `cdbc-min-interval` (u64, ms)

### 4.8 gst-fluxsync — Multi-Stream Synchronisation barrier (§6.3)

**Crate:** `gst-fluxsync`  
**File:** `tools/gstreamer/gst-fluxsync/src/lib.rs`  
**GStreamer type:** `BaseTransform` (AlwaysInPlace)

| Pad | Caps |
|-----|------|
| sink | `application/x-flux` |
| src  | `application/x-flux` |

**Function:** Implements the MSS Sync Barrier from §6.3. Each `fluxsync` instance represents one stream within a sync group. Frames from all `nstreams` are aligned by `GROUP_TIMESTAMP_NS` before being released downstream.

**Slot-based synchronisation algorithm:**

A process-global registry (`lazy_static! REGISTRY`) maps `group_id → Arc<GroupEntry>`. Each `GroupEntry` holds a `Mutex<GroupState>` and a `Condvar`.

`GroupState` contains:
- `slots: BTreeMap<u64 (group_timestamp_ns), Slot>` — alignment slots
- `newest_ts` — highest `group_timestamp_ns` seen across all streams
- `latency_ns` — eviction window (default 200 ms)
- `frames_synced`, `frames_dropped`, `max_skew_ns` — statistics

`Slot` contains:
- `buffers: Vec<Option<gst::Buffer>>` — one slot per stream (indexed by `stream`)
- `count: u32` — number of streams that have deposited
- `ready: bool` — set when `count == nstreams`
- `evicted: bool` — set on timeout

**Per-buffer processing** (`transform_ip`):
1. Read `GROUP_TIMESTAMP_NS` from FLUX header
2. Deposit buffer into slot for that timestamp
3. If `count == nstreams`: mark `ready`, notify_all
4. Evict slots older than `newest_ts - latency_ns`
5. Condvar-wait (in `CHECK_INTERVAL_MS = 10ms` increments) until `ready || evicted`
6. On wakeup: copy aligned buffer bytes back into `buf`, update timestamps

**Early eviction:** If `newest_ts > ts + latency_ns` during the wait loop, the slot is evicted immediately rather than waiting for the full timeout. This bounds the per-frame stall to `CHECK_INTERVAL_MS` when a stream is artificially delayed.

**Pause/resume handling:** On `PlayingToPaused`, all slots are evicted and cleared. On resume, `newest_ts` is reset to 0 to prevent all post-resume slots from being immediately evicted as stale.

**Queue requirement:** In poc002, a `queue(max-size-buffers=600, leaky=downstream)` is inserted between `fluxcdbc` and `fluxsync` per stream. Without this, `fluxsync`'s blocking condvar wait would stall the `fluxsrc::create()` streaming thread, causing `raw_tx full` datagram drops.

**Properties:**

| Property | Type | Default | Description |
|----------|------|---------|-------------|
| `group` | u32 | 0 | GROUP_ID (must match server-side `group-id`) |
| `stream` | u32 | 0 | 0-based index of this stream in the group |
| `nstreams` | u32 | 1 | Total number of streams in the group |
| `latency` | u64 | 200 | Alignment window / eviction timeout (ms) |
| `frames-synced` | u64 | — | Read-only: slots released with all streams aligned |
| `frames-dropped` | u64 | — | Read-only: frames passed through after eviction |
| `max-skew-ns` | u64 | — | Read-only: maximum observed GROUP_TIMESTAMP_NS skew within a slot |

---

## 5. poc001 — Single-Stream Unicast

**Location:** `poc001/`  
**Purpose:** Demonstrates end-to-end FLUX unicast delivery: one H.265 stream from server to client over QUIC, with CDBC feedback and FLUX-C upstream control commands.

### 5.1 Server Pipeline

```
videotestsrc pattern=smpte is-live=true
  → videoconvertscale
  → video/x-raw, width=1280, height=720, framerate=60/1
  → vtenc_h265 realtime=true allow-frame-reordering=false bitrate=4000
  → h265parse config-interval=-1
  → video/x-h265, stream-format=byte-stream, alignment=au
  → fluxframer channel-id=0 group-id=1
  → fluxsink port=7400
```

The server also subscribes to `fluxsink.subscribe_flux_control()` to receive FLUX-C commands from the client. It handles:

| FLUX-C command | Action |
|----------------|--------|
| `test_pattern` | Changes `videotestsrc pattern` property |
| `ptz` | Logs PTZ preset (pan/tilt) |
| `audio_mix` | Logs audio mute state |
| `routing_info` | Responds with routing info log |

### 5.2 Client Pipeline

```
fluxsrc address=127.0.0.1 port=7400
  → fluxdemux
      [media_0] → fluxcdbc cdbc-interval=50 cdbc-min-interval=10
                   → fluxdeframer
                   → h265parse
                   → video/x-h265, stream-format=hvc1, alignment=au
                   → vtdec_hw
                   → videoconvertscale
                   → fpsdisplaysink(osxvideosink) sync=false
      [cdbc]    → fakesink
```

`fpsdisplaysink` wraps `osxvideosink` and overlays a live FPS counter. The FPS overlay can be toggled at runtime with the `D` key.

CDBC feedback is wired: `fluxcdbc.set_send_callback(|data| fluxsrc.send_datagram(data))`.

### 5.3 Keyboard Controls (FLUX-C)

All FLUX-C commands are sent as `MetadataFrame (0xC)` QUIC datagrams via `fluxsrc.send_datagram()`, matching §12 / §14.

| Key | Action | FLUX-C command |
|-----|--------|----------------|
| Space | Pause / resume pipeline | — |
| Q | Quit | — |
| S | Print live stats | — |
| P | Send PTZ preset (ch 0, pan=0°, tilt=0°) | `ptz` |
| A | Toggle audio mute on channel 0 | `audio_mix` |
| R | Send routing info request | `routing_info` |
| D | Toggle FPS overlay | — |
| T | Cycle videotestsrc pattern | `test_pattern` |
| L / l | NetSim loss +5% / -5% | — |
| Y / y | NetSim delay +20ms / -20ms | — |
| B / b | NetSim BW +1000kbps / -1000kbps | — |

### 5.4 macOS-specific Notes

- `gst::macos_main()` is required: `osxvideosink` must run in a Cocoa application main thread loop (`[NSApp run]`). `run()` executes on a background thread.
- `/dev/tty` is opened in raw mode (not `stdin`) because `osxvideosink` takes keyboard focus away from the terminal. The TTY file descriptor is opened before `macos_main()` is called, before the process becomes a foreground Cocoa application.
- `OPOST` / `ONLCR` output processing flags are preserved (not disabled by `cfmakeraw`) to avoid staircase output.

### 5.5 Build and Run

```bash
# Build all GStreamer plugins + poc001
cd tools/gstreamer
cargo build --release

cd ../../poc001
# Terminal 1 — server
cargo run --bin server --release

# Terminal 2 — client
cargo run --bin client --release
```

---

## 6. poc002 — Four-Stream Mosaic with MSS

**Location:** `poc002/`  
**Purpose:** Demonstrates Multi-Stream Synchronisation (MSS, §6.3): four independent H.265 streams are aligned by `GROUP_TIMESTAMP_NS` into a 2×2 mosaic. Per-stream artificial delays verify that the sync barrier compensates for transport skew up to the alignment window (200 ms).

### 6.1 Multi-server

Four independent GStreamer pipelines run in one process, each encoding a distinct `videotestsrc` pattern at 640×360 30 fps.

**Per-stream pipeline:**

```
videotestsrc pattern=PATTERNS[i] is-live=true
  → videoconvertscale
  → video/x-raw, format=I420, width=640, height=360, framerate=30/1
  → clockoverlay time-format="%H:%M:%S.%2N" halignment=center font="Sans Bold 36"
  → timeoverlay time-mode=running-time halignment=left valignment=bottom font="Monospace Bold 18"
  → textoverlay text="CAM N" halignment=left valignment=top font="Sans Bold 24"
  → identity sleep-time=0      ← artificial delay knob (microseconds)
  → vtenc_h265 realtime=true allow-frame-reordering=false bitrate=2000
  → h265parse config-interval=-1
  → video/x-h265, stream-format=byte-stream, alignment=au
  → fluxframer channel-id=N group-id=1
  → fluxsink port=740N         (ports 7400–7403)
```

Patterns used: `pinwheel`, `snow`, `smpte`, `ball`.  
Stream labels: `"CAM 0"`, `"CAM 1"`, `"CAM 2"`, `"CAM 3"`.

The `clockoverlay` shows wall-clock time with centisecond precision (`%2N`). In a well-synchronised mosaic, all four tiles show the **same** centisecond digit. Any delay injected via `identity sleep-time` appears as a visible offset in the timestamp display.

**Keyboard controls (multi-server):**

| Key | Action |
|-----|--------|
| 1–4 | Select active stream |
| + / = | Increase selected stream delay by 10 ms |
| - | Decrease selected stream delay by 10 ms |
| R | Reset all delays to 0 |
| S | Show delay table |
| Q | Quit |

### 6.2 Mosaic Client

One GStreamer pipeline receives all four streams and composites them into a 1280×720 output at 30 fps.

**Full pipeline (per stream):**

```
fluxsrc address=127.0.0.1 port=740N
  → fluxdemux
      [media_0] → fluxcdbc cdbc-interval=50 cdbc-min-interval=10
                   → queue max-size-buffers=600 leaky=downstream
                   → fluxsync group=1 stream=N nstreams=4 latency=200
                   → fluxdeframer
                   → h265parse
                   → vtdec_hw qos=false
                   → videoconvertscale
                   → capsfilter video/x-raw,format=NV12,width=640,height=360
                   → compositor.sink_N
      [cdbc]    → fakesink
```

**Compositor output chain:**

```
compositor
  min-upstream-latency=400_000_000   (must match TOTAL_LATENCY_NS in fluxdeframer)
  force-live=true
  → capsfilter video/x-raw,format=NV12,width=1280,height=720,framerate=30/1
  → queue max-size-buffers=4
  → videoconvertscale
  → osxvideosink sync=false async=false
```

**Compositor tile layout:**

| Stream | Column | Row | Position | Size |
|--------|--------|-----|----------|------|
| 0 | 0 | 0 | (0, 0) | 640×360 |
| 1 | 1 | 0 | (640, 0) | 640×360 |
| 2 | 0 | 1 | (0, 360) | 640×360 |
| 3 | 1 | 1 | (640, 360) | 640×360 |

**Key design decisions:**

- `force-live=true` on compositor: prevents a live-source deadlock during `Paused→Playing` transition when dynamic demux pads are not yet linked.
- `qos=false` on `vtdec_hw`: disables QoS-based frame drops. Without this, `vtdec_hw` drops the first several frames whose PTS is in the past (vtdec's decode latency can exceed the PTS headroom at startup).
- `min-upstream-latency=400_000_000` on compositor: must match `TOTAL_LATENCY_NS` (400 ms) in `fluxdeframer`. This tells the compositor that all upstream paths add 400 ms of latency, so it does not stall waiting for a frame it will never receive at the wrong timestamp.
- Output caps locked to `NV12` before compositor starts. If caps are not fixed before any stream connects, the compositor may fixate to a 1×1 output from an early negotiation.
- `fluxsync` latency window of 200 ms: absorbs per-stream delays up to ±200 ms. Beyond this window the barrier evicts the slot and passes the stream through unaligned.

**Keyboard controls (mosaic client):**

| Key | Action |
|-----|--------|
| Space | Pause / resume |
| S | Print fluxsync stats (frames-synced, frames-dropped, max-skew-ns) |
| Q | Quit |

**Diagnostic probes:** Buffer probes on four key src pads per stream, logged every 3 seconds:
- `fluxsrc_src` — frames received from QUIC
- `deframer_src` — reassembled H.265 AUs
- `vtdec_src` — decoded frames
- `tile_caps_src` — frames entering compositor

### 6.3 Build and Run

```bash
cd tools/gstreamer
cargo build --release

cd ../../poc002
# Terminal 1 — multi-server (all 4 streams)
cargo run --bin multi-server --release

# Terminal 2 — mosaic client
cargo run --bin mosaic-client --release
```

---

## 7. poc003 — fluxvideotex Live Video Texture on Filament 3D Cube

**Location:** `poc003/` (C++ binary), `tools/filament/` (GStreamer C plugin + C++ renderer)  
**Purpose:** Demonstrates `fluxvideotex` (§16): a GStreamer element that uploads each incoming RGBA video frame as a GPU texture onto a Filament-rendered 3D scene. The scene is a GLB cube that binds its base colour texture to the `flux://channel/0` URI (§10.10.2).

### 7.1 Architecture

```
main.cpp (C++)
  └─ gst_parse_launch() builds pipeline:
       videotestsrc
         → videoconvert
         → video/x-raw,format=RGBA,width=1280,height=720,framerate=30/1
         → fluxvideotex (C GStreamer plugin)
               │ calls filament_scene_create() on first buffer
               │ calls filament_scene_render() per frame
         → video/x-raw,format=RGBA,width=1280,height=720
         → videoconvert
         → osxvideosink sync=false
```

### 7.2 fluxvideotex GStreamer Element

**File:** `tools/filament/gst-fluxvideotex/fluxvideotex.c`  
**GStreamer type:** `BaseTransform` (C implementation)  
**Category:** `Filter/Effect/Video`  
**Spec reference:** §16

| Pad | Caps |
|-----|------|
| sink | `video/x-raw, format=RGBA` (any size) |
| src  | `video/x-raw, format=RGBA, width=out_width, height=out_height` |

The element accepts any RGBA input size and produces a fixed-size RGBA output (default 1280×720). The transform is non-passthrough; output size differs from input size when the GLB cube is rendered at a different resolution than the source video.

**Properties:**

| Property | Type | Default | Description |
|----------|------|---------|-------------|
| `width` | u32 | 1280 | Render output width (pixels) |
| `height` | u32 | 720 | Render output height (pixels) |
| `rotation-period-x` | f64 | 150.0 | Seconds per full X-axis rotation |
| `rotation-period-y` | f64 | 200.0 | Seconds per full Y-axis rotation |
| `rotation-period-z` | f64 | 300.0 | Seconds per full Z-axis rotation |

**Per-frame processing** (`flux_videotex_transform`):
1. Lazy-init `FilamentScene` on first buffer via `filament_scene_create(w, h, cube_glb_data, cube_glb_len)`.
2. Compute elapsed time from buffer PTS: `elapsed_s = (pts - start_pts) / 1e9`.
3. Call `filament_scene_render(scene, in_rgba, in_w, in_h, elapsed_s, period_x, period_y, period_z, out_rgba)`.
4. Copy PTS/DTS/duration from input to output buffer.

**GLB asset:** The cube GLB is compiled into the plugin binary at build time using `xxd -i cube.glb > cube_glb.h`. This avoids any runtime file-system dependency.

### 7.3 Filament Offscreen Renderer

**File:** `tools/filament/gst-fluxvideotex/filament_scene.cpp`

**Engine setup:**
- Backend: `Backend::OPENGL` (headless, no window)
- SwapChain: `SWAP_CHAIN_CONFIG_READABLE` (enables `readPixels`)
- MaterialProvider: `UbershaderProvider` loaded from `UBERARCHIVE_DEFAULT_DATA`

**GLB loading:**
- `gltfio::AssetLoader` + `ResourceLoader`
- The cube GLB contains an image with URI `flux://channel/0` (§10.10.2). The `ResourceLoader` leaves this texture slot empty (URI unrecognised).
- On each frame, the `flux://channel/0` slot is filled manually by creating a new `Texture::Builder` from the incoming RGBA frame and calling `setParameter("baseColorMap", videoTexture, sampler)` on all material instances.

**Per-frame render loop:**
1. Destroy previous frame's `videoTexture` (Filament textures are not reusable across frames in this design).
2. Create new `Texture` from incoming RGBA: `Texture::Builder().width(w).height(h).format(RGBA8).build(engine)`.
3. `setParameter("baseColorMap", ...)` on all material instances.
4. Apply rotation: `engine.getTransformManager().setTransform(root, rotation_matrix(elapsed_s, period_x, period_y, period_z))`.
5. `renderer->beginFrame(swapChain)` → `renderer->render(view)` → `renderer->readPixels(...)` → `renderer->endFrame()`.
6. Pump message queue: `engine->pumpMessageQueues()` with `sleep_for(100µs)` until `readback_done` atomic flag is set (see §8.1).
7. Vertical flip: Filament framebuffer origin is bottom-left; GStreamer expects top-left. Output is flipped in place.
8. `memcpy` flipped result to `out_rgba`.

**Rotation formula:**
```cpp
filament::math::mat4f rotation =
    mat4f::rotation(elapsed_s / period_x * 2π, float3{1,0,0}) *
    mat4f::rotation(elapsed_s / period_y * 2π, float3{0,1,0}) *
    mat4f::rotation(elapsed_s / period_z * 2π, float3{0,0,1});
```

### 7.4 cube.glb — Asset Generation

**File:** `tools/filament/gst-fluxvideotex/assets/gen_cube.py`

The cube is a standard unit cube (±0.5 in each axis) with UV-mapped faces. Key design decision: the material uses the `KHR_materials_unlit` glTF extension. Without this, the UbershaderProvider uses PBR/LIT shading, and with no scene lighting, the cube renders as black regardless of the texture. `KHR_materials_unlit` makes the renderer output `baseColorTexture` directly with no lighting computation.

The GLB `images` array contains one entry:
```json
{ "uri": "flux://channel/0" }
```
This is the §10.10.2 `flux://` URI scheme. The `ResourceLoader` does not resolve this URI (no registered handler), leaving the texture slot empty. The element fills it each frame with the live video texture.

A 1×1 grey PNG fallback is included as a `bufferView` image per §10.10.3 (used before the first frame arrives).

### 7.5 Build and Run

```bash
# Build Filament plugin and poc003 binary
cd tools/filament
cmake -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build -j$(nproc)

cd ../../poc003
cmake -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build -j$(nproc)
./build/poc003
```

The binary runs for 300 seconds (5 minutes) then exits automatically. A `SIGINT` handler (`Ctrl-C`) triggers clean GStreamer shutdown.

---

## 8. Key Engineering Problems Solved

### 8.1 `readPixels` Asynchronous Completion

**Problem:** Filament's `readPixels` is asynchronous — it schedules a GPU DMA transfer and fires a callback when complete. Calling `memcpy` immediately after `endFrame()` reads stale data.

**Solution:** Set an `std::atomic<bool> readback_done` flag in the callback. After `endFrame()`, spin-pump `engine->pumpMessageQueues()` with `sleep_for(100µs)` until the flag is set. Note: on macOS, `engine->execute()` is a no-op; only `pumpMessageQueues()` drives the message pump.

### 8.2 Black Cube (PBR Lighting)

**Problem:** The UbershaderProvider uses PBR/LIT shading. With no scene lights, the cube renders as solid black even with a valid texture.

**Solution:** Add `KHR_materials_unlit` to the cube GLB material. This instructs the renderer to output `baseColorTexture` directly without lighting computation.

### 8.3 MSS Timestamp Alignment

**Problem:** Wall-clock `now()` at `transform()` time varies slightly between the four server pipelines due to `vtenc_h265` encode latency variation (up to several milliseconds). Using wall-clock time for `GROUP_TIMESTAMP_NS` would cause `fluxsync` slots to never fill.

**Solution:** Use the buffer's DTS (GStreamer pipeline clock, identical for the same logical frame across all pipelines) snapped to a 33 ms grid. The snapping absorbs sub-frame DTS jitter while the pipeline clock guarantees alignment.

### 8.4 Compositor Deadlock on `Paused→Playing` (poc002)

**Problem:** GStreamer's `compositor` requires all upstream sources to pre-roll (deliver one frame in `Paused` state) before transitioning to `Playing`. Live sources that use dynamic demux pads cannot pre-roll in `Paused` state — the QUIC connection is not active until `Playing`.

**Solution:** Set `force-live=true` on the compositor. This makes the compositor treat itself as a live element and skip the pre-roll requirement.

### 8.5 Compositor Caps Negotiation Race (poc002)

**Problem:** If compositor output caps are not locked before any stream connects, the compositor may fixate to a 1×1 output from an early caps negotiation with an unlinked pad.

**Solution:** Build and add the `capsfilter` with locked `video/x-raw,format=NV12,width=1280,height=720,framerate=30/1` caps *before* any `fluxsrc` starts. Link compositor → capsfilter before requesting compositor sink pads.

### 8.6 vtdec_hw QoS Frame Drops (poc002)

**Problem:** `vtdec_hw` honours `GstQosEvent` from `osxvideosink` and drops frames whose PTS is in the past. At startup, the pipeline clock has already advanced past the PTS of the first few decoded frames (vtdec decode latency can exceed the 400 ms PTS headroom).

**Solution:** Set `qos=false` on `vtdec_hw`. For a live mosaic, frame drops at the decoder level are never desired.

### 8.7 PTS Explosion After Pause/Resume (poc001/poc002)

**Problem:** After pause/resume, `GROUP_TIMESTAMP_NS` (a wall-clock value) continues to advance during the pause. `delta_ns = gts - gts_epoch` includes the entire pre-pause running time plus pause duration, producing PTS values tens of seconds in the future that the compositor silently drops.

**Solution:** On `PausedToPlaying`, reset both `rt_anchor` and `gts_epoch` in `fluxdeframer`. The next frame re-anchors both clocks.

### 8.8 Stale Channel on Reconnect (fluxsrc)

**Problem:** On `stop()` + `start()`, the old QUIC recv task may still be running briefly and pushing datagrams into the `raw_tx` channel. A `OnceLock` cannot be reset after first set.

**Solution:** On each `start()`, create a fresh `(raw_tx, raw_rx)` channel pair and atomically replace the `raw_rx_slot` (`Arc<Mutex<Receiver>>`). The old sender becomes disconnected when the old recv task exits, and `try_send` to it returns `SendError`.

---

## 9. Known Limitations and Future Work

| Area | Limitation | Spec reference |
|------|------------|----------------|
| TLS | `SkipVerify` — no certificate validation | §2.5 (`crypto_quic` with PKI) |
| FEC | Not implemented | §7 |
| BW Governor | State machine present (`BwGovernor`) but server-side rate adaptation not wired to encoder bitrate | §5.3–§5.4 |
| CDBC RTT | `rtt_ms` always 0.0 in `CdbcFeedback` — no round-trip measurement | §5.2 |
| Audio | Audio mute toggle (FLUX-C) is logged but no audio pipeline exists | §8 |
| Multicast | Not implemented; all PoCs use unicast QUIC | §9 |
| GLB streaming | `fluxvideotex` loads a static embedded GLB; dynamic GLB loading over FLUX not implemented | §10.10 |
| Session resumption | No QUIC session resumption (0-RTT) | §3.4 |
| Multiple channels | `fluxdemux` only routes `media_0`; multi-channel demux not implemented | §4.4 |
| Windows/Linux | All PoCs use macOS-specific elements (`vtenc_h265`, `vtdec_hw`, `osxvideosink`) | — |
