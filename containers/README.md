# containers

Containerfiles for external tools, pinned by version and referenced by digest
from `configs/tools.toml`. Built in CI by `.github/workflows/containers.yml`.

| Image | Purpose | Status |
|---|---|---|
| `segmentation/` | masks backend: OneFormer ADE20K Swin-L, CPU torch, weights baked in ([ADR-0006](../docs/adr/0006-masks-segmentation-container.md)) | `linux/arm64`, pushed to `ghcr.io/laurigates/shoreline-segmentation` |
| `colmap` | COLMAP with CUDA (M2) | planned; M0 used `colmap/colmap` under Apptainer |
| `openmvs` | OpenMVS (M3) | planned (#8) |

Build the segmentation image locally:

```sh
docker build -f containers/segmentation/Containerfile -t shoreline-segmentation:local containers/segmentation
```
