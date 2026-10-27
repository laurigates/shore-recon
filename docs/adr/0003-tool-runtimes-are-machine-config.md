# 0003. Tool runtimes are machine configuration

- Status: accepted
- Date: 2026-10-24

## Context

Stages run external tools (ffmpeg, COLMAP, Brush, the segmentation backend) through one
`ToolRunner` (kickoff § Tool execution). How a tool runs differs per machine, not per dataset:
in M0, ffmpeg ran natively on the Mac, while the GPU host ran COLMAP from an OCI image under
Apptainer, a static ffmpeg build and a Brush release binary (ADR-0002). Dataset configs are
committed and must not name a host or its paths (`.claude/rules/remote-gpu-host.md`).

## Decision

1. Tool settings live in `tools.toml` beside the dataset config, as `defaults.toml` does, with
   one table per tool: `runtime` (`native`, `podman`, `docker` or `apptainer`), `program`, `image`, `gpu`.
   The committed `configs/tools.toml` holds native defaults.
2. A host file given with `--tools` or `SHORELINE_TOOLS` replaces whole tool tables. It is not
   a deep merge, so switching a tool from a container to native does not inherit its `image`.
3. Container images are pinned by digest (`name@sha256:…`). Loading rejects unknown keys, an
   `image` or `gpu` on a native tool, and a container tool without an image.
4. Containers bind-mount each directory an invocation names at the same path (`-v /p:/p`), so
   arguments are host paths under every runtime. GPU passthrough is `--gpus all` for docker and
   `--device nvidia.com/gpu=all` for podman (CDI, podman ≥ 4.1). Apptainer runs
   `apptainer exec [--nv] --bind /p:/p docker://<image> <program> …`: the same OCI digest, which
   Apptainer converts to a SIF once and caches under `APPTAINER_CACHEDIR` (#13).
5. Each run writes its command line, stdout and stderr to one log and returns a record (tool,
   runtime, image, argv, duration, log) for the stage manifest. A non-zero exit is an error
   naming the log.
6. The tool version a stage passes to its fingerprint is the image reference for containers,
   and for native tools the first stdout line of a probe the stage chooses (`ffmpeg -version`).

## Consequences

- Moving a run to another machine changes native tool versions and therefore fingerprints, so
  cached outputs are not reused across hosts with different builds.
- The runtime is not fingerprinted: the same image under docker or podman reuses the cache.
- Apptainer passes the host environment into the container and mounts `$HOME` by default,
  unlike docker and podman. The spike ran COLMAP the same way (ADR-0002, decision 5).
- Docker runs as root by default, so on Linux its bind-mounted outputs would be root-owned and an
  interrupted stage's directory could not be discarded (#20). The docker runtime therefore passes
  `--user <uid>:<gid>` of the pipeline process (on a remote host, the ssh user's, from
  `$(id -u):$(id -g)` in the remote shell) and `--env HOME=/tmp`, since that uid usually has no
  passwd entry in the image. Images must run as a non-root user: no writes outside the mounts
  and `/tmp`. Rootless podman and Apptainer already run as the invoking user and get neither.
  Rootless docker would map that uid to a subordinate id; use podman on such a machine.
- Paths containing `:` cannot be expressed in `-v` or `--bind` and are rejected; under Apptainer
  so are paths containing `,`, which `--bind` treats as a list separator.
- Host tools files are not committed. Keep them outside the repository or name them
  `*.local.toml`, which is git-ignored.
