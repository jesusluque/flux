# FLUX Protocol — Proof of Concept

A Rust + C/C++ implementation of the [FLUX Protocol](spec/FLUX_Protocol_Spec_v0_6_3_EN.md) (v0.6.3) — a low-latency, multi-stream media-transport protocol over QUIC/TLS.

Three proof-of-concept demonstrations are included:

| PoC | What it shows |
|-----|---------------|
| **poc001** | Single-stream unicast: H.265 video over QUIC with CDBC feedback and FLUX-C upstream control |
| **poc002** | Four-stream 2×2 mosaic using the Multi-Stream Synchronisation (MSS) barrier (§6.3) |
| **poc003** | `fluxvideotex` (§16): live video frame applied as a GPU texture on a Filament 3D cube |

**Platform:** macOS (Apple Silicon / x86_64)  
**Full documentation:** [docs/DOCUMENTATION.md](docs/DOCUMENTATION.md)

---

## Architecture

All PoCs share a common Rust workspace of seven GStreamer plugins:

```
tools/gstreamer/
├── flux-framing/       # Shared wire-format library (no GStreamer dep)
├── gst-fluxframer/     # Server-side FLUX framer   (BaseTransform)
├── gst-fluxdeframer/   # Client-side FLUX deframer (BaseTransform)
├── gst-fluxsink/       # Server-side QUIC sender   (BaseSink, quinn)
├── gst-fluxsrc/        # Client-side QUIC receiver (PushSrc, quinn)
├── gst-fluxdemux/      # FLUX frame router          (Element, dynamic pads)
├── gst-fluxcdbc/       # CDBC observer             (BaseTransform passthrough)
└── gst-fluxsync/       # MSS sync barrier          (BaseTransform)
```

poc003 additionally uses a C/C++ Filament offscreen renderer:

```
tools/filament/
└── gst-fluxvideotex/
    ├── fluxvideotex.c       # GStreamer BaseTransform (C)
    └── filament_scene.cpp   # Headless OpenGL Filament renderer (C++)
```

---

## Prerequisites

- Rust toolchain (stable, 1.75+)
- GStreamer 1.22+ with development headers (`gstreamer`, `gstreamer-base`, `gstreamer-video`, `gstreamer-app`)
- macOS GStreamer plugins: `gst-plugins-good`, `gst-plugins-bad` (for `vtenc_h265`, `vtdec_hw`, `osxvideosink`)
- CMake 3.20+ (poc003 only)
- Python 3 with `pygltflib` (to regenerate `cube.glb` — pre-generated copy included)

---

## Quick Start

### poc001 — Single-Stream Unicast

```bash
# Build all GStreamer plugins
cd tools/gstreamer && cargo build --release && cd ../..

# Terminal 1 — server (binds QUIC on port 7400)
cd poc001 && cargo run --bin server --release

# Terminal 2 — client (connects to 127.0.0.1:7400)
cd poc001 && cargo run --bin client --release
```

**Client keyboard controls:**

| Key | Action |
|-----|--------|
| Space | Pause / resume |
| T | Cycle test pattern on server (FLUX-C) |
| P | Send PTZ preset (FLUX-C) |
| A | Toggle audio mute (FLUX-C) |
| L / l | NetSim loss +5% / -5% |
| Y / y | NetSim delay +20ms / -20ms |
| B / b | NetSim bandwidth cap +/-1000 kbps |
| Q | Quit |

### poc002 — Four-Stream Mosaic

```bash
cd tools/gstreamer && cargo build --release && cd ../..

# Terminal 1 — 4 server streams on ports 7400–7403
cd poc002 && cargo run --bin multi-server --release

# Terminal 2 — mosaic client (2×2 compositor)
cd poc002 && cargo run --bin mosaic-client --release
```

**Server keyboard controls:** 1–4 select stream, `+`/`-` inject delay (10 ms steps), `R` reset, `S` status, `Q` quit.

**Client keyboard controls:** Space pause/resume, `S` print sync stats, `Q` quit.

The `clockoverlay` on each tile shows wall-clock time with centisecond precision (`HH:MM:SS.cc`). A well-synchronised mosaic shows the **same** centisecond digit on all four tiles. Use the server's `+` key to inject delay on individual streams and observe the MSS barrier compensate.

### poc003 — fluxvideotex 3D Cube

```bash
# Build Filament plugin
cd tools/filament
cmake -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build -j$(nproc)

# Build poc003 binary
cd ../../poc003
cmake -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build -j$(nproc)
./build/poc003
```

Runs for 300 seconds. The window shows a rotating 3D cube with the live `videotestsrc` SMPTE bars rendered as a GPU texture on all six faces.

---

## Protocol Coverage

| Spec section | Feature | PoC |
|---|---|---|
| §3.1–§3.2 | SESSION handshake (JSON over QUIC Stream 0) | poc001, poc002, poc003 |
| §3.3 | KEEPALIVE / session-dead detection | poc001, poc002 |
| §4.1 | 32-byte FLUX header | all |
| §4.3 | FLAGS bits (keyframe, end-of-stream, metadata) | all |
| §4.4 | Frame types (MediaData, CDBC, KEEPALIVE, StreamAnnounce…) | all |
| §5.1–§5.2 | CDBC adaptive feedback (50 ms / 10 ms under loss) | poc001, poc002 |
| §5.3–§5.4 | BwGovernor state machine (PROBE→STABLE→RAMP) | poc001, poc002 |
| §6.3 | MSS Sync Barrier (`fluxsync`) | poc002 |
| §10.8 | `video_texture_bindings` | poc003 |
| §10.10.2 | `flux://channel/0` URI in GLB | poc003 |
| §10.10.3 | `bufferView` fallback PNG | poc003 |
| §12 | FLUX-C upstream control commands | poc001 |
| §16 | `fluxvideotex` element | poc003 |

---

## Key Design Notes

- **GROUP_TIMESTAMP_NS snapping:** Server pipelines snap buffer DTS to a 33 ms grid (`floor(dts + 16.7ms) / 33.3ms * 33.3ms`) so all four streams assign the same timestamp to the same logical frame — the key required by `fluxsync`.
- **400 ms PTS offset:** `fluxdeframer` stamps `pts = rt_anchor + delta_ns + 400ms`, giving the decoder and compositor headroom. `compositor min-upstream-latency` must be set to the same value.
- **stream-per-AU:** Each compressed Access Unit is sent on its own short-lived unidirectional QUIC stream (not QUIC datagrams), giving reliable, ordered, flow-controlled delivery per AU.
- **KHR_materials_unlit:** The cube GLB uses this glTF extension to bypass PBR lighting and output the video texture directly. Without it, the cube renders black under the UbershaderProvider.
- **readPixels async:** Filament `readPixels` is asynchronous. The renderer spins `pumpMessageQueues()` (not `execute()`, which is a no-op on macOS) with 100 µs sleep until the DMA readback callback sets an atomic flag.
