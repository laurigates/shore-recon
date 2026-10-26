# 0014. Download bundles carry the splat as SPZ v3, encoded by our own port

- Status: accepted
- Date: 2026-10-26

## Context

The export stage's `splat.ply` is the Brush/INRIA layout in 32-bit floats: 137 MB (laajasalo,
579,344 Gaussians) and 102 MB (vartiosaari). Zip barely shrinks it. With the 21 and 41 MB
meshes, a dataset is too large for the channels the deliverables go through: Telegram bots take
files up to 50 MB, and the chat's file card takes 30 MiB.

SPZ (nianticlabs/spz, MIT) quantises a splat to about a tenth of the PLY: positions as 24-bit
fixed point, 8-bit scales, alphas and colours, quantised SH, and gzip. Version 3 stores
rotations as the smallest three quaternion components (10 bits each); version 2 used 8-bit xyz.
Version 4 (the reference library's default since v3.0.0) switches to ZSTD streams, which
loaders that predate it cannot read.

The reference encoder is C++ (`cli_tools/ply_to_spz`). Building and running it here was refused
by the session's permission check, as code fetched from outside.

## Decision

1. `just bundle` (`scripts/bundle.sh`) puts `splat/splat.spz` in each dataset's zip instead of
   the PLY. The PLY stays in the export output.
2. The file is **SPZ version 3** (gzip, smallest-three rotations), for compatibility with
   loaders that do not read version 4 yet.
3. `scripts/spz.py` is a port of the reference's `packGaussians` and `serializePackedGaussians`
   (`src/cc/load-spz.cc` at v3.0.0), with the same defaults as `ply_to_spz`: coordinates
   `UNSPECIFIED`, so the PLY's axes are kept; 12 fractional bits; SH with 5 bits (degree 1)
   and 4 bits (degrees 2 and up).
4. Every bundle checks the round trip: `spz.py check` decodes the file, compares it with the
   PLY, and fails past the format's limits (position half a step, rotation 0.5°, opacity
   0.0025).

## Consequences

- Sizes: 16.1 MB and 11.6 MB, and bundles of 30.2 MB and 37.7 MB. Round-trip errors on both:
  position ≤ 0.00012, rotation ≤ 0.235°, opacity ≤ 0.002.
- The port's decoder reads Niantic's two version 2 sample files consistently (unit
  quaternions, scales within range, the antialias flag), and a version 2 encode of ours fails
  `check` on rotation (12° max). No file was opened in a third-party SPZ viewer while this was
  decided.
- Viewers that assume SPZ's internal RUB axes may show the scene flipped, because the axes are
  stored as in the PLY, as `ply_to_spz` does. The bundle README says to rotate 180° about X.
- If the reference library can be built here later, its `spz_to_ply` can confirm the port
  byte for byte. Moving to version 4 needs ZSTD and a check that the target viewers read it.
