# 0010. SfM metrics come from the model files

- Status: accepted
- Date: 2026-10-25
- Amends: kickoff § sfm ("Metrics come from `colmap model_analyzer`") and § Quality gates

## Context

The sfm gates (`configs/defaults.toml` `[gates]`) need the registered ratio, the mean reprojection
error and the mean track length. The M0 spike read them from `colmap model_analyzer`'s log
(`spike/m0/local/make_meta.py`). The post stage (M4) fits the water plane to sparse points, so the
pipeline needs a reader for the COLMAP model either way (kickoff § Testing strategy).

Read from COLMAP 4.2.1, the version in the pinned image (`src/colmap/scene/`):

- `WriteImagesBinary` writes only registered images, so a model on disk holds no unregistered
  image. `model_analyzer`'s `Registered images / Images` is therefore 1.0 by construction, and the
  spike's ratio could not fail its gate.
- `ComputeMeanReprojectionError` is the plain mean of the per-point errors, skipping points whose
  error is unset (-1). `ComputeMeanTrackLength` is the number of observations (2D points with a 3D
  point, over registered images) divided by the number of 3D points.

## Decision

Decided by the project owner on 2026-10-25:

1. The sfm crate reads `cameras.bin`, `images.bin` and `points3D.bin` itself and computes the
   metrics with COLMAP's definitions above. `model_analyzer` does not run.
2. The registered ratio's denominator is the number of input images (the frames stage's output),
   not a count from the model.
3. `rigs.bin` and `frames.bin` (COLMAP 4) are not read. The metrics and the water-plane fit need
   only images, cameras and points, and v1 captures are one camera.
4. A test reads the spike's model and must reproduce its `model_analyzer.txt` values.

## Consequences

- A COLMAP upgrade that changes the binary format breaks the reader. The pinned image digest
  makes that a deliberate change, and the spike-model test catches it.
- The text model format is not read yet. COLMAP writes binary by default; text matters only for
  models converted by hand.
- The report can name the frames that failed to register.
