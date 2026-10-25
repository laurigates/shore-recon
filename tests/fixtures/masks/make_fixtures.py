# Golden fixtures for the masks stage's dilation: a random-blob binary mask that
# touches the borders, and OpenCV's dilation of it with the elliptical kernel the
# M0 spike used (spike/m0/remote/masks.py). Re-run only when the fixture set changes:
#
#   uv run --no-project --with opencv-python-headless==5.0.0.93 --with numpy==2.5.4 \
#     python -I tests/fixtures/masks/make_fixtures.py tests/fixtures/masks
import sys
from pathlib import Path

import cv2
import numpy as np

out = Path(sys.argv[1])
rng = np.random.default_rng(20261025)
h, w = 61, 97  # odd sizes
mask = np.zeros((h, w), np.uint8)
for _ in range(9):
    cy, cx, r = rng.integers(0, h), rng.integers(0, w), rng.integers(1, 9)
    cv2.circle(mask, (int(cx), int(cy)), int(r), 255, -1)
mask[0, 40:44] = 255  # touches the top border
mask[h - 1, w - 1] = 255  # a single corner pixel
mask[30, 0] = 255  # a single left-border pixel
cv2.imwrite(str(out / "mask.png"), mask)
# A raw class mask for the stage tests' stub segment tool: the size of the frames
# fixtures (320x240), top 80 rows set.
raw = np.zeros((240, 320), np.uint8)
raw[:80] = 255
cv2.imwrite(str(out / "raw_320x240.png"), raw)
for radius in (1, 3, 8):
    kernel = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2 * radius + 1, 2 * radius + 1))
    cv2.imwrite(str(out / f"dilated_{radius}.png"), cv2.dilate(mask, kernel))
    print(radius, int(kernel.sum()), "kernel pixels")
