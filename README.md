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
fingerprinting, `Stage` trait, mask path convention, tool runner (native,
podman, docker). Stages are not yet executable.

The M0 spike ran end to end with the scripts in [`spike/m0/`](spike/m0/README.md),
driven by `just` (GPU recipes need a CUDA host: see `.env.example`). Findings:
[ADR-0002](docs/adr/0002-m0-spike-findings.md).

## Usage

```sh
cargo run -- config configs/reposalmi-laajasalo.toml   # resolved config
cargo run -- plan   configs/reposalmi-laajasalo.toml   # stage order
cargo run -- tools  configs/reposalmi-laajasalo.toml   # resolved tools
cargo run -- run    configs/reposalmi-laajasalo.toml   # not implemented yet
```

Dataset configs are deep-merged over `configs/defaults.toml` in the same
directory. Unknown keys are rejected.

How each external tool runs is machine configuration: `tools.toml` beside the
dataset config, with whole tool tables replaced by a host file passed as
`--tools <file>` or `SHORELINE_TOOLS` ([ADR-0003](docs/adr/0003-tool-runtimes-are-machine-config.md)).

## Layout

```
crates/
  cli/      clap entrypoint (binary: shoreline-recon)
  core/     config, DAG, fingerprint, Stage trait, tool runner
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
