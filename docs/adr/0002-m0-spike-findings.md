# 0002. M0 spike: backends and mask handling for the Reposalmi captures

- Status: accepted
- Date: 2026-10-24

## Context

M0 asks for a manual run on real Reposalmi data, a check of mask handling in the dense and
splat backends, and a segmentation-model choice. On 2026-10-23 two DJI 4K clips were captured:
`DJI_0849` (Laajasalo side, 64.4 s) and `DJI_0703` (Vartiosaari side, 65.8 s), 3840×2160 at
29.97 fps with per-frame SRT telemetry, flown 0.3–4.1 m below the takeoff point. Exposure was
not locked: shutter varies from 1/50 to 1/160 s within each clip.

The spike ran with the scripts in `spike/m0/` (see its README) on a CUDA host (RTX 4090, 24 GB):
frames at 2 fps (long edge 3200 px, variance-of-Laplacian < 100 dropped), masks, COLMAP SfM,
COLMAP MVS, and Brush splats, each with and without masks.

## Decision

1. **Masks: OneFormer ADE20K Swin-L** (`shi-labs/oneformer_ade20k_swin_large`, MIT) through
   `transformers`, classes water, sea, river, lake, sky, person, boat, dilated 8 px. SegFormer
   and Mask2Former ADE20K weights are tagged `license:other` and were not used. Masks follow
   the COLMAP convention (`masks/00001.jpg.png`, black = ignore).
2. **SfM: COLMAP 4.2.1 (CUDA)** with masks at feature extraction, 16,384 SIFT features, single
   camera; sequential matcher for Laajasalo, exhaustive for Vartiosaari. DSP-SIFT stayed off
   because it runs on the CPU only.
3. **Dense: COLMAP MVS** (`patch_match_stereo` with geometric consistency, `stereo_fusion` with
   `--StereoFusion.mask_path`, `poisson_mesher`), not OpenMVS. No maintained OpenMVS image
   exists (the only Docker Hub image dates from 2019) and building 2.4.0 with CUDA was not
   attempted. OpenMVS native mask support therefore remains unverified.
4. **Splat: Brush 0.3.0** on COLMAP-undistorted images (PINHOLE). Brush reads only focal length
   and principal point, ignoring distortion. Brush supports masks, but finds them **by file
   stem**: `images/00001.jpg` needs `masks/00001.png`; COLMAP-named masks are silently not
   found. The spike copies masks under Brush's names.
5. **Runtime on the GPU host: Apptainer** running `colmap/colmap@sha256:c1de5161…` with `--nv`.
   The host's distro podman (3.4.4) predates CDI GPU passthrough. `ToolRunner` (M1) needs an
   Apptainer arm beside podman/docker; images stay OCI, pinned by digest, so all three runtimes
   consume the same artefact.
6. **Preview videos: gsplat 1.5.3** rasterising the Brush PLYs along paths built from the SfM
   cameras, encoded to AV1 (SVT-AV1). Not part of the v1 pipeline.

## Consequences

Measured results (`model_analyzer`, step timings from the run log):

| | Laajasalo | Vartiosaari |
|---|---|---|
| Frames kept / extracted | 121 / 129 | 117 / 132 |
| SfM unmasked: registered, reproj., track length | 121/121, 0.72 px, 7.09 | 117/117, 0.70 px, 8.29 |
| SfM masked: registered, reproj., track length | 121/121, 0.72 px, 7.12 | 117/117, 0.70 px, 8.32 |
| Mean masked area (undistorted) | 50 % | 40 % |
| Brush 30k steps, masked / unmasked | 14 m 27 s / 14 m 08 s | 14 m 24 s / 12 m 24 s |
| Gaussians, masked / unmasked | 595,317 / 691,124 | 446,435 / 545,676 |
| Patch-match, masked @1600 / unmasked @1200 | 44 / 33 min | 43 / 32 min |

- All v1 SfM gates pass with and without masks; masking did not cost registration.
- Masked splats hold 14–18 % fewer Gaussians. Their water renders dark with unconstrained
  floaters, as Brush never trained on those pixels; the post stage (water plane, cleanup)
  must remove them. Unmasked splats render textured water, which is the reflection problem
  the kickoff describes.
- Masked and unmasked dense runs used different resolutions, so their sizes are not
  comparable; a same-resolution comparison is still open.
- OneFormer over-masks wet gravel as water: the "over-masking removes shoreline features" risk.
- Brush prints no evaluation metrics headless, so no PSNR was recorded.
- Open: OpenMVS build and mask check; DSP-SIFT; sequential matching with loop detection;
  near-duplicate frame dropping; mesh inspection; locked-exposure recapture.
