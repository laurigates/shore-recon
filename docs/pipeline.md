# Pipeline

Water reflects the shore and moves between frames, so photogrammetry and splat training
treat it as geometry and fill it with noise. The pipeline masks water, sky and transient
objects out of every reconstruction stage, then adds a flat synthetic water plane back
in post-processing. Design and milestones: [kickoff.md](kickoff.md).

## Stages

| Stage | Does | Backend | Decisions |
|---|---|---|---|
| `frames` | extracts frames at `frames.fps`, scales them and drops blurry ones | ffmpeg, variance-of-Laplacian filter | [ADR-0005](adr/0005-frames-stage.md) |
| `masks` | segments water, sky, people and boats; dilates and writes COLMAP masks and review overlays | OneFormer ADE20K Swin-L in a pinned image | [ADR-0006](adr/0006-masks-segmentation-container.md), [ADR-0007](adr/0007-masks-stage-output.md) |
| `sfm` | camera poses and sparse points from the masked frames; gates registration, reprojection error and track length | COLMAP 4.2.1 | [ADR-0010](adr/0010-sfm-metrics-from-the-model.md) |
| `dense` | dense points, a mesh and its texture, with the masks applied | OpenMVS v2.4.0 | [ADR-0011](adr/0011-splat-and-dense-images.md) |
| `splat` | trains a Gaussian splat on the masked, undistorted images | Brush 0.3.0 | [ADR-0011](adr/0011-splat-and-dense-images.md) |
| `post` | fits the water plane, removes geometry below it, decimates the mesh, culls floaters over the water | Rust | [ADR-0012](adr/0012-post-stage.md) |
| `video` | optional (`export.videos`): showcase videos of the splat and the mesh, and a clip per step | gsplat 1.5.3 and OpenGL, SVT-AV1 | [ADR-0008](adr/0008-v1-deliverables-and-gpu-stages.md), [ADR-0017](adr/0017-installed-cli-and-config-layers.md) |
| `export` | gathers the deliverables and writes `report.json` | Rust | [ADR-0008](adr/0008-v1-deliverables-and-gpu-stages.md) |

Each stage's output is cached under a fingerprint of its config, its inputs' fingerprints
and its tool versions; a rerun with nothing changed is a cache hit, and a change reruns
the stage and everything after it ([ADR-0004](adr/0004-stage-cache-and-manifest.md)).

## Technology

| Area | Technology |
|---|---|
| Language | Rust (edition 2024, toolchain 1.99.1), clap |
| Frame extraction | ffmpeg |
| Segmentation | OneFormer ADE20K Swin-L (transformers, CPU torch) |
| Structure from motion | COLMAP 4.2.1 |
| Dense reconstruction | OpenMVS v2.4.0 |
| Gaussian splatting | Brush 0.3.0 |
| Rendering | gsplat 1.5.3, OpenGL (moderngl), SVT-AV1 |
| Tool runtimes | native, podman, Docker, Apptainer; local or over ssh |
| Viewer | Python 3, three.js, Spark |
| Testing | cargo-nextest, Python `unittest` |
| Releases | release-please, dist, Homebrew tap ([ADR-0016](adr/0016-releases-with-release-please-and-dist.md)) |
