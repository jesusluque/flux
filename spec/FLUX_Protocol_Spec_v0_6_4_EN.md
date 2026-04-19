# FLUX Protocol Specification v0.6.4

## Fabric for Low-latency Unified eXchange

**Status:** Draft — LUCAB Internal
**Revision:** 2026-04-19
**Author:** LUCAB Media Technology

**Changelog v0.6.4:**

- **§1** — **Rewritten: Motivation and positioning**
  - “Seven pillars” framing replaced with mechanisms / conventions / profiles taxonomy
  - Comparison table corrected: CDBC column renamed to “Content-aware scheduling”; sub-ms claims removed
  - §1.1 — New: “Expected end-to-end latency ranges” (non-normative latency budget table)
  - §1.2 — New: positioning statement clarifying FLUX scope vs SMPTE 2110 / NDI / SRT / DASH
- **§4.1** — **TYPE field extended to 5 bits** (amendment)
  - Extended type mechanism: when `TYPE=0xF`, the `RSVD` byte (offset 7) carries the extended type code
  - FLUX/M frame types `0x10`–`0x12` now encodable on the wire
  - §4.4 frame type table updated with extended type column
- **§5** — **CDBC reframed as content-aware scheduler**
  - §5 preamble rewritten: CDBC complements transport CCA, does not replace it
  - §5.7 — New: “Interaction with transport-layer congestion control” (time-scale separation, hysteresis)
- **§7.4** — **New: NMOS IS-04 interoperability profile** — normative mapping table
- **§11.7** — Moved to **Annex D (informative)**: QUEEN-v1 codec profile
  - §10.9 codec registry updated: `queen-v1` marked as informative reference
- **§18.2** — Jumbo frames upgraded from RECOMMENDED to **REQUIRED** for streams ≥ 10 Mbit/s
- **§18.7.x** — New: “FEC block latency budget” — normative table of block duration vs bitrate and MTU
- **§20** — **Extracted to separate document** (FLUX Recording Profile v1.0); replaced with normative reference
- §19 — v0.6.4 backward-compatibility notes added

**Changelog v0.6.3:**

- **§10.10** — **New section: `flux://` URI scheme for live video textures in GLB**
  - glTF `image.uri` convention: `flux://channel/{channel_id}` binds a GLB texture to a live FLUX video channel
  - Fallback behaviour for non-FLUX renderers (static `bufferView` placeholder)
  - Precedence rules with `video_texture_bindings` (§10.8)
- §10 FLUX/M constraints table — row added for `flux://` image URIs
- §16 (amendment) — `fluxvideotex` URI resolution note added
- §19 — v0.6.3 backward-compatibility notes added

**Changelog v0.6.2:**

- **§20** — **New section: FLUX/R — Recording Profile**
  - §20.1 — Scope and design principles
  - §20.2 — Storage architecture: two-moment model
  - §20.3 — Container format: Fragmented MP4 (fMP4/CMAF)
  - §20.4 — Sidecar format: `.fluxmeta`
  - §20.5 — Asset recording: GLB keyframe + delta model
  - §20.6 — Production storage (clear)
  - §20.7 — Distribution storage (CENC/cbcs encrypted)
  - §20.8 — Transition: production → distribution
  - §20.9 — GStreamer pipeline examples
- §19 — v0.6.2 backward-compatibility notes added

**Changelog v0.6.1:**

- §3.2 — `gs_codecs` field added to `embed_support`; `application/vnd.flux.gs-residual` added to example `mime_types`
- §10.1 — `application/vnd.flux.gs-residual` MIME type added; `application/vnd.flux.gs-delta` annotated as `codec=raw-attr` legacy alias
- §10 FLUX/M constraints table — two new rows: GS residual sequences and GS codec negotiation
- §10.4 — `gs_codec`, `gs_codec_params`, `anchor_asset_id`, `anchor_sha256` fields added to `EMBED_MANIFEST`
- **§10.9** — **New section: GS Residual Codec Framework** (codec registry, anchor registration flow, anchor mismatch handling, FLUX/M anchor pre-fetch window)
- **§11.7** — **New section: QUEEN-v1 codec profile** (`gs_codec_params` schema, delivery parameters, anchor keyframe retransmission, receiver FSM, FLUX/M session descriptor declaration, CDBC interaction)
- §14 — `gs_residual_refs` added to per-frame metadata schema
- §16 — `fluxgsresidualdec` GStreamer element added; `fluxembeddec` codec dispatch table updated; QUEEN pipeline examples (FLUX/QUIC and FLUX/M) added; Rust crate additions (`candle-core`, `half`)
- §19 — v0.6.1 backward-compatibility notes added

**Changelog v0.6:**

- §1 — Comparison table updated: FLUX/M row added; FLUX/QUIC column differentiated
- §2 — Protocol stack diagram updated to show dual-mode architecture (FLUX/QUIC + FLUX/M)
- §2.3 — New: FLUX/M transport profile overview and mode identifier
- §16 — GStreamer element inventory updated: `fluxmcastsrc`, `fluxmcastsink`, `fluxmcastrelay` added
- §18 — **New section: FLUX/M — Multicast Group Distribution**
  - §18.1 — Scope and design constraints (no QUIC, no per-session handshake, no ARQ)
  - §18.2 — Network requirements (SSM/IGMPv3/MLDv2; unicast fallback via AMT RFC 7450)
  - §18.3 — Multicast group addressing (SSM channel model, address allocation)
  - §18.4 — Out-of-band session setup and FLUX/M Session Descriptor
  - §18.5 — Group key management: AES-256-GCM per-epoch, FLUX Key Server protocol
  - §18.6 — FLUX/M frame encapsulation (32-byte FLUX header over raw UDP; FRAG field usage)
  - §18.7 — Proactive FEC: RaptorQ (RFC 6330) repair symbol delivery model
  - §18.8 — Unicast feedback channel: NACK, receiver statistics, key refresh requests
  - §18.9 — MSS synchronization in FLUX/M (PTP anchor via multicast SYNC_ANCHOR)
  - §18.10 — AMT tunneling for receivers behind non-multicast infrastructure
  - §18.11 — FLUX/M ↔ FLUX/QUIC gateway (ingest relay, distribution bridge)
  - §18.12 — FLUX/M discovery and Registry extension
  - §18.13 — GStreamer pipeline examples for FLUX/M
- §19 — Version negotiation and backwards compatibility (renumbered from §17; FLUX/M notes added)

**Changelog v0.5:**

- §10.8 — `video_texture_bindings`: normative binding of live FLUX video channels to GLB PBR material texture slots, synchronized via `GROUP_TIMESTAMP_NS`
- §10.8.2 — `video_texture_bindings` array field added to `EMBED_MANIFEST`
- §6.2 (amendment) — normative receiver behaviour for texture upload synchronised to the sync barrier
- §6.2 (amendment) — `glb_texture_role` optional field added to `STREAM_ANNOUNCE`
- §10.8.5 — precedence rules: `texture_swap` delta overrides `video_texture_bindings` during declared `frame_assoc` range
- §10.8.6 — multi-channel compositing in ascending `channel_id` order
- §10.8.7 — `binding_control` GLB delta operation
- §16 (amendment) — `fluxvideotex` GStreamer element added
- §18 — backward-compatibility note for v0.4 receivers

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

-----

## 1. Motivation and positioning

FLUX is a media transport protocol for production and contribution environments in which in-band binary asset delivery (3D scenes, Gaussian Splats, tracking data, per-frame metadata) must be synchronized to video frames with sub-frame precision. FLUX is designed to complement existing broadcast infrastructure (NMOS, SMPTE ST 2110) where asset synchronization is the primary requirement. FLUX is not a replacement for SMPTE ST 2110 in baseband studio networks, nor for DASH/HLS in OTT distribution, nor for SRT in conventional video contribution over WAN.

### 1.0.1 Comparison table

