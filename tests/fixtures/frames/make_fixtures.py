# Golden fixtures for the frames stage's blur metric: synthetic colour JPEGs and
# their variance of the Laplacian as OpenCV computes it. OpenCV is the reference
# because the M0 spike measured blur with it and set blur_threshold = 100 on that
# scale (ADR-0005). Re-run only when the fixture set changes:
#
#   uv run --no-project --with opencv-python-headless==5.0.0.93 --with numpy==2.5.4 \
#     python -I tests/fixtures/frames/make_fixtures.py tests/fixtures/frames
import csv
import sys
from pathlib import Path

import cv2
import numpy as np

out = Path(sys.argv[1])
rng = np.random.default_rng(20261025)

# (name, width, height, gaussian sigma); odd sizes exercise borders and MCU padding.
FIXTURES = [
    ("sharp", 320, 240, 0.0),
    ("soft", 320, 240, 1.2),
    ("blurry", 320, 240, 3.0),
    # Either side of blur_threshold = 100, where agreement with OpenCV matters most.
    ("near_keep", 320, 240, 0.93),
    ("near_drop", 320, 240, 1.0),
    ("odd", 37, 23, 0.0),
]

rows = []
for name, w, h, sigma in FIXTURES:
    yy, xx = np.mgrid[0:h, 0:w]
    gradient = np.stack([xx * 255 / w, yy * 255 / h, (xx + yy) * 127 / (w + h)], axis=-1)
    texture = rng.integers(0, 256, (h, w, 3))
    img = (0.5 * gradient + 0.5 * texture).astype(np.uint8)
    if sigma:
        img = cv2.GaussianBlur(img, (0, 0), sigma)
    path = out / f"{name}.jpg"
    cv2.imwrite(str(path), img, [cv2.IMWRITE_JPEG_QUALITY, 90])
    # Same calls as spike/m0/local/frames_manual.py.
    gray = cv2.imread(str(path), cv2.IMREAD_GRAYSCALE)
    rows.append((path.name, repr(float(cv2.Laplacian(gray, cv2.CV_64F).var()))))

with open(out / "expected.csv", "w", newline="") as f:
    writer = csv.writer(f)
    writer.writerow(["image", "laplacian_var"])
    writer.writerows(rows)
for row in rows:
    print(*row)
