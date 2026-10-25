# 0008. v1 deliverables, and COLMAP on the GPU host

- Status: accepted
- Date: 2026-10-25
- Amends: ADR-0002, decision 6

## Context

The kickoff left open whether the primary deliverable is the mesh, the splat or both (#17). The M0
spike also rendered preview videos of the splats with gsplat (ADR-0002, decision 6), and recorded
them as not part of the v1 pipeline. M1 runs frames and masks on the Mac: ffmpeg natively, and the
segmentation image (`linux/arm64`, CPU) in local Docker (ADR-0006). SfM is next (M2).

## Decision

Decided by the project owner on 2026-10-25:

1. The v1 outputs are the mesh (GLB with a water plane), the splats (PLY) and a showcase video
   rendered from the splats, with `report.json` alongside. The showcase video becomes a pipeline
   output instead of a spike-only preview.
2. COLMAP runs on the GPU host (ADR-0002: RTX 4090, Apptainer). The dense and splat stages, which
   also need the GPU, follow it there.

## Consequences

- M2 depends on the Apptainer runtime (#13) and on a way to run stages on the GPU host. Nothing
  supports that yet: the pipeline runs every tool on the machine where `shoreline-recon` runs.
- Fingerprints include native tool versions (ADR-0004), so frames run with the Mac's ffmpeg do
  not hit the cache on a host with another ffmpeg build. Where frames and masks run relative to
  SfM is part of the M2 design.
- The segmentation image is `linux/arm64` only; masks on the GPU host need an `amd64` variant
  (ADR-0006).
- The showcase video needs a stage of its own, built from the spike's renderer
  (`spike/m0/remote/render_orbit.py`, `timeline.py`) and the `splat-videos` project skill.
