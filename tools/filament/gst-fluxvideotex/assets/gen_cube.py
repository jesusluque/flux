#!/usr/bin/env python3
"""
gen_cube.py — Generate a minimal GLB cube whose baseColorTexture
references flux://channel/0 (FLUX Protocol Spec v0.6.3 §10.10).

A 1×1 grey PNG is embedded as a fallback bufferView so standard glTF
viewers can still display the mesh.

Output: cube.glb  (in the same directory as this script)
"""

import json
import struct
import zlib
import os

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
OUT_PATH   = os.path.join(SCRIPT_DIR, "cube.glb")

# ---------------------------------------------------------------------------
# Geometry: unit cube, 24 verts (4 per face), UV 0..1 per face, 36 indices
# Each vertex: POSITION (3×f32) + TEXCOORD_0 (2×f32) = 20 bytes
# ---------------------------------------------------------------------------

faces = [
    # (normal-dir comment)  4 verts: (x,y,z, u,v)
    # +Z  front
    [(-1,-1, 1, 0,0),( 1,-1, 1, 1,0),( 1, 1, 1, 1,1),(-1, 1, 1, 0,1)],
    # -Z  back
    [( 1,-1,-1, 0,0),(-1,-1,-1, 1,0),(-1, 1,-1, 1,1),( 1, 1,-1, 0,1)],
    # +X  right
    [( 1,-1, 1, 0,0),( 1,-1,-1, 1,0),( 1, 1,-1, 1,1),( 1, 1, 1, 0,1)],
    # -X  left
    [(-1,-1,-1, 0,0),(-1,-1, 1, 1,0),(-1, 1, 1, 1,1),(-1, 1,-1, 0,1)],
    # +Y  top
    [(-1, 1, 1, 0,0),( 1, 1, 1, 1,0),( 1, 1,-1, 1,1),(-1, 1,-1, 0,1)],
    # -Y  bottom
    [(-1,-1,-1, 0,0),( 1,-1,-1, 1,0),( 1,-1, 1, 1,1),(-1,-1, 1, 0,1)],
]

vertex_data = b""
for face in faces:
    for (x, y, z, u, v) in face:
        vertex_data += struct.pack("<5f", x, y, z, u, v)

index_data = b""
for fi in range(6):
    base = fi * 4
    for i in [0,1,2, 2,3,0]:
        index_data += struct.pack("<H", base + i)

# Pad index buffer to 4-byte alignment
while len(index_data) % 4 != 0:
    index_data += b"\x00"

# ---------------------------------------------------------------------------
# Minimal 1×1 grey PNG fallback image (raw PNG bytes, no external dep)
# ---------------------------------------------------------------------------
def make_1x1_grey_png():
    def png_chunk(tag, data):
        crc = zlib.crc32(tag + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)

    sig   = b"\x89PNG\r\n\x1a\n"
    ihdr  = png_chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0))
    # RGBA pixel (128,128,128,255) compressed
    raw   = b"\x00\x80\x80\x80\xff"   # filter byte 0 + R G B A
    comp  = zlib.compress(raw, 9)
    idat  = png_chunk(b"IDAT", comp)
    iend  = png_chunk(b"IEND", b"")
    return sig + ihdr + idat + iend

fallback_png = make_1x1_grey_png()

# Pad to 4-byte alignment
png_padded = fallback_png
while len(png_padded) % 4 != 0:
    png_padded += b"\x00"

# ---------------------------------------------------------------------------
# Build binary buffer: [vertex_data][index_data][png_padded]
# ---------------------------------------------------------------------------
vert_offset  = 0
vert_len     = len(vertex_data)
idx_offset   = vert_len
idx_len      = len(index_data)
png_offset   = idx_offset + idx_len
png_len      = len(png_padded)

bin_buffer = vertex_data + index_data + png_padded
buf_len    = len(bin_buffer)

