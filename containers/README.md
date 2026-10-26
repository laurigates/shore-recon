# containers

Containerfiles for external tools, pinned by version and referenced by digest
from `configs/tools.toml`. Built in CI by `.github/workflows/containers.yml`.

| Image | Purpose | Status |
|---|---|---|
| `segmentation/` | masks backend: OneFormer ADE20K Swin-L, CPU torch, weights baked in ([ADR-0006](../docs/adr/0006-masks-segmentation-container.md)) | `linux/arm64`, pushed to `ghcr.io/laurigates/shoreline-segmentation` |
| `colmap` | COLMAP with CUDA (M2) | planned; M0 used `colmap/colmap` under Apptainer |
| `brush/` | splat backend: Brush 0.3.0 release binary, Vulkan ([ADR-0011](../docs/adr/0011-splat-and-dense-images.md)) | `linux/amd64`, pushed to `ghcr.io/laurigates/shoreline-brush` |
| `openmvs/` | dense backend: OpenMVS v2.4.0, CUDA sm_89 ([ADR-0011](../docs/adr/0011-splat-and-dense-images.md)) | `linux/amd64`, pushed to `ghcr.io/laurigates/shoreline-openmvs` |
| `render/` | video backend: `render_video` (gsplat 1.5.3, CUDA sm_89; SVT-AV1 ffmpeg) renders the showcase video and its share copy ([ADR-0008](../docs/adr/0008-v1-deliverables-and-gpu-stages.md), #29) | `linux/amd64`, pushed to `ghcr.io/laurigates/shoreline-render` |

Build the segmentation image locally:

```sh
docker build -f containers/segmentation/Containerfile -t shoreline-segmentation:local containers/segmentation
```
