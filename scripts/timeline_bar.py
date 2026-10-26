"""Append a timeline bar under every frame of a chaptered video (ADR-0013).

    timeline_bar.py <in.mp4> <out.mp4> [--bar 80] [--font F] [--mono F]

The bar has three rows: the groups (the text before " · " in each chapter title, e.g. the
dataset side) over their spans; a slim track with one unlabelled block per chapter, coloured
by kind, with a playhead and progress through the current chapter; and a caption naming the
current chapter in full (with its position in the group), the next chapter, and elapsed /
total time. No text is drawn inside a block, so any number of chapters fits. Chapters and metadata are copied from the
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
STEP = (99, 102, 241)  # processing steps
NEUTRAL = (120, 128, 140)  # the title card


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
    if label.lower() == "title":
        return NEUTRAL
    if label.lower().startswith("step"):
        return STEP
    for key, colour in ACCENT.items():
        if key in label.lower():
            return colour
    return STEP


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

    # Layout, top to bottom: group labels over their spans; a slim track with one unlabelled
    # block per chapter (no text, so any number of chapters fits); a caption with the current
    # chapter in full, the next one, and the time. Text never has to fit inside a block.
    f_group = ImageFont.truetype(a.font, 12, index=1)
    f_cap = ImageFont.truetype(a.font, 17, index=0)
    f_cap_b = ImageFont.truetype(a.font, 17, index=1)
    f_next = ImageFont.truetype(a.font, 14, index=0)
    f_time = ImageFont.truetype(a.mono, 16)
    M = 24
    X0, X1 = M, W - M
    gy = 6                      # group label row
    ty0, ty1 = 24, 34           # track
    cy = 46                     # caption row
    GAP = 2
    px = lambda frame: X0 + (X1 - X0) * frame / total

    def fmt(t):
        return f"{int(t // 60):02d}:{int(t % 60):02d}"

    def mix(c, amount):
        return tuple(int(v * amount + b * (1 - amount)) for v, b in zip(c, BG))

    groups = []
    for s in segs:
        if s["group"] and s["group"] not in groups:
            groups.append(s["group"])

    def caption(i):
        s = segs[i]
        same = [k for k, x in enumerate(segs) if x["group"] == s["group"]]
        prefix = f"{s['group']} · {same.index(i) + 1}/{len(same)} · " if s["group"] else ""
        nxt = segs[i + 1] if i + 1 < len(segs) else None
        if nxt is None:
            following = ""
        elif nxt["group"] and nxt["group"] != s["group"]:
            following = f"next: {nxt['group']} · {nxt['label']}"
        else:
            following = f"next: {nxt['label']}"
        return prefix, s["label"], following

    def base_bar(active):
        img = Image.new("RGB", (W, B), BG)
        d = ImageDraw.Draw(img)
        d.line([0, 0, W, 0], fill=(40, 44, 52), width=1)
        active_group = segs[active]["group"]
        for g in groups:
            idx = [i for i, s in enumerate(segs) if s["group"] == g]
            gx0, gx1 = px(segs[idx[0]]["f0"]), px(segs[idx[-1]]["f1"])
            text = g.upper()
            on = g == active_group
            d.text((gx0 + 1, gy), text, font=f_group, fill=(225, 230, 238) if on else (120, 126, 138))
            tw = d.textlength(text, font=f_group)
            d.line([gx0 + tw + 8, gy + 8, gx1 - 3, gy + 8], fill=(90, 96, 108) if on else (50, 54, 62), width=1)
        for i, s in enumerate(segs):
            x0, x1 = px(s["f0"]) + GAP / 2, px(s["f1"]) - GAP / 2
            c = accent(s["label"])
            fill = c if i == active else mix(c, 0.55 if i < active else 0.3)
            d.rounded_rectangle([x0, ty0, max(x1, x0 + 2), ty1], radius=3, fill=fill)
        prefix, label, following = caption(active)
        x = X0
        d.text((x, cy), prefix, font=f_cap, fill=(170, 176, 188))
        x += d.textlength(prefix, font=f_cap)
        d.text((x, cy), label, font=f_cap_b, fill=accent(segs[active]["label"]))
        x += d.textlength(label, font=f_cap_b) + 18
        d.text((x, cy + 2), following, font=f_next, fill=(110, 116, 128))
        return img

    bars = [base_bar(i) for i in range(len(segs))]

    def bar_frame(frame):
        i = next((k for k, s in enumerate(segs) if s["f0"] <= frame < s["f1"]), len(segs) - 1)
        b = bars[i].copy()
        d = ImageDraw.Draw(b)
        x = px(frame)
        x0 = px(segs[i]["f0"]) + GAP / 2
        d.rectangle([x0, ty1 - 3, max(x, x0 + 1), ty1], fill=(255, 255, 255))  # progress in chapter
        d.line([x, ty0 - 4, x, ty1 + 4], fill=(255, 255, 255), width=2)          # playhead
        t = f"{fmt(frame / fps)} / {fmt(total / fps)}"
        d.text((W - M - d.textlength(t, font=f_time), cy), t, font=f_time, fill=(235, 238, 245))
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
