"""Append a timeline bar under every frame of a chaptered video (ADR-0013).

    timeline_bar.py <in.mp4> <out.mp4> [--bar 80] [--font F] [--mono F]

The bar shows one block per MP4 chapter, grouped by the text before " · " in the chapter
title (the dataset side), with the current chapter highlighted, a playhead, progress through
the current chapter, and elapsed / total time. Chapters and metadata are copied from the
input. Ported from the M0 spike's spike/m0/remote/timeline.py, which drew the same bar while
rendering; here it is applied to a finished video, so any chaptered video can carry one.
Encodes AV1 (libsvtav1, CRF 30) with the input's frame rate.
"""
import argparse
import json
import shutil
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

BG = (10, 12, 16)
ACCENT = {"splat": (20, 184, 166), "mesh": (245, 158, 11)}
NEUTRAL = (120, 128, 140)


def die(msg):
    raise SystemExit(f"timeline_bar: {msg}")


def probe(path):
    out = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries",
                          "stream=width,height,r_frame_rate,nb_frames:format=duration", "-show_chapters",
                          "-of", "json", str(path)], capture_output=True, text=True)
    if out.returncode != 0:
        die(f"ffprobe {path}: {out.stderr.strip()}")
    info = json.loads(out.stdout)
    s = info["streams"][0]
    num, den = map(int, s["r_frame_rate"].split("/"))
    chapters = [(float(c["start_time"]), float(c["end_time"]), c.get("tags", {}).get("title", ""))
                for c in info.get("chapters", [])]
    if not chapters:
        die(f"{path} has no chapters; the bar is drawn from them")
    return s["width"], s["height"], num / den, float(info["format"]["duration"]), chapters


def accent(label):
    for key, colour in ACCENT.items():
        if key in label.lower():
            return colour
    return NEUTRAL


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("src", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--bar", type=int, default=80, help="bar height in px (even)")
    ap.add_argument("--font", default="/System/Library/Fonts/Helvetica.ttc")
    ap.add_argument("--mono", default="/System/Library/Fonts/Menlo.ttc")
    a = ap.parse_args(argv)
    if a.bar % 2:
        ap.error("--bar must be even (yuv420p)")
    for tool in ("ffmpeg", "ffprobe"):
        if not shutil.which(tool):
            die(f"{tool} not found on PATH")

    W, H, fps, duration, chapters = probe(a.src)
    B = a.bar
    total = round(duration * fps)
    segs = []
    for start, end, title in chapters:
        group, _, label = title.partition(" · ")
        if not label:
            group, label = "", title
        segs.append(dict(group=group, label=label, f0=round(start * fps), f1=round(end * fps)))

    f_group = ImageFont.truetype(a.font, 13, index=1)
    f_seg = ImageFont.truetype(a.font, 14, index=0)
    f_seg_b = ImageFont.truetype(a.font, 14, index=1)
    f_time = ImageFont.truetype(a.mono, 16)
    M, TIME_W = 24, 170
    X0, X1 = M, W - M - TIME_W
    gy0, gy1 = 8, 26
    sy0, sy1 = 32, B - 14
    px = lambda frame: X0 + (X1 - X0) * frame / total

    def fmt(t):
        return f"{int(t // 60):02d}:{int(t % 60):02d}"

    def base_bar(active):
        img = Image.new("RGB", (W, B), BG)
        d = ImageDraw.Draw(img)
        d.line([0, 0, W, 0], fill=(40, 44, 52), width=1)
        groups = [g for i, g in enumerate(s["group"] for s in segs) if g and g not in [x["group"] for x in segs[:i]]]
        for g in groups:
            idx = [i for i, s in enumerate(segs) if s["group"] == g]
            gx0, gx1 = px(segs[idx[0]]["f0"]), px(segs[idx[-1]]["f1"])
            text = g.upper()
            d.text((gx0 + 2, gy0), text, font=f_group, fill=(150, 156, 168))
            tw = d.textlength(text, font=f_group)
            d.line([gx0 + tw + 10, (gy0 + gy1) / 2 + 1, gx1 - 4, (gy0 + gy1) / 2 + 1], fill=(55, 60, 70), width=1)
        for i, s in enumerate(segs):
            x0, x1 = px(s["f0"]) + 2, px(s["f1"]) - 2
            c = accent(s["label"])
            on = i == active
            fill = c if on else tuple(int(v * 0.28 + b * 0.72) for v, b in zip(c, BG))
            d.rounded_rectangle([x0, sy0, x1, sy1], radius=6, fill=fill)
            font = f_seg_b if on else f_seg
            label = s["label"]
            while d.textlength(label, font=font) > x1 - x0 - 12 and len(label) > 3:
                label = label[:-2] + "…"
            tx = x0 + (x1 - x0 - d.textlength(label, font=font)) / 2
            d.text((tx, sy0 + (sy1 - sy0 - 16) / 2), label, font=font, fill=BG if on else (205, 210, 220))
        return img

    bars = [base_bar(i) for i in range(len(segs))]

    def bar_frame(frame):
        i = next((k for k, s in enumerate(segs) if s["f0"] <= frame < s["f1"]), len(segs) - 1)
        b = bars[i].copy()
        d = ImageDraw.Draw(b)
        x = px(frame)
        x0 = px(segs[i]["f0"]) + 2
        d.rounded_rectangle([x0, sy1 - 4, max(x, x0 + 1), sy1], radius=2, fill=(255, 255, 255))
        d.line([x, sy0 - 6, x, sy1 + 4], fill=(255, 255, 255), width=3)
        t = f"{fmt(frame / fps)} / {fmt(total / fps)}"
        d.text((W - M - d.textlength(t, font=f_time), (B - 20) / 2), t, font=f_time, fill=(235, 238, 245))
        return np.asarray(b)

    dec = subprocess.Popen(["ffmpeg", "-v", "error", "-i", str(a.src), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
                           stdout=subprocess.PIPE)
    tmp = a.out.with_name(a.out.stem + ".partial.mp4")
    enc = subprocess.Popen(["ffmpeg", "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H + B}",
                            "-r", f"{fps:g}", "-i", "-", "-i", str(a.src), "-map", "0:v", "-map_metadata", "1",
                            "-map_chapters", "1", "-c:v", "libsvtav1", "-preset", "4", "-crf", "30", "-g", "60",
                            "-pix_fmt", "yuv420p", "-movflags", "+faststart", "-f", "mp4", str(tmp)],
                           stdin=subprocess.PIPE)
    size = W * H * 3
    n = 0
    while (buf := dec.stdout.read(size)) and len(buf) == size:
        top = np.frombuffer(buf, np.uint8).reshape(H, W, 3)
        enc.stdin.write(np.concatenate([top, bar_frame(n)], axis=0).tobytes())
        n += 1
    enc.stdin.close()
    if dec.wait() != 0 or enc.wait() != 0:
        tmp.unlink(missing_ok=True)
        die(f"ffmpeg failed (decode {dec.returncode}, encode {enc.returncode})")
    if abs(n - total) > 1:
        tmp.unlink(missing_ok=True)
        die(f"decoded {n} frames, expected {total}")
    tmp.replace(a.out)
    print(f"timeline_bar: {a.out} {W}x{H + B}, {n} frames, {len(segs)} chapters", file=sys.stderr)


if __name__ == "__main__":
    main()
