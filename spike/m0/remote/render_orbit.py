# Render a seamless looping video of a Brush splat with an info sidebar, encoded to AV1.
# Usage: python render_orbit.py <splat.ply> <colmap_txt_dir> <meta.json> <out.mp4>
#        [--path arc|drone] [--seconds 16] [--size 1920x1080] [--sidebar 600] [--fov-scale 0.8]
#        [--ffmpeg PATH] [--still N]   (--still N: write frame N as PNG next to out instead)
#        [--check]                     (render training view 0 only, as PNG)
import argparse, json, math, os, subprocess, sys
from pathlib import Path

import numpy as np
import torch
from gsplat import rasterization
from PIL import Image, ImageDraw, ImageFont
from plyfile import PlyData

ap = argparse.ArgumentParser()
ap.add_argument("ply", type=Path)
ap.add_argument("model", type=Path)
ap.add_argument("meta", type=Path)
ap.add_argument("out", type=Path)
ap.add_argument("--path", choices=["arc", "drone"], default="arc")
ap.add_argument("--seconds", type=float, default=16)
ap.add_argument("--fps", type=int, default=30)
ap.add_argument("--size", default="1920x1080")
ap.add_argument("--sidebar", type=int, default=600)
ap.add_argument("--fov-scale", type=float, default=0.8, help="<1 zooms out")
ap.add_argument("--pullback", type=float, default=0.35, help="drone path: step back, as a fraction of scene distance")
ap.add_argument("--ffmpeg", default="ffmpeg")
ap.add_argument("--still", type=int)
ap.add_argument("--check", action="store_true")
a = ap.parse_args()
RAW = None
if str(a.out) == "-":  # keep the real stdout for frames; send every other write (incl. C level) to stderr
    RAW = os.fdopen(os.dup(1), "wb")
    os.dup2(2, 1)
W, H = map(int, a.size.split("x"))
dev = "cuda"


# ---------- splat ----------
def load_splat(path):
    v = PlyData.read(str(path))["vertex"]
    col = lambda n: np.asarray(v[n], dtype=np.float32)
    n_rest = sum(1 for p in v.properties if p.name.startswith("f_rest_"))
    k = n_rest // 3 + 1
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
        colors=t(sh), sh_degree=int(math.isqrt(k)) - 1, count=len(v))


# ---------- cameras (COLMAP TXT, PINHOLE after undistortion) ----------
def qvec2rot(q):
    w, x, y, z = q
    return np.array([[1 - 2 * (y * y + z * z), 2 * (x * y - w * z), 2 * (x * z + w * y)],
                     [2 * (x * y + w * z), 1 - 2 * (x * x + z * z), 2 * (y * z - w * x)],
                     [2 * (x * z - w * y), 2 * (y * z + w * x), 1 - 2 * (x * x + y * y)]])


def load_cameras(d):
    cam = next(l.split() for l in open(d / "cameras.txt") if l.strip() and not l.startswith("#"))
    assert cam[1] == "PINHOLE", f"expected undistorted PINHOLE camera, got {cam[1]}"
    w, h, fx, fy, cx, cy = int(cam[2]), int(cam[3]), *map(float, cam[4:8])
    lines = [l for l in open(d / "images.txt") if l.strip() and not l.startswith("#")]
    poses = []
    for l in lines[::2]:  # each image is two lines; the second holds 2D points
        f = l.split()
        poses.append((f[9], qvec2rot(list(map(float, f[1:5]))), np.array(list(map(float, f[5:8])))))
    poses.sort(key=lambda p: p[0])
    return (w, h, fx, fy, cx, cy), poses


SWEEP = 0.85  # fraction of the filmed azimuth range (10th-90th percentile) the arc covers


def arc_path(poses, n):
    C = np.array([-R.T @ t for _, R, t in poses])
    fwd = np.array([R.T @ [0, 0, 1] for _, R, _ in poses])
    # A level gimbal keeps every camera's x axis horizontal, so world up is the direction
    # orthogonal to all of them; the mean camera up only fixes its sign (it tilts with pitch).
    rights = np.array([R.T @ [1, 0, 0] for _, R, _ in poses])
    up = np.linalg.svd(rights)[2][-1]
    if up @ -np.mean([R.T @ [0, 1, 0] for _, R, _ in poses], axis=0) < 0:
        up = -up
    A = sum(np.eye(3) - np.outer(f, f) for f in fwd)  # point closest to all optical axes
    target = np.linalg.solve(A, sum((np.eye(3) - np.outer(f, f)) @ c for f, c in zip(fwd, C)))
    v = C - target
    h = v @ up
    p = v - np.outer(h, up)
    e1 = p.mean(0); e1 /= np.linalg.norm(e1)
    e2 = np.cross(up, e1)
    az = np.arctan2(p @ e2, p @ e1)
    lo, hi = np.percentile(az, [10, 90])
    r, hh = np.median(np.linalg.norm(p, axis=1)), np.median(h)
    views = []
    for i in range(n):
        ang = (lo + hi) / 2 + SWEEP * (hi - lo) / 2 * math.sin(2 * math.pi * i / n)
        pos = target + r * (math.cos(ang) * e1 + math.sin(ang) * e2) + hh * up
        f = target - pos; f /= np.linalg.norm(f)
        right = np.cross(f, up); right /= np.linalg.norm(right)
        down = np.cross(f, right)
        Rc2w = np.stack([right, down, f], 1)
        V = np.eye(4); V[:3, :3] = Rc2w.T; V[:3, 3] = -Rc2w.T @ pos
        views.append(V)
    info = dict(arc_deg=round(math.degrees(SWEEP * (hi - lo)), 1), n_cams=len(poses))
    return views, info


