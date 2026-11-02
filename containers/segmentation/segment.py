#!/usr/bin/env python3
"""Semantic class masks for the masks stage (ADR-0006).

    segment --classes water,sea,sky IMAGES OUT [--water-min-prob P] [--water-min-area A] [--water-bridge B] [--water-seed-prob S]
    segment --check --classes water,sea,sky

For every IMAGES/*.jpg, writes OUT/<image name>.png: 8-bit, 255 where the
pixel belongs to one of the classes, 0 elsewhere, at the image's resolution
and undilated. Prints "<image name> <class fraction>" per image. Classes are
ADE20K label names (the first synonym, e.g. "person" for
"person;individual;someone"). --check loads the model and validates the
classes without reading images. Exits 2 on an empty or unknown class list or
a threshold outside [0, 1].

The water classes among the given ones (water, sea, river, lake) are gated so
that wet gravel is not masked as water (#12); other classes are plain argmax.
Seeds are the pixels whose argmax is a water class, whose water probability
(the water classes' share of the summed class scores) is at least
--water-min-prob (default 0.5), and whose 4-connected seed component touches
the bottom row or covers at least --water-min-area of the image (default
0.02). Every argmax-water pixel connected to a seed is then kept, with gaps up
to --water-bridge of the image width bridged (default 0.02; railing bars), so
low-probability water next to the main body stays masked while wet gravel away
from it does not. A water region whose mean water probability is at least
--water-seed-prob (default 0.7) seeds itself, so confident distant water cut
off by a dock or a treeline stays masked. --water-min-prob and --water-min-area
at 0, or --water-seed-prob at 0, give plain argmax for every class.
"""

import argparse
import os
import sys
from pathlib import Path

import numpy as np
import torch
from PIL import Image

import regions

MODEL = os.environ.get("SEGMENT_MODEL", "/opt/models/oneformer")
# Class maps upsampled at once; bounds peak memory (see class_scores).
CHUNK = 16
# ADE20K labels that take part in the water gating, if they are among --classes.
WATER_CLASSES = ("water", "sea", "river", "lake")


def label_name(label: str) -> str:
    return label.split(",")[0].split(";")[0].strip()


