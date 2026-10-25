# 0006. Masks: OneFormer in a pinned container, post-processing in Rust

- Status: accepted
- Date: 2026-10-25

## Context

ADR-0002 chose OneFormer ADE20K Swin-L (`shi-labs/oneformer_ade20k_swin_large`, MIT) through
`transformers` for masks. The kickoff's default masks backend is ONNX through `ort`, but exporting
OneFormer (#11) is unproven. The project owner chose a container backend first, running in local
Docker on the CPU, built in CI and pushed to GHCR, with the weights baked into the image.

Measured on the M-series Mac with CPU torch 2.14.1 and transformers 5.20.2: 2.2–3.6 s per
3200×1800 frame natively (the processor resizes to 640×1137). In this image under OrbStack, with a
4 GB memory limit, the 121 Laajasalo frames took 437 s (3.6 s per frame), about 7 min per dataset.

Against the spike's masks (GPU, fp16), after the same 8 px elliptical dilation, the image's masks
on those 121 frames reach a mean IoU of 0.995 and a mean masked fraction of 0.503 (spike: 0.504).
The lowest frame, `00049.jpg` at 0.87, is the spike masking a dry gravel path as water (#12); this
run does not. Every other frame is at 0.955 or above.

## Decision

1. `containers/segmentation/` builds an image from `python:3.12-slim-bookworm` (pinned by digest),
   with every Python package pinned by `constraints.txt` (the built image's `pip freeze`; CPU torch
   comes from PyTorch's CPU index), and the model
   weights at revision `4a5bac8e64f82681a12db2e151a4c2f4ce6092b2` under `/opt/models/oneformer`.
   The processor's class metadata (`ade20k_panoptic.json`), which it would otherwise download from
   the dataset repo `shi-labs/oneformer_demo` at run time, is baked beside the weights at revision
   `4d683bd5bf84e9c8b5537dce306230bde409fe89`. Runs are offline. The image is `linux/arm64` for
   now; an `amd64` CUDA variant for the GPU host comes with the Apptainer runtime (#13).
2. `.github/workflows/containers.yml` builds the image on every change to it and pushes
   `ghcr.io/laurigates/shoreline-segmentation` only from `main`; the QEMU and BuildKit images the
   build uses are pinned by digest. The masks stage pins the digest as the
   `segment` tool in `configs/tools.toml`, so the digest pins code and weights, and it is the
   stage's tool version (ADR-0003).
3. The container contract: `segment --classes <c,…> IMAGES OUT` writes `OUT/<image name>.png` for
   every `IMAGES/*.jpg`: 8-bit, 255 where the pixel belongs to one of the classes (ADE20K label
   names, first synonym), 0 elsewhere, at the image's resolution and undilated. It prints the class
   fraction per image and exits 2 on an empty or unknown class list. Class scores are upsampled 16
   classes at a time (same result as the processor's `post_process_semantic_segmentation`), which
   keeps peak memory near 3 GB for a 3200×1800 frame; upsampling all 150 at once needs about 5.2 GB
   and is killed under a 4 GB container limit. `segment --check --classes …` loads
   the model and validates the classes; the image build runs it with the default classes.
4. The masks stage in Rust turns these into COLMAP masks (dilation, black = ignore,
   `masks/<image>.png`), writes the review overlays, and computes the masked-fraction metrics and
   the warn-only gate.

## Consequences

- The image is 2.2 GB (torch comes from PyTorch's CPU index; PyPI's aarch64 wheel bundles CUDA 13
  and made it 6.9 GB); the first pull takes a while. The package is private, like the
  repository, so pulling needs `docker login ghcr.io` with a token that has `read:packages`
  (`gh auth refresh -s read:packages`, then `gh auth token | docker login ghcr.io -u laurigates
  --password-stdin`).
- A rebuild resolves the same package versions; it still yields a new digest (new layers), which
  reruns masks and everything after it.
- CI builds the arm64 image under QEMU, which is slow but needs no arm64 runner.
- On OrbStack, a read-only bind mount (`-v dir:dir:ro`) whose target is under `/Users` shows up
  as a missing directory inside the container; read-write mounts at the same path work (observed
  2026-10-25, Docker 29.4.0). The ToolRunner mounts read-write, so stages are unaffected; manual
  `docker run` commands should not add `:ro`.
- With transformers 5.20.2 the checkpoint loads with the Swin backbone's final `layernorm` reported
  as missing; the comparison with the spike's masks above shows no effect on the output.
- `transformers` builds OneFormer's training loss at construction, so scipy is installed although
  inference does not use it.