def smooth(x, sigma):
    """Gaussian smoothing along axis 0 with reflected edges."""
    r = int(3 * sigma)
    k = np.exp(-0.5 * (np.arange(-r, r + 1) / sigma) ** 2); k /= k.sum()
    xp = np.concatenate([x[r:0:-1], x, x[-2:-r - 2:-1]])
    return np.stack([np.convolve(xp[:, j], k, mode="valid") for j in range(x.shape[1])], 1)


def drone_path(poses, n, pullback, sigma=2.5):
    """The recorded flight (poses in frame order), smoothed, played forward then back.
    pullback moves each camera back along its view direction by that fraction of the median
    camera-to-scene-centre distance."""
    C0 = np.array([-R.T @ t for _, R, t in poses])
    F0 = np.array([R.T @ [0, 0, 1] for _, R, _ in poses])
    A = sum(np.eye(3) - np.outer(f, f) for f in F0)
    target = np.linalg.solve(A, sum((np.eye(3) - np.outer(f, f)) @ c for f, c in zip(F0, C0)))
    back = pullback * np.median(np.linalg.norm(C0 - target, axis=1))
    C, F = smooth(C0, sigma), smooth(F0, sigma)
    X = smooth(np.array([R.T @ [1, 0, 0] for _, R, _ in poses]), sigma)
    views = []
    for i in range(n):
        u = (1 - math.cos(2 * math.pi * i / n)) / 2 * (len(poses) - 1)  # 0 -> end -> 0, eased
        j, w = int(min(u, len(poses) - 2)), u - int(min(u, len(poses) - 2))
        lerp = lambda A: (1 - w) * A[j] + w * A[j + 1]
        pos, f, x = lerp(C), lerp(F), lerp(X)
        f /= np.linalg.norm(f)
        down = np.cross(f, x); down /= np.linalg.norm(down)
        right = np.cross(down, f)
        pos = pos - back * f
        Rc2w = np.stack([right, down, f], 1)
        V = np.eye(4); V[:3, :3] = Rc2w.T; V[:3, 3] = -Rc2w.T @ pos
        views.append(V)
    idx =[int(Path(name).stem) for name, _, _ in poses]  # frames were extracted at 2 fps
    flight_s = (max(idx) - min(idx)) / 2
    return views, dict(flight_s=flight_s, n_cams=len(poses))


# ---------- overlay panel ----------
FONT_DIR = os.environ.get("SHORELINE_FONT_DIR", "/usr/share/fonts/truetype/noto")
FONT = FONT_DIR + "/NotoSans-{}.ttf"
MONO = FONT_DIR + "/NotoSansMono-Regular.ttf"
ACCENT = {"masked": (20, 184, 166), "unmasked": (245, 158, 11)}
GOOD, BAD = (34, 197, 94), (239, 68, 68)


