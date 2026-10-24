# Concatenate splat renders into one AV1 video with a timeline strip and MP4 chapters.
# Usage: python timeline.py <segments.json> <out.mp4> --ffmpeg PATH [--bar 80]
# segments.json: {"groups": [{"label": ..., "segments": [{"label", "kind", "args": [...render_orbit args]}]}]}
# Each segment's frames come from `render_orbit.py <args> - ...` (raw RGB on stdout).
import argparse, json, os, subprocess, sys, tempfile
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

ap = argparse.ArgumentParser()
ap.add_argument("segments", type=Path)
ap.add_argument("out", type=Path)
ap.add_argument("--ffmpeg", required=True)
ap.add_argument("--size", default="1920x1080")
ap.add_argument("--bar", type=int, default=80)
ap.add_argument("--fps", type=int, default=30)
a = ap.parse_args()
W, H = map(int, a.size.split("x"))
B = a.bar
RENDER = [sys.executable, str(Path(__file__).with_name("render_orbit.py"))]

spec = json.loads(a.segments.read_text())
segs = [dict(s, group=g["label"]) for g in spec["groups"] for s in g["segments"]]
for s in segs:
    s["frames"] = round(s["seconds"] * a.fps)
total = sum(s["frames"] for s in segs)
start = np.cumsum([0] + [s["frames"] for s in segs])

FONT_DIR = os.environ.get("SHORELINE_FONT_DIR", "/usr/share/fonts/truetype/noto")
FONT = FONT_DIR + "/NotoSans-{}.ttf"
MONO = FONT_DIR + "/NotoSansMono-Regular.ttf"
ACCENT = {"masked": (20, 184, 166), "unmasked": (245, 158, 11)}
BG = (10, 12, 16)
f_group = ImageFont.truetype(FONT.format("Bold"), 13)
f_seg = ImageFont.truetype(FONT.format("Regular"), 14)
f_seg_b = ImageFont.truetype(FONT.format("Bold"), 14)
f_time = ImageFont.truetype(MONO, 16)

# Layout: [margin | track ... | time label]
M, TIME_W = 24, 170
X0, X1 = M, W - M - TIME_W
gy0, gy1 = 8, 26        # group label row
sy0, sy1 = 32, B - 14   # segment blocks
px = lambda frame: X0 + (X1 - X0) * frame / total


def fmt(t):
    return f"{int(t // 60):02d}:{int(t % 60):02d}"


def base_bar(active):
    """Static parts of the bar, with segment `active` highlighted."""
    img = Image.new("RGB", (W, B), BG)
    d = ImageDraw.Draw(img)
    d.line([0, 0, W, 0], fill=(40, 44, 52), width=1)
    for g in spec["groups"]:
        idx = [i for i, s in enumerate(segs) if s["group"] == g["label"]]
        gx0, gx1 = px(start[idx[0]]), px(start[idx[-1] + 1])
        d.text((gx0 + 2, gy0), g["label"].upper(), font=f_group, fill=(150, 156, 168))
        tw = d.textlength(g["label"].upper(), font=f_group)
        d.line([gx0 + tw + 10, (gy0 + gy1) / 2 + 1, gx1 - 4, (gy0 + gy1) / 2 + 1], fill=(55, 60, 70), width=1)
    for i, s in enumerate(segs):
        x0, x1 = px(start[i]) + 2, px(start[i + 1]) - 2
        c = ACCENT[s["kind"]]
        on = i == active
        fill = c if on else tuple(int(v * 0.28 + b * 0.72) for v, b in zip(c, BG))
        d.rounded_rectangle([x0, sy0, x1, sy1], radius=6, fill=fill)
        font = f_seg_b if on else f_seg
        label = s["label"]
        while d.textlength(label, font=font) > x1 - x0 - 12 and len(label) > 3:
            label = label[:-2] + "…"
        tx = x0 + (x1 - x0 - d.textlength(label, font=font)) / 2
        d.text((tx, sy0 + (sy1 - sy0 - 18) / 2), label, font=font, fill=BG if on else (205, 210, 220))
    return img


bars = [np.asarray(base_bar(i)).copy() for i in range(len(segs))]


def bar_frame(i_seg, frame):
    b = Image.fromarray(bars[i_seg])
    d = ImageDraw.Draw(b)
    x = px(frame)
    x0 = px(start[i_seg]) + 2
    d.rounded_rectangle([x0, sy1 - 4, max(x, x0 + 1), sy1], radius=2, fill=(255, 255, 255))  # progress in segment
    d.line([x, sy0 - 6, x, sy1 + 4], fill=(255, 255, 255), width=3)                           # playhead
    t = f"{fmt(frame / a.fps)} / {fmt(total / a.fps)}"
    d.text((W - M - d.textlength(t, font=f_time), (B - 20) / 2), t, font=f_time, fill=(235, 238, 245))
    return np.asarray(b)


# Chapters (ffmetadata, millisecond timebase)
meta = [";FFMETADATA1", f"title={spec.get('title', 'shoreline-recon splats')}"]
for i, s in enumerate(segs):
    meta += ["[CHAPTER]", "TIMEBASE=1/1000", f"START={round(start[i] * 1000 / a.fps)}",
             f"END={round(start[i + 1] * 1000 / a.fps)}", f"title={s['group']} · {s['label']}"]
chap = Path(tempfile.mkstemp(suffix=".ffmeta")[1])
chap.write_text("\n".join(meta) + "\n")

enc = subprocess.Popen([a.ffmpeg, "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H + B}",
                        "-r", str(a.fps), "-i", "-", "-i", str(chap), "-map", "0:v", "-map_metadata", "1",
                        "-map_chapters", "1", "-c:v", "libsvtav1", "-preset", "4", "-crf", "30",
                        "-g", str(2 * a.fps), "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(a.out)],
                       stdin=subprocess.PIPE)
fsize = W * H * 3
for i, s in enumerate(segs):
    print(f"segment {i + 1}/{len(segs)}: {s['group']} · {s['label']}", file=sys.stderr, flush=True)
    r = subprocess.Popen(RENDER + s["args"] + ["-", "--size", f"{W}x{H}", "--fps", str(a.fps),
                                               "--seconds", str(s["seconds"])], stdout=subprocess.PIPE)
    for k in range(s["frames"]):
        buf = r.stdout.read(fsize)
        if len(buf) != fsize:
            raise SystemExit(f"segment {i}: short read at frame {k}")
        top = np.frombuffer(buf, np.uint8).reshape(H, W, 3)
        enc.stdin.write(np.concatenate([top, bar_frame(i, start[i] + k)], axis=0).tobytes())
    if r.wait() != 0:
        raise SystemExit(f"segment {i}: renderer exit {r.returncode}")
enc.stdin.close()
if enc.wait() != 0:
    raise SystemExit(f"ffmpeg exit {enc.returncode}")
chap.unlink()
print("wrote", a.out, total, "frames", file=sys.stderr)
