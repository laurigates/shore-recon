#!/usr/bin/env python3
"""Render a seamless looping showcase video of a Gaussian splat, with a stats sidebar, as AV1.

Ported from the M0 spike's spike/m0/remote/render_orbit.py (ADR-0002 decision 6, ADR-0008, #29).

    render_video <splat.ply> <sparse_dir> <meta.json> <out.mp4>
                 [--path arc|drone] [--seconds S] [--fps N] [--size WxH] [--sidebar PX]
                 [--fov-scale F] [--pullback F] [--share <share.mp4>] [--share-max-mib 30]

Inputs
  splat.ply   INRIA-layout Gaussian splat (Brush export): x y z, f_dc_*, f_rest_*, opacity,
              scale_*, rot_*.
  sparse_dir  COLMAP binary model (cameras.bin, images.bin) of undistorted PINHOLE or
              SIMPLE_PINHOLE cameras: the splat stage's training cameras. The first camera's
              intrinsics set the focal length.
  meta.json   Sidebar content. Keys:
                title     str, required.
                subtitle  str, optional (default "").
                kind      "masked" | "unmasked", optional (default "masked"); sets the accent colour.
                badges    [[text, [r, g, b]], ...], optional (default []).
                sections  [[name, [[label, value], [label, value, ok], ...]], ...], optional
                          (default []). A row with a third element draws a green (true) or red
                          (false) status dot.
              The script appends a "Camera" section describing the path. Unknown keys are ignored.

The camera path depends only on the cameras and the arguments, so the same inputs give the same
frames. The video is encoded with SVT-AV1; --share also writes a 1280 px wide copy and fails if it
is larger than --share-max-mib. Every failure exits non-zero; there is no CPU fallback.
"""
import argparse
import json
import math
import os
import shutil
import struct
import subprocess
import sys
from pathlib import Path

import numpy as np


def die(msg):
    raise SystemExit(f"render_video: {msg}")


def parse_args(argv=None):
    ap = argparse.ArgumentParser(prog="render_video", description=__doc__.split("\n\n")[0])
    ap.add_argument("ply", type=Path, help="splat PLY")
    ap.add_argument("model", type=Path, help="COLMAP binary model directory (sparse/0)")
    ap.add_argument("meta", type=Path, help="sidebar content (JSON, see module docstring)")
    ap.add_argument("out", type=Path, help="output .mp4")
    ap.add_argument("--path", choices=["arc", "drone"], default="arc")
    ap.add_argument("--seconds", type=float, default=16)
    ap.add_argument("--fps", type=int, default=30)
    ap.add_argument("--size", default="1920x1080", help="WxH, both even")
    ap.add_argument("--sidebar", type=int, default=600, help="sidebar width in px")
    ap.add_argument("--fov-scale", type=float, default=0.8, help="<1 zooms out")
    ap.add_argument("--pullback", type=float, default=0.35,
                    help="drone path: step back, as a fraction of scene distance")
    ap.add_argument("--share", type=Path, help="also write a 1280 px wide copy here")
    ap.add_argument("--share-max-mib", type=float, default=30)
    a = ap.parse_args(argv)
    try:
        a.W, a.H = map(int, a.size.lower().split("x"))
    except ValueError:
        ap.error(f"--size must be WxH, got {a.size!r}")
    if a.W % 2 or a.H % 2 or a.sidebar % 2:
        ap.error("--size and --sidebar must be even (yuv420p)")
    if not 0 <= a.sidebar < a.W:
        ap.error("--sidebar must be smaller than the width")
    if a.seconds <= 0 or a.fps <= 0 or a.fov_scale <= 0:
        ap.error("--seconds, --fps and --fov-scale must be positive")
    a.frames = round(a.seconds * a.fps)
    if a.frames < 2:
        ap.error("the video needs at least two frames")
    return a


# ---------- cameras (COLMAP binary model) ----------
# model_id -> (name, number of params)
CAMERA_MODELS = {0: ("SIMPLE_PINHOLE", 3), 1: ("PINHOLE", 4)}