def panel(meta, s=1.0):
    f = lambda style, px: ImageFont.truetype(FONT.format(style), int(px * s))
    title, sub, sect, lab = f("Bold", 30), f("Regular", 19), f("Bold", 15), f("Regular", 18)
    val, badge = ImageFont.truetype(MONO, int(18 * s)), f("Bold", 14)
    pad, row_h, width = int(22 * s), int(27 * s), int(540 * s)
    rows = []  # (kind, payload)
    rows.append(("title", meta["title"]))
    rows.append(("sub", meta["subtitle"]))
    rows.append(("badges", meta["badges"]))
    for name, items in meta["sections"]:
        rows.append(("section", name))
        rows += [("row", it) for it in items]
    height = pad * 2 + sum({"title": 42, "sub": 30, "badges": 40, "section": 34, "row": 27}[k] for k, _ in rows) * s
    img = Image.new("RGBA", (width, int(height)), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([0, 0, width - 1, height - 1], radius=int(16 * s), fill=(10, 12, 16, 215),
                        outline=(255, 255, 255, 38), width=max(1, int(s)))
    accent = ACCENT[meta["kind"]]
    d.rounded_rectangle([0, 0, int(6 * s), height - 1], radius=int(3 * s), fill=accent + (255,))
    y = pad
    for kind, x in rows:
        if kind == "title":
            d.text((pad, y), x, font=title, fill=(255, 255, 255)); y += 42 * s
        elif kind == "sub":
            d.text((pad, y), x, font=sub, fill=(180, 186, 196)); y += 30 * s
        elif kind == "badges":
            bx = pad
            for text, color in x:
                tw = d.textlength(text, font=badge)
                d.rounded_rectangle([bx, y + 4 * s, bx + tw + 18 * s, y + 30 * s], radius=int(13 * s), fill=tuple(color) + (255,))
                d.text((bx + 9 * s, y + 9 * s), text, font=badge, fill=(10, 12, 16))
                bx += tw + 28 * s
            y += 40 * s
        elif kind == "section":
            y += 8 * s
            d.text((pad, y), x.upper(), font=sect, fill=accent)
            d.line([pad + d.textlength(x.upper(), font=sect) + 10 * s, y + 10 * s, width - pad, y + 10 * s],
                   fill=(255, 255, 255, 40), width=1)
            y += 26 * s
        else:
            label, value, *status = x
            d.text((pad, y), label, font=lab, fill=(160, 166, 178))
            vx = width - pad - d.textlength(value, font=val)
            if status:
                c = GOOD if status[0] else BAD
                d.ellipse([vx - 18 * s, y + 8 * s, vx - 8 * s, y + 18 * s], fill=c)
            d.text((vx, y), value, font=val, fill=(255, 255, 255))
            y += row_h
    return img


# ---------- main ----------
torch.set_grad_enabled(False)
sp = load_splat(a.ply)
(cw, ch, fx, fy, cx, cy), poses = load_cameras(a.model)
meta = json.loads(a.meta.read_text())
if str(a.out) != "-":
    a.out.parent.mkdir(parents=True, exist_ok=True)


def render(V, w, h, K):
    img, _, _ = rasterization(sp["means"], sp["quats"], sp["scales"], sp["opacities"], sp["colors"],
                              torch.tensor(V, dtype=torch.float32, device=dev)[None],
                              torch.tensor(K, dtype=torch.float32, device=dev)[None], w, h,
                              sh_degree=sp["sh_degree"])  # default background is black
    return (img[0].clamp(0, 1).cpu().numpy() * 255).astype(np.uint8)


if a.check:  # training view 0 at its own intrinsics, scaled to width W
    s = W / cw
    _, R, t = poses[0]
    V = np.eye(4); V[:3, :3] = R; V[:3, 3] = t
    Image.fromarray(render(V, W, round(ch * s), [[fx * s, 0, cx * s], [0, fy * s, cy * s], [0, 0, 1]])).save(a.out.with_suffix(".check.png"))
    print("check", poses[0][0], "splats", sp["count"])
    raise SystemExit

n = round(a.seconds * a.fps)
if a.path == "arc":
    views, info = arc_path(poses, n)
    cam = f"{info['arc_deg']}° arc · {a.seconds:.0f} s loop"
else:
    views, info = drone_path(poses, n, a.pullback)
    cam = f"drone path · {2 * info['flight_s'] / a.seconds:.1f}× · {a.seconds:.0f} s loop"
meta["sections"][-1][1].append(["Camera", cam])

# Sidebar on the left, splat view on the right. Focal from the training camera, scaled to the
# view height; fov_scale < 1 widens the field of view (zooms out).
VW = W - a.sidebar
f = fy * H / ch * a.fov_scale
K = [[f, 0, VW / 2], [0, f, H / 2], [0, 0, 1]]
side = Image.new("RGB", (a.sidebar, H), (10, 12, 16))
ov = panel(meta, s=H / 1080)
side.paste(ov, ((a.sidebar - ov.width) // 2, (H - ov.height) // 2), ov)
side = np.asarray(side)


def frame(V):
    return np.concatenate([side, render(V, VW, H, K)], axis=1)


if a.still is not None:
    Image.fromarray(frame(views[a.still])).save(a.out.with_suffix(f".{a.still:04d}.png"))
    print("still", a.still, info)
    raise SystemExit

if RAW:  # raw RGB frames on the original stdout, for a downstream compositor
    for V in views:
        RAW.write(frame(V).tobytes())
    RAW.flush()
    print("raw", n, "frames", info, file=sys.stderr)
    raise SystemExit

enc = subprocess.Popen([a.ffmpeg, "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}",
                        "-r", str(a.fps), "-i", "-", "-c:v", "libsvtav1", "-preset", "4", "-crf", "30",
                        "-g", str(8 * a.fps), "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(a.out)],
                       stdin=subprocess.PIPE)
for V in views:
    enc.stdin.write(frame(V).tobytes())
enc.stdin.close()
if enc.wait() != 0:
    raise SystemExit(f"ffmpeg failed with exit {enc.returncode}")
print("encoded", n, "frames", info, a.out)