|Protocol     |Transport                        |Media delivery                        |Content-aware scheduling  |Multi-stream sync      |Discovery                         |Tally   |Binary embed                                             |Encryption                  |WAN           |Multicast       |
|-------------|---------------------------------|--------------------------------------|--------------------------|-----------------------|----------------------------------|--------|---------------------------------------------------------|----------------------------|--------------|----------------|
|SRT          |UDT/UDP                          |Reliable                              |No                        |No                     |No                                |No      |No                                                       |AES-128/256                 |Yes           |No              |
|RIST         |RTP/UDP                          |Unreliable+ARQ                        |No                        |No                     |No                                |No      |No                                                       |Profile-dependent           |Yes           |Experimental    |
|NDI          |TCP+UDP                          |Mixed                                 |No                        |No                     |DNS-SD                            |XML     |No                                                       |Partial                     |Bridge        |No              |
|SMPTE 2110   |RTP/UDP                          |Unreliable                            |No                        |PTP (hardware required)|SDP/NMOS IS-04                    |No      |No                                                       |None (IPMX advancing)       |No            |Yes (native)    |
|**FLUX/QUIC**|**QUIC Datagram+Stream**         |**Unreliable media, reliable control**|**CDBC**                  |**PTP (sw or hw)**     |**DNS-SD + NMOS-aligned Registry**|**JSON**|**FLUX-E (any MIME, delta, live video tex, flux:// URI)**|**None / TLS 1.3 / TLS+AES**|**Yes (QUIC)**|**No (unicast)**|
|**FLUX/M**   |**UDP Multicast (SSM) + RaptorQ**|**Unreliable + proactive FEC**        |**No (sender-side fixed)**|**PTP (sw or hw)**     |**DNS-SD + NMOS-aligned Registry**|**JSON**|**FLUX-E (session-scoped assets only)**                  |**AES-256-GCM (group key)** |**LAN/MPLS**  |**Yes (native)**|

**Notes on the comparison table:**

- **Content-aware scheduling** is not congestion control. SRT, RIST, and QUIC each have their own transport-layer CCAs. CDBC operates at the application layer to decide *which content* (base layer, enhancement, embed, delta) is scheduled when bandwidth is constrained. See §5 for the full distinction.
- **Multi-stream sync** precision refers to clock alignment, not end-to-end media latency. See §1.1 for realistic latency budgets.

### 1.0.2 Mechanisms, conventions, and profiles

FLUX defines the following **core mechanisms:**

- **Transport framing** — a 32-byte header applied to media carried over QUIC Datagrams (FLUX/QUIC) or encrypted UDP multicast (FLUX/M).
- **Multi-stream synchronization (MSS)** — a receiver-side barrier anchored on PTP (software or hardware) and `GROUP_TIMESTAMP_NS`.
- **In-band asset embedding (FLUX-E)** — manifest + chunk delivery of arbitrary MIME-typed binary payloads with temporal association to media frames. Includes delta operations for incremental GLB and codec-agnostic GS residuals, and the `flux://` URI scheme for live video texture binding inside GLB assets.
- **Content-aware scheduling (CDBC)** — an application-layer scheduler that prioritises content under bandwidth constraint. Complements the underlying transport congestion controller; does not replace it. FLUX/QUIC only.

FLUX also defines the following **conventions:**

- **Discovery (FLUX-D)** — DNS-SD and an NMOS IS-04-aligned registry.
- **Bidirectional tally (FLUX-T)** — JSON and compact binary (3-bit) encodings.
- **Automatic monitor sub-stream (FLUX-Mon)** — confidence monitoring via server-side transcoded sub-stream.
- **Upstream control (FLUX-C)** — PTZ, audio mix, routing with rate limiting.

FLUX defines two **transport profiles:**

- **FLUX/QUIC** — unicast over QUIC (RFC 9000 / RFC 9221). Core profile.
- **FLUX/M** — unidirectional multicast over UDP SSM with proactive RaptorQ FEC and mandatory AES-256-GCM encryption.

FLUX defines one **offline profile** (published as a separate specification document):

- **FLUX/R** — Recording profile. See *FLUX Recording Profile v1.0* [FLUX-R].

### 1.1 Expected end-to-end latency ranges (non-normative)

PTP synchronization precision (±50–500 µs in software mode, ±10–100 ns in hardware mode) measures **clock alignment between streams**, not end-to-end media latency. The glass-to-glass latency is dominated by encode, jitter buffer, decode, and display vsync.

|Stage                                 |Typical contribution (60 fps, H.265)|
|--------------------------------------|------------------------------------|
|Sensor → framebuffer                  |~16 ms                              |
|H.265 encode (`zerolatency` preset)   |16–33 ms                            |
|Framing + QUIC TLS encrypt + kernel TX|0.3–0.8 ms                          |
|Network propagation (LAN)             |< 1 ms                              |
|Kernel RX + QUIC decrypt + demux      |0.5–1.2 ms                          |
|Jitter buffer (≥ 1 frame)             |16–33 ms                            |
|MSS sync barrier (FRAME_SYNC)         |0–16 ms                             |
|H.265 decode                          |16–33 ms                            |
|GPU upload + composite + vsync        |16–33 ms                            |
|**Total (FLUX/QUIC, LAN, H.265)**     |**~80–165 ms**                      |

|Scenario                         |Expected glass-to-glass latency|Notes                                           |
|---------------------------------|-------------------------------|------------------------------------------------|
|FLUX/QUIC, LAN, H.265 @ 60 fps   |80–165 ms                      |Comparable to NDI HX                            |
|FLUX/QUIC, LAN, JPEG-XS @ 60 fps |5–20 ms                        |Requires hardware JPEG-XS; comparable to 2110-22|
|FLUX/QUIC, WAN (30 ms RTT), H.265|120–220 ms                     |Jitter buffer increases with path variance      |
|FLUX/M, LAN, MTU 1500, H.265     |130–215 ms                     |FEC block latency adds ~50 ms (see §18.7.x)     |
|FLUX/M, LAN, MTU 9000, H.265     |95–180 ms                      |Jumbo frames reduce FEC block latency to ~14 ms |

These figures are representative. Actual latency depends on codec, encoder tuning, network conditions, and receiver implementation.

### 1.2 Scope boundaries

FLUX occupies a specific gap that no existing protocol covers: **frame-synchronized binary asset delta delivery with live video texture binding**, suitable for virtual production pipelines and dynamic 3D scene distribution.

- **vs NDI (LAN production):** FLUX does not beat NDI on ecosystem maturity or implementation simplicity for 2D-only workflows. FLUX is preferred when the workflow requires in-band 3D asset delta delivery or GS residual codec transport.
- **vs SRT (WAN contribution):** FLUX does not beat SRT on hardware encoder/decoder availability. FLUX is preferred when metadata embedding, multi-stream sync, or 3D asset delivery is required in-band.
- **vs SMPTE ST 2110 (facility baseband replacement):** FLUX does not compete in the 2110 latency class and is not a replacement for 2110 in dedicated studio networks.
- **vs DASH / HLS (OTT distribution):** FLUX is not an OTT distribution protocol. FLUX/R produces files that downstream OTT systems can repackage.

-----

## 2. Protocol stack

### 2.1 FLUX/QUIC (unicast)

```
┌───────────────────────────────────────────────────────────────────────────┐
│                         FLUX Application Layer                            │
│                                                                           │
│  ┌─────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌──────────┐ ┌─────────┐  │
│  │  Media  │ │  CDBC  │ │  MSS   │ │FLUX-T  │ │ FLUX-E   │ │ FLUX-C  │  │
│  │  Video  │ │Feed-   │ │ Sync   │ │ Tally  │ │ Embed    │ │ Control │  │
│  │  Audio  │ │ back   │ │Barrier │ │ JSON   │ │ + Delta  │ │ Upstrm  │  │
│  │  ≤240fps│ │Adaptive│ │ swPTP  │ │ 3-bit  │ │ + Seq    │ │ RateLim │  │
│  └─────────┘ └────────┘ └────────┘ └────────┘ │ + VidTex │ └─────────┘  │
│                                                └──────────┘               │
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
```

### 2.2 FLUX/M (multicast)

```
┌───────────────────────────────────────────────────────────────────────────┐
│                         FLUX Application Layer                            │
│                                                                           │
│  ┌─────────┐ ┌────────────┐ ┌────────┐ ┌────────┐ ┌────────────────────┐ │
│  │  Media  │ │  Sender BW │ │  MSS   │ │FLUX-T  │ │ FLUX-E             │ │
│  │  Video  │ │  Governor  │ │ Sync   │ │ Tally  │ │ (session assets,   │ │
│  │  Audio  │ │(sender-side│ │Barrier │ │ JSON   │ │  no live delta)    │ │
│  │  ≤240fps│ │  only)     │ │ swPTP  │ │ 3-bit  │ └────────────────────┘ │
│  └─────────┘ └────────────┘ └────────┘ └────────┘                        │
├───────────────────────────────────────────────────────────────────────────┤
│               FLUX Framing Layer — fixed 32-byte header                   │
│        Channel_ID | Layer_ID | Group_TS(PTP) | FEC_Group | Type           │
├───────────────────────────────────────────────────────────────────────────┤
│  RaptorQ Proactive FEC (RFC 6330) — repair symbols interleaved in stream  │
│  Source blocks: configurable (default 64 source + 16 repair symbols)      │
├───────────────────────────────────────────────────────────────────────────┤
│               AES-256-GCM (group key, per-epoch)                          │
│          Nonce: epoch_id (32 bits) ‖ packet_number (64 bits)              │
├───────────────────────────────────────────────────────────────────────────┤
│                    UDP Multicast — IPv4 SSM / IPv6 SSM                    │
│         Source: sender unicast IP  │  Group: 239.x.x.x / ff3x::/32       │
├───────────────────────────────────────────────────────────────────────────┤
│  IGMPv3 (IPv4) / MLDv2 (IPv6) — PIM-SSM routing                          │
│  Optional AMT tunnel (RFC 7450) for non-multicast networks                │
└───────────────────────────────────────────────────────────────────────────┘

Parallel unicast control plane (TCP/HTTPS):
┌───────────────────────────────────────────────────────────┐
│  FLUX Key Server — group key distribution (TLS 1.3)       │
│  FLUX Registry API — session descriptor, routing, tally   │
│  Unicast feedback channel (UDP) — NACK, stats (optional)  │
└───────────────────────────────────────────────────────────┘
```

### 2.3 FLUX/M profile identification

The FLUX profile is declared in the session descriptor and discovery records:

|Profile identifier|Transport          |Handshake       |ARQ            |FEC                  |Crypto                  |
|------------------|-------------------|----------------|---------------|---------------------|------------------------|
|`flux_quic`       |QUIC over UDP      |Per-session QUIC|Yes (selective)|Dynamic (BW Governor)|None / TLS 1.3 / TLS+AES|
|`flux_m`          |UDP multicast (SSM)|Out-of-band     |**No**         |**Proactive RaptorQ**|AES-256-GCM (group key) |

An implementation MUST declare its supported profiles in DNS-SD TXT records and FLUX Registry entries (§18.12). A FLUX/M stream and a FLUX/QUIC stream MAY share the same `GROUP_ID` and `GROUP_TIMESTAMP_NS` clock domain, enabling mixed-profile deployments where some receivers access via unicast QUIC and others via multicast.

### 2.4 QUIC transport modes (FLUX/QUIC only)

FLUX/QUIC uses two distinct QUIC delivery mechanisms within the same connection:

|Data class                            |QUIC mechanism              |Retransmit|Rationale                                                  |
|--------------------------------------|----------------------------|----------|-----------------------------------------------------------|
|Media frames (video, audio)           |**QUIC Datagram** (RFC 9221)|**No**    |A late frame is worse than a lost frame. Zero HoL blocking.|
|FEC repair packets                    |QUIC Datagram               |No        |FEC is inherently loss-tolerant.                           |
|CDBC feedback, tally, probe           |QUIC Datagram               |No        |Stale feedback is harmful; the next report supersedes.     |
|Control (SESSION, ANNOUNCE, KEEPALIVE)|QUIC Stream 0               |Yes       |Handshake must be reliable.                                |
|EMBED_MANIFEST                        |QUIC Stream                 |Yes       |Manifest integrity is required before chunk reassembly.    |
|EMBED_CHUNK (full assets)             |QUIC Stream (low priority)  |Yes       |Binary assets must arrive complete.                        |
|EMBED_CHUNK (delta/realtime)          |QUIC Datagram               |No        |Delta frames are time-critical; stale deltas are skipped.  |
|Selective ARQ retransmit              |QUIC Stream (dedicated)     |Yes       |Only for base-layer keyframes on explicit request.         |

**Implementation note:** The QUIC connection MUST advertise `max_datagram_frame_size` ≥ 1350 bytes in transport parameters.

### 2.5 Crypto modes (FLUX/QUIC)

|Mode              |Identifier       |Transport        |Payload              |Use case                                        |
|------------------|-----------------|-----------------|---------------------|------------------------------------------------|
|**None**          |`crypto_none`    |Raw UDP, no QUIC |None                 |Trusted LAN, maximum performance, lowest latency|
|**QUIC TLS**      |`crypto_quic`    |QUIC with TLS 1.3|QUIC-encrypted       |Default — WAN and general use                   |
|**QUIC TLS + AES**|`crypto_quic_aes`|QUIC with TLS 1.3|AES-256-GCM over QUIC|DRM, classified content                         |

-----

## 3. Session model (FLUX/QUIC)

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
  "flux_version": "0.6.4",
  "flux_profile": "flux_quic",
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
    "video_texture_binding_support": true,
    "gs_codecs": ["raw-attr", "queen-v1"],
    "mime_types": [
      "model/gltf-binary",
      "model/vnd.usd",
      "model/vnd.gaussian-splat",
      "application/vnd.flux.gs-residual",
      "application/vnd.flux.gs-delta",
      "application/vnd.flux.gs-sequence",
      "application/json",
      "text/csv",
      "image/x-exr",
      "application/vnd.flux.tracking",
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

**v0.6 note:** A new top-level `flux_profile` field is added (`"flux_quic"` | `"flux_m"`). For FLUX/QUIC sessions this field is informative. For FLUX/M the session is set up out-of-band (§18.4) and this field appears only in the FLUX/M Session Descriptor, not in a per-session handshake JSON.

**v0.6.1 note — `gs_codecs`:** The `gs_codecs` field in `embed_support` declares the set of GS residual codec identifiers the receiver can decode (see §10.9 and Annex D). If `gs_codecs` is absent in `FLUX_SESSION_REQUEST`, the server MUST assume `["raw-attr"]` for backward compatibility. If a receiver declares `gs_codecs` but does not include `"raw-attr"`, the server MUST NOT send `application/vnd.flux.gs-delta` frames with `gs_codec: "raw-attr"` to that receiver. The server MAY still send full `model/vnd.gaussian-splat` keyframes.

|`gs_codecs` value|Meaning                                                                                                                                   |
|-----------------|------------------------------------------------------------------------------------------------------------------------------------------|
|`raw-attr`       |Per-splat position / color / SH / opacity delta in raw attribute space (legacy — equivalent to `application/vnd.flux.gs-delta` pre-v0.6.1)|
|`queen-v1`       |NVIDIA QUEEN quantized residual frame (Annex D)                                                                                           |

### 3.3 KEEPALIVE specification

|Parameter                |Value                               |Negotiation                                                         |
|-------------------------|------------------------------------|--------------------------------------------------------------------|
|`keepalive_interval_ms`  |Default: 1000 ms                    |Server sets in SESSION_ACCEPT; client may request in SESSION_REQUEST|
|`keepalive_timeout_count`|Default: 3                          |Number of missed keepalives before declaring session dead           |
|Effective timeout        |`interval × count` = 3000 ms default|—                                                                   |

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
1. If reconnect fails within 2 × timeout, declare session dead and emit `SESSION_LOST` event.

**FLUX/M:** KEEPALIVE frames are sent on the multicast channel as `MEDIA_DATA`-class datagrams (TYPE=`0x9`). No unicast response is expected. FLUX/M receivers detect source loss when no KEEPALIVE is received for `keepalive_timeout_count × keepalive_interval_ms` milliseconds, and emit `FLUXM_SOURCE_LOST`.

-----

## 4. FLUX frame format (wire format)

### 4.1 Header — fixed 32 bytes

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|VER(4)|TYPE(4)|    FLAGS(8)   |        CHANNEL_ID (16)        |  [0–3]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|LAYER(4)|FRAG(4)|             GROUP_ID (16)    |EXT_TYPE/RSVD |  [4–7]
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

The wire format is **identical** in FLUX/QUIC and FLUX/M. In FLUX/M, the 32-byte header is placed directly inside a UDP datagram (after the AES-256-GCM authentication tag — see §18.6).

#### Extended TYPE mechanism (v0.6.4)

The `TYPE` field is 4 bits (values `0x0`–`0xF`). When `TYPE = 0xF`, the byte at offset 7 (`EXT_TYPE`) carries the extended frame type code instead of being reserved. The effective frame type is then the `EXT_TYPE` value.

When `TYPE ≠ 0xF`, byte 7 MUST be set to `0x00` (reserved) by senders and MUST be ignored by receivers.

Implementations MUST resolve the effective type as:

```
if TYPE == 0xF:
    effective_type = EXT_TYPE   # byte at offset 7
else:
    effective_type = TYPE       # 4-bit field at offset 0, low nibble
```

This mechanism allows the FLUX header to remain exactly 32 bytes while supporting frame types beyond `0xE`. The FLUX/M-specific frame types `0x10`–`0x12` (§4.4) MUST use this extended encoding.

### 4.2 CAPTURE_TS_NS_LO wraparound reconstruction

`CAPTURE_TS_NS_LO` stores only the low 32 bits of the capture timestamp in nanoseconds. Receivers MUST reconstruct the full 64-bit capture timestamp using:

```
Let G = GROUP_TIMESTAMP_NS (full 64-bit)
Let C = CAPTURE_TS_NS_LO (low 32 bits)
Let G_lo = G & 0xFFFFFFFF

candidate_hi = G & 0xFFFFFFFF00000000

if C > G_lo + 2^31:
    full_capture_ts = (candidate_hi - 2^32) | C
elif G_lo > C + 2^31:
    full_capture_ts = (candidate_hi + 2^32) | C
else:
    full_capture_ts = candidate_hi | C
```

### 4.3 FLAGS (8 bits)

```
Bit 7: KEYFRAME       — independent frame (IDR/keyframe)
Bit 6: ENCRYPTED      — payload has additional AES-256-GCM (crypto_quic_aes / FLUX/M)
Bit 5: DROP_ELIGIBLE  — may be dropped under congestion (B-frames, enhancement layers)
Bit 4: EMBED_ASSOC    — this frame has an associated embedded asset (see ASSET_ID in metadata)
Bit 3: MONITOR_COPY   — this frame also feeds the monitor stream
Bit 2: SYNC_MASTER    — this stream is master of the sync group
Bit 1: LAST_IN_GOP    — last frame of the group of pictures
Bit 0: HAS_METADATA   — payload begins with a metadata length + JSON block
```

In FLUX/M, the `ENCRYPTED` flag (bit 6) is always set. It indicates the FLUX/M group-key AES-256-GCM layer, not the QUIC TLS layer.

### 4.4 Frame types

|Effective code|Wire encoding       |Name             |Dir |Delivery (QUIC)   |Delivery (FLUX/M)   |Description                                     |
|--------------|--------------------|-----------------|----|------------------|--------------------|------------------------------------------------|
|`0x0`         |`TYPE=0x0`          |`MEDIA_DATA`     |S→C |Datagram          |UDP multicast       |Media payload (video, audio, compressed ANC)    |
|`0x1`         |`TYPE=0x1`          |`CDBC_FEEDBACK`  |C→S |Datagram          |**Unicast UDP only**|Receiver bandwidth report (FLUX/QUIC only)      |
|`0x2`         |`TYPE=0x2`          |`SYNC_ANCHOR`    |S→C |Datagram          |UDP multicast       |PTP anchor for the sync group                   |
|`0x3`         |`TYPE=0x3`          |`LAYER_STATUS`   |S→C |Stream            |UDP multicast       |Available layer change notification             |
|`0x4`         |`TYPE=0x4`          |`QUALITY_REQUEST`|C→S |Stream            |**Unicast TCP only**|Receiver requests quality/layer change          |
|`0x5`         |`TYPE=0x5`          |`STREAM_ANNOUNCE`|S→C |Stream            |Out-of-band (SD)    |New stream declaration                          |
|`0x6`         |`TYPE=0x6`          |`STREAM_END`     |Both|Stream            |UDP multicast       |Stream termination                              |
|`0x7`         |`TYPE=0x7`          |`FEC_REPAIR`     |S→C |Datagram          |UDP multicast       |FEC repair symbol (RaptorQ in FLUX/M)           |
|`0x8`         |`TYPE=0x8`          |`SESSION_INFO`   |Both|Stream            |Out-of-band (SD)    |Handshake and capabilities (JSON)               |
|`0x9`         |`TYPE=0x9`          |`KEEPALIVE`      |Both|Datagram          |UDP multicast       |Heartbeat with timestamp                        |
|`0xA`         |`TYPE=0xA`          |`TALLY_UPDATE`   |C→S |Datagram          |**Unicast UDP only**|Per-channel tally state                         |
|`0xB`         |`TYPE=0xB`          |`ANC_DATA`       |S→C |Datagram          |UDP multicast       |Broadcast ancillary data (VANC/HANC)            |
|`0xC`         |`TYPE=0xC`          |`METADATA_FRAME` |Both|Datagram          |UDP multicast       |Per-frame or out-of-band JSON metadata          |
|`0xD`         |`TYPE=0xD`          |`BANDWIDTH_PROBE`|S→C |Datagram          |**Not used**        |Probe packet for BW measurement (FLUX/QUIC only)|
|`0xE`         |`TYPE=0xE`          |`EMBED_MANIFEST` |S→C |Stream            |UDP multicast       |Declares an asset embedded in the stream        |
|`0x0F`        |`TYPE=0xF, EXT=0x0F`|`EMBED_CHUNK`    |S→C |Stream or Datagram|UDP multicast       |Fragment of an embedded asset                   |
|`0x10`        |`TYPE=0xF, EXT=0x10`|`FLUXM_KEY_EPOCH`|S→C |N/A               |UDP multicast       |Group key epoch change notice (v0.6)            |
|`0x11`        |`TYPE=0xF, EXT=0x11`|`FLUXM_NACK`     |C→S |N/A               |**Unicast UDP only**|Receiver NACK report (v0.6)                     |
|`0x12`        |`TYPE=0xF, EXT=0x12`|`FLUXM_STAT`     |C→S |N/A               |**Unicast UDP only**|Receiver statistics report (v0.6)               |

**Extended type encoding (v0.6.4):** Frame types with effective code ≥ `0x0F` MUST be encoded with `TYPE=0xF` in the 4-bit header field and the effective code in the `EXT_TYPE` byte (offset 7). `EMBED_CHUNK` (previously `TYPE=0xF`) is migrated to extended encoding `EXT=0x0F` for consistency. Receivers MUST support both `TYPE=0xF` with `EXT_TYPE=0x0F` (v0.6.4+) and `TYPE=0xF` with `EXT_TYPE=0x00` (v0.6.3 and earlier, where byte 7 was reserved/zero) as `EMBED_CHUNK` — see §19 for backward-compatibility rules.

Frame types `0x10`–`0x12` are defined exclusively for FLUX/M. FLUX/QUIC receivers MUST silently ignore these frame types if encountered. Receivers MUST silently ignore any `EXT_TYPE` value they do not recognise.

### 4.5 Metadata payload layout

When `HAS_METADATA=1` in FLAGS:

```
[ meta_length: uint16 ]                              — FIRST: metadata length
[ meta_json: meta_length bytes, UTF-8 JSON ]         — metadata block
[ media_bytes: PAYLOAD_LENGTH - meta_length - 2 ]    — media data
```

-----

## 5. CDBC — Content-aware scheduling (FLUX/QUIC only)

CDBC (Client-Driven Bandwidth Control) is a **content-aware application-layer scheduler**. It operates in addition to the transport-layer congestion controller of the underlying QUIC implementation (typically BBRv3 or CUBIC) and does not replace it. Where the transport CCA determines how many bytes may be sent per interval, CDBC determines which FLUX content is scheduled into that interval.

|Dimension                    |Transport CCA (e.g. BBRv3)      |CDBC (application layer)                                                 |
|-----------------------------|--------------------------------|-------------------------------------------------------------------------|
|Layer                        |L4 (inside QUIC)                |L7                                                                       |
|Reaction granularity         |Per-ACK (sub-ms to ms)          |50 ms nominal, 10 ms in EMERGENCY                                        |
|Input signals                |Delivery rate, RTprop, loss, ECN|`rx_bps`, `avail_bps`, `rtt`, `loss_pct`, `jitter`, `preferred_max_layer`|
|Output                       |Pacing rate + cwnd (bytes/s)    |Which layer to send, which FEC to enable, which embed priority           |
|Semantic awareness           |None — packets are opaque       |Full — knows base/enhancement/monitor/embed/delta                        |
|Requires receiver cooperation|No                              |Yes (explicit `CDBC_FEEDBACK` datagram)                                  |


> **FLUX/M note:** CDBC is not applicable to FLUX/M. The sender governs bitrate based on static configuration or operator input. Receivers cannot send bandwidth feedback over the multicast channel (no ACK implosion). See §18 for FLUX/M bandwidth governance.

### 5.1 Adaptive CDBC interval

|BW Governor state|CDBC interval                         |Rationale                                 |
|-----------------|--------------------------------------|------------------------------------------|
|PROBE            |`cdbc_interval_ms` (default 50 ms)    |Normal probing rate                       |
|STABLE           |`cdbc_interval_ms` (default 50 ms)    |Low overhead when link is healthy         |
|RAMP_UP          |`cdbc_interval_ms` (default 50 ms)    |Conservative ramp requires stable readings|
|RAMP_DOWN        |`cdbc_interval_min_ms` (default 10 ms)|Fast convergence under degradation        |
|EMERGENCY        |`cdbc_interval_min_ms` (default 10 ms)|Maximum responsiveness during crisis      |

At high frame rates (>120 fps), the minimum practical CDBC interval is 1 frame period:

- 120 fps → min 8.33 ms
- 200 fps → min 5 ms
- 240 fps → min 4.17 ms

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

```
States: PROBE → STABLE → RAMP_UP / RAMP_DOWN → EMERGENCY
```

### 5.4 EMERGENCY state — priority order

```
STEP 1 — Shed load:
  a) Drop all enhancement layers (layer > 0)
  b) Disable monitor streams
  c) Pause EMBED_CHUNK transfers (non-delta)
  d) Reduce frame rate if negotiated

STEP 2 — Evaluate remaining loss on base layer only:
  Wait 2 × cdbc_interval_min_ms

STEP 3 — Protect base layer (only if loss persists):
  if loss_pct > 5%  → enable XOR Row FEC on base layer only
  if loss_pct > 15% → switch to Reed-Solomon 2D + force IDR-only mode

STEP 4 — Recovery:
  if loss_pct < 1% for 5 consecutive reports → transition to RAMP_UP
```

### 5.5 Per-layer QUIC priority

```
Layer 0 (base):         urgency = 0 (highest), incremental = true
Layer 1 (enhancement):  urgency = 2, incremental = true
Layer 2 (enhancement):  urgency = 3, incremental = true
Layer 3 (enhancement):  urgency = 4, incremental = true
Monitor stream:         urgency = 5, incremental = true
Embed chunks (bulk):    urgency = 6, incremental = true
Embed delta (realtime): urgency = 1, incremental = true
```

### 5.6 High frame-rate considerations (120–240 fps)

|FPS|Frame period|Notes                              |
|---|------------|-----------------------------------|
|120|8.33 ms     |Comfortable with standard CDBC     |
|144|6.94 ms     |Common gaming/LED wall refresh     |
|200|5.00 ms     |High-speed capture, LED sync       |
|240|4.17 ms     |Maximum specified; sport/scientific|

### 5.7 Interaction with transport-layer congestion control

CDBC and the QUIC transport CCA (typically BBRv3 or CUBIC) coexist as two independent control loops. Without explicit coordination, they can oscillate: BBRv3 reduces pacing under congestion; CDBC detects reduced `rx_bps` and drops a layer; the dropped layer reduces demand; BBRv3 observes headroom and increases pacing; CDBC re-enables the layer; congestion recurs.

To prevent oscillation, the following normative requirements apply:

**Time-scale separation:** CDBC MUST NOT react to a sustained-loss threshold faster than `3 × observed_rtt`. Specifically, the BW Governor MUST NOT transition from `STABLE` to `RAMP_DOWN` or `EMERGENCY` unless the triggering condition (e.g. `loss_pct > threshold`) persists for at least 3 consecutive CDBC reports spanning ≥ `3 × observed_rtt` wall-clock time.

**Hysteresis on layer re-enable:** After the BW Governor drops an enhancement layer (EMERGENCY step 1), re-enabling that layer requires `N` consecutive stable CDBC reports with `loss_pct < 1%` and `avail_bps` exceeding the layer’s bitrate by ≥ 20%. Default `N = 5`. This prevents rapid on/off cycling.

**CDBC interval floor relative to RTT:** When the QUIC transport CCA is BBRv3, the CDBC feedback interval SHOULD be at least `5 × observed_rtt`. If the negotiated `cdbc_interval_ms` is less than this floor, the server SHOULD defer BW Governor state transitions until sufficient RTT-spaced samples accumulate.

**Non-normative note:** These constraints ensure that CDBC operates on a slower timescale than the transport CCA, allowing BBRv3 to converge before CDBC reacts. Implementations that use CUBIC (which converges more slowly than BBRv3) MAY increase the time-scale separation factor from 3 to 5.

-----

## 6. MSS — Multi-Stream Synchronization

### 6.1 PTP modes

|Mode            |Identifier    |Precision |Requirements                                                                 |Use case                                           |
|----------------|--------------|----------|-----------------------------------------------------------------------------|---------------------------------------------------|
|**Software PTP**|`ptp_software`|±50–500 µs|NTP/PTP daemon, `clock_gettime(CLOCK_REALTIME)`                              |Default. Sufficient for FRAME_SYNC and SAMPLE_SYNC.|
|**Hardware PTP**|`ptp_hardware`|±10–100 ns|IEEE 1588 PTP grandmaster, NIC with hardware timestamping (`SO_TIMESTAMPING`)|Required for LINE_SYNC.                            |

All FLUX implementations (both FLUX/QUIC and FLUX/M) MUST support software PTP. FLUX/M receivers derive sync from the `SYNC_ANCHOR` frames emitted on the multicast channel (§18.9).

### 6.2 Sync groups

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

**`glb_texture_role` optional field in STREAM_ANNOUNCE (v0.5+):**

```json
{
  "channel_id": 2,
  "glb_texture_role": {
    "asset_id": "scene-glb-take-003",
    "material_path": "/materials/screen_mat",
    "slot": "baseColorTexture",
    "hint_resolution": "1920x1080",
    "hint_format": "rgba8_unorm"
  }
}
```

**Video texture binding synchronisation:** When a receiver has an active GLB with `video_texture_bindings`, the receiver MUST for every `GROUP_TIMESTAMP_NS`:

1. Decode the video frame in the normal pipeline.
1. Apply the declared `color_transform`.
1. Upload the result as a GPU texture to the declared `slot`.
1. If `opacity_node` is declared, read `extras.flux_opacity`.
1. Apply the declared `blend_mode`.
1. Commit the scene before issuing the draw call for that `GROUP_TIMESTAMP_NS`.

### 6.3 Sync Barrier on the receiver

```
FRAME_SYNC:   tolerance = ±1 frame period (auto-scales with fps)
SAMPLE_SYNC:  tolerance = ±1 audio sample (±20.8 µs at 48 kHz)
LINE_SYNC:    tolerance = ±1 scan line (≈ ±18 µs at 1080p50) — requires ptp_hardware
```

### 6.4 SYNC_ANCHOR frame

|PTP mode      |Default interval — FLUX/QUIC|Default interval — FLUX/M                                   |
|--------------|----------------------------|------------------------------------------------------------|
|`ptp_software`|500 ms                      |**250 ms** (higher drift risk without per-receiver feedback)|
|`ptp_hardware`|1000 ms                     |500 ms                                                      |

FLUX/M senders SHOULD increase the `SYNC_ANCHOR` rate to 250 ms in software PTP mode to compensate for the absence of per-receiver RTT measurement.

-----

## 7. FLUX-D — Discovery

### 7.1 Layer 1: DNS-SD / mDNS

Senders announce themselves. v0.6 adds `flux_profile` and `mcast_group` TXT fields:

```
Name: "CAM_A (LUCAB Studio)"
TXT records:
  flux_version=0.6
  flux_profile=flux_quic,flux_m        (comma-separated if both supported)
  channels=2
  groups=1
  port=7400
  crypto=crypto_quic
  max_fps=240
  registry=http://192.168.1.100:7500
  mcast_group=239.100.1.1              (SSM group address, if flux_m supported)
  mcast_src=192.168.1.50               (SSM source address)
  mcast_port=7500
```

### 7.2 Layer 2: FLUX Registry Server

```json
{
  "sources": [
    {
      "id": "cam-a-lucab-studio",
      "name": "CAM_A (LUCAB Studio)",
      "host": "192.168.1.50",
      "port": 7400,
      "flux_profiles": ["flux_quic", "flux_m"],
      "multicast": {
        "group": "239.100.1.1",
        "source": "192.168.1.50",
        "port": 7500,
        "fec_overhead_pct": 25,
        "session_descriptor_url": "https://192.168.1.100:7500/api/sd/cam-a-lucab-studio"
      },
      "channels": 2,
      "groups": [1],
      "codecs": ["h265", "jpegxs"],
      "max_fps": 240,
      "hdr": true,
      "crypto_modes": ["crypto_none", "crypto_quic"],
      "embed_catalog": ["scene_glb", "tracking_freed"],
      "delta_support": true,
      "video_texture_binding_support": true,
      "tally": { "program": false, "preview": true },
      "uptime_s": 3600,
      "monitor_url": "flux://192.168.1.50:7401/monitor"
    }
  ]
}
```

### 7.3 Dynamic routing

```json
// POST /api/sources/virtual-main/route
{ "target_id": "cam-b-lucab-studio" }
```

### 7.4 NMOS IS-04 / IS-05 interoperability profile (v0.6.4)

The FLUX Registry is designed as an **extension of the NMOS IS-04 registration and discovery model**, not a replacement for it. Facilities operating NMOS registries SHOULD expose FLUX sources, flows, and senders as standard NMOS IS-04 v1.3 resources with FLUX-specific fields carried in `tags` and `manifest_href` payloads.

**Normative mapping — FLUX → NMOS IS-04 v1.3:**

|FLUX Registry concept                  |NMOS IS-04 resource                          |Mapping notes                                                                                              |
|---------------------------------------|---------------------------------------------|-----------------------------------------------------------------------------------------------------------|
|Source entry (`sources[]`)             |**Node** + **Source** + **Sender**           |One NMOS Node per FLUX sender host; one Source per `group_id`; one Sender per `(channel_id, layer_id)` pair|
|`flux_profiles`                        |Sender `tags.flux_profiles`                  |Array of `"flux_quic"`, `"flux_m"`                                                                         |
|`session_descriptor_url`               |Sender `manifest_href`                       |URL of the FLUX/M Session Descriptor                                                                       |
|`multicast.group` / `.source` / `.port`|Sender `interface_bindings` + `manifest_href`|SSM `(S,G)` pair in `manifest_href` payload                                                                |
|`video_texture_binding_support`        |Sender `tags.flux_video_texture`             |Boolean                                                                                                    |
|`embed_catalog`                        |Source `tags.flux_embed_catalog`             |JSON array of asset IDs                                                                                    |
|`codec_support`                        |Flow `media_type` + `tags.flux_codecs`       |Standard NMOS `media_type` for video/audio; FLUX codecs in tags                                            |
|`tally`                                |Sender `tags.flux_tally`                     |Tally state object                                                                                         |

**IS-05 connection management:** FLUX/QUIC senders SHOULD implement IS-05 v1.1 `staged` / `active` connection management for the QUIC transport parameters (host, port, crypto mode). FLUX/M senders expose the SSM `(S,G,port)` tuple as the IS-05 transport file.

**Dual registration:** A FLUX sender operating in a facility with an NMOS registry SHOULD register with both the NMOS IS-04 registry and the FLUX Registry (if deployed). The FLUX Registry provides FLUX-specific APIs (session descriptor hosting, dynamic routing, embed catalog) that are not covered by NMOS IS-04.

**Non-normative note:** Full IS-12 (NMOS Control) and IS-14 (NMOS Configuration) mapping for FLUX-C upstream control and BW Governor parameters is deferred to a future FLUX-NMOS interoperability specification.

-----

## 8. FLUX-T — Bidirectional tally

### 8.1 Upstream from receiver (C→S) — JSON mode

```json
{
  "session_id": "sess-001",
  "ts_ns": 1743580812345678901,
  "channels": {
    "0": { "program": true,  "preview": false, "standby": false, "iso_rec": true,  "streaming": false },
    "1": { "program": false, "preview": true,  "standby": false, "iso_rec": false, "streaming": true  }
  },
  "mixer_id": "LUCAB-MIXER-01",
  "transition": "cut"
}
```

**FLUX/M:** `TALLY_UPDATE` frames MUST be sent as unicast UDP to the feedback address declared in the FLUX/M Session Descriptor (§18.4), not on the multicast channel.

### 8.2 Compact binary mode

3 bits per channel, packed big-endian. 8 tally states (Idle, Preview, Program, Standby, ISO Recording, Streaming, Clean Feed, Reserved).

### 8.3 Downstream from server (S→C)

```json
{
  "type": "tally_confirm",
  "channel": 0,
  "state": "program",
  "color": "#FF0000",
  "label": "PGM"
}
```

-----

## 9. FLUX-M — Monitor Stream

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

**FLUX/M note:** Monitor streams in a FLUX/M session are delivered on a separate multicast group declared in the session descriptor (`monitor_mcast_group`). This avoids polluting the primary media group with sub-stream traffic. Receivers MAY subscribe independently to the monitor group without joining the primary group.

### 9.2 Monitor stream at high fps

When the source operates at ≥120 fps, the monitor stream SHOULD be decimated to 25/30/50/60 fps server-side.

-----

## 10. FLUX-E — Embedding arbitrary data in-stream

FLUX allows any MIME-typed binary payload to be multiplexed into the media stream, with optional temporal association to specific frames. In v0.5, FLUX-E was extended to support live video channel-to-texture binding. In v0.6.3, GLB assets can reference FLUX video channels directly via `flux://` URIs in the glTF `image.uri` field (§10.10).

**FLUX/M constraints on FLUX-E:**

FLUX/M supports FLUX-E for session-scoped assets (`frame_assoc.mode = "session"` or `"range"`). The following asset types and delivery modes are NOT supported over FLUX/M:

|Feature                                     |FLUX/QUIC|FLUX/M                                                                                                                                                   |
|--------------------------------------------|---------|---------------------------------------------------------------------------------------------------------------------------------------------------------|
|Full asset transfer (`background` / `burst`)|✅        |✅ (multicast UDP stream)                                                                                                                                 |
|GLB delta (`realtime`, per-frame)           |✅        |⚠️ Supported; loss of delta datagram causes visible glitch until next keyframe. No ARQ.                                                                   |
|GS delta sequences                          |✅        |⚠️ Same as GLB delta                                                                                                                                      |
|**GS residual sequences (`queen-v1`)**      |✅        |**⚠️ Supported; loss of residual datagram causes visual artifact until next anchor keyframe. No ARQ. Anchor keyframe interval MUST be ≤ 3 s (Annex D.4).**|
|`video_texture_bindings`                    |✅        |✅ (binding declared in session descriptor)                                                                                                               |
|`binding_control` delta                     |✅        |✅ (sent on multicast; no ACK)                                                                                                                            |
|`feed_uri_override` delta                   |✅        |✅ (sent on multicast; no ACK)                                                                                                                            |
|Selective ARQ for EMBED_CHUNK               |✅        |**No**                                                                                                                                                   |
|Cache negotiation (embed_cache in handshake)|✅        |**No** (session descriptor declares asset list; receiver pre-fetches via HTTP if needed)                                                                 |
|**GS codec negotiation (`gs_codecs`)**      |✅        |**No (session descriptor declares `gs_codec` used; no per-receiver negotiation)**                                                                        |
|**`flux://` image URIs in GLB**             |✅        |**✅ (channel resolved from multicast session; `channel_id` MUST appear in session descriptor `channels` list)**                                          |

### 10.1 Supported MIME types

|MIME Type                             |Broadcast / VP use case                                                           |
|--------------------------------------|----------------------------------------------------------------------------------|
|`model/gltf-binary`                   |3D scene (.glb) for VP — the LED volume environment                               |
|`model/vnd.usd` / `model/vnd.usdz`    |OpenUSD scene for nDisplay                                                        |
|`model/vnd.gaussian-splat`            |3D GS asset (SPZ, PLY, HAC format) — full keyframe or anchor frame                |
|`application/vnd.flux.glb-delta`      |Incremental GLB update                                                            |
|`application/vnd.flux.gs-delta`       |Gaussian Splat raw attribute delta frame (`codec=raw-attr`; legacy)               |
|**`application/vnd.flux.gs-residual`**|**Codec-identified GS residual frame — QUEEN or other learned codecs (see §10.9)**|
|`application/vnd.flux.gs-sequence`    |Gaussian Splat sequence header                                                    |
|`application/json`                    |Telemetry, production data, cue sheets                                            |
|`text/csv`                            |Time series, scores, sports statistics                                            |
|`image/x-exr`                         |HDR reference frame, environment HDRI                                             |
|`image/png` / `image/webp`            |Thumbnails, textures, overlays                                                    |
|`application/vnd.flux.tracking`       |Camera tracking data (extended FreeD)                                             |
|`application/vnd.flux.anc`            |Broadcast ancillary (SMPTE 291M over IP)                                          |
|`application/vnd.flux.mocap`          |Per-frame motion capture data                                                     |
|`application/vnd.flux.led-config`     |LED processor configuration (Brompton/Tessera)                                    |
|`application/octet-stream`            |Generic binary payload                                                            |

`application/vnd.flux.gs-delta` is preserved as a valid type for backward compatibility. Implementations MUST treat it as equivalent to `application/vnd.flux.gs-residual` with `gs_codec: "raw-attr"` when no `gs_codec` field is present in the `EMBED_MANIFEST`. New implementations SHOULD use `application/vnd.flux.gs-residual` with an explicit `gs_codec` field for all GS residual traffic.

### 10.2–10.3, 10.5–10.7

(Unchanged from v0.5 — embed priority modes, embedding flow, EMBED_CHUNK wire format, temporal association, hash-based deduplication.)

### 10.4 — EMBED_MANIFEST (amendment)

(Base format unchanged from v0.5. The following fields are added in v0.6.1.)

These fields are OPTIONAL for all existing MIME types and REQUIRED when `mime_type` is `application/vnd.flux.gs-residual`.

|Field            |Type  |Required for `gs-residual`|Description                                                                                                                                          |
|-----------------|------|--------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------|
|`gs_codec`       |string|REQUIRED                  |Codec identifier for the GS residual bitstream. See §10.9.                                                                                           |
|`gs_codec_params`|object|OPTIONAL                  |Codec-specific parameters. Schema is codec-defined. See Annex D for `queen-v1`.                                                                      |
|`anchor_asset_id`|string|REQUIRED                  |`asset_id` of the canonical GS keyframe this residual applies to. MUST match an asset previously received with `mime_type: model/vnd.gaussian-splat`.|
|`anchor_sha256`  |string|REQUIRED                  |SHA-256 of the canonical GS keyframe. Receivers MUST reject residual frames if their cached anchor does not match this hash.                         |

**`delta_base` and `anchor_asset_id` relationship:** For `application/vnd.flux.gs-residual`, both `delta_base` and `anchor_asset_id` MAY be present. If both are present, `anchor_asset_id` / `anchor_sha256` take normative precedence. The `delta_base` field is retained for backward-compat with v0.6 parsers that inspect it.

**Example EMBED_MANIFEST for a QUEEN-v1 residual frame:**

```json
{
  "asset_id": "queen-residual-seq01-frame-0042",
  "mime_type": "application/vnd.flux.gs-residual",
  "gs_codec": "queen-v1",
  "gs_codec_params": {
    "quant_bits": 8,
    "encoder_version": "queen-1.0",
    "sh_degree": 3,
    "num_gaussians": 120000
  },
  "anchor_asset_id": "queen-anchor-seq01",
  "anchor_sha256": "a3f2c1e8d4b9ff21...",
  "delta_base": {
    "asset_id": "queen-anchor-seq01",
    "sha256": "a3f2c1e8d4b9ff21..."
  },
  "total_bytes": 716800,
  "chunk_size": 65536,
  "chunk_count": 11,
  "sha256": "7bc3a9...",
  "compression": "none",
  "priority": "realtime",
  "frame_assoc": {
    "mode": "sequence",
    "sequence_id": "queen-seq-live-01",
    "step_index": 42,
    "step_ts_ns": 1743580812345678901
  }
}
```

-----

## 10.8 — Video channel to GLB texture binding

(Unchanged from v0.5 — §10.8.1 through §10.8.7.)

In FLUX/M, `video_texture_bindings` are declared in the FLUX/M Session Descriptor (§18.4) in addition to the `EMBED_MANIFEST`, since there is no per-receiver handshake. The normative binding remains the `EMBED_MANIFEST` declaration when the manifest is received over the multicast channel.

-----

## 10.9 — GS Residual Codec Framework

### §10.9.1 Motivation

The `application/vnd.flux.gs-delta` type defined in §11 (v0.4) assumed a specific delta model: per-splat changes to position, color, SH coefficients, and opacity in raw attribute space. This model does not accommodate GS codec pipelines where residual frames are encoded in a learned latent space — such as NVIDIA QUEEN — or any future quantized or neural GS codec.

FLUX is a transport protocol and MUST remain agnostic to the internal representation of GS codec bitstreams. The GS Residual Codec Framework introduces:

1. A **codec identification mechanism** (`gs_codec` field in `EMBED_MANIFEST`).
1. A **codec-specific parameter object** (`gs_codec_params`).
1. An **anchor registration flow** for codecs that require a decoded canonical GS to be resident in receiver memory before residuals are applied.
1. A **receiver capability declaration** (`gs_codecs` in `embed_support`).

FLUX does not define the internal encoding format for any GS codec. The codec bitstream is opaque to FLUX transport and framing layers. FLUX defines only the framing, identification, temporal association, and delivery mechanics.

### §10.9.2 Codec registry

The following `gs_codec` identifiers are normatively defined in this specification:

|Identifier|Description                                                                                                          |Defined in |
|----------|---------------------------------------------------------------------------------------------------------------------|-----------|
|`raw-attr`|Per-splat position / color / SH / opacity in raw attribute space. Legacy behavior of `application/vnd.flux.gs-delta`.|§11.1–§11.6|
|`queen-v1`|NVIDIA QUEEN quantized residual frames (NeurIPS 2024). **Informative reference only** — see Annex D.                 |Annex D    |

Additional identifiers MAY be registered by extending this table in future amendments. Implementations MUST silently skip `EMBED_MANIFEST` frames whose `gs_codec` value is not in their supported set (declared in `embed_support.gs_codecs`). Implementations MUST NOT attempt to decode an unsupported codec bitstream.

### §10.9.3 Anchor frame registration flow

Codec pipelines such as QUEEN operate on a **canonical anchor frame** (a full 3DGS scene) that is held in decoder memory. Residual frames carry only the quantized difference relative to that anchor. The anchor is not re-transmitted with every residual; it is sent once (or periodically, at the keyframe interval), and the receiver caches it.

The anchor frame MUST be transmitted as `mime_type: model/vnd.gaussian-splat` with `priority: background` before any residual frames referencing it are sent. The server MUST NOT transmit `application/vnd.flux.gs-residual` frames referencing an anchor until the receiver has confirmed anchor receipt via `EMBED_ACK` (FLUX/QUIC) or until the declared anchor pre-fetch window has elapsed (FLUX/M).

```
Server                                          Receiver
  |                                                |
  |-- EMBED_MANIFEST (anchor, background) ------> |
  |   { asset_id: "queen-anchor-seq01",           |
  |     mime_type: model/vnd.gaussian-splat,      |
  |     sha256: "a3f2c1...", priority: background }|
  |                                                |
  |-- EMBED_CHUNK × N (reliable stream) --------> |
  |   (full canonical GS — PLY/SPZ/HAC format)    |
  |                                                |
  |<-- EMBED_ACK { asset_id, status: "ready" } -- |
  |                                                |
  |-- EMBED_MANIFEST (residual, realtime) -------> |
  |   { gs_codec: "queen-v1",                     |
  |     anchor_asset_id: "queen-anchor-seq01",    |
  |     anchor_sha256: "a3f2c1...",               |
  |     frame_assoc: { mode: "sequence",          |
  |       step_index: 0, step_ts_ns: ... } }      |
  |                                                |
  |-- EMBED_CHUNK (datagram, realtime) ----------> |
  |-- EMBED_CHUNK (datagram, realtime) ----------> |
  |   (repeating at frame rate)                   |
```

### §10.9.4 Anchor mismatch — receiver MUST behaviour

If a receiver receives an `application/vnd.flux.gs-residual` frame and:

- The `anchor_asset_id` does not match any cached asset, OR
- The cached asset’s SHA-256 does not match `anchor_sha256`,

the receiver MUST discard the residual frame. The receiver MUST NOT attempt partial decoding. The receiver SHOULD log an `ANCHOR_MISMATCH` diagnostic event and wait for the next anchor keyframe retransmission before resuming residual decoding.

### §10.9.5 Anchor caching and `embed_cache`

Anchor frames SHOULD be included in the `embed_cache` field of `FLUX_SESSION_REQUEST` (§3.2), declared by SHA-256, so the server can skip re-transmission when the receiver has already cached the anchor from a previous session or pre-fetch.

```json
"embed_cache": [
  { "asset_id": "queen-anchor-seq01", "sha256": "a3f2c1..." }
]
```

In FLUX/M, the anchor is declared in the session descriptor `embed_catalog` and receivers SHOULD pre-fetch it via HTTP before joining the multicast group, to avoid a cold-start window where residuals arrive before the anchor is ready.

### §10.9.6 FLUX/M — anchor pre-fetch window

Because FLUX/M has no per-receiver handshake, the server cannot wait for `EMBED_ACK` before starting the residual stream. FLUX/M senders MUST declare an `anchor_prefetch_window_ms` value in the session descriptor for each GS residual sequence. This value is the minimum time between the first anchor `EMBED_CHUNK` transmission and the first residual frame, giving receivers time to fetch the anchor.

```json
"embed_catalog": [
  {
    "asset_id": "queen-anchor-seq01",
    "mime_type": "model/vnd.gaussian-splat",
    "sha256": "a3f2c1...",
    "fetch_url": "https://registry.lan:7500/assets/queen-anchor-seq01.ply",
    "size_bytes": 52428800,
    "anchor_prefetch_window_ms": 5000
  }
]
```

Receivers that fail to fetch the anchor before residual frames begin MUST discard all residuals until the next anchor keyframe retransmission (Annex D.4).

-----

## 10.10 — `flux://` URI scheme for live video textures in GLB

### §10.10.1 Overview

glTF 2.0 `image` objects support a `uri` field that references the image source. FLUX defines the URI scheme `flux://channel/{channel_id}` to declare that a GLB texture is sourced from a live FLUX video channel rather than from static image data.

This mechanism allows scene authors to embed video feed references directly inside the GLB asset. Any material whose texture chain resolves to a `flux://` image will display live decoded frames from the referenced FLUX channel, synchronised to `GROUP_TIMESTAMP_NS`.

### §10.10.2 URI format

```
flux://channel/{channel_id}
```

|Component   |Description                                                                                               |
|------------|----------------------------------------------------------------------------------------------------------|
|`channel_id`|Unsigned 16-bit integer matching a `CHANNEL_ID` declared via `STREAM_ANNOUNCE` in the active FLUX session.|

Examples: `flux://channel/2`, `flux://channel/5`.

The URI MUST appear in the `uri` field of a glTF `image` object. The `mimeType` field SHOULD be set to `"image/png"` or another valid glTF image MIME type for parser compatibility; FLUX-aware renderers MUST ignore the `mimeType` when a `flux://` URI is present.

### §10.10.3 GLB authoring convention

The GLB SHOULD include a static fallback image via `bufferView` on the same `image` object. Non-FLUX renderers (standard glTF viewers, editing tools) will display the fallback; FLUX-aware renderers MUST ignore `bufferView` when `uri` begins with `flux://`.

```json
{
  "images": [
    {
      "name": "studio_cam_feed",
      "uri": "flux://channel/2",
      "mimeType": "image/png",
      "bufferView": 3
    },
    {
      "name": "static_logo",
      "mimeType": "image/png",
      "bufferView": 4
    }
  ]
}
```

In this example, `images[0]` resolves to live video from FLUX channel 2 on a FLUX-aware renderer and to the static PNG in `bufferView` 3 on a standard glTF viewer. `images[1]` is a conventional static texture unaffected by this mechanism.

Any number of materials MAY reference the same `flux://` image. The renderer uploads the decoded frame once per `GROUP_TIMESTAMP_NS` and shares the GPU texture across all referencing materials.

### §10.10.4 Receiver behaviour

A FLUX-aware receiver that loads a GLB containing `flux://` image URIs MUST:

1. Parse each `image.uri` and extract `channel_id`.
1. Verify that the referenced `channel_id` exists in the active session (via `STREAM_ANNOUNCE`). If not, the receiver MUST fall back to the static `bufferView` image and SHOULD log a `FLUX_URI_UNRESOLVED` warning.
1. For each resolved `flux://` image, substitute the texture source with decoded video frames from the corresponding channel, applying the `color_transform` declared in the channel’s `glb_texture_role` (§6.2) if present, or `"none"` otherwise.
1. Upload the frame to the GPU texture slot before the scene draw call for the associated `GROUP_TIMESTAMP_NS`, following the same synchronisation rules as `video_texture_bindings` (§10.8, §6.2).

If the video channel is temporarily unavailable (jitter buffer miss), the receiver MUST hold the last successfully decoded frame. It MUST NOT revert to the static fallback during transient unavailability.

### §10.10.5 Precedence with `video_texture_bindings`

`video_texture_bindings` (§10.8) is a server-side declaration in `EMBED_MANIFEST`. `flux://` URIs are a scene-side declaration inside the GLB. When both target the same material slot:

1. **`video_texture_bindings` takes precedence.** The `EMBED_MANIFEST` declaration overrides the GLB-internal `flux://` URI for the duration of the binding.
1. If the `video_texture_bindings` entry is deactivated via `binding_control` (§10.8.7), the `flux://` URI binding resumes automatically.
1. `texture_swap` delta operations (§10.8.5) override both mechanisms during their declared `frame_assoc` range.

Evaluation order per `GROUP_TIMESTAMP_NS`:

```
1. Active texture_swap delta for this material + slot? → use static image.
2. Active video_texture_bindings entry for this material + slot? → use EMBED_MANIFEST binding.
3. Image has flux:// URI with resolved channel? → use GLB-internal live feed.
4. None of the above → use GLB static texture.
```

### §10.10.6 `feed_uri_override` GLB delta operation

A new GLB delta operation type `feed_uri_override` allows the server to remap a `flux://` image URI to a different channel at runtime without re-transmitting the GLB:

```json
{
  "op": "feed_uri_override",
  "image_index": 0,
  "channel_id": 7
}
```

|Field        |Type  |Required    |Description                                                                                            |
|-------------|------|------------|-------------------------------------------------------------------------------------------------------|
|`image_index`|uint16|**REQUIRED**|Index into the GLB `images` array. The target image MUST have a `flux://` URI.                         |
|`channel_id` |uint16|**REQUIRED**|New FLUX channel to source. Set to `0xFFFF` to disable the live feed and revert to the static fallback.|

The override persists until a new `feed_uri_override` targets the same `image_index`, or the GLB asset is replaced. Delivery uses the standard GLB delta mechanism (§11.3) with `priority: "realtime"`.

### §10.10.7 FLUX/M considerations

In FLUX/M, the `channel_id` referenced by a `flux://` URI MUST appear in the session descriptor `channels` list. Receivers that encounter a `flux://` URI referencing a channel not present in the session descriptor MUST fall back to the static `bufferView` and log a warning.

`feed_uri_override` deltas are delivered over multicast without acknowledgement, consistent with all FLUX/M delta operations.

-----

## 11. FLUX-E Delta — Incremental asset updates

(Unchanged from v0.5 — §11.1 through §11.6: GLB delta, GS delta, GS sequences, per-frame camera tracking.)

**FLUX/M specific note on delta delivery:** In FLUX/M all delta frames are delivered as unreliable UDP multicast datagrams. Because there is no ARQ:

- GLB delta sequences MUST emit a full keyframe at least every 5 seconds (stricter than the v0.5 default of 10 s) to limit visible glitch duration for receivers that experience burst loss.
- GS delta sequences MUST emit a keyframe at least every `keyframe_interval` steps AND at least every 3 seconds, whichever is shorter.

-----

## 11.7 — GS residual codec profiles (informative references)

The GS Residual Codec Framework (§10.9) is codec-agnostic. FLUX does not endorse or require any specific GS residual codec. Codec profiles are documented as informative annexes to enable interoperable implementations.

**Currently documented codec profiles:**

|`gs_codec`|Profile document                    |Status         |
|----------|------------------------------------|---------------|
|`raw-attr`|§11.1–§11.6 (this document)         |Normative      |
|`queen-v1`|**Annex D** — QUEEN-v1 Codec Profile|**Informative**|

Additional codec profiles (e.g. Scaniverse HAC, 3DGStream, Spacetime Gaussians) MAY be contributed as additional informative annexes in future amendments. Implementations MUST use the `gs_codec` field in `EMBED_MANIFEST` and the `gs_codecs` field in `embed_support` for codec negotiation, as defined in §10.9.

-----

## 12. FLUX-C — Upstream control channel (FLUX/QUIC only)

> **FLUX/M:** FLUX-C upstream commands MUST be sent via the unicast feedback TCP channel declared in the FLUX/M Session Descriptor (§18.8). The rate limiting and command format defined here apply unchanged.

### 12.1 Rate limiting

|Parameter                |Default|
|-------------------------|-------|
|`max_commands_per_second`|60     |
|`burst_allowance`        |10     |

-----

## 13. FEC and error recovery

### FLUX/QUIC — dynamic FEC (BW Governor driven)

|Mechanism          |Activation                        |Overhead|Latency cost     |
|-------------------|----------------------------------|--------|-----------------|
|**None**           |loss < 0.5%                       |0%      |0                |
|**XOR Row FEC**    |loss > 0.5% (base layer)          |~25%    |+1 FEC row period|
|**Reed-Solomon 2D**|loss > 2% (base layer)            |~50%    |+1 block period  |
|**Selective ARQ**  |Base layer keyframes only         |Variable|+1 RTT           |
|**Layer drop**     |Insufficient BW (EMERGENCY step 1)|0%      |0                |

### FLUX/M — proactive RaptorQ FEC (always active)

See §18.7 for the full FLUX/M FEC specification. Summary:

|Parameter                   |Default      |Range    |
|----------------------------|-------------|---------|
|Source symbols per block (K)|64           |32–256   |
|Repair symbols per block (T)|16 (25%)     |8–128    |
|FEC overhead                |25%          |12.5%–50%|
|Repair latency              |0 (proactive)|—        |

Unlike FLUX/QUIC, FLUX/M FEC is always active and statically configured in the Session Descriptor. There is no dynamic activation.

-----

## 14. Per-frame metadata — recommended JSON schema

```json
{
  "ts_ns": 1743580812345678901,
  "frame_index": 5000,
  "fps": 30,
  "scene": "VP_Take_003",
  "take": 3,
  "production": "LUCAB_Production_01",
  "embed_refs": ["queen-anchor-seq01"],
  "delta_refs": ["glb-delta-seq-42"],
  "gs_residual_refs": [
    {
      "sequence_id": "queen-seq-live-01",
      "step_index": 42,
      "gs_codec": "queen-v1",
      "anchor_asset_id": "queen-anchor-seq01"
    }
  ],
  "tally": { "program": true, "preview": false },
  "custom": {}
}
```

`gs_residual_refs` is an array of GS residual sequence references active at this frame timestamp. Receivers MAY use this to correlate per-frame metadata with in-flight residual payloads. The field is OPTIONAL; its absence does not indicate the absence of GS residual streams.

The `fps` field above reflects the GS sequence render rate (30 fps in this example). The primary media `fps` field (e.g. `240` for a high-frame-rate video channel) remains unchanged in the media metadata block.

-----

## 15. Security

|Feature             |`crypto_none`                       |`crypto_quic`            |`crypto_quic_aes`        |**FLUX/M**                     |
|--------------------|------------------------------------|-------------------------|-------------------------|-------------------------------|
|Transport encryption|None                                |TLS 1.3 (QUIC native)    |TLS 1.3 (QUIC native)    |**AES-256-GCM (group key)**    |
|Payload encryption  |None                                |None (QUIC-encrypted)    |AES-256-GCM per-frame    |**AES-256-GCM per-packet**     |
|Authentication      |JWT/API key over TCP control        |mTLS or JWT in handshake |mTLS or JWT in handshake |**TLS 1.3 to FLUX Key Server** |
|Asset integrity     |SHA-256 in EMBED_MANIFEST           |SHA-256 in EMBED_MANIFEST|SHA-256 in EMBED_MANIFEST|SHA-256 in EMBED_MANIFEST      |
|Access control      |FLUX Registry (OAuth 2.0 / API keys)|Same                     |Same                     |**FLUX Key Server (OAuth 2.0)**|
|Receiver revocation |N/A                                 |Session termination      |Session termination      |**Key epoch rotation (§18.5)** |

**FLUX/M has mandatory encryption.** The `crypto_none` mode is not available in FLUX/M — all multicast packets are AES-256-GCM encrypted with the group epoch key. This is required because multicast packets are visible to all hosts on the L2 domain regardless of IGMPv3 snooping.

-----

## 16. Implementation — notes for GStreamer / Rust

### GStreamer element inventory

|`fluxsrc`       |FLUX/QUIC   |QUIC/UDP receiver, emits pads per channel/layer                                                                         |
|`fluxsink`      |FLUX/QUIC   |QUIC/UDP transmitter                                                                                                    |
|`fluxdemux`     |Both        |Splits media / embed / delta / metadata into pads                                                                       |
|`fluxsync`      |Both        |MSS barrier (multi-stream jitter buffer, software or hardware PTP)                                                      |
|`fluxembedsrc`  |Both        |Injects FLUX-E assets into the pipeline                                                                                 |
|`fluxembeddec`  |Both        |Receives and reassembles assets, emits on downstream pad; routes by `mime_type` (see codec dispatch table below)        |
|`fluxdeltadec`  |Both        |Applies GLB/GS delta operations to the CPU-side scene graph; emits updated scene state. Does NOT perform GPU uploads.  |
|`fluxvideotex`  |Both        |Resolves `video_texture_bindings` and `flux://` image URIs, composites multi-channel bindings, uploads GPU textures (OpenGL / Vulkan)|
|`fluxcdbc`      |FLUX/QUIC   |Measures BW, generates adaptive CDBC_FEEDBACK                                                                           |
|`fluxtally`     |Both        |Manages bidirectional tally (JSON + compact binary)                                                                     |
|`fluxcrypto`    |FLUX/QUIC   |Handles QUIC crypto mode selection (none/quic/quic+aes)                                                                 |
|**`fluxmcastsrc`** |**FLUX/M**|**New (v0.6).** UDP multicast receiver. Joins SSM group (IGMPv3/MLDv2), decrypts AES-256-GCM with epoch key, reassembles FLUX frames (FRAG field), passes to `fluxdemux`. Handles AMT tunnel if configured.|
|**`fluxmcastsink`**|**FLUX/M**|**New (v0.6).** UDP multicast transmitter. Encrypts with group epoch key, applies RaptorQ FEC (§18.7), sends to SSM group. Manages epoch rotation, FLUXM_KEY_EPOCH emission.|
|**`fluxmcastrelay`**|**FLUX/M**|**New (v0.6).** FLUX/M ↔ FLUX/QUIC gateway. Ingests FLUX/QUIC unicast from an upstream FLUX/QUIC source, repackages for multicast delivery including group key encryption and RaptorQ FEC. Decrypts and re-encrypts; does NOT transcode media. See §18.11.|
|**`fluxgsresidualdec`**|**Both**|**New (v0.6.1).** GS residual codec dispatcher and decoder. Receives `application/vnd.flux.gs-residual` EMBED_CHUNKs from `fluxembeddec`, dispatches to the appropriate codec backend based on `gs_codec` field in `EMBED_MANIFEST`. Manages anchor frame caching, SHA-256 verification, and the FSM defined in Annex D.5. Outputs decoded per-frame Gaussian attribute tensors to downstream renderer.|

**Element responsibility notes:**

- `fluxmcastsrc` and `fluxmcastsink` handle FLUX/M-specific concerns (multicast join, key management, FEC, FRAG reassembly). Their downstream/upstream pads present the same FLUX frame types as `fluxsrc`/`fluxsink`, so the rest of the pipeline is profile-agnostic.
- `fluxdeltadec` operates exclusively on CPU-side scene graph state and MUST NOT attempt GPU texture uploads.
- `fluxvideotex` owns all GPU texture upload and compositing logic.
- `fluxvideotex` MUST scan loaded GLB `images` arrays for `flux://channel/{id}` URIs and register them as live texture sources alongside any `video_texture_bindings` declared in `EMBED_MANIFEST`. Precedence follows §10.10.5.

### `fluxgsresidualdec` element (v0.6.1)

**Properties:**

|Property            |Type       |Default              |Description                                                                                                                   |
|--------------------|-----------|---------------------|------------------------------------------------------------------------------------------------------------------------------|
|`supported-codecs`  |string list|`"raw-attr,queen-v1"`|Comma-separated list of `gs_codec` identifiers this instance will accept. Others are silently discarded.                      |
|`anchor-cache-dir`  |string     |`""` (in-memory)     |If set, anchor frames are cached to disk at this path, enabling warm restarts.                                                |
|`max-anchor-size-mb`|int        |512                  |Maximum size of a single anchor frame. Frames exceeding this limit cause `ANCHOR_REJECT` event.                               |
|`mismatch-policy`   |enum       |`discard`            |Behaviour on `ANCHOR_MISMATCH`: `discard` (drop residuals, wait) or `freeze` (output last valid frame until new anchor ready).|

**Pad layout:**

```
                        ┌──────────────────────────────────┐
  fluxembeddec src ────►│  fluxgsresidualdec               │
  (gs-residual)         │                                  │
  fluxembeddec src ────►│  anchor_sink (model/vnd.gs)      ├──► src (GS frame tensors)
  (model/vnd.gs anchor) │                                  │
                        │  Signals:                        │
                        │    anchor-ready (asset_id)       │
                        │    anchor-mismatch (asset_id)    │
                        │    residual-dropped (step_index) │
                        └──────────────────────────────────┘
```

### Updated `fluxembeddec` codec dispatch

`fluxembeddec` MUST inspect the `mime_type` field of each received `EMBED_MANIFEST` and route accordingly:

|`mime_type`                       |Downstream element                                      |
|----------------------------------|--------------------------------------------------------|
|`model/gltf-binary`               |`fluxdeltadec`                                          |
|`model/vnd.gaussian-splat`        |`fluxgsresidualdec.anchor_sink` or application          |
|`application/vnd.flux.gs-residual`|`fluxgsresidualdec`                                     |
|`application/vnd.flux.gs-delta`   |`fluxgsresidualdec` (dispatched as `gs_codec: raw-attr`)|
|`application/vnd.flux.tracking`   |application / `appsink`                                 |
|All others                        |`appsink` or application                                |

### GStreamer pipeline example — QUEEN volumetric stream over FLUX/QUIC

```
fluxsrc uri=flux://192.168.1.50:7400 crypto=crypto_quic \
  ! fluxdemux name=d

# Primary video channel
d.video_0_0 ! fluxsync group=1 ! h265parse ! nvh265dec ! videoconvert ! autovideosink

# Audio
d.audio_0   ! fluxsync group=1 ! audio/x-raw,format=F32LE ! autoaudiosink

# GS anchor frame path
d.embed_gs  ! fluxembeddec mime=model/vnd.gaussian-splat \
            ! fluxgsresidualdec.anchor_sink name=gsdec

# GS residual frame path (realtime datagrams)
d.gs_residual ! fluxembeddec mime=application/vnd.flux.gs-residual \
              ! gsdec.

# Decoded GS output → application renderer
gsdec.src ! appsink name=gs_sink emit-signals=true
```

### GStreamer pipeline example — QUEEN volumetric stream over FLUX/M

```
fluxmcastsrc \
  sd-url=https://registry.lan:7500/api/sd/vol-cam-a \
  feedback-enabled=true \
  ! fluxdemux name=d

d.video_0_0 ! fluxsync group=1 ! h265parse ! nvh265dec ! autovideosink
d.audio_0   ! fluxsync group=1 ! audio/x-raw,format=F32LE ! autoaudiosink

d.embed_gs  ! fluxembeddec mime=model/vnd.gaussian-splat \
            ! fluxgsresidualdec.anchor_sink name=gsdec \
                supported-codecs=queen-v1 \
                mismatch-policy=freeze

d.gs_residual ! fluxembeddec mime=application/vnd.flux.gs-residual \
              ! gsdec.

gsdec.src ! appsink name=gs_sink emit-signals=true
```

### `fluxmcastsrc` element design (v0.6)

```
                   ┌─────────────────────────────────────────────────────┐
 UDP multicast ───►│  fluxmcastsrc                                        │
 (SSM group)       │                                                      │
 Key Server ──────►│  1. IGMPv3 JOIN (SSM source, group)                  ├──► src (FLUX frames, decrypted)
 (TLS unicast)     │  2. AES-256-GCM decrypt (epoch key)                  │
                   │  3. RaptorQ FEC recovery                             │
                   │  4. FRAG reassembly → full FLUX frames               │
                   │  5. KEEPALIVE watchdog                               │
                   └─────────────────────────────────────────────────────┘
```

Properties:

- `sd-url`: URL of the FLUX/M Session Descriptor (§18.4)
- `keyserver-url`: URL of the FLUX Key Server (§18.5); overrides SD if specified
- `amt-relay`: AMT relay address (if AMT tunnel required; §18.10)
- `fec-engine`: `"raptorq"` (default) or `"none"` (testing only)
- `feedback-enabled`: boolean; enables unicast NACK/stats feedback channel (§18.8)
- `feedback-port`: local UDP port for feedback (default: ephemeral)

### GStreamer pipeline example (FLUX/M receiver)

```
fluxmcastsrc sd-url=https://registry.lan:7500/api/sd/cam-a feedback-enabled=true \
  ! fluxdemux name=d
d.video_0_0 ! fluxsync group=1 ptp-mode=software ! h265parse ! nvh265dec ! videoconvert ! autovideosink
d.audio_0   ! fluxsync group=1 ptp-mode=software ! audio/x-raw,format=F32LE ! autoaudiosink
d.embed_glb ! fluxembeddec mime=model/gltf-binary ! fluxdeltadec ! appsink name=glb_sink
d.metadata  ! appsink name=meta_sink
```

### GStreamer pipeline example (FLUX/M with live video texture, v0.6)

```
fluxmcastsrc sd-url=https://registry.lan:7500/api/sd/vp-scene feedback-enabled=true \
  ! fluxdemux name=d

d.video_0_0 ! fluxsync group=1 ! nvh265dec ! glimagesink
d.video_2_0 ! fluxsync group=1 ! nvh265dec ! video/x-raw(memory:GLMemory) ! fluxvideotex.video_2
d.video_5_0 ! fluxsync group=1 ! nvh265dec ! video/x-raw(memory:GLMemory) ! fluxvideotex.video_5

d.embed_glb   ! fluxembeddec ! fluxdeltadec name=scene_dec
d.delta_glb   ! scene_dec.delta_sink
scene_dec.src ! fluxvideotex.scene_in

fluxvideotex. ! appsink name=scene_sink emit-signals=true
```

### GStreamer pipeline example (FLUX/M ↔ FLUX/QUIC relay, v0.6)

```
# Ingest from FLUX/QUIC source, distribute via multicast
fluxsrc uri=flux://192.168.1.50:7400 crypto=crypto_quic \
  ! fluxmcastrelay \
      sd-url=https://registry.lan:7500/api/sd/cam-a-relay \
      mcast-group=239.100.1.1 \
      mcast-src=192.168.1.100 \
      mcast-port=7500 \
      fec-overhead-pct=25 \
      keyserver-url=https://keyserver.lan:7600
```

### Relevant Rust crates

```toml
[dependencies]
quinn              = "0.11"   # QUIC (backed by rustls), datagrams
s2n-quic           = "1"      # Alternative: AWS QUIC
serde_json         = "1"      # JSON metadata
zstd               = "0.13"   # asset/delta compression
sha2               = "0.10"   # EMBED integrity
mdns-sd            = "0.10"   # DNS-SD discovery
tokio              = { version = "1", features = ["full"] }
bytes              = "1"      # buffer management
gstreamer          = "0.22"   # GStreamer-rs
half               = "2"      # float16 for GS delta encoding and quantized residual values
glam               = "0.27"   # vector/quaternion math
gstreamer-gl       = "0.22"   # GstGLMemory (fluxvideotex OpenGL path)
gstreamer-vulkan   = "0.22"   # GstVulkanImageMemory (fluxvideotex Vulkan path)
# v0.6 additions for FLUX/M:
raptorq            = "1"      # RaptorQ FEC encoder/decoder (RFC 6330)
# v0.6.1 additions for GS Residual Codec Framework:
candle-core        = "0.6"   # Tensor operations for QUEEN residual decoding (optional; CPU path)
aes-gcm            = "0.10"   # AES-256-GCM group key crypto
socket2            = "0.5"    # Multicast socket options (IP_ADD_SOURCE_MEMBERSHIP)
nix                = "0.29"   # IGMPv3/MLDv2 socket control (SO_BINDTODEVICE, etc.)
reqwest            = { version = "0.12", features = ["rustls-tls"] } # Key Server HTTPS client
```

-----

## 17. QUIC transport summary (FLUX/QUIC only)

### When `crypto_quic` or `crypto_quic_aes`

|QUIC mechanism   |Content                                         |Direction    |Urgency (RFC 9218)|
|-----------------|------------------------------------------------|-------------|------------------|
|Stream 0 (bidi)  |Control (SESSION, ANNOUNCE, KEEPALIVE)          |Bidirectional|0 (critical)      |
|Stream 2 (uni)   |Selective ARQ retransmits (base layer keyframes)|S→C          |0                 |
|Datagram         |CDBC_FEEDBACK + TALLY                           |C→S          |— (unreliable)    |
|Datagram         |SYNC_ANCHOR                                     |S→C          |— (unreliable)    |
|Datagram         |Media: all channels, all layers                 |S→C          |— (unreliable)    |
|Datagram         |FEC_REPAIR                                      |S→C          |— (unreliable)    |
|Datagram         |EMBED_CHUNK (realtime delta, binding_control)   |S→C          |— (unreliable)    |
|Stream 4..N (uni)|EMBED_MANIFEST                                  |S→C          |0                 |
|Stream 4..N (uni)|EMBED_CHUNK (background/burst)                  |S→C          |3 or 6            |
|Datagram         |BANDWIDTH_PROBE                                 |S→C          |— (unreliable)    |
|Datagram         |KEEPALIVE                                       |Both         |— (unreliable)    |
|Datagram         |UPSTREAM_CONTROL (FLUX-C)                       |C→S          |— (unreliable)    |

### When `crypto_none`

|Transport         |Content                                                 |Direction|
|------------------|--------------------------------------------------------|---------|
|UDP (main port)   |All datagram-class frames                               |Both     |
|TCP (control port)|SESSION, ANNOUNCE, STREAM_END, EMBED_MANIFEST, EMBED_ACK|Both     |
|TCP (control port)|EMBED_CHUNK (background/burst)                          |S→C      |
|UDP (main port)   |EMBED_CHUNK (realtime delta, binding_control)           |S→C      |

-----

## 18. FLUX/M — Multicast Group Distribution

### 18.1 Scope and design constraints

FLUX/M is a **unidirectional multicast delivery profile** of FLUX, designed for the following scenarios:

|Scenario                                    |Typical N|Notes                                 |
|--------------------------------------------|---------|--------------------------------------|
|Monitor wall in a facility                  |10–100   |Single stream, all screens subscribe  |
|Confidence monitoring (edit suites)         |5–50     |Multiple feeds, selective subscription|
|Contribution from OB truck to master control|1–10     |SSM over MPLS/L2                      |
|Distribution within a campus or data centre |10–500   |Routed multicast within a single AS   |

FLUX/M is **not** designed for WAN distribution to arbitrary internet receivers. For WAN distribution, use FLUX/QUIC with a relay tree (§18.11) or an AMT gateway (§18.10).

**Design constraints (non-negotiable):**

1. **No ARQ.** Retransmission requests are architecturally impossible in multicast — the sender cannot address individual receivers. Loss recovery is exclusively via proactive FEC (§18.7).
1. **No per-receiver CDBC.** There is no feedback-driven bandwidth control. The sender operates at a fixed configured bitrate. Optional NACK/stats feedback (§18.8) is informational only and MUST NOT influence the sender’s bitrate in real time.
1. **No per-session handshake.** Session parameters are published out-of-band via the FLUX/M Session Descriptor (§18.4). Receivers join by reading the SD and subscribing to the SSM group.
1. **Mandatory AES-256-GCM encryption.** All multicast packets are encrypted with the group epoch key (§18.5). `crypto_none` is not permitted.
1. **FLUX framing preserved.** The 32-byte FLUX header is used unchanged. FLUX/M adds only the AES-256-GCM authentication tag before the header on the wire.

### 18.2 Network requirements

#### IP multicast routing

FLUX/M requires a network that supports IP multicast routing. The RECOMMENDED configuration is:

- **IGMPv3** (IPv4) or **MLDv2** (IPv6) on all hosts and switches
- **PIM-SSM** (Protocol Independent Multicast — Source-Specific Multicast) between routers
- **IGMP/MLD snooping** on L2 switches to limit multicast flooding

For deployments without multicast routing, see §18.10 (AMT tunneling).

#### SSM vs ASM

FLUX/M MUST use **Source-Specific Multicast (SSM)** per RFC 4607:

```
SSM channel: (S, G)
  S = sender unicast IP address
  G = multicast group address (SSM range: 232.0.0.0/8 for IPv4, ff3x::/32 for IPv6)
```

SSM provides:

- Unambiguous source identification (no rogue source injection)
- PIM-SSM is simpler and more widely deployed than PIM-SM with RP
- IGMPv3 `INCLUDE` mode maps directly to SSM subscriptions

**Private address allocation:** For facility-internal deployments, FLUX/M RECOMMENDS using the Organisation-Local scope multicast range `239.0.0.0/8` (RFC 2365). Groups MUST be allocated by the facility network administrator to avoid collisions. The FLUX Registry SHOULD be used as the allocation authority.

#### Minimum switch/router requirements

|Feature                 |Requirement                                                                                            |
|------------------------|-------------------------------------------------------------------------------------------------------|
|IGMPv3 snooping         |REQUIRED on all L2 switches in the path                                                                |
|PIM-SSM                 |REQUIRED on all L3 routers in the path                                                                 |
|IGMP querier            |REQUIRED on at least one router per L2 segment                                                         |
|Multicast queue priority|RECOMMENDED (QoS marking, DSCP EF or CS6)                                                              |
|Jumbo frames            |**REQUIRED** (MTU ≥ 9000) for any stream with aggregate bitrate ≥ 10 Mbit/s; RECOMMENDED for all others|

Senders MUST refuse to instantiate a FLUX/M session whose aggregate bitrate (media + FEC overhead) exceeds 10 Mbit/s on a path with PMTUD-reported MTU < 9000. This is a hard constraint derived from the FEC block latency analysis in §18.7.x: at standard Ethernet MTU (1500), a 15 Mbit/s stream produces ~49 ms of FEC block latency per source block, which exceeds acceptable latency for broadcast contribution.

### 18.3 Multicast group addressing

Each FLUX/M stream (or group of streams sharing a session) is assigned an SSM channel `(S, G)`:

```
Source address (S): The sender's unicast IP address (the interface used for multicast output)
Group address (G):  Chosen from the SSM range by the facility network administrator
Port:               The UDP destination port (default: 7500 for FLUX/M media, 7501 for monitor)
```

**Multiple channels in one session:** Multiple FLUX channels (video, audio, tally, FLUX-E) are multiplexed onto a **single SSM group** using the FLUX `CHANNEL_ID` field in the frame header. There is no need to allocate one multicast group per channel. A single `(S, G, port)` tuple carries all channels for a session.

**Multiple sessions (multiple senders):** Each sender uses its own SSM source address `S`. Receivers distinguish sessions by the `(S, G)` pair.

```
Example layout:
  CAM_A session:  (192.168.1.50, 239.100.1.1, port 7500) — all channels
  CAM_B session:  (192.168.1.51, 239.100.1.1, port 7500) — same group, different source (SSM)
  CAM_C session:  (192.168.1.52, 239.100.1.2, port 7500) — different group
  Monitor feeds:  (192.168.1.50, 239.100.2.1, port 7501) — separate monitor group
```

### 18.4 Out-of-band session setup: FLUX/M Session Descriptor

Because FLUX/M has no per-receiver handshake, all session parameters are published as a **FLUX/M Session Descriptor (SD)** — a JSON document available over HTTPS from the FLUX Registry or a standalone HTTP server.

**Retrieval:**

```
GET https://registry.lan:7500/api/sd/{session_id}
Authorization: Bearer <JWT>
Content-Type: application/json
```

Receivers MUST retrieve the SD before joining the multicast group. The SD is versioned; receivers MUST poll for updates at the interval specified by `sd_refresh_interval_s`.

**FLUX/M Session Descriptor schema:**

```json
{
  "flux_version": "0.6.4",
  "flux_profile": "flux_m",
  "session_id": "flux-m-cam-a-studio",
  "name": "CAM_A (LUCAB Studio) — Multicast",
  "description": "Primary camera A — facility multicast feed",
  "sd_version": 4,
  "sd_refresh_interval_s": 30,

  "multicast": {
    "source_ip": "192.168.1.50",
    "group_ip": "239.100.1.1",
    "port": 7500,
    "ip_version": 4,
    "ttl": 32,
    "dscp": "EF"
  },

  "monitor_multicast": {
    "group_ip": "239.100.2.1",
    "port": 7501
  },

  "keyserver": {
    "url": "https://keyserver.lan:7600",
    "auth": "oauth2_client_credentials",
    "token_url": "https://keyserver.lan:7600/oauth/token",
    "audience": "flux-m-cam-a-studio"
  },

  "feedback": {
    "enabled": true,
    "server_ip": "192.168.1.50",
    "server_port": 7502,
    "nack_enabled": true,
    "stats_interval_s": 5
  },

  "fec": {
    "algorithm": "raptorq",
    "source_symbols_per_block": 64,
    "repair_symbols_per_block": 16,
    "interleaving": "frame_spread"
  },

  "streams": [
    {
      "channel_id": 0,
      "layer_id": 0,
      "name": "CAM_A_VIDEO",
      "content_type": "video",
      "codec": "h265",
      "group_id": 1,
      "sync_role": "master",
      "frame_rate": "50/1",
      "resolution": "1920x1080",
      "hdr": "sdr",
      "colorspace": "bt709",
      "bitrate_kbps": 15000
    },
    {
      "channel_id": 1,
      "name": "CAM_A_AUDIO",
      "content_type": "audio",
      "codec": "pcm_f32",
      "group_id": 1,
      "sync_role": "slave",
      "sample_rate": 48000,
      "channels": 8,
      "bitrate_kbps": 12288
    }
  ],

  "ptp": {
    "mode": "ptp_software",
    "sync_anchor_interval_ms": 250
  },

  "embed_catalog": [
    {
      "asset_id": "scene-glb-take-003",
      "mime_type": "model/gltf-binary",
      "sha256": "a3f2c1...",
      "total_bytes": 48234567,
      "prefetch_url": "https://assets.lan:7503/flux/scene-glb-take-003.bin",
      "video_texture_bindings": [
        {
          "channel_id": 2,
          "group_id": 1,
          "material_path": "/materials/screen_mat",
          "slot": "baseColorTexture",
          "color_transform": "bt709_to_linear",
          "blend_mode": "normal",
          "active": true
        }
      ]
    }
  ],

  "keepalive": {
    "interval_ms": 1000,
    "timeout_count": 3
  }
}
```

**`prefetch_url`:** Assets in `embed_catalog` MAY include an HTTPS URL from which receivers can pre-fetch the binary asset before the session starts, avoiding the need to receive the full `EMBED_CHUNK` sequence over multicast. The asset MUST be verified against `sha256` after download.

**SD versioning:** When the SD changes (e.g. a new take begins, the GLB scene changes, or the key epoch changes), `sd_version` is incremented. Receivers MUST detect version changes on poll and re-read all fields.

### 18.5 Group key management

#### Key architecture

FLUX/M uses **per-epoch AES-256-GCM symmetric keys** shared among all authorized receivers. The epoch model allows key rotation for access control without re-joining the multicast group.

```
┌─────────────────────────────────────────────────────────────────┐
│                    FLUX Key Server                               │
│                                                                  │
│  OAuth 2.0 client credentials  →  access_token (JWT)            │
│  GET /keys/{session_id}/current → { epoch_id, key_b64, ttl_s }  │
│  GET /keys/{session_id}/next    → { epoch_id, key_b64, ttl_s }  │
│                                                                  │
│  Epoch rotation:                                                 │
│    1. Server announces FLUXM_KEY_EPOCH on multicast (§18.5.3)   │
│    2. Receiver fetches new key from Key Server (unicast HTTPS)   │
│    3. Receiver begins decrypting with new key at epoch_start_ns  │
└─────────────────────────────────────────────────────────────────┘
```

#### AES-256-GCM wire format per packet

Every FLUX/M UDP datagram has the following structure:

```
[ epoch_id: uint32 ]             — identifies the key epoch (4 bytes)
[ packet_number: uint64 ]        — monotonically increasing per session (8 bytes)
[ auth_tag: 16 bytes ]           — AES-256-GCM authentication tag
[ ciphertext: N bytes ]          — encrypted FLUX frame(s) (32-byte header + payload)
```

**Total per-packet overhead vs. plaintext FLUX/QUIC datagram:** 28 bytes (4 + 8 + 16).

**Nonce construction:**

```
nonce (96 bits) = epoch_id (32 bits, big-endian) ‖ packet_number (64 bits, big-endian)
```

The nonce uniquely identifies each packet within an epoch. `packet_number` MUST NOT be reused within a single epoch. If the epoch is not rotated before `packet_number` overflows (2⁶⁴ packets), the sender MUST force an epoch rotation. At 240 fps × 16 channels, this theoretical limit is never reached in practice.

**AAD (Additional Authenticated Data):**

```
aad = epoch_id (4 bytes) ‖ packet_number (8 bytes)
```

The AAD protects the epoch_id and packet_number from tampering without encrypting them, allowing receivers to detect key epoch from the plaintext header.

#### Key Server REST API

```
POST /oauth/token
  grant_type=client_credentials&client_id=...&client_secret=...&audience=flux-m-{session_id}
  → { access_token, expires_in }

GET /keys/{session_id}/current
  Authorization: Bearer {access_token}
  → {
      "epoch_id": 7,
      "key_b64": "base64-encoded 32-byte AES key",
      "epoch_start_ns": 1743580800000000000,
      "epoch_ttl_s": 3600,
      "next_epoch_id": 8,
      "next_epoch_start_ns": 1743584400000000000
    }

GET /keys/{session_id}/epoch/{epoch_id}
  Authorization: Bearer {access_token}
  → { "epoch_id": 7, "key_b64": "...", "epoch_start_ns": ... }
```

Receivers SHOULD pre-fetch the next epoch key before `next_epoch_start_ns` to ensure seamless decryption across epoch boundaries. The Key Server MUST make the next epoch key available at least `max_rtt + 2 × sd_refresh_interval_s` seconds before the epoch transition.

#### FLUXM_KEY_EPOCH frame (TYPE=0x10)

When the sender rotates to a new epoch, it emits `FLUXM_KEY_EPOCH` frames encrypted with the **current** epoch key, giving receivers advance notice to pre-fetch the next key:

```json
{
  "session_id": "flux-m-cam-a-studio",
  "current_epoch_id": 7,
  "next_epoch_id": 8,
  "next_epoch_start_ns": 1743584400000000000,
  "keyserver_url": "https://keyserver.lan:7600"
}
```

The `FLUXM_KEY_EPOCH` frame is emitted at 1-second intervals for the 30 seconds preceding the epoch transition. Receivers MUST initiate key pre-fetch within 10 seconds of receiving this frame.

#### Receiver revocation

To revoke a receiver’s access, the operator issues a key rotation with a new epoch key that is NOT distributed to the revoked receiver (i.e., the Key Server returns HTTP 403 for that receiver’s client credentials for future epochs). The revoked receiver loses decryption ability after the current epoch expires.

### 18.6 FLUX/M frame encapsulation

#### MTU and fragmentation

FLUX/M uses the FRAG field (4 bits) in the FLUX frame header for application-level fragmentation. This is the same mechanism defined in §4.1.

|FRAG value |Meaning                                       |
|-----------|----------------------------------------------|
|`0x0`      |Complete frame, no fragmentation              |
|`0x1`–`0xD`|Fragment index (1-based) of a fragmented frame|
|`0xE`      |Last fragment of a fragmented frame           |
|`0xF`      |Reserved                                      |

**MTU requirement:** FLUX/M senders MUST target a UDP payload size of:

```
UDP payload = MTU - IP header (20/40) - UDP header (8) - FLUX/M overhead (28: epoch_id + packet_number + auth_tag)
            = 1500 - 28 - 28 = 1444 bytes  (standard Ethernet MTU — only permitted for streams < 10 Mbit/s)
            = 9000 - 28 - 28 = 8944 bytes  (jumbo frames, REQUIRED for streams ≥ 10 Mbit/s; see §18.2)
```

With jumbo frames, a 4K JPEG-XS frame (typically 2–8 MB) requires far fewer UDP datagrams and correspondingly fewer RaptorQ source symbols per block, which improves FEC efficiency and reduces FEC block latency (see §18.7.x).

#### Per-packet structure (complete)

```
UDP payload (encrypted):
  [ epoch_id: uint32 BE ]
  [ packet_number: uint64 BE ]
  [ auth_tag: 16 bytes ]          ← AES-256-GCM auth tag
  [ ciphertext ]                  ← encrypted content below:
    [ FLUX header: 32 bytes ]
    [ FLUX payload: variable ]
    [ RaptorQ source symbol padding, if applicable ]
```

### 18.7 Proactive FEC: RaptorQ (RFC 6330)

#### Rationale

In a multicast environment with no ARQ, proactive FEC is the only mechanism for loss recovery. RaptorQ is chosen over XOR row FEC and Reed-Solomon 2D because:

- **Rateless:** the number of repair symbols can be tuned independently of the source block size
- **Near-MDS:** can recover any K source symbols from any K+ε repair symbols with high probability
- **Streaming-friendly:** source symbols can be emitted in parallel with media packets
- **RFC-standardised:** reference implementations exist in multiple languages (`raptorq` Rust crate; `openRQ` Java)

#### Source block model

FLUX/M divides the media stream into **source blocks** of K source symbols. Each source symbol is one UDP payload (FLUX/M packet). The sender generates T repair symbols per block and interleaves them with the source symbols.

```
Block N:
  Source symbols:  [S₀, S₁, S₂, ..., S_{K-1}]   → K media packets
  Repair symbols:  [R₀, R₁, R₂, ..., R_{T-1}]   → T FEC packets (TYPE=0x7)
```

**Default parameters (operator-configurable in Session Descriptor):**

|Parameter                   |Default                                   |Min  |Max|
|----------------------------|------------------------------------------|-----|---|
|K (source symbols per block)|64                                        |32   |256|
|T (repair symbols per block)|16                                        |8    |128|
|Effective FEC overhead      |25%                                       |12.5%|50%|
|Max recoverable loss        |Up to T symbols from any position in block|     |   |

**Block boundary policy:** A new source block begins on IDR (keyframe) boundaries whenever possible. This ensures that FEC block boundaries align with decoder restart points, allowing late-joining receivers to begin recovery from a clean block.

#### FEC_REPAIR frame payload (TYPE=0x7, FLUX/M extension)

In FLUX/QUIC, `FEC_REPAIR` carries a raw repair packet. In FLUX/M, the `FEC_REPAIR` payload is extended with a RaptorQ encoding symbol ID (ESI):

```
[ source_block_number: uint32 ]   — identifies the source block
[ encoding_symbol_id: uint32 ]    — RaptorQ ESI (distinguishes repair symbols)
[ source_block_length: uint16 ]   — K (source symbols in this block)
[ symbol_data: N bytes ]          — RaptorQ encoded symbol data
```

The `FEC_GROUP` field in the FLUX header (byte 27) carries the low 8 bits of `source_block_number` for fast lookup.

#### Interleaving strategy

FLUX/M senders SHOULD use `"interleaving": "frame_spread"` (default), which distributes repair symbols evenly across the frame period:

```
50 fps stream, K=64, T=16 (80 packets per second × 1 block per frame):
  Frame period: 20 ms
  Source packets: 64 per frame
  Repair packets: 16 per frame, distributed every 1.25 ms within the frame period
```

This ensures that burst losses within a single frame do not exceed the FEC correction capacity.

#### Receiver FEC recovery

The receiver maintains a **source block buffer** per `source_block_number`. When the block is complete (K source symbols received), FEC decoding is skipped. When source symbols are missing, the receiver attempts RaptorQ decoding as soon as K symbols (source + repair) have been received for that block.

If decoding fails (more than T losses in a block), the receiver:

1. Emits `FLUXM_NACK` for the failed block (§18.8) — informational only.
1. Holds the last successfully decoded frame (concealment).
1. Waits for the next IDR-aligned block.

#### §18.7.x FEC block latency budget (v0.6.4)

RaptorQ decoding cannot begin until the receiver holds K symbols from a source block. The minimum additional receiver buffer to make FEC useful is therefore one block duration. This duration depends on the aggregate bitrate and the UDP payload size (determined by MTU).

**Block duration = (K × symbol_size) / aggregate_bitrate**

Where `symbol_size` = MTU − IP header (20/40 bytes) − UDP header (8 bytes) − FLUX/M overhead (28 bytes).

|Aggregate bitrate|MTU 1500 (symbol ≈ 1444 B)|MTU 9000 (symbol ≈ 8944 B)|
|-----------------|--------------------------|--------------------------|
|5 Mbit/s         |~148 ms (K=64)            |~92 ms (K=64)             |
|10 Mbit/s        |~74 ms                    |~46 ms                    |
|15 Mbit/s        |~49 ms                    |~31 ms                    |
|25 Mbit/s        |~30 ms                    |~18 ms                    |
|50 Mbit/s        |~15 ms                    |~9 ms                     |
|100 Mbit/s       |~7 ms                     |~5 ms                     |

**With K=32 (reduced source block):**

|Aggregate bitrate|MTU 1500 (symbol ≈ 1444 B)|MTU 9000 (symbol ≈ 8944 B)|
|-----------------|--------------------------|--------------------------|
|5 Mbit/s         |~74 ms                    |~46 ms                    |
|10 Mbit/s        |~37 ms                    |~23 ms                    |
|15 Mbit/s        |~25 ms                    |~15 ms                    |
|25 Mbit/s        |~15 ms                    |~9 ms                     |
|50 Mbit/s        |~7 ms                     |~5 ms                     |

**Normative requirements:**

- Senders MUST select K such that the resulting block duration does not exceed **100 ms** at the configured aggregate bitrate and known path MTU.
- For aggregate bitrates ≥ 10 Mbit/s at MTU 1500, the default K=64 produces block durations ≤ 74 ms, which is within the 100 ms ceiling. However, jumbo frames (MTU ≥ 9000) are REQUIRED at these bitrates to keep block latency acceptable for broadcast contribution (see §18.2).
- For latency-sensitive configurations, senders SHOULD reduce K to 32, which halves the block duration at the cost of reduced FEC burst-loss resilience per block.

### 18.8 Unicast feedback channel

FLUX/M includes an optional unicast UDP feedback channel from receivers to the sender. This channel is **informational only** — the sender MUST NOT use feedback to drive real-time bitrate changes.

**Channel setup:** The sender advertises its feedback address in the Session Descriptor (`feedback.server_ip`, `feedback.server_port`). Receivers send UDP datagrams to this address. The sender MAY aggregate statistics from multiple receivers.

#### FLUXM_NACK frame (TYPE=0x11, C→S, unicast UDP)

Sent when a receiver fails to recover a source block after FEC:

```json
{
  "session_id": "flux-m-cam-a-studio",
  "receiver_id": "lucab-monitor-wall-01",
  "ts_ns": 1743580812345678901,
  "failed_block": {
    "source_block_number": 12345,
    "lost_source_symbols": 23,
    "received_source_symbols": 41,
    "received_repair_symbols": 14,
    "group_ts_ns_start": 1743580811000000000
  }
}
```

The sender SHOULD log NACKs for network health monitoring. The sender MAY increase the FEC repair ratio in response to persistent NACKs by updating the Session Descriptor — but this change takes effect only after receivers poll the updated SD.

#### FLUXM_STAT frame (TYPE=0x12, C→S, unicast UDP)

Periodic receiver statistics (interval configurable in SD `feedback.stats_interval_s`):

```json
{
  "session_id": "flux-m-cam-a-studio",
  "receiver_id": "lucab-monitor-wall-01",
  "ts_ns": 1743580812345678901,
  "stats_window_s": 5,
  "rx_packets": 25000,
  "lost_packets_before_fec": 312,
  "lost_packets_after_fec": 4,
  "fec_recoveries": 308,
  "fec_failures": 4,
  "loss_pct_before_fec": 1.248,
  "loss_pct_after_fec": 0.016,
  "jitter_ms": 0.9,
  "key_epoch_current": 7,
  "nack_count": 2,
  "concealment_frames": 4
}
```

#### Key refresh request (unicast HTTPS to Key Server)

When a receiver loses the current epoch key (e.g. process restart), it re-authenticates to the Key Server via HTTPS and fetches the current epoch key. This is not a new frame type — it is a standard Key Server API call (§18.5.2).

### 18.9 MSS synchronization in FLUX/M

MSS synchronization in FLUX/M follows the same `GROUP_TIMESTAMP_NS` model as FLUX/QUIC, with the following adaptations:

- `SYNC_ANCHOR` frames are emitted on the **multicast channel** (not unicast) at the intervals defined in §6.4.
- FLUX/M senders MUST increase the `SYNC_ANCHOR` emission rate to **250 ms** in `ptp_software` mode (vs. 500 ms in FLUX/QUIC) because there is no per-receiver RTT measurement to detect clock drift.
- Receivers implement the same sync barrier (FRAME_SYNC / SAMPLE_SYNC / LINE_SYNC) as in FLUX/QUIC.
- The `estimated_drift_ppb` field in `SYNC_ANCHOR` is especially important in FLUX/M to allow receivers to extrapolate clock correction between anchors.

**Late-joining receivers:** A receiver joining mid-session MUST wait for the next `SYNC_ANCHOR` frame before presenting any output. The maximum wait time is bounded by the `SYNC_ANCHOR` interval (250 ms in software PTP). Receivers SHOULD begin buffering frames immediately upon joining, and start presenting output after the first `SYNC_ANCHOR` is received.

### 18.10 AMT tunneling (RFC 7450)

For receivers in networks without native IP multicast support (e.g. remote edit suites, cloud-based monitoring), FLUX/M supports **Automatic Multicast Tunneling (AMT)** per RFC 7450.

```
┌──────────────────────────────────────────────────────────────────────┐
│                                                                      │
│  Remote receiver (no native multicast)                               │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │  fluxmcastsrc (amt-relay=amt.facility.lan)                  │    │
│  │  ↓ sends AMT Relay Discovery (anycast 192.52.193.1)         │    │
│  └──────────────────┬──────────────────────────────────────────┘    │
│                     │ UDP unicast tunnel                             │
│  ┌──────────────────┴──────────────────────────────────────────┐    │
│  │  AMT Relay (facility or cloud)                              │    │
│  │  ↓ joins SSM group on behalf of remote receiver             │    │
│  └──────────────────┬──────────────────────────────────────────┘    │
│                     │ IP multicast (SSM)                            │
│  ┌──────────────────┴──────────────────────────────────────────┐    │
│  │  Multicast network (facility)                               │    │
│  └─────────────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────┘
```

**AMT operation:**

1. `fluxmcastsrc` sends an AMT Relay Discovery packet to the AMT anycast address (192.52.193.1 for IPv4, or a configured relay address).
1. The AMT relay responds with its unicast address.
1. `fluxmcastsrc` establishes an AMT pseudo-interface and sends IGMPv3 membership reports through it.
1. The AMT relay replicates the multicast stream to the receiver as UDP unicast.

**AMT latency overhead:** Typically 1–5 ms additional latency depending on the tunnel RTT. The FLUX/M framing and FEC are unaffected — the AMT relay passes packets transparently.

**`fluxmcastsrc` AMT configuration:**

```
amt-relay=192.168.1.200   (explicit relay; omit for anycast discovery)
amt-enabled=true
```

When `amt-relay` is not specified, `fluxmcastsrc` uses the IANA AMT anycast address for relay discovery. Operators SHOULD deploy a facility AMT relay for predictable latency.

### 18.11 FLUX/M ↔ FLUX/QUIC gateway

The `fluxmcastrelay` GStreamer element (§16) implements the gateway between FLUX/QUIC and FLUX/M. This enables the following deployment pattern:

```
FLUX/QUIC source (camera)
        │ unicast QUIC
        ▼
FLUX/QUIC → FLUX/M relay server
        │ UDP multicast (SSM)
        ├──────────────────────────────────────────────┐
        ▼                                              ▼
Monitor wall receiver 1 (FLUX/M)           Monitor wall receiver N (FLUX/M)
```

**Gateway responsibilities:**

1. **Ingest:** Receive the FLUX/QUIC stream from the upstream source. No transcoding — the encoded media bitstream is passed through unchanged.
1. **Reframing:** Repackage FLUX frames for FLUX/M UDP multicast delivery (apply AES-256-GCM group key, apply RaptorQ FEC, fragment to MTU).
1. **Session Descriptor publication:** Publish the FLUX/M Session Descriptor to the FLUX Registry.
1. **Key distribution:** Integrate with the FLUX Key Server to distribute epoch keys to authorized receivers.
1. **Feedback aggregation:** Collect `FLUXM_NACK` and `FLUXM_STAT` from multicast receivers and expose aggregated statistics via the FLUX Registry monitoring API.

**What the gateway does NOT do:**

- Transcode or re-encode media
- Convert FLUX-E assets (assets received via FLUX/QUIC are re-emitted on the multicast channel with the same `EMBED_MANIFEST` and `EMBED_CHUNK` frames)
- Implement CDBC (the upstream FLUX/QUIC session has its own CDBC; the multicast output is at a fixed configured rate)

**Bandwidth budget:**

```
Multicast bitrate = FLUX/QUIC received bitrate × (1 + fec_overhead_pct / 100)

Example: 15 Mbps video + 12 Mbps audio = 27 Mbps × 1.25 = 33.75 Mbps multicast
```

Operators MUST ensure the multicast network segment has sufficient capacity for the budgeted multicast bitrate across all active sessions.

### 18.12 FLUX/M discovery and Registry extension

The FLUX Registry (§7.2) is extended with FLUX/M-specific fields:

```
GET  /api/sd/{session_id}                    → FLUX/M Session Descriptor (§18.4)
GET  /api/sd/{session_id}/version            → { sd_version: N, updated_at: ... }
POST /api/sd/{session_id}                    → create/update SD (sender/relay)
GET  /api/sources/{session_id}/mcast-stats   → aggregated receiver statistics
WS   /api/events                             → includes FLUXM_SD_UPDATED events
```

**FLUXM_SD_UPDATED WebSocket event:**

```json
{
  "type": "fluxm_sd_updated",
  "session_id": "flux-m-cam-a-studio",
  "sd_version": 5,
  "updated_at": "2026-04-04T10:00:00Z",
  "changes": ["embed_catalog", "fec.repair_symbols_per_block"]
}
```

Receivers with an active WebSocket connection to the Registry SHOULD respond to `fluxm_sd_updated` events by immediately re-fetching the SD, rather than waiting for the next poll interval.

### 18.13 FLUX/M GStreamer pipeline examples

See §16 for the primary pipeline examples. Additional scenarios:

**Monitor wall with AMT tunnel (remote edit suite):**

```
fluxmcastsrc \
  sd-url=https://registry.lan:7500/api/sd/cam-a \
  amt-enabled=true \
  amt-relay=amt.facility.lan \
  feedback-enabled=true \
  ! fluxdemux name=d
d.video_0_0 ! fluxsync group=1 ! h265parse ! nvh265dec ! videoconvert ! autovideosink
d.audio_0   ! fluxsync group=1 ! audio/x-raw,format=F32LE ! autoaudiosink
```

**FLUX/QUIC → FLUX/M relay (standalone process):**

```
fluxsrc uri=flux://192.168.1.50:7400 crypto=crypto_quic \
  ! fluxmcastrelay \
      sd-url=https://registry.lan:7500/api/sd/cam-a-relay \
      mcast-group=239.100.1.1 \
      mcast-src=192.168.1.100 \
      mcast-port=7500 \
      fec-overhead-pct=25 \
      keyserver-url=https://keyserver.lan:7600 \
      feedback-port=7502
```

-----

## 19. Version negotiation and backwards compatibility

### v0.6.4 — Extended TYPE field, CDBC reframing, NMOS alignment, QUEEN-v1 to informative

**Extended TYPE mechanism (§4.1, §4.4):** The `EXT_TYPE` byte (offset 7) is used when `TYPE=0xF`. v0.6.3 senders set byte 7 to `0x00` for `EMBED_CHUNK` frames (which used `TYPE=0xF`). v0.6.4 senders set byte 7 to `0x0F` for `EMBED_CHUNK`. Receivers MUST accept both encodings:

- `TYPE=0xF` with byte 7 = `0x00` → `EMBED_CHUNK` (v0.6.3 compat)
- `TYPE=0xF` with byte 7 = `0x0F` → `EMBED_CHUNK` (v0.6.4 canonical)
- `TYPE=0xF` with byte 7 = `0x10`–`0x12` → FLUX/M extended types

v0.6.3 receivers encountering `TYPE=0xF` with byte 7 ≠ `0x00` MUST treat the frame as `EMBED_CHUNK` (they ignore byte 7 per the v0.6.3 RSVD rule). This provides full backward compatibility for FLUX/QUIC sessions. FLUX/M extended types (`0x10`–`0x12`) were already FLUX/M-only and never appeared in FLUX/QUIC streams.

**CDBC reframing (§5):** The §5 rewrite is editorial. No wire-format or handshake changes. The new §5.7 (time-scale separation, hysteresis) introduces normative implementation requirements for BW Governor behaviour that do not affect interoperability — they affect server-side scheduling decisions only.

**NMOS IS-04 alignment (§7.4):** The NMOS mapping is additive. The FLUX Registry API (§7.2) is unchanged. §7.4 defines how FLUX resources map to IS-04 objects for facilities that operate NMOS registries. No session-level changes.

**QUEEN-v1 to informative (§11.7, Annex D):** Moving QUEEN-v1 from normative §11.7 to informative Annex D does not change the wire format. The `gs_codec: "queen-v1"` identifier, `gs_codec_params` schema, anchor registration flow, and receiver FSM are preserved verbatim in Annex D. Implementations that supported QUEEN-v1 under v0.6.1–v0.6.3 remain fully interoperable.

**Jumbo frames REQUIRED (§18.2):** This is a deployment-time constraint change. v0.6.3 implementations on standard MTU networks at ≥ 10 Mbit/s continue to function but are now non-conformant. Operators SHOULD upgrade to jumbo frames.

**FLUX/R extracted (§20):** §20 is replaced with a normative reference to the separate *FLUX Recording Profile v1.0* document. The recording format itself is unchanged; only its location in the document set has changed.

**`flux_version` field:** Sessions using v0.6.4 features SHOULD advertise `"flux_version": "0.6.4"` in `FLUX_SESSION_REQUEST` and FLUX/M Session Descriptors.

-----

### v0.6.3 — `flux://` URI scheme for GLB video textures

The `flux://channel/{channel_id}` URI scheme is an **additive convention** inside GLB assets. It does not affect the FLUX wire protocol, handshake, or session descriptor schema.

**v0.6.2 and earlier receivers** loading a GLB that contains `flux://` URIs in `image.uri` fields: compliant glTF 2.0 parsers will treat the URI as an unresolvable external reference and fall back to the `bufferView` static image (if present) or to a missing-texture placeholder. No session error occurs.

**`feed_uri_override` delta operation:** This is an additive GLB delta type. v0.6.2 receivers that encounter an unknown `op` value in a delta frame MUST silently ignore it per the general FLUX delta parsing rule (§11.3). The live feed will remain bound to the original `channel_id` declared in the GLB URI.

**`flux_version` field:** Sessions using `flux://` URIs in GLB assets SHOULD advertise `"flux_version": "0.6.3"` in `FLUX_SESSION_REQUEST` and FLUX/M Session Descriptors.

-----

### v0.6.2 — FLUX/R Recording Profile

FLUX/R is an **additive offline profile**. It does not affect the live FLUX/QUIC or FLUX/M wire protocol in any way. There is no handshake negotiation for FLUX/R — it is a recording format consumed by dedicated recorder and packager components.

**Live FLUX/QUIC and FLUX/M receivers** connecting to a v0.6.2 server experience no change. The `flux_version` field in `FLUX_SESSION_REQUEST` and FLUX/M Session Descriptors SHOULD be set to `"0.6.2"` when a session is being recorded with FLUX/R, to signal to downstream tools that a compatible recording may exist.

**v0.6 and v0.6.1 tools** interoperating with FLUX/R components: The `.fluxmeta` JSON format uses forward-compatible field conventions; unknown fields MUST be silently ignored. FLUX/R production packages are readable by any MP4-compatible tool for the video/audio tracks; the timed metadata track and `.fluxmeta` sidecar require FLUX/R-aware software.

**Illustrative GStreamer elements in v0.6.2:** The FLUX/R specification (now published separately as *FLUX Recording Profile v1.0*) uses the element names `fluxrec`, `fluxassetrec`, `fluxmetaenc`, and `fluxassetenc` in its pipeline examples. These names are illustrative and are not part of the formal §16 element inventory. They have no effect on existing pipelines that do not include them.

-----

### v0.6.1 — GS Residual Codec Framework

**FLUX/QUIC clients connecting to v0.6.1 servers:**

- v0.6 receivers that do not declare `gs_codecs` in `embed_support` MUST be treated as supporting only `raw-attr`. The server MUST NOT send `application/vnd.flux.gs-residual` frames with `gs_codec: "queen-v1"` to such receivers.
- v0.6 receivers that do not include `application/vnd.flux.gs-residual` in their `embed_support.mime_types` MUST be treated identically.
- The server MAY continue sending `model/vnd.gaussian-splat` anchor frames to all receivers regardless of `gs_codecs` support, as these are standard GS assets.

**v0.6 clients connecting to v0.6.1 servers:**

- `gs_codec`, `gs_codec_params`, `anchor_asset_id`, `anchor_sha256` are additive JSON fields in `EMBED_MANIFEST`. v0.6 parsers MUST silently ignore unknown fields (per the general FLUX JSON parsing rule). A v0.6 receiver encountering `application/vnd.flux.gs-residual` without `gs_codecs` support SHOULD treat it as `application/octet-stream` and pass to application layer or discard.
- The `fluxgsresidualdec` element is new in v0.6.1. v0.6 GStreamer pipelines that do not include it will not decode GS residual streams, but the FLUX session itself will not fail — the residual `EMBED_CHUNK` frames will be silently dropped after `fluxembeddec` emits a `codec-not-supported` signal.

**`flux_version` field:** Sessions using GS Residual Codec Framework features SHOULD advertise `"flux_version": "0.6.1"` in `FLUX_SESSION_REQUEST` and FLUX/M Session Descriptors.

-----

### v0.6 — FLUX/M profile

FLUX/M is a new **operational profile**, not a version increment of the FLUX/QUIC handshake. There is no FLUX/QUIC handshake version negotiation for FLUX/M — compatibility is determined by the Session Descriptor `flux_version` field.

**FLUX/QUIC clients connecting to v0.6 FLUX/QUIC servers** experience no change. The `flux_profile` field in capabilities JSON (§3.2) is new but optional; servers treat its absence as `"flux_quic"`.

**FLUX/M receivers that do not support v0.6 features** (e.g. a v0.5-compatible receiver accessing FLUX/M via a custom implementation): The FLUX/M Session Descriptor carries `flux_version: "0.6"`. A receiver SHOULD check this field and warn if it does not support the declared version. Unknown fields in the SD MUST be silently ignored per the general FLUX JSON parsing rule.

**v0.5 FLUX/QUIC features in v0.6:** All v0.5 features (`video_texture_bindings`, `glb_texture_role`, `binding_control`) are unchanged in v0.6 and apply to both FLUX/QUIC and FLUX/M profiles.

### v0.5 clients connecting to v0.4 servers

(Unchanged from v0.5 spec.)

- The server will not return `video_texture_binding_support` in `SESSION_ACCEPT`.
- `video_texture_bindings` will not be populated in `EMBED_MANIFEST` responses.

### v0.4 clients connecting to v0.5/v0.6 servers

(Unchanged from v0.5 spec.)

- `video_texture_bindings`, `glb_texture_role`, `binding_control`, `flux_profile` are additive JSON fields and MUST be silently ignored.
- `FLUXM_KEY_EPOCH`, `FLUXM_NACK`, `FLUXM_STAT` frame types (0x10–0x12) are new; v0.4 clients MUST silently ignore unknown frame types.

### Mixed-version environments

In a facility deploying both FLUX/QUIC and FLUX/M:

- Receivers that do not implement FLUX/M (`flux_m` not in supported profiles) MUST use the FLUX/QUIC unicast path.
- The FLUX Registry `flux_profiles` field and FLUX/M `session_descriptor_url` enable operators to route receivers to the appropriate profile.
- A FLUX/M session and a FLUX/QUIC session carrying the same content MAY share `GROUP_ID` and `GROUP_TIMESTAMP_NS`, enabling seamless switching or side-by-side monitoring across profiles.

### v0.4 clients connecting to v0.3 servers (unchanged)

- The server will reject `crypto_none` — fall back to `crypto_quic`.
- Delta embed types not in server catalog — fall back to full asset transfers.
- `max_datagram_frame_size` absent → fall back to QUIC Streams for media.

-----

## 20. FLUX/R — Recording Profile (extracted)

> **v0.6.4 note:** The FLUX Recording Profile has been extracted from this specification into a separate document: *FLUX Recording Profile v1.0* [FLUX-R].

FLUX sessions MAY be recorded using the FLUX Recording Profile. Recording is an offline concern and does not affect the live FLUX/QUIC or FLUX/M wire protocol.

The FLUX/R specification defines:

- **Container format:** Fragmented MP4 (fMP4/CMAF) for video and audio essence.
- **Sidecar format:** `.fluxmeta` JSON temporal index for seek, asset resolution, and key lookup.
- **Asset recording:** GLB keyframe + delta model and GS anchor + residual model for random-access playback.
- **Two-moment storage architecture:** production storage (clear, for editing and QC) and distribution storage (CENC/cbcs encrypted, for CDN and OTT delivery).
- **Key derivation:** HKDF-SHA256 from a master CEK per GLB keyframe interval, deriving both CENC (128-bit) and asset (256-bit) operational keys.
- **Asset encryption envelope:** AES-256-GCM binary envelope (magic `0x464C5845` / “FLXE”) for GLB keyframe and delta files in distribution storage.

The FLUX/R specification normatively references ISO 23000-19 (CMAF), ISO 23001-7 (CENC), RFC 5869 (HKDF), and the respective DRM system specifications (Widevine, PlayReady, FairPlay). It does not redefine these standards.

**Normative reference:** [FLUX-R] LUCAB Media Technology, “FLUX Recording Profile v1.0”, 2026.

-----

## Appendix A — FLUX/M deployment checklist

|Item      |Action                                                                                                 |
|----------|-------------------------------------------------------------------------------------------------------|
|Network   |Confirm IGMPv3 snooping on all L2 switches in path                                                     |
|Network   |Confirm PIM-SSM configured on all L3 routers                                                           |
|Network   |Allocate SSM (S,G) pair from `239.0.0.0/8` (facility-scoped, RFC 2365) or `232.0.0.0/8` (SSM, RFC 4607)|
|Network   |Configure DSCP marking (EF recommended) for multicast traffic                                          |
|Network   |**Verify MTU ≥ 9000 (REQUIRED for streams ≥ 10 Mbit/s; see §18.2)**                                    |
|Key Server|Deploy FLUX Key Server with TLS 1.3 and OAuth 2.0                                                      |
|Key Server|Enroll sender and all authorized receivers as OAuth clients                                            |
|Registry  |Publish FLUX/M Session Descriptor via FLUX Registry                                                    |
|Registry  |Verify SD is accessible over HTTPS from all receiver locations                                         |
|Sender    |Configure `fluxmcastsink` or `fluxmcastrelay` with (S,G,port) and Key Server URL                       |
|Receiver  |Configure `fluxmcastsrc` with SD URL; verify IGMPv3 JOIN succeeds                                      |
|Receiver  |Verify epoch key retrieval from Key Server                                                             |
|Receiver  |Enable feedback channel if NACK/stats monitoring required                                              |
|Monitoring|Subscribe to `fluxm_sd_updated` WebSocket events on FLUX Registry                                      |
|Monitoring|Review `FLUXM_STAT` loss_pct_after_fec; if > 0.1% sustained, increase FEC overhead                     |

-----

## Appendix B — FLUX/M vs SMPTE 2110 comparison

|Feature                |SMPTE ST 2110                  |FLUX/M                              |
|-----------------------|-------------------------------|------------------------------------|
|Transport              |RTP/UDP multicast              |FLUX/UDP multicast                  |
|FEC                    |SMPTE ST 2022-5 (Pro-MPEG COP3)|RaptorQ (RFC 6330)                  |
|Encryption             |None (relies on network)       |AES-256-GCM (mandatory)             |
|Key management         |None                           |FLUX Key Server (OAuth 2.0)         |
|Multi-stream sync      |PTP (hardware required)        |PTP (software or hardware)          |
|Embedding              |No                             |FLUX-E (session assets)             |
|Discovery              |SDP/NMOS IS-04                 |DNS-SD + NMOS IS-04-aligned Registry|
|Tally                  |No                             |FLUX-T (JSON + compact binary)      |
|3D / VP assets         |No                             |GLB, USD, GS via FLUX-E             |
|Video texture binding  |No                             |Yes (§10.8 + `flux://` URI §10.10)  |
|Receiver access control|Network-level only             |OAuth 2.0 + epoch key rotation      |

FLUX/M is not a replacement for SMPTE 2110 in zero-latency baseband-replacement scenarios (studio production on dedicated 25/100GbE networks). FLUX/M targets contribution and distribution scenarios where encryption, asset embedding, and tally integration are required alongside multicast delivery.

-----

## Appendix C — Reference integration: Gracia Web SDK + QUEEN + FLUX (informative)

This appendix is non-normative and documents **one possible** integration that motivated the GS Residual Codec Framework amendment. The Gracia Web SDK is used here as a reference; other GS renderers and decoder runtimes are equally valid integration targets.

### Data flow

```
┌─────────────────────────────────────────────────────────────────────┐
│  Live volumetric capture pipeline                                    │
│                                                                      │
│  Multi-camera array → COLMAP reconstruction → 3DGS training         │
│  → QUEEN encoder (learned latent-decoder quantization)               │
│                                                                      │
│  Output:                                                             │
│    anchor.ply          (full canonical GS, ~50 MB)                  │
│    frame_XXXX.pkl      (quantized residuals, ~0.7 MB each)          │
└─────────────────────────────────────┬───────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────┐
│  FLUX Server (fluxsink / fluxmcastsink)                              │
│                                                                      │
│  EMBED_MANIFEST → mime: model/vnd.gaussian-splat (anchor)           │
│  EMBED_MANIFEST → mime: application/vnd.flux.gs-residual            │
│                          gs_codec: queen-v1                          │
│                          frame_assoc: { mode: sequence, step_* }    │
│  EMBED_CHUNKs (anchor) → QUIC Stream, priority: background          │
│  EMBED_CHUNKs (residual) → QUIC Datagram, priority: realtime        │
└─────────────────────────────────────┬───────────────────────────────┘
                                      │
                           QUIC / FLUX-E stream
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────┐
│  FLUX→WebTransport Gateway (edge node)                               │
│                                                                      │
│  fluxsrc → fluxdemux → fluxgsresidualdec                            │
│  Re-exposes GS anchor + residuals via Gracia streaming API           │
│  (streamingId + viewToken model, §Content Access in Gracia SDK)     │
└─────────────────────────────────────┬───────────────────────────────┘
                                      │
                           HTTP/WebTransport
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Gracia Web SDK — browser                                            │
│                                                                      │
│  GraciaWebCore.js (WASM) ← anchor.ply + residual .pkl per frame     │
│  QUEEN decoder → per-frame Gaussian attributes                       │
│  WebGPU rasterizer → display (flat / VR / AR)                       │
│                                                                      │
│  Targets: Chrome 113+, Meta Quest Browser, Apple Vision Pro         │
└─────────────────────────────────────────────────────────────────────┘
```

### Bandwidth budget at 30 fps

|Component                         |Bitrate                                                     |
|----------------------------------|------------------------------------------------------------|
|Video (H.265, 1080p HDR)          |~15 Mbit/s                                                  |
|Audio (PCM F32, 48 kHz stereo)    |~3 Mbit/s                                                   |
|GS anchor (50 MB, re-tx every 3 s)|~133 Mbit/s peak (burst, 3 s window) / ~1.3 Mbit/s amortized|
|GS residuals (0.7 MB × 30 fps)    |~168 Mbit/s                                                 |
|**Total sustained**               |**~186 Mbit/s**                                             |

The GS residual stream dominates. CDBC bandwidth reporting MUST account for the ~168 Mbit/s residual component as a fixed overhead. For FLUX/M deployments this is a multicast cost, not per-receiver — making FLUX/M the preferred transport for facilities with multiple simultaneous volumetric viewers.

Note: QUEEN GPU decoding is expected to run inside the application renderer (e.g. WASM/WebGPU in Gracia SDK, or native CUDA in server-side pipelines). `fluxgsresidualdec` outputs decoded Gaussian attribute tensors as `GstBuffer` with custom meta; the rasterization step is outside FLUX scope.

-----

## Annex D — QUEEN-v1 codec profile (informative)

> **Status: Informative.** This annex is not normative. The `queen-v1` codec identifier is registered in the GS Residual Codec Framework (§10.9) as an informative reference. Implementations MAY support QUEEN-v1; it is not required for FLUX conformance. The licensing terms of any QUEEN decoder implementation are outside FLUX scope and MUST be evaluated independently by implementors.

### D.1 Overview

QUEEN (QUantized Efficient ENcoding, NeurIPS 2024, Girish et al., NVIDIA Research) is a framework for efficient, streamable free-viewpoint video (FVV) using dynamic 3D Gaussians. QUEEN achieves high-quality dynamic scene reconstruction at approximately 0.7 MB per frame with real-time decoding at ≥350 FPS on GPU hardware.

QUEEN operates on the following model:

- **Anchor frame:** A full 3DGS scene in PLY (or SPZ/HAC) format. This is the canonical Gaussian set for the sequence. The anchor is transmitted as `model/vnd.gaussian-splat`.
- **Residual frames:** Per-frame quantized outputs from the learned latent-decoder, representing the latent-space difference between the anchor Gaussians and the current frame’s Gaussians. Each residual frame is transmitted as `application/vnd.flux.gs-residual` with `gs_codec: "queen-v1"`.
- **Decoder:** Applies quantized residuals to the anchor in latent space, reconstructing per-frame Gaussian attributes.

FLUX is agnostic to the QUEEN encoding algorithm. The QUEEN bitstream is opaque to FLUX transport. FLUX carries the anchor and residual frames as standard `EMBED_MANIFEST` + `EMBED_CHUNK` payloads, with the `gs_codec: "queen-v1"` identifier enabling correct receiver dispatch.

### D.2 `gs_codec_params` schema for `queen-v1`

When `gs_codec: "queen-v1"`, the `gs_codec_params` object in `EMBED_MANIFEST` SHOULD contain:

|Field            |Type   |Description                                                                                             |
|-----------------|-------|--------------------------------------------------------------------------------------------------------|
|`quant_bits`     |integer|Quantization bit depth (typically 8).                                                                   |
|`encoder_version`|string |QUEEN encoder version string, e.g. `"queen-1.0"`.                                                       |
|`sh_degree`      |integer|Spherical harmonics degree used during training (0–3).                                                  |
|`num_gaussians`  |integer|Number of Gaussians in the anchor frame’s canonical set. Receivers use this to pre-allocate GPU buffers.|
|`render_fps`     |number |Target render frame rate for this sequence, in frames per second.                                       |

All fields in `gs_codec_params` are OPTIONAL. Receivers MUST NOT fail if unknown fields are present. Receivers SHOULD use `num_gaussians` for GPU buffer pre-allocation when present.

### D.3 Delivery parameters

|Parameter                  |Value                                      |Notes                                           |
|---------------------------|-------------------------------------------|------------------------------------------------|
|Anchor MIME type           |`model/vnd.gaussian-splat`                 |PLY, SPZ, or HAC format                         |
|Anchor priority            |`background`                               |Must arrive complete before residuals begin     |
|Residual MIME type         |`application/vnd.flux.gs-residual`         |With `gs_codec: "queen-v1"`                     |
|Residual priority          |`realtime`                                 |QUIC Datagram, urgency 1                        |
|Residual `frame_assoc.mode`|`sequence`                                 |`step_index` and `step_ts_ns` REQUIRED          |
|Typical residual size      |~0.7 MB/frame                              |At default QUEEN quantization settings          |
|Typical aggregate bitrate  |~21 MB/s at 30 fps                         |Varies with scene complexity and quantization   |
|Residual compression       |`none` (QUEEN output is already compressed)|`zstd` MAY be applied; check `compression` field|

### D.4 Anchor keyframe retransmission interval

The server MUST re-transmit the anchor frame at a regular interval to enable late-joining or recovering receivers. This interval is the **anchor keyframe interval**.

|Transport |Maximum anchor keyframe interval            |
|----------|--------------------------------------------|
|FLUX/QUIC |10 seconds (or on explicit receiver request)|
|**FLUX/M**|**3 seconds (mandatory; no ARQ available)** |

The anchor re-transmission MUST use the same `asset_id` and `sha256` as the original. Receivers that already hold the anchor (verified by SHA-256 match) MUST silently ignore the re-transmission body but MUST reset their `ANCHOR_MISMATCH` state if they had previously discarded residuals due to a missing anchor.

When QUEEN recomputes a new anchor frame (scene change, model re-training), the server MUST use a new `asset_id` and `sha256`. Receivers MUST flush their cached anchor and pending residuals immediately on receipt of a new anchor `EMBED_MANIFEST` with a different `sha256`.

**FLUX/M anchor delivery:** In FLUX/M, anchor transmission SHOULD use its own dedicated RaptorQ FEC block with ≥ 50% repair overhead, distinct from the media FEC parameters. Anchor re-transmission SHOULD be scheduled during low-demand intervals (e.g. immediately after I-frame boundaries of the video track) to reduce FEC contention with media traffic. The `anchor_prefetch_window_ms` minimum MUST be at least `(anchor_size_bytes / link_bitrate_bps) × 1.5` to provide headroom for FEC recovery.

### D.5 Receiver state machine

```
       ┌─────────────────────────────────────────────────────────┐
       │                  QUEEN-v1 Receiver FSM                  │
       └─────────────────────────────────────────────────────────┘

  [IDLE] ─── EMBED_MANIFEST (anchor) received ──► [FETCHING_ANCHOR]
                                                         │
                                              All EMBED_CHUNKs received
                                              SHA-256 verified
                                                         │
                                                         ▼
                                                 [ANCHOR_READY]
                                                         │
                                        EMBED_MANIFEST (residual) received
                                        anchor_sha256 matches cached anchor
                                                         │
                                                         ▼
                                                  [DECODING] ◄──────────────────┐
                                                         │                       │
                                          EMBED_CHUNK (residual, datagram)       │
                                          Decode: apply residuals to anchor       │
                                          Rasterize → display                    │
                                                         │                       │
                                                         │  next residual frame  │
                                                         └───────────────────────┘
                                                         │
                                              anchor_sha256 MISMATCH
                                              or new anchor EMBED_MANIFEST
                                                         │
                                                         ▼
                                               [ANCHOR_MISMATCH]
                                          Discard residuals; await new anchor
                                                         │
                                              New anchor EMBED_MANIFEST
                                                         │
                                                         ▼
                                                [FETCHING_ANCHOR]
```

State transition requirements (informative — implementations SHOULD follow these):

- In state `FETCHING_ANCHOR`: receiver SHOULD buffer any arriving residual `EMBED_MANIFEST` frames but SHOULD NOT begin decoding.
- In state `ANCHOR_MISMATCH`: receiver SHOULD discard all `application/vnd.flux.gs-residual` EMBED_CHUNKs until a new anchor is confirmed ready.
- In state `DECODING`: a lost residual datagram SHOULD be silently skipped. The decoder continues with the next available residual frame. The decoder SHOULD NOT attempt interpolation or partial application of a partially received residual.
- The `EMBED_ACK` for the anchor serves as the trigger for the `FETCHING_ANCHOR` → `ANCHOR_READY` transition in FLUX/QUIC. In FLUX/M this transition occurs after the `anchor_prefetch_window_ms` has elapsed and SHA-256 is verified.

### D.6 FLUX/M — `gs_codec` declaration in Session Descriptor

In FLUX/M, GS residual codec information is declared in the session descriptor `embed_catalog` entry:

```json
"embed_catalog": [
  {
    "asset_id": "queen-anchor-seq01",
    "mime_type": "model/vnd.gaussian-splat",
    "sha256": "a3f2c1...",
    "fetch_url": "https://registry.lan:7500/assets/queen-anchor-seq01.ply",
    "size_bytes": 52428800,
    "anchor_prefetch_window_ms": 5000,
    "gs_sequence": {
      "sequence_id": "queen-seq-live-01",
      "gs_codec": "queen-v1",
      "gs_codec_params": {
        "quant_bits": 8,
        "encoder_version": "queen-1.0",
        "sh_degree": 3,
        "num_gaussians": 120000,
        "render_fps": 30
      },
      "keyframe_interval_s": 3,
      "residual_bitrate_kbps": 168000
    }
  }
]
```

### D.7 Interaction with CDBC (FLUX/QUIC)

QUEEN residual frames at ~0.7 MB/frame and 30 fps impose approximately 168 Mbit/s on the media path. The BW Governor SHOULD account for residual frame bandwidth as a fixed-overhead component, separate from video and audio media bitrates. The BW Governor SHOULD NOT shed GS residual frames as a congestion response; instead it SHOULD reduce video enhancement layers first, following the standard EMERGENCY sequence (§5.4). GS residual delivery has implicit `priority: realtime` and SHOULD be treated equivalently to base-layer video for CDBC shedding purposes.

### D.8 QUEEN packet density considerations (FLUX/M)

A 0.7 MB/frame residual at 30 fps generates approximately 486 datagrams/frame at MTU 1500, or ~14,580 pps. This does not align cleanly with K=64 RaptorQ blocks (approximately 7.6 blocks per frame). Implementations SHOULD:

- Configure K to divide evenly into the expected number of datagrams per frame where possible.
- Schedule anchor re-transmission using dedicated FEC blocks with ≥ 50% repair overhead.
- Use jumbo frames (MTU ≥ 9000) to reduce packet count to ~78 datagrams/frame, which aligns better with small source block sizes.

-----

*FLUX v0.6.4 — LUCAB Media Technology — draft for internal review*