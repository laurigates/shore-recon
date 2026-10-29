# Configuration

## Config layers

A run's config is built from these layers, each deep-merged over the one before
([ADR-0017](adr/0017-installed-cli-and-config-layers.md)). Tables merge recursively;
any other value replaces the one below it.

1. the defaults built into the binary ([`configs/defaults.toml`](../configs/defaults.toml),
   which documents every key);
2. the user's `config.toml`;
3. `defaults.toml` beside the dataset config, when there is one;
4. the dataset config, or `--config` for a video;
5. command-line options (`--fps`, `--steps`, ...).

`shoreline-recon config [<dataset config>]` prints the result.

## Tool layers

How each external tool runs is machine configuration, not dataset configuration
([ADR-0003](adr/0003-tool-runtimes-are-machine-config.md)). Tools come from these layers;
a table in a later layer replaces the whole table for that tool:

1. the tools built into the binary ([`configs/tools.toml`](../configs/tools.toml), which
   documents the keys);
2. the user's `tools.toml`;
3. `tools.toml` beside the dataset config;
4. `--tools <file>` or `SHORELINE_TOOLS`.

`shoreline-recon tools [<dataset config>]` prints the result. Each tool table:

| Key | Meaning |
|---|---|
| `runtime` | `native`, `podman`, `docker` or `apptainer` |
| `program` | the executable (native) or the command inside the image |
| `image` | OCI image pinned by digest, containers only |
| `gpu` | pass the host GPUs through, containers only |
| `[<tool>.remote]` | run the container on another host over ssh ([ADR-0009](adr/0009-remote-tools-over-ssh.md)) |

The built-in tools run ffmpeg natively and the segmentation model in Docker
(`linux/arm64`, CPU; [ADR-0006](adr/0006-masks-segmentation-container.md)). COLMAP,
OpenMVS, Brush and the video renderer come from a user or host tools file for now.

## User files

The user directory is `SHORELINE_CONFIG_DIR`, else `$XDG_CONFIG_HOME/shoreline-recon`,
else `~/.config/shoreline-recon` (on macOS too). It may hold `config.toml` and
`tools.toml`. Subcommands given a dataset config read them as well; the library's
`Config::load` does not, so tests and recipes stay independent of the user's files.

## The remote GPU host

The CUDA images for COLMAP, OpenMVS, Brush and the video renderer are `linux/amd64`,
built for sm_89 ([containers/README.md](../containers/README.md),
[ADR-0011](adr/0011-splat-and-dense-images.md)). They run on a CUDA host over ssh under
Apptainer: each call mirrors its mounts to the host with rsync, runs the container there
and pulls the outputs back. The host needs ssh key access in batch mode, rsync and
Apptainer, logged in to `ghcr.io`.

A host tools file names the host and pins the images, for example
`configs/<host>.local.toml` (git-ignored, never committed):

```toml
[colmap]
runtime = "apptainer"
program = "colmap"
image   = "<image>@sha256:<digest>"
gpu     = true

[colmap.remote]
host = "<ssh destination>"
root = "<absolute directory on the host>"
env  = { APPTAINER_CACHEDIR = "<cache directory on the host>" }
```

Pass it with `--tools` or `SHORELINE_TOOLS`, or put the tables in the user's
`tools.toml`.
