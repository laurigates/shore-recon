# Build the overlay metadata JSON for each splat video from the run's own artefacts.
# Usage: python3 -I make_meta.py <repo_root> <out_dir>
import csv, json, re, subprocess, sys
from datetime import datetime
from pathlib import Path

root, out = Path(sys.argv[1]), Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)
TAG = "manual-2026-10-23"
# Clip ids from the staging rename (DJI_0849 -> laajasalo, DJI_0703 -> vartiosaari).
SIDES = {"reposalmi-laajasalo": ("Laajasalo side", "DJI_0849", "sequential"),
         "reposalmi-vartiosaari": ("Vartiosaari side", "DJI_0703", "exhaustive")}
BADGE = {"masked": ("WATER MASKED", (20, 184, 166)), "unmasked": ("UNMASKED", (245, 158, 11))}


def analyzer(p):
    t = p.read_text()
    g = lambda k: re.search(rf"{k}: ([\d.]+)", t).group(1)
    return dict(reg=int(g("Registered images")), imgs=int(g(r"\] Images")), pts=int(g("Points")),
                track=float(g("Mean track length")), reproj=float(g("Mean reprojection error")))


def srt(p):
    t = p.read_text(encoding="utf-8")
    ts = re.findall(r"\n(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\.\d+)\n", t)
    alt = [float(x) for x in re.findall(r"rel_alt: ([-\d.]+)", t)]
    a, b = (datetime.strptime(x, "%Y-%m-%d %H:%M:%S.%f") for x in (ts[0], ts[-1]))
    return dict(start=a.strftime("%Y-%m-%d %H:%M"), dur=(b - a).total_seconds(), alt=(min(alt), max(alt)))


def probe(p):
    o = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries",
                        "stream=width,height,r_frame_rate", "-of", "json", str(p)], capture_output=True, check=True)
    s = json.loads(o.stdout)["streams"][0]
    n, d = map(int, s["r_frame_rate"].split("/"))
    return f"{s['width']}×{s['height']} @ {n / d:.2f} fps"


def splat_count(p):
    with open(p, "rb") as f:
        head = f.read(4096).split(b"end_header")[0].decode("ascii", "replace")
    return int(re.search(r"element vertex (\d+)", head).group(1))


def brush_secs(p):
    return int(re.search(r"STEP-OK .* brush (\d+)s", p.read_text()).group(1))


for ds, (side, clip, matcher) in SIDES.items():
    w = root / "work" / ds
    cap = srt(root / "data" / "raw" / f"{ds}.srt")
    rows = list(csv.DictReader(open(w / "frames" / TAG / "frames.csv")))
    kept = sum(r["kept"] == "True" for r in rows)
    for kind in ("masked", "unmasked"):
        sfm = analyzer(w / ("sfm-masked" if kind == "masked" else "sfm") / TAG / "model_analyzer.txt")
        sp = w / f"splat-{kind}" / TAG
        secs = brush_secs(sp / "brush.log")
        sections = [
            ["Capture", [
                ["Captured", f"{cap['start']} · {clip}"],
                ["Clip", f"{cap['dur']:.1f} s · {probe(root / 'data' / 'raw' / f'{ds}.mp4')}"],
                ["Height above takeoff", f"{cap['alt'][0]:.1f} to {cap['alt'][1]:.1f} m"],
                ["Frames used", f"{kept} / {len(rows)} · 2 fps"]]]]
        if kind == "masked":
            mf = json.loads((w / "dense-masked" / TAG / "masked_fraction.json").read_text()).values()
            sections.append(["Water mask", [
                ["Model", "OneFormer ADE20K Swin-L"],
                ["Classes", "water, sea, river, lake, sky, …"],
                ["Mean masked area", f"{100 * sum(mf) / len(mf):.0f} %"]]])
        sections.append(["Structure from motion", [
            ["Registered frames", f"{sfm['reg']} / {sfm['imgs']}", sfm["reg"] / sfm["imgs"] >= 0.85],
            ["Reprojection error", f"{sfm['reproj']:.2f} px", sfm["reproj"] <= 1.0],
            ["Mean track length", f"{sfm['track']:.2f}", sfm["track"] >= 3],
            ["Sparse points", f"{sfm['pts']:,}"],
            ["Matcher", f"COLMAP 4.2.1 {matcher}"]]])
        sections.append(["Gaussian splat", [
            ["Gaussians", f"{splat_count(sp / 'splat_30000.ply'):,}"],
            ["Training", f"30,000 steps · {secs // 60} m {secs % 60:02d} s"],
            ["Resolution", "1920 px max · RTX 4090"]]])
        meta = dict(kind=kind, title="Reposalmi pier", subtitle=f"{side} · Helsinki",
                    badges=[BADGE[kind], ("GAUSSIAN SPLAT", (129, 140, 248)), ("BRUSH 0.3.0", (148, 163, 184))],
                    sections=sections)
        (out / f"{ds}-{kind}.json").write_text(json.dumps(meta, indent=1, ensure_ascii=False))
        print(ds, kind, "ok")
