# Manual frames stage: score blur, move frames below the threshold to rejected/.
# Usage: python -I frames_manual.py <frames_dir> <blur_threshold>
import csv, shutil, sys
from pathlib import Path
import cv2

root, thr = Path(sys.argv[1]), float(sys.argv[2])
imgs, rej = root / "images", root / "rejected"
rej.mkdir(exist_ok=True)
rows = []
for p in sorted(imgs.glob("*.jpg")):
    g = cv2.imread(str(p), cv2.IMREAD_GRAYSCALE)
    v = float(cv2.Laplacian(g, cv2.CV_64F).var())
    keep = v >= thr
    if not keep:
        shutil.move(p, rej / p.name)
    rows.append((p.name, round(v, 2), keep))
with open(root / "frames.csv", "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["image", "laplacian_var", "kept"])
    w.writerows(rows)
vs = sorted(r[1] for r in rows)
print(f"{root}: total={len(rows)} kept={sum(r[2] for r in rows)} "
      f"min={vs[0]} p10={vs[len(vs)//10]} median={vs[len(vs)//2]} max={vs[-1]}")
