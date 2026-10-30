# 0019. Local tool profile: every stage but the videos without a GPU host

- Status: accepted
- Date: 2026-10-30
- Amends: ADR-0017 consequence 2

## Context

ADR-0017 built `configs/tools.toml` into the binary, but only with ffmpeg and the
segmentation image, so `shoreline-recon <video>` stopped after masks unless a user or
host tools file supplied COLMAP, OpenMVS and Brush, which so far meant the CUDA
images on a remote host (ADR-0009, ADR-0011). The README's quick start promised a
single command. The error came only after frames and masks had run, because each
stage resolved its tool when it was reached.

Measured on an Apple Silicon Mac against the GPU host (RTX 4090), on
`reposalmi-vartiosaari` (117 frames), tool seconds from each stage's manifest:

| Stage | GPU host | Local | Local tool |
|---|---|---|---|
| sfm | 373 s | 533 s | Homebrew COLMAP 4.2.1, CPU |
| splat | 1,015 s | 2,172 s | Brush 0.3.0 release binary, Metal |
| dense | 1,513–1,586 s | 2,130 s | OpenMVS v2.4.0 without CUDA, in Docker |

Both registered all 117 frames (0.70 and 0.69 px mean reprojection error). The splats
have 445,500 and 443,403 Gaussians. The local dense cloud has 3.16 M points and 2.75 M
faces against 3.48 M and 3.03 M, 9% fewer from the CPU depth-map path.

## Decision

Decided by the project owner on 2026-10-30:

1. The built-in `tools.toml` gains `[colmap]` and `[brush]` as native tools and
   `[openmvs]` as `ghcr.io/laurigates/shoreline-openmvs-cpu` in Docker, so every stage
   but `video` runs on the local machine with no tools file. A host file still
   replaces these tables whole, so a remote GPU host is unchanged.
2. `containers/openmvs-cpu/` builds the same pinned OpenMVS sources as
   `containers/openmvs` without CUDA, for `linux/arm64` and `linux/amd64`, each on a
   native runner. On `main` a manifest job joins them into one index; the default pins
   the index digest, so Docker picks the host's architecture. The image stays pinned
   by digest, as ADR-0011 requires.
3. The pipeline resolves every selected stage's version before the first stage runs,
   so a missing tool table or a native program that is not installed fails at once,
   naming the table. Versions and fingerprints are unchanged; only the time they are
   computed moves.
4. The Homebrew formula depends on `ffmpeg`, `colmap` and
   `laurigates/tap/shoreline-brush` (dist `[dist.dependencies.homebrew]`, run stage
   only). Homebrew core's `brush` is an unrelated shell, so the tap carries a
   hand-written formula for the Brush release binary. Docker remains a prerequisite.
5. No runtime is picked by probing the machine: the same command and tools files
   give the same tools on every machine (ADR-0003).

## Consequences

- `brew install shoreline-recon` plus a running Docker is enough for
  `shoreline-recon pier.mp4` on Apple Silicon. Both GHCR packages it pulls
  (segmentation, openmvs-cpu) are public, so no `docker login` is needed; ADR-0006's
  login note predates that.
- A local run is about 1.6× the GPU host's tool time on the three stages above
  (4,835 s against about 2,950 s, before the remote host's transfers).
- CPU runs are new fingerprints: COLMAP's version string and the openmvs image differ
  from the CUDA ones, so local and GPU outputs never share a cache entry. ADR-0018's
  estimates still mix both under one work root (#90).
- `--videos` still needs a `render` tool from a host file; gsplat needs CUDA.
- On x86_64 Linux the segmentation image was `linux/arm64` only until #89, which added
  `linux/amd64` to its index.
- Bumping Brush means changing `containers/brush/Containerfile` and the tap formula
  together.
