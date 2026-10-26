"""SPZ (Niantic) encoder/decoder for Brush/INRIA splat PLYs, version 3 (gzip, smallest-three
quaternions) by default. A line-by-line port of nianticlabs/spz v3.0.0 src/cc/load-spz.cc
(packGaussians, serializePackedGaussians, unpack), with the same defaults as its ply_to_spz:
coordinates UNSPECIFIED (stored as in the PLY), fractionalBits 12, sh1Bits 5, shRestBits 4.

    spz.py encode <in.ply> <out.spz> [--version 2|3]
    spz.py check <in.ply> <out.spz>     # decode out.spz, compare with in.ply; exit 1 past the limits
    spz.py info <file.spz>

Not built from Niantic's C++ (the tool is a port): the decoder reads Niantic's v2 samples
(samples/*.spz at v3.0.0) consistently, and `check` holds every round trip to the format's
quantisation limits (see LIMITS).
"""
import gzip
import struct
import sys

import numpy as np

MAGIC = 0x5053474E
# Round-trip limits `check` enforces: half a position step (1/4096) plus float slack, the
# smallest-three rotation quantisation, half an 8-bit opacity step plus slack.
LIMITS = dict(position=0.5 / 4096 + 1e-5, rotation_deg=0.5, opacity=0.0025)
COLOR_SCALE = 0.15
SQRT1_2 = np.float32(0.707106781186547524401)


def read_splat_ply(path):
    data = open(path, "rb").read()
    end = data.index(b"end_header\n") + len(b"end_header\n")
    props, count = [], None
    for line in data[:end].decode().splitlines():
        w = line.split()
        if w[:2] == ["element", "vertex"]:
            count = int(w[2])
        elif w[:2] == ["property", "float"]:
            props.append(w[2])
        elif w[:1] == ["property"]:
            raise SystemExit(f"{path}: non-float property {line!r}")
    rows = np.frombuffer(data[end:], dtype=np.float32, count=count * len(props)).reshape(count, len(props))
    col = {p: rows[:, i] for i, p in enumerate(props)}
    n_rest = sum(p.startswith("f_rest_") for p in props)
    sh_dim = n_rest // 3
    g = dict(
        positions=np.stack([col["x"], col["y"], col["z"]], 1),
        scales=np.stack([col[f"scale_{i}"] for i in range(3)], 1),
        rotations=np.stack([col["rot_1"], col["rot_2"], col["rot_3"], col["rot_0"]], 1),  # xyzw
        alphas=col["opacity"],
        colors=np.stack([col[f"f_dc_{i}"] for i in range(3)], 1),
        # [N, C, S] -> [N, S, C]
        sh=np.stack([np.stack([col[f"f_rest_{c * sh_dim + j}"] for c in range(3)], 1)
                     for j in range(sh_dim)], 1) if sh_dim else np.zeros((count, 0, 3), np.float32),
    )
    degree = {0: 0, 3: 1, 8: 2, 15: 3, 24: 4}[sh_dim]
    return g, degree


def to_u8(x):
    return np.clip(np.round(x), 0, 255).astype(np.uint8)


