# shoreline-recon

Water-aware photogrammetry and Gaussian splatting pipeline for shoreline
structures. Water, sky and transient objects are masked out of every
reconstruction stage; a synthetic water plane replaces the water surface in
post-processing.

First datasets: Reposalmi pier, Laajasalo and Vartiosaari sides.

Design and milestones: [docs/kickoff.md](docs/kickoff.md). Decisions:
[docs/adr/](docs/adr/).

## Status

M1 to M4 are implemented. `run` takes a capture through `frames`, `masks`, `sfm`,
`dense`, `splat`, `post`, `video` and `export`. The v1 deliverables end up in
`work/<dataset>/export/<fingerprint>/`: `model.glb` (the mesh with its water
surface), `splat.ply`, one video per scene and view (`splat-arc.mp4`,
`splat-drone.mp4`, `mesh-arc.mp4`, `mesh-drone.mp4`, from `video.views`) with share
copies, a short clip per processing step (`step-1-frames.mp4` to
`step-6-splat-raw.mp4`: frames, masks, sparse points and cameras, dense points, the
mesh and the splat before post), `water_plane.json` and `report.json`.

| Stage | Backend | Runs on | Decisions |
|---|---|---|---|
| `frames` | ffmpeg, blur filter | this machine | [ADR-0005](docs/adr/0005-frames-stage.md) |
| `masks` | OneFormer segmentation image | this machine (Docker) | [ADR-0006](docs/adr/0006-masks-segmentation-container.md), [ADR-0007](docs/adr/0007-masks-stage-output.md) |
| `sfm` | COLMAP 4.2.1; metrics read from the model | GPU host | [ADR-0008](docs/adr/0008-v1-deliverables-and-gpu-stages.md), [ADR-0010](docs/adr/0010-sfm-metrics-from-the-model.md) |
| `dense` | OpenMVS v2.4.0, native masks, textured GLB | GPU host | [ADR-0011](docs/adr/0011-splat-and-dense-images.md) |
| `splat` | Brush 0.3.0 | GPU host | [ADR-0011](docs/adr/0011-splat-and-dense-images.md) |
| `video` | gsplat (splat) and OpenGL (mesh) renderer, AV1 plus share copies | GPU host | [ADR-0008](docs/adr/0008-v1-deliverables-and-gpu-stages.md) |
| `post` | water plane, mesh cleanup and decimation, splat floater cull | this machine | [ADR-0012](docs/adr/0012-post-stage.md) |
| `export` | deliverables and `report.json` | this machine | [ADR-0008](docs/adr/0008-v1-deliverables-and-gpu-stages.md) |

The tool runner supports native, podman, docker and Apptainer runtimes. A
container tool can run on a remote host over ssh, with its mounts mirrored by
rsync ([ADR-0009](docs/adr/0009-remote-tools-over-ssh.md)). The images are
private GHCR packages: Docker needs `docker login ghcr.io`, and Apptainer on the
GPU host needs `apptainer registry login`, each with a token that has
`read:packages`.

`just demo` joins each dataset's splat and mesh videos for every view (from its latest export)
behind a title card into `work/demo/shoreline-recon-v1-demo.mp4`, with a chapter
per part, a timeline bar under the picture for seeking, and a share copy
([ADR-0013](docs/adr/0013-demo-video-format.md)).

`just overnight` runs every dataset detached and logs to `tmp/runs/`. It can
stop a GPU service on the host for the run (`SHORELINE_STOP_SERVICE`; see
`.env.example`).

The M0 spike ran end to end with the scripts in [`spike/m0/`](spike/m0/README.md),
driven by `just` (GPU recipes need a CUDA host: see `.env.example`). Findings:
[ADR-0002](docs/adr/0002-m0-spike-findings.md).

## Processing a new capture

Copy the clip to `data/raw/<name>.mp4`, write `configs/<name>.toml` (`[dataset] name` and
`input`), set `SHORELINE_DATASETS`, then `just overnight`, `just demo` and `just bundle`. The
`process-capture` project skill (`.claude/skills/process-capture/SKILL.md`) has the whole
runbook: footage checks, timings, result checks and delivery.

## Usage

```sh
cargo run -- config configs/reposalmi-laajasalo.toml   # resolved config
cargo run -- plan   configs/reposalmi-laajasalo.toml   # stage order
cargo run -- tools  configs/reposalmi-laajasalo.toml   # resolved tools
cargo run --release -- run configs/reposalmi-laajasalo.toml   # implemented stages
cargo run --release -- run --stage frames configs/reposalmi-laajasalo.toml
```

`run` caches each stage's output under `work/<dataset>/<stage>/<fingerprint>/`
(`--work` or `SHORELINE_WORK` moves the root); a rerun with an unchanged
config and input is a cache hit ([ADR-0004](docs/adr/0004-stage-cache-and-manifest.md)).
Relative paths in a config, such as `dataset.input`, are resolved against the
working directory, so run from the repository root.

Dataset configs are deep-merged over `configs/defaults.toml` in the same
directory. Unknown keys are rejected.

How each external tool runs is machine configuration: `tools.toml` beside the
dataset config, with whole tool tables replaced by a host file passed as
`--tools <file>` or `SHORELINE_TOOLS` ([ADR-0003](docs/adr/0003-tool-runtimes-are-machine-config.md)).

## Layout

```
crates/
  cli/      clap entrypoint (binary: shoreline-recon)
  core/     config, DAG, fingerprint, Stage trait, cache, pipeline, tool runner
  frames/   masks/   sfm/   dense/   splat/   video/   post/
containers/ Containerfiles, pinned versions
configs/    defaults.toml + per-dataset overrides, tools.toml
docs/       kickoff, ADRs
tests/fixtures/
```

Runtime directories `work/`, `data/` and `models/` are git-ignored.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run                       # unit tests
cargo nextest run --profile integration # tests named *::integration::*, need tools/GPU
```
