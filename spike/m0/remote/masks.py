# Manual masks stage: OneFormer ADE20K semantic segmentation -> COLMAP-convention masks.
# Usage: python masks.py <images_dir> <masks_dir> [--dilate 8] [--overlays N]
# Writes <masks_dir>/<image name>.png: white = keep, black = ignore (water/sky/...).
import argparse, json, sys
from pathlib import Path

import cv2
import numpy as np
import torch
from PIL import Image
from transformers import OneFormerForUniversalSegmentation, OneFormerProcessor

MODEL = "shi-labs/oneformer_ade20k_swin_large"
CLASSES = ["water", "sea", "river", "lake", "sky", "person", "boat"]

ap = argparse.ArgumentParser()
ap.add_argument("images", type=Path)
ap.add_argument("masks", type=Path)
ap.add_argument("--dilate", type=int, default=8)
ap.add_argument("--overlays", type=int, default=12, help="write every Nth overlay for review")
a = ap.parse_args()

proc = OneFormerProcessor.from_pretrained(MODEL)
model = OneFormerForUniversalSegmentation.from_pretrained(MODEL).cuda().eval().half()
labels = model.config.id2label
ids = sorted(i for i, n in labels.items() if n.split(",")[0].split(";")[0].strip() in CLASSES)
found = sorted({labels[i].split(",")[0].split(";")[0].strip() for i in ids})
if set(found) != set(CLASSES):
    sys.exit(f"class mismatch: wanted {CLASSES}, found {found}")

a.masks.mkdir(parents=True, exist_ok=True)
(a.masks.parent / "mask_overlays").mkdir(exist_ok=True)
kernel = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2 * a.dilate + 1, 2 * a.dilate + 1))
stats = {}
for n, p in enumerate(sorted(a.images.glob("*.jpg"))):
    img = Image.open(p).convert("RGB")
    inp = proc(images=img, task_inputs=["semantic"], return_tensors="pt")
    inp = {k: (v.cuda().half() if v.dtype.is_floating_point else v.cuda()) for k, v in inp.items()}
    with torch.no_grad():
        out = model(**inp)
    seg = proc.post_process_semantic_segmentation(out, target_sizes=[img.size[::-1]])[0].cpu().numpy()
    ignore = np.isin(seg, ids).astype(np.uint8)
    if a.dilate:
        ignore = cv2.dilate(ignore, kernel)
    keep = (1 - ignore) * 255
    cv2.imwrite(str(a.masks / f"{p.name}.png"), keep.astype(np.uint8))
    stats[p.name] = round(float(ignore.mean()), 4)
    if a.overlays and n % a.overlays == 0:
        rgb = np.asarray(img)[:, :, ::-1].copy()
        rgb[ignore == 1] = (0.4 * rgb[ignore == 1] + 0.6 * np.array([255, 0, 255])).astype(np.uint8)
        cv2.imwrite(str(a.masks.parent / "mask_overlays" / p.name), cv2.resize(rgb, None, fx=0.4, fy=0.4))
    print(f"{p.name} masked={stats[p.name]:.3f}", flush=True)

(a.masks.parent / "masked_fraction.json").write_text(json.dumps(stats, indent=1))
v = list(stats.values())
print(f"DONE masks n={len(v)} mean_masked={np.mean(v):.3f} max_masked={max(v):.3f} over_0.7={sum(x > 0.7 for x in v)}")