def read_cameras_bin(path):
    """{camera_id: (width, height, fx, fy, cx, cy)} for pinhole models; fails on any other model."""
    cams = {}
    with open(path, "rb") as f:
        (count,) = struct.unpack("<Q", f.read(8))
        for _ in range(count):
            cam_id, model_id, w, h = struct.unpack("<IiQQ", f.read(24))
            if model_id not in CAMERA_MODELS:
                die(f"{path}: camera {cam_id} has model id {model_id}; expected undistorted "
                    "PINHOLE (1) or SIMPLE_PINHOLE (0)")
            name, n = CAMERA_MODELS[model_id]
            p = struct.unpack(f"<{n}d", f.read(8 * n))
            cams[cam_id] = (w, h, p[0], p[0], p[1], p[2]) if name == "SIMPLE_PINHOLE" else (w, h, *p)
        if f.read(1):
            die(f"{path}: trailing bytes after {count} cameras")
    return cams


def read_images_bin(path):
    """[(name, camera_id, R, t)] with cam_from_world rotation R and translation t."""
    images = []
    with open(path, "rb") as f:
        (count,) = struct.unpack("<Q", f.read(8))
        for _ in range(count):
            _img_id, qw, qx, qy, qz, tx, ty, tz, cam_id = struct.unpack("<I7dI", f.read(64))
            name = bytearray()
            while (c := f.read(1)) != b"\0":
                if not c:
                    die(f"{path}: truncated image name")
                name += c
            (n_pts,) = struct.unpack("<Q", f.read(8))
            f.seek(24 * n_pts, os.SEEK_CUR)  # x, y (f64), point3D_id (u64)
            images.append((name.decode(), cam_id, qvec2rot((qw, qx, qy, qz)), np.array([tx, ty, tz])))
        if f.read(1):
            die(f"{path}: trailing bytes after {count} images")
    return images


def qvec2rot(q):
    w, x, y, z = q
    return np.array([[1 - 2 * (y * y + z * z), 2 * (x * y - w * z), 2 * (x * z + w * y)],
                     [2 * (x * y + w * z), 1 - 2 * (x * x + z * z), 2 * (y * z - w * x)],
                     [2 * (x * z - w * y), 2 * (y * z + w * x), 1 - 2 * (x * x + y * y)]])


def load_cameras(d):
    for f in ("cameras.bin", "images.bin"):
        if not (d / f).is_file():
            die(f"{d / f} not found (expected a COLMAP binary model)")
    cams = read_cameras_bin(d / "cameras.bin")
    images = read_images_bin(d / "images.bin")
    if len(images) < 2:
        die(f"{d}: need at least two registered images, got {len(images)}")
    for name, cam_id, _, _ in images:
        if cam_id not in cams:
            die(f"{d}: image {name} references missing camera {cam_id}")
    images.sort(key=lambda im: im[0])  # frame order; makes the path independent of file order
    intrinsics = cams[images[0][1]]
    return intrinsics, [(name, R, t) for name, _, R, t in images]


# ---------- camera paths ----------
SWEEP = 0.85  # fraction of the filmed azimuth range (10th-90th percentile) the arc covers


def scene_target(C, F):
    """Least-squares point closest to every camera's optical axis."""
    A = sum(np.eye(3) - np.outer(f, f) for f in F)
    return np.linalg.solve(A, sum((np.eye(3) - np.outer(f, f)) @ c for f, c in zip(F, C)))


def view_matrix(right, down, fwd, pos):
    Rc2w = np.stack([right, down, fwd], 1)
    V = np.eye(4)
    V[:3, :3] = Rc2w.T
    V[:3, 3] = -Rc2w.T @ pos
    return V


def arc_path(poses, n):
    C = np.array([-R.T @ t for _, R, t in poses])
    fwd = np.array([R.T @ [0, 0, 1] for _, R, _ in poses])
    # A level gimbal keeps every camera's x axis horizontal, so world up is the direction
    # orthogonal to all of them; the mean camera up only fixes its sign (it tilts with pitch).
    rights = np.array([R.T @ [1, 0, 0] for _, R, _ in poses])
    up = np.linalg.svd(rights)[2][-1]
    if up @ -np.mean([R.T @ [0, 1, 0] for _, R, _ in poses], axis=0) < 0:
        up = -up
    target = scene_target(C, fwd)
    v = C - target
    h = v @ up
    p = v - np.outer(h, up)
    e1 = p.mean(0)
    e1 /= np.linalg.norm(e1)
    e2 = np.cross(up, e1)
    az = np.arctan2(p @ e2, p @ e1)
    lo, hi = np.percentile(az, [10, 90])
    r, hh = np.median(np.linalg.norm(p, axis=1)), np.median(h)
    views = []
    for i in range(n):
        ang = (lo + hi) / 2 + SWEEP * (hi - lo) / 2 * math.sin(2 * math.pi * i / n)
        pos = target + r * (math.cos(ang) * e1 + math.sin(ang) * e2) + hh * up
        f = target - pos
        f /= np.linalg.norm(f)
        right = np.cross(f, up)
        right /= np.linalg.norm(right)
        views.append(view_matrix(right, np.cross(f, right), f, pos))
    return views, f"{math.degrees(SWEEP * (hi - lo)):.1f}° arc"


