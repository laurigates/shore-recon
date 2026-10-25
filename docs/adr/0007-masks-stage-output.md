# 0007. Masks stage: dilation, output layout and the masked-fraction warning

- Status: accepted
- Date: 2026-10-25

## Context

ADR-0006 puts segmentation in a pinned container (`segment`, which writes undilated 0/255 class
masks) and leaves the rest of the masks stage to Rust. The kickoff asks for COLMAP-convention
masks (`images/00001.jpg` → `masks/00001.jpg.png`, black = ignore), a configurable dilation to
cover reflections at the waterline, overlays for manual review, and a quality gate that warns when
an image's masked fraction exceeds 0.7. The M0 spike dilated with
`cv2.dilate(mask, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2r+1, 2r+1)))`.

## Decision

1. The stage depends on `frames` and segments its `images/` with the `segment` tool, passing
   `masks.classes`. Its version is `masks/<revision> <segment version>`, the image reference for
   the container (ADR-0003).
2. Dilation by `masks.dilate_px` reproduces OpenCV's elliptical structuring element exactly,
   including its rounding (radius 1 is a 5-pixel cross, radius 8 has 213 pixels), with pixels
   outside the image never contributing.
3. Output: `masks/<image>.png` (255 = keep, 0 = ignore), `raw/` (the tool's undilated masks),
   `overlays/<image>` for every 12th frame (ignored pixels tinted magenta, scaled to 0.4, as in
   the spike), `masks.csv` (each frame's masked fraction at full precision) and
   `logs/segment.log`. A frame without a raw mask, or with one of another size, fails the stage.
4. Metrics: `frames`, `masked_mean`, `masked_max`. The gate warns, without failing, when
   `masked_max` exceeds `gates.max_masked_fraction`; per-frame values are in `masks.csv`.
5. `masks.backend` is `container` or `onnx`; config validation rejects `onnx` until #11,
   `temporal = true` (not implemented), a `model` path with the container backend (the image
   carries its weights) and an empty class list.

## Consequences

Measured on the 121 Laajasalo frames, with the raw masks from the image's run (ADR-0006): the
stage's masks equal OpenCV's dilation of the same raw masks on every pixel of every frame; mean
masked fraction 0.503, maximum 0.647; IoU with the spike's masks mean 0.995. Dilation and writing
take about 8 s for the 121 frames (release build).

- Brush finds masks by file stem, not COLMAP naming (ADR-0002); the splat stage, not this one,
  provides masks under Brush's names.
- The warning reports only the maximum; which frames exceed the limit is in `masks.csv`.
- Overlays are sampled, so a bad mask on an unsampled frame is visible only in `masks.csv`'s
  numbers.
