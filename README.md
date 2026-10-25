# shoreline-recon

Water-aware photogrammetry and Gaussian splatting pipeline for shoreline
structures. Water, sky and transient objects are masked out of every
reconstruction stage; a synthetic water plane replaces the water surface in
post-processing.

First datasets: Reposalmi pier, Laajasalo and Vartiosaari sides.

Design and milestones: [docs/kickoff.md](docs/kickoff.md). Decisions:
[docs/adr/](docs/adr/).

## Status

M1 in progress. Implemented: config loading and merging, stage DAG,
fingerprinting, `Stage` trait, stage cache and pipeline runner, tool runner
(native, podman, docker), the `frames` stage for video input
([ADR-0005](docs/adr/0005-frames-stage.md)), and the `masks` stage on the
segmentation container ([ADR-0006](docs/adr/0006-masks-segmentation-container.md),
[ADR-0007](docs/adr/0007-masks-stage-output.md)). `run` executes these; SfM and
later stages are M2 onwards. The masks stage pulls a private GHCR image, so
Docker needs `docker login ghcr.io` with a token that has `read:packages`.

The M0 spike ran end to end with the scripts in [`spike/m0/`](spike/m0/README.md),
driven by `just` (GPU recipes need a CUDA host: see `.env.example`). Findings:
[ADR-0002](docs/adr/0002-m0-spike-findings.md).

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
  frames/   masks/   sfm/   dense/   splat/   post/
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
