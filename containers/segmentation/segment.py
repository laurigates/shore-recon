#!/usr/bin/env python3
"""Semantic class masks for the masks stage (ADR-0006).

    segment --classes water,sea,sky IMAGES OUT
    segment --check --classes water,sea,sky

For every IMAGES/*.jpg, writes OUT/<image name>.png: 8-bit, 255 where the
pixel belongs to one of the classes, 0 elsewhere, at the image's resolution
and undilated. Prints "<image name> <class fraction>" per image. Classes are
ADE20K label names (the first synonym, e.g. "person" for
"person;individual;someone"). --check loads the model and validates the
classes without reading images. Exits non-zero on an unknown class.
"""

import argparse
import os
import sys
from pathlib import Path

import numpy as np
import torch
from PIL import Image
from transformers import OneFormerForUniversalSegmentation, OneFormerProcessor

MODEL = os.environ.get("SEGMENT_MODEL", "/opt/models/oneformer")


def label_name(label: str) -> str:
    return label.split(",")[0].split(";")[0].strip()


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--classes", required=True, help="comma-separated ADE20K label names")
    ap.add_argument("--check", action="store_true", help="validate the model and classes, then exit")
    ap.add_argument("images", type=Path, nargs="?")
    ap.add_argument("out", type=Path, nargs="?")
    args = ap.parse_args()
    if not args.check and (args.images is None or args.out is None):
        ap.error("IMAGES and OUT are required unless --check is given")

    classes = [c.strip() for c in args.classes.split(",") if c.strip()]
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
        seg = processor.post_process_semantic_segmentation(outputs, target_sizes=[image.size[::-1]])[0]
        mask = np.isin(seg.cpu().numpy(), ids)
        Image.fromarray((mask * 255).astype(np.uint8), mode="L").save(args.out / f"{path.name}.png")
        print(f"{path.name} {mask.mean():.6f}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
