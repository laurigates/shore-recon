# 0011. Brush and OpenMVS run from pinned images on the GPU host

- Status: accepted
- Date: 2026-10-26
- Amends: ADR-0002, decisions 3 and 4

## Context

The splat and dense stages run on the GPU host (ADR-0008) through the ToolRunner's remote
container path (ADR-0009). The M0 spike ran the Brush 0.3.0 release binary natively there, and
COLMAP MVS for dense because no maintained OpenMVS image existed (ADR-0002). The kickoff's dense
backend is OpenMVS. Remote tools must be containers, because the bind maps the mirrored mounts
back to the Mac's paths.

Probed on the GPU host on 2026-10-26 (Apptainer 1.5.5, driver 580.126.18):

- `apptainer exec --nv` binds the host's NVIDIA Vulkan ICD (`nvidia_icd.json`,
  `libGLX_nvidia.so.0`). On Ubuntu 24.04 that library needs `libX11` and `libXext`. With both
  installed, `vulkaninfo` inside the container lists the RTX 4090.
- Brush 0.3.0, 500 steps on the spike's masked laajasalo dataset: 19.9 s in the container and
  18.4 s natively, model load included.
- Without `--nv`, and with no software Vulkan driver in the image, Brush exits 101 ("No possible
  adapter available").

Read from OpenMVS v2.4.0's source: `DensifyPointCloud --mask-path <dir>` loads
`<image stem>.mask.png` per image, and `--ignore-mask-label <v>` drops pixels with value `v`
from the depth maps. Both the CPU and the CUDA (`PatchMatchCUDA`) estimators apply the mask.

## Decision

Decided by the project owner on 2026-10-26: Brush is packaged as an image, and dense uses
OpenMVS.

1. `containers/brush` wraps the checksum-verified Brush 0.3.0 release binary on Ubuntu 24.04
   with `libvulkan1`, `libx11-6` and `libxext6`. Mesa's Vulkan drivers are left out, so a
   missing GPU fails the run instead of training on the CPU.
2. `containers/openmvs` builds OpenMVS v2.4.0 with CUDA for sm_89 (the RTX 4090), with Eigen
   3.4.0, CGAL v6.0.1, nanoflann v1.9.0 and VCG pinned to a commit. It also builds OpenCV 4.12.0:
   OpenMVS v2.4.0 writes JPEG XL through `cv::IMWRITE_JPEGXL_QUALITY` (OpenCV 4.11 and later),
   and Ubuntu 24.04 ships OpenCV 4.6. 4.12.0 is an established release with JPEG XL that
   predates OpenMVS v2.4.0. The runtime stage installs only the packages
   that own the binaries' shared libraries. OpenMVS is built at commit `eeedab7`: v2.4.0 plus
   upstream's fix for cdcseacave/openMVS#1251, without which `TextureMesh`'s seam leveling
   turns every textured face black (added 2026-10-26, after the first M3 runs).
3. Both images are `linux/amd64`, built in CI and pinned by digest in the host tools file.
4. Dense masks use OpenMVS's native support (`--mask-path`, `--ignore-mask-label 0`, black
   being ignore). Checked on laajasalo on 2026-10-26: with every mask all black, densify
   produced 0 points. Unmasked it gave 2,474,072, and with the real masks 2,291,871. A
   missing mask file stops the run.

## Consequences

- The OpenMVS image runs only on sm_89 GPUs. Another GPU needs a rebuild with its
  `CUDA_ARCH`.
- Brush and OpenMVS read undistorted images, so their masks must be undistorted too, and named
  as each tool expects: `00001.png` for Brush and `00001.mask.png` for OpenMVS, against
  COLMAP's `00001.jpg.png`.
- Brush runs about 8% slower in the container than natively in the probe.

## Decimation before texturing

Added 2026-10-31 (#55). Post's seam-keeping decimation stops above `post.target_faces` on
vartiosaari (ADR-0012), so dense now decimates inside `TextureMesh`, which then builds its atlas
for the reduced mesh. After `ReconstructMesh`, dense reads the face count of `mesh.ply` and
passes `--decimate <ratio>` with ratio = `post.target_faces * dense.texture_headroom / faces`.

- `dense.texture_decimate` (default `true`) turns it on. Dense reads `post.target_faces`
  itself; there is no second target key. With decimation on, `post.target_faces` is part of
  the dense stage config, so changing it re-runs dense.
- `dense.texture_headroom` (default 0.95, in (0, 1]) leaves room for the faces OpenMVS adds
  after decimating: with `--decimate`, `TextureMesh` also closes holes of up to 30 boundary
  edges (`--close-holes` default) in the same `Mesh::Clean` pass. Post only removes faces, so
  it cannot push the count back over the target.
- The flag is omitted when the ratio would be 1 or more, or the mesh has no faces. The ratio is
  written with six decimals and clamped to 1e-6, because OpenMVS reads 0 as "auto".
- `mesh.ply` and the `mesh_faces` metric remain the undecimated `ReconstructMesh` output.
- Pending: re-running dense on both datasets on the GPU host, the resulting face counts, and a
  speckle check of the mesh videos.
