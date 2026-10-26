# 0012. The post stage: a level water plane, mesh cleanup and a masked-water splat cull

- Status: accepted
- Date: 2026-10-26
- Amends: kickoff § post ("Fit a plane with RANSAC to sparse points sampled along mask
  boundaries at the waterline") and § export / report (`model.glb` layout)

## Context

The post stage (M4, #4, #5, #40) turns the dense mesh and the splat into the v1 deliverables
(ADR-0008): a mesh with a water plane, and a splat without the floaters that the masked water
leaves behind. The inputs share one frame: the COLMAP model's. The OpenMVS mesh's vertices
lie within a median 0.023 units of the sparse points, and Gaussians with opacity above 0.5
within 0.032.

The captures are DJI clips with a gimbal that keeps the horizon level. Each camera's x axis is
within 0.86° (laajasalo) and 0.60° (vartiosaari) of a common horizontal plane.

The masks are binary (ADR-0007), so a waterline cannot be told from a skyline by its class. In
an image, though, water lies below its boundary and sky lies above. The kickoff's RANSAC fit was
prototyped on both datasets, using sparse points that sit just above a masked region in most of
their views:

| | laajasalo | vartiosaari |
|---|---|---|
| waterline points | 2,112 | 1,708 |
| free RANSAC plane, tilt from camera up | 3.0° | 3.1° |
| height clusters along camera up (lowest first) | −1.74, −1.47 | −1.34, −1.08 (taller) |

The tilted planes cut through the pier side. Pier deck edges also border water from above in
steep views, and they form the second, higher cluster. On vartiosaari that cluster is the taller
one.

## Decision

1. **Water plane.** The normal is the direction orthogonal to all camera x axes (the smallest
   eigenvector of their scatter matrix), signed against the cameras' y axes. A gate fails the
   stage when any camera's x axis is more than 5° from level. The height is the lowest local
   maximum, at ≥ 25 % of the highest, of a Gaussian kernel density of the waterline points'
   heights. The bandwidth is 1 % of their median distance from their median. The height is then
   refined to the median of the points within two bandwidths. A waterline point is a sparse point
   observed at least twice, at least half of the time on a kept pixel with an ignored pixel within
   `post.waterline_band_px` (24) rows below it. `post.water_plane` can instead fix the height
   along the same normal. The drawn planes meet the rock waterline in both datasets.
2. **Model frame.** `model.glb` is in a frame with +Y along the normal and the water at y = 0. The
   origin is the mean camera centre projected onto the plane, and +X is the mean camera x axis.
   `splat.ply` stays in the reconstruction frame: rotating degree-3 SH coefficients buys nothing
   for v1. `water_plane.json` carries the plane, the fit's numbers and the 4x4
   `model_from_reconstruction` transform. Units are the reconstruction's, not metres.
3. **Mesh.** Faces with any vertex below the plane are removed. So are connected components
   (positions welded by exact equality, because texture seams duplicate vertices) with fewer than
   `post.min_component` of the faces. The result is decimated to `post.target_faces` with
   meshoptimizer, which collapses onto existing vertices, so UVs stay valid. The first pass keeps
   seams; if seams block the target, a second pass collapses across them, weighted by UV error,
   and the `decimation_across_seams` metric records it. The texture is downscaled to
   `post.texture_max_size` (4096) and embedded as JPEG (quality 90). The water is a separate node:
   a rectangle around the mesh with a 50 % margin and an alpha-blended material.
4. **Splat.** A Gaussian is removed when its centre is below the plane, when no training view sees
   it, or when more than `post.floater_max_water` (0.5) of the views that see it see it on an
   ignored pixel along a ray that points down. That last condition separates water from sky, so
   treetops seen against the sky stay. Visibility uses the splat stage's undistorted cameras and
   masks. Culling by opacity (< 0.05, 17 % of Gaussians) made no visible difference in GPU
   renders, so it is not done.
5. **Gates.** The stage fails if any mesh vertex or splat centre is below the plane after post
   (kickoff § Quality gates: 0), or if the cameras are not level (decision 1).

## Consequences

- Results on the 2026-10-26 inputs:

  | | laajasalo | vartiosaari |
  |---|---|---|
  | plane inliers / waterline points | 803 / 2,112 | 473 / 1,708 |
  | mesh faces in → below water → out | 1,959,305 → 22,372 → 500,000 | 3,032,860 → 18,451 → 499,994 |
  | small components removed | 23 | 45 |
  | Gaussians in → below / unseen / over water → kept | 595,136 → 9,304 / 2,064 / 4,424 → 579,344 | 445,500 → 6,938 / 1,621 / 3,017 → 433,924 |

  Vartiosaari needs the cross-seam pass to reach 500,000 faces: the seam-keeping pass stops at
  743,787.
- The stage runs on this machine and needs no tools: about 12 s per dataset.
- A capture without a level gimbal (handheld phone) fails the tilt gate. It then needs a fixed
  height, or another source for the normal.