def quantize_sh(x, bucket):
    q = (np.round(x * 128.0) + 128.0).astype(np.int32)
    q = (q + bucket // 2) // bucket * bucket
    return np.clip(q, 0, 255).astype(np.uint8)


def pack_smallest_three(q):
    q = q / np.linalg.norm(q, axis=1, keepdims=True)
    largest = np.argmax(np.abs(q), axis=1)
    negate = q[np.arange(len(q)), largest] < 0
    comp = largest.astype(np.uint32)
    for i in range(4):
        keep = largest != i
        negbit = ((q[:, i] < 0) ^ negate).astype(np.uint32)
        mag = (np.float32(511.0) * (np.abs(q[:, i]) / SQRT1_2) + np.float32(0.5)).astype(np.uint32)
        comp = np.where(keep, (comp << 10) | (negbit << 9) | mag, comp)
    return comp.astype("<u4").view(np.uint8).reshape(-1, 4)


def pack_first_three(q):
    q = q / np.linalg.norm(q, axis=1, keepdims=True)
    q = q * np.where(q[:, 3:4] < 0, -127.5, 127.5) + 127.5
    return to_u8(q[:, :3])


def encode(g, degree, version=3):
    n = len(g["alphas"])
    fixed = np.round(g["positions"].astype(np.float64) * (1 << 12)).astype(np.int32)
    pos = np.stack([fixed & 0xFF, (fixed >> 8) & 0xFF, (fixed >> 16) & 0xFF], -1).astype(np.uint8)
    scales = to_u8((g["scales"] + 10.0) * 16.0)
    rot = pack_smallest_three(g["rotations"]) if version >= 3 else pack_first_three(g["rotations"])
    alphas = to_u8(1 / (1 + np.exp(-g["alphas"].astype(np.float64))) * 255.0)
    colors = to_u8(g["colors"] * (COLOR_SCALE * 255.0) + (0.5 * 255.0))
    sh = np.zeros((n, 0), np.uint8)
    if degree:
        s = g["sh"]
        sh = np.concatenate([quantize_sh(s[:, :3], 1 << (8 - 5)),
                             quantize_sh(s[:, 3:], 1 << (8 - 4))], 1).reshape(n, -1)
    header = struct.pack("<IIIBBBB", MAGIC, version, n, degree, 12, 0, 0)
    body = b"".join(a.tobytes() for a in (pos, alphas, colors, scales, rot, sh))
    return gzip.compress(header + body, compresslevel=6)


def decode(blob):
    d = gzip.decompress(blob)
    magic, version, n, degree, frac, flags, _ = struct.unpack("<IIIBBBB", d[:16])
    if magic != MAGIC or version not in (2, 3):
        raise SystemExit(f"not a gzip SPZ v2/v3 (magic {magic:#x}, version {version})")
    sh_dim = {0: 0, 1: 3, 2: 8, 3: 15, 4: 24}[degree]
    rot_bytes = 4 if version >= 3 else 3
    sizes = [n * 9, n, n * 3, n * 3, n * rot_bytes, n * sh_dim * 3]
    if 16 + sum(sizes) != len(d):
        raise SystemExit(f"length {len(d)} != expected {16 + sum(sizes)}")
    off = 16
    parts = []
    for s in sizes:
        parts.append(np.frombuffer(d, np.uint8, s, off))
        off += s
    pos_b, alpha_b, color_b, scale_b, rot_b, sh_b = parts
    p = pos_b.reshape(n, 3, 3).astype(np.int32)
    fixed = p[..., 0] | (p[..., 1] << 8) | (p[..., 2] << 16)
    fixed = np.where(fixed & 0x800000, fixed | ~0xFFFFFF, fixed)
    positions = fixed.astype(np.float32) / (1 << frac)
    scales = scale_b.reshape(n, 3) / 16.0 - 10.0
    a = alpha_b / 255.0
    alphas = np.log(np.clip(a, 1e-6, 1 - 1e-6) / (1 - np.clip(a, 1e-6, 1 - 1e-6)))
    colors = (color_b.reshape(n, 3) / 255.0 - 0.5) / COLOR_SCALE
    if version >= 3:
        comp = rot_b.reshape(n, 4).copy().view("<u4").reshape(n).astype(np.int64)
        largest = comp >> 30
        q = np.zeros((n, 4))
        rest = comp.copy()
        for i in (3, 2, 1, 0):
            take = largest != i
            mag = (rest & 0x1FF) / 511.0 * float(SQRT1_2)
            neg = (rest >> 9) & 1
            q[:, i] = np.where(take, np.where(neg == 1, -mag, mag), q[:, i])
            rest = np.where(take, rest >> 10, rest)
        q[np.arange(n), largest] = np.sqrt(np.clip(1 - (q ** 2).sum(1), 0, 1))
    else:
        xyz = rot_b.reshape(n, 3) / 127.5 - 1.0
        q = np.concatenate([xyz, np.sqrt(np.clip(1 - (xyz ** 2).sum(1), 0, 1))[:, None]], 1)
    sh = (sh_b.reshape(n, sh_dim, 3).astype(np.float32) - 128.0) / 128.0
    return dict(version=version, degree=degree, flags=flags, positions=positions, scales=scales,
                rotations=q, alphas=alphas, colors=colors, sh=sh)


def main():
    cmd = sys.argv[1]
    if cmd == "encode":
        version = int(sys.argv[sys.argv.index("--version") + 1]) if "--version" in sys.argv else 3
        g, degree = read_splat_ply(sys.argv[2])
        blob = encode(g, degree, version)
        open(sys.argv[3], "wb").write(blob)
        print(f"{sys.argv[3]}: {len(g['alphas'])} gaussians, SH degree {degree}, v{version}, {len(blob) / 1e6:.1f} MB")
    elif cmd == "info":
        s = decode(open(sys.argv[2], "rb").read())
        n = len(s["alphas"])
        qn = np.linalg.norm(s["rotations"], axis=1)
        print(f"v{s['version']} {n} gaussians SH{s['degree']} flags {s['flags']}; "
              f"pos range {s['positions'].min(0).round(2)}..{s['positions'].max(0).round(2)}; "
              f"|q| {qn.min():.3f}..{qn.max():.3f}; scale(log) {s['scales'].min():.2f}..{s['scales'].max():.2f}; "
              f"opacity median {np.median(1 / (1 + np.exp(-s['alphas']))):.2f}")
    elif cmd == "check":
        g, degree = read_splat_ply(sys.argv[2])
        s = decode(open(sys.argv[3], "rb").read())
        q0 = g["rotations"] / np.linalg.norm(g["rotations"], axis=1, keepdims=True)
        dot = np.abs((q0 * s["rotations"]).sum(1))
        ang = np.degrees(2 * np.arccos(np.clip(dot, 0, 1)))
        dpos = np.abs(s["positions"] - g["positions"]).max()
        op0 = 1 / (1 + np.exp(-g["alphas"]))
        op1 = 1 / (1 + np.exp(-s["alphas"]))
        col = np.abs(np.clip(s["colors"], -3.3, 3.3) - np.clip(g["colors"], -3.3, 3.3))
        inrange = np.abs(g["positions"]).max() < (1 << 23) / (1 << 12)
        print(f"positions max error {dpos:.5f} (step {1 / 4096:.5f}; all within 24-bit range: {inrange})")
        print(f"rotation error deg: median {np.median(ang):.3f}, p99 {np.percentile(ang, 99):.3f}, max {ang.max():.3f}")
        print(f"opacity max error {np.abs(op1 - op0).max():.4f}; colour (in range) p99 error {np.percentile(col, 99):.4f}")
        if degree:
            dsh = np.abs(s["sh"] - np.clip(g["sh"], -1, 127 / 128))
            print(f"SH p99 error {np.percentile(dsh, 99):.4f} (bucket 1/16 = 0.0625 for degree 2+)")
        over = [k for k, v in (("position", dpos), ("rotation_deg", ang.max()),
                               ("opacity", np.abs(op1 - op0).max())) if v > LIMITS[k]]
        if over or not inrange:
            raise SystemExit(f"round trip past the limits: {over or 'positions out of 24-bit range'}")


if __name__ == "__main__":
    main()