def class_scores(outputs, size, water_ids):
    """Per-pixel argmax class and water probability at `size` (height, width).

    The argmax equals the processor's post_process_semantic_segmentation(
    outputs, target_sizes=[size]): the same per-class scores, bilinear
    upsampling (align_corners=False) and first-maximum argmax. That method
    upsamples all 150 class maps at once, 3.5 GB in float32 for a 3200x1800
    frame; upsampling CHUNK maps at a time keeps a running maximum instead.
    Bilinear upsampling is per channel, so no value changes.

    The water probability is the summed scores of `water_ids` divided by the
    summed scores of all classes (0 where that sum is 0), kept as two running
    sums so peak memory stays near that of the argmax alone. Returns
    (best_class, water_prob) as numpy arrays.
    """
    classes = outputs.class_queries_logits.softmax(dim=-1)[..., :-1]
    masks = outputs.masks_queries_logits.sigmoid()
    scores = torch.einsum("bqc, bqhw -> bchw", classes, masks)[0].float()
    best = best_class = None
    total = torch.zeros(size, device=scores.device)
    water = torch.zeros(size, device=scores.device)
    for start in range(0, scores.shape[0], CHUNK):
        chunk = scores[None, start : start + CHUNK]
        up = torch.nn.functional.interpolate(chunk, size=size, mode="bilinear", align_corners=False)[0]
        value, index = up.max(dim=0)
        if best is None:
            best, best_class = value, index + start
        else:
            better = value > best  # strict: ties keep the earlier class, as argmax does
            best = torch.where(better, value, best)
            best_class = torch.where(better, index + start, best_class)
        total = total + up.sum(dim=0)
        local = [i - start for i in water_ids if start <= i < start + up.shape[0]]
        if local:
            water = water + up[local].sum(dim=0)
    water_prob = torch.where(total > 0, water / torch.where(total > 0, total, 1.0), 0.0)
    return best_class.cpu().numpy(), water_prob.cpu().numpy()


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--classes", required=True, help="comma-separated ADE20K label names")
    ap.add_argument("--check", action="store_true", help="validate the model and classes, then exit")
    # The defaults repeat configs/defaults.toml for standalone runs; the masks stage always passes
    # both flags from [masks], which is the source of truth.
    ap.add_argument("--water-min-prob", type=float, default=0.5, help="minimum water probability (default 0.5)")
    ap.add_argument("--water-min-area", type=float, default=0.02, help="minimum inland water area fraction (default 0.02)")
    ap.add_argument("--water-bridge", type=float, default=0.02, help="gap bridged between water regions, fraction of width (default 0.02)")
    ap.add_argument("--water-seed-prob", type=float, default=0.7, help="mean water probability that seeds a region (default 0.7)")
    ap.add_argument("images", type=Path, nargs="?")
    ap.add_argument("out", type=Path, nargs="?")
    args = ap.parse_args()
    if not args.check and (args.images is None or args.out is None):
        ap.error("IMAGES and OUT are required unless --check is given")

    thresholds = (
        ("--water-min-prob", args.water_min_prob),
        ("--water-min-area", args.water_min_area),
        ("--water-bridge", args.water_bridge),
        ("--water-seed-prob", args.water_seed_prob),
    )
    for flag, value in thresholds:
        if not 0.0 <= value <= 1.0:  # also rejects nan
            print(f"segment: {flag} must be within [0, 1]", file=sys.stderr)
            return 2
    classes = [c.strip() for c in args.classes.split(",") if c.strip()]
    if not classes:
        print("segment: no classes given", file=sys.stderr)
        return 2
    # Imported here so that test_class_scores.py can import this module with torch alone.
    from transformers import OneFormerForUniversalSegmentation, OneFormerProcessor

    processor = OneFormerProcessor.from_pretrained(MODEL)
    model = OneFormerForUniversalSegmentation.from_pretrained(MODEL).eval()
    ids = sorted(i for i, label in model.config.id2label.items() if label_name(label) in classes)
    missing = sorted(set(classes) - {label_name(model.config.id2label[i]) for i in ids})
    if missing:
        print(f"segment: unknown classes {missing}", file=sys.stderr)
        return 2
    if args.check:
        print(f"segment: model {MODEL} ok, class ids {ids}, torch {torch.__version__}")
        return 0

    water_ids = [i for i in ids if label_name(model.config.id2label[i]) in WATER_CLASSES]
    other_ids = [i for i in ids if i not in water_ids]

    device = "cuda" if torch.cuda.is_available() else "cpu"
    dtype = torch.float16 if device == "cuda" else torch.float32
    model = model.to(device=device, dtype=dtype)
    images = sorted(args.images.glob("*.jpg"))
    if not images:
        print(f"segment: no *.jpg in {args.images}", file=sys.stderr)
        return 2
    args.out.mkdir(parents=True, exist_ok=True)
    for path in images:
        image = Image.open(path).convert("RGB")
        inputs = processor(images=image, task_inputs=["semantic"], return_tensors="pt")
        inputs = {k: v.to(device=device, dtype=dtype) if v.is_floating_point() else v.to(device) for k, v in inputs.items()}
        with torch.no_grad():
            outputs = model(**inputs)
        best_class, water_prob = class_scores(outputs, image.size[::-1], water_ids)
        mask = regions.combine(best_class, water_prob, water_ids, other_ids, args.water_min_prob, args.water_min_area, args.water_bridge, args.water_seed_prob)
        Image.fromarray((mask * 255).astype(np.uint8), mode="L").save(args.out / f"{path.name}.png")
        print(f"{path.name} {mask.mean():.6f}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