def smooth(x, sigma):
    """Gaussian smoothing along axis 0 with reflected edges."""
    r = min(int(3 * sigma), len(x) - 1)
    k = np.exp(-0.5 * (np.arange(-r, r + 1) / sigma) ** 2)
    k /= k.sum()
    xp = np.concatenate([x[r:0:-1], x, x[-2:-r - 2:-1]])
    return np.stack([np.convolve(xp[:, j], k, mode="valid") for j in range(x.shape[1])], 1)


def drone_path(poses, n, pullback, sigma=2.5):
    """The recorded flight (poses in frame order), smoothed, played forward then back.
    pullback moves each camera back along its view direction by that fraction of the median
    camera-to-scene-centre distance."""
    C0 = np.array([-R.T @ t for _, R, t in poses])
    F0 = np.array([R.T @ [0, 0, 1] for _, R, _ in poses])
    back = pullback * np.median(np.linalg.norm(C0 - scene_target(C0, F0), axis=1))
    C, F = smooth(C0, sigma), smooth(F0, sigma)
    X = smooth(np.array([R.T @ [1, 0, 0] for _, R, _ in poses]), sigma)
    views = []
    for i in range(n):
        u = (1 - math.cos(2 * math.pi * i / n)) / 2 * (len(poses) - 1)  # 0 -> end -> 0, eased
        j = int(min(u, len(poses) - 2))
        w = u - j
        pos, f, x = ((1 - w) * A[j] + w * A[j + 1] for A in (C, F, X))
        f = f / np.linalg.norm(f)
        down = np.cross(f, x)
        down /= np.linalg.norm(down)
        views.append(view_matrix(np.cross(down, f), down, f, pos - back * f))
    return views, "drone path"


# ---------- sidebar ----------
FONT_DIR = os.environ.get("SHORELINE_FONT_DIR", "/usr/share/fonts/truetype/noto")
FONTS = {k: f"{FONT_DIR}/{v}" for k, v in
         {"Bold": "NotoSans-Bold.ttf", "Regular": "NotoSans-Regular.ttf",
          "Mono": "NotoSansMono-Regular.ttf"}.items()}
ACCENT = {"masked": (20, 184, 166), "unmasked": (245, 158, 11)}
GOOD, BAD = (34, 197, 94), (239, 68, 68)


def load_meta(path):
    try:
        m = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as e:
        die(f"cannot read {path}: {e}")
    if not isinstance(m, dict) or not isinstance(m.get("title"), str):
        die(f"{path}: needs an object with a string 'title'")
    meta = dict(title=m["title"], subtitle=m.get("subtitle", ""), kind=m.get("kind", "masked"),
                badges=m.get("badges", []), sections=[[s, list(rows)] for s, rows in m.get("sections", [])])
    if meta["kind"] not in ACCENT:
        die(f"{path}: kind must be one of {sorted(ACCENT)}, got {meta['kind']!r}")
    return meta