# ---------------------------------------------------------------------------
# Build glTF JSON
# ---------------------------------------------------------------------------
gltf = {
    "asset": {"version": "2.0", "generator": "FLUX poc003 gen_cube.py"},
    "scene": 0,
    "scenes": [{"nodes": [0]}],
    "nodes": [{"mesh": 0, "name": "Cube"}],
    "meshes": [{
        "name": "Cube",
        "primitives": [{
            "attributes": {
                "POSITION":    0,
                "TEXCOORD_0":  1
            },
            "indices":   2,
            "material":  0
        }]
    }],
    # KHR_materials_unlit: baseColor is output directly, no lighting math.
    # This ensures the video texture is visible without any light source.
    "extensionsUsed": ["KHR_materials_unlit"],
    "extensionsRequired": ["KHR_materials_unlit"],
    "materials": [{
        "name": "VideoMaterial",
        "pbrMetallicRoughness": {
            "baseColorTexture": {"index": 0},
            "metallicFactor":  0.0,
            "roughnessFactor": 1.0
        },
        "extensions": {
            "KHR_materials_unlit": {}
        },
        "doubleSided": True
    }],
    "textures": [{"source": 0}],
    # §10.10: flux:// URI + static fallback bufferView
    "images": [{
        "name":       "live_video",
        "uri":        "flux://channel/0",
        "mimeType":   "image/png",
        "bufferView": 2
    }],
    "accessors": [
        # 0 — POSITION
        {
            "bufferView":    0,
            "byteOffset":    0,
            "componentType": 5126,   # FLOAT
            "count":         24,
            "type":          "VEC3",
            "min": [-1.0, -1.0, -1.0],
            "max": [ 1.0,  1.0,  1.0]
        },
        # 1 — TEXCOORD_0
        {
            "bufferView":    0,
            "byteOffset":    12,     # 3 floats * 4 bytes
            "componentType": 5126,
            "count":         24,
            "type":          "VEC2"
        },
        # 2 — indices
        {
            "bufferView":    1,
            "byteOffset":    0,
            "componentType": 5123,   # UNSIGNED_SHORT
            "count":         36,
            "type":          "SCALAR"
        }
    ],
    "bufferViews": [
        # 0 — vertex (interleaved POSITION+TEXCOORD_0, stride=20)
        {
            "buffer":     0,
            "byteOffset": vert_offset,
            "byteLength": vert_len,
            "byteStride": 20,
            "target":     34962   # ARRAY_BUFFER
        },
        # 1 — index
        {
            "buffer":     0,
            "byteOffset": idx_offset,
            "byteLength": len(index_data),   # unpadded length
            "target":     34963   # ELEMENT_ARRAY_BUFFER
        },
        # 2 — fallback PNG image
        {
            "buffer":     0,
            "byteOffset": png_offset,
            "byteLength": len(fallback_png)  # unpadded length
        }
    ],
    "buffers": [{"byteLength": buf_len}]
}

json_bytes = json.dumps(gltf, separators=(',',':')).encode("utf-8")
# glTF JSON chunk must be padded to 4-byte boundary with spaces (0x20)
while len(json_bytes) % 4 != 0:
    json_bytes += b" "

# ---------------------------------------------------------------------------
# Assemble GLB  (spec: https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#glb-file-format-specification)
# ---------------------------------------------------------------------------
JSON_CHUNK_TYPE = 0x4E4F534A   # "JSON"
BIN_CHUNK_TYPE  = 0x004E4942   # "BIN\0"

json_chunk = struct.pack("<II", len(json_bytes), JSON_CHUNK_TYPE) + json_bytes
bin_chunk  = struct.pack("<II", buf_len, BIN_CHUNK_TYPE) + bin_buffer

total_len = 12 + len(json_chunk) + len(bin_chunk)
header = struct.pack("<III", 0x46546C67, 2, total_len)   # magic "glTF", version 2

glb = header + json_chunk + bin_chunk

with open(OUT_PATH, "wb") as f:
    f.write(glb)

print(f"Written {len(glb)} bytes → {OUT_PATH}")
print(f"  vertices : {len(vertex_data)//20} (stride 20, interleaved POS+UV)")
print(f"  indices  : 36")
print(f"  image[0] : flux://channel/0  (fallback PNG: {len(fallback_png)} bytes)")