def panel(meta, s=1.0):
    from PIL import Image, ImageDraw, ImageFont

    f = lambda style, px: ImageFont.truetype(FONTS[style], int(px * s))
    title, sub, sect, lab = f("Bold", 30), f("Regular", 19), f("Bold", 15), f("Regular", 18)
    val, badge = f("Mono", 18), f("Bold", 14)
    pad, row_h, width = int(22 * s), int(27 * s), int(540 * s)
    rows = [("title", meta["title"])]
    if meta["subtitle"]:
        rows.append(("sub", meta["subtitle"]))
    if meta["badges"]:
        rows.append(("badges", meta["badges"]))
    for name, items in meta["sections"]:
        rows.append(("section", name))
        rows += [("row", it) for it in items]
    heights = {"title": 42, "sub": 30, "badges": 40, "section": 34, "row": 27}
    height = pad * 2 + sum(heights[k] for k, _ in rows) * s
    img = Image.new("RGBA", (width, int(height)), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([0, 0, width - 1, height - 1], radius=int(16 * s), fill=(10, 12, 16, 215),
                        outline=(255, 255, 255, 38), width=max(1, int(s)))
    accent = ACCENT[meta["kind"]]
    d.rounded_rectangle([0, 0, int(6 * s), height - 1], radius=int(3 * s), fill=accent + (255,))
    y = pad
    for kind, x in rows:
        if kind == "title":
            d.text((pad, y), x, font=title, fill=(255, 255, 255))
        elif kind == "sub":
            d.text((pad, y), x, font=sub, fill=(180, 186, 196))
        elif kind == "badges":
            bx = pad
            for text, color in x:
                tw = d.textlength(text, font=badge)
                d.rounded_rectangle([bx, y + 4 * s, bx + tw + 18 * s, y + 30 * s], radius=int(13 * s),
                                    fill=tuple(color) + (255,))
                d.text((bx + 9 * s, y + 9 * s), text, font=badge, fill=(10, 12, 16))
                bx += tw + 28 * s
        elif kind == "section":
            y += 8 * s
            d.text((pad, y), x.upper(), font=sect, fill=accent)
            d.line([pad + d.textlength(x.upper(), font=sect) + 10 * s, y + 10 * s, width - pad, y + 10 * s],
                   fill=(255, 255, 255, 40), width=1)
            y -= 8 * s
        else:
            label, value, *status = x
            value = str(value)
            d.text((pad, y), str(label), font=lab, fill=(160, 166, 178))
            vx = width - pad - d.textlength(value, font=val)
            if status:
                d.ellipse([vx - 18 * s, y + 8 * s, vx - 8 * s, y + 18 * s], fill=GOOD if status[0] else BAD)
            d.text((vx, y), value, font=val, fill=(255, 255, 255))
        y += heights[kind] * s
    return img


# ---------- splat + rasterizer ----------
def load_splat(path, dev):
    import torch
    from plyfile import PlyData

    v = PlyData.read(str(path))["vertex"]
    col = lambda n: np.asarray(v[n], dtype=np.float32)
    n_rest = sum(1 for p in v.properties if p.name.startswith("f_rest_"))
    k = n_rest // 3 + 1
    if n_rest % 3 or math.isqrt(k) ** 2 != k:
        die(f"{path}: {n_rest} f_rest_* properties is not a valid SH layout")
    sh = np.zeros((len(v), k, 3), np.float32)
    sh[:, 0] = np.stack([col(f"f_dc_{i}") for i in range(3)], -1)
    if n_rest:  # INRIA layout: channel-major, f_rest_[c * (k-1) + j]
        rest = np.stack([col(f"f_rest_{i}") for i in range(n_rest)], -1).reshape(-1, 3, k - 1)
        sh[:, 1:] = rest.transpose(0, 2, 1)
    t = lambda x: torch.from_numpy(np.ascontiguousarray(x)).to(dev)
    return dict(
        means=t(np.stack([col("x"), col("y"), col("z")], -1)),
        quats=torch.nn.functional.normalize(t(np.stack([col(f"rot_{i}") for i in range(4)], -1)), dim=-1),
        scales=torch.exp(t(np.stack([col(f"scale_{i}") for i in range(3)], -1))),
        opacities=torch.sigmoid(t(col("opacity"))),
        colors=t(sh), sh_degree=math.isqrt(k) - 1, count=len(v))


def require_cuda():
    """Fail unless a CUDA device and gsplat's compiled extension are both usable."""
    import torch

    if not torch.cuda.is_available():
        die("CUDA is not available (run under apptainer --nv); refusing to render on CPU")
    from gsplat.cuda import _backend

    if _backend._C is None:
        die("gsplat's CUDA extension is not available")
    return "cuda"


# ---------- encoding ----------
def ffmpeg_bin():
    exe = shutil.which("ffmpeg")
    if not exe:
        die("ffmpeg not found on PATH")
    return exe


def encode(frames, a, out):
    """Pipe raw RGB frames into SVT-AV1; writes out atomically."""
    tmp = out.with_name(out.stem + ".partial.mp4")
    enc = subprocess.Popen([ffmpeg_bin(), "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24",
                            "-s", f"{a.W}x{a.H}", "-r", str(a.fps), "-i", "-", "-c:v", "libsvtav1",
                            "-preset", "4", "-crf", "30", "-g", str(8 * a.fps), "-pix_fmt", "yuv420p",
                            "-movflags", "+faststart", "-f", "mp4", str(tmp)], stdin=subprocess.PIPE)
    try:
        for fr in frames:
            enc.stdin.write(fr.tobytes())
        enc.stdin.close()
    except BrokenPipeError:
        pass  # ffmpeg exited; its return code below says why
    if enc.wait() != 0:
        tmp.unlink(missing_ok=True)
        die(f"ffmpeg failed with exit {enc.returncode}")
    tmp.replace(out)


def share_copy(src, dst, max_mib):
    """1280 px wide copy (as `just share-copy`); fails if it exceeds max_mib."""
    tmp = dst.with_name(dst.stem + ".partial.mp4")
    r = subprocess.run([ffmpeg_bin(), "-v", "error", "-y", "-i", str(src), "-map", "0:v",
                        "-map_chapters", "0", "-map_metadata", "0", "-vf", "scale=1280:-2:flags=lanczos",
                        "-c:v", "libsvtav1", "-preset", "6", "-crf", "38", "-g", "60", "-pix_fmt", "yuv420p",
                        "-movflags", "+faststart", "-f", "mp4", str(tmp)])
    if r.returncode != 0:
        tmp.unlink(missing_ok=True)
        die(f"ffmpeg (share copy) failed with exit {r.returncode}")
    mib = tmp.stat().st_size / 1048576
    if mib > max_mib:
        tmp.unlink()
        die(f"share copy is {mib:.1f} MiB, over the {max_mib:g} MiB limit")
    tmp.replace(dst)
    return mib


# ---------- main ----------
def main(argv=None):
    a = parse_args(argv)
    meta = load_meta(a.meta)
    (_cw, ch, _fx, fy, _cx, _cy), poses = load_cameras(a.model)
    for p in FONTS.values():
        if not Path(p).is_file():
            die(f"font {p} not found (set SHORELINE_FONT_DIR)")

    import torch
    from PIL import Image

    dev = require_cuda()
    torch.set_grad_enabled(False)
    from gsplat import rasterization

    sp = load_splat(a.ply, dev)

    if a.path == "arc":
        views, label = arc_path(poses, a.frames)
    else:
        views, label = drone_path(poses, a.frames, a.pullback)
    meta["sections"].append(["Camera", [["Path", label], ["Loop", f"{a.seconds:g} s · {a.fps} fps"]]])

    # Sidebar on the left, splat view on the right. Focal from the training camera, scaled to the
    # view height; fov_scale < 1 widens the field of view (zooms out).
    VW = a.W - a.sidebar
    f = fy * a.H / ch * a.fov_scale
    K = torch.tensor([[[f, 0, VW / 2], [0, f, a.H / 2], [0, 0, 1]]], dtype=torch.float32, device=dev)
    side = Image.new("RGB", (a.sidebar, a.H), (10, 12, 16))
    if a.sidebar:
        ov = panel(meta, s=a.H / 1080)
        if ov.width > a.sidebar or ov.height > a.H:
            die(f"sidebar content is {ov.width}x{ov.height} px, larger than {a.sidebar}x{a.H}")
        side.paste(ov, ((a.sidebar - ov.width) // 2, (a.H - ov.height) // 2), ov)
    side = np.asarray(side)

    def frames():
        for V in views:
            img, _, _ = rasterization(sp["means"], sp["quats"], sp["scales"], sp["opacities"], sp["colors"],
                                      torch.tensor(V, dtype=torch.float32, device=dev)[None], K, VW, a.H,
                                      sh_degree=sp["sh_degree"])  # default background is black
            view = (img[0].clamp(0, 1).cpu().numpy() * 255).astype(np.uint8)
            yield np.concatenate([side, view], axis=1)

    a.out.parent.mkdir(parents=True, exist_ok=True)
    encode(frames(), a, a.out)
    print(f"render_video: {a.out} {a.frames} frames, {label}, {sp['count']} splats, "
          f"{a.out.stat().st_size / 1048576:.1f} MiB")
    if a.share:
        a.share.parent.mkdir(parents=True, exist_ok=True)
        print(f"render_video: {a.share} {share_copy(a.out, a.share, a.share_max_mib):.1f} MiB")


if __name__ == "__main__":
    sys.exit(main())
