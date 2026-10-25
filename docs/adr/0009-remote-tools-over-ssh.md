# 0009. Container tools run on a remote host over ssh

- Status: accepted
- Date: 2026-10-25
- Extends: ADR-0003

## Context

ADR-0008 puts COLMAP, and the dense and splat stages after it, on the GPU host, while frames and
masks run on the Mac. The ToolRunner (ADR-0003) runs every tool on the machine where
`shoreline-recon` runs. #28 listed three ways to reach the host: run the whole pipeline there,
split the run and sync `work/` between machines, or let the ToolRunner run single tools remotely.
Running on the host needs an `amd64` segmentation image (ADR-0006), and under either of the first
two options the host's ffmpeg changes the frames fingerprint (ADR-0004), so frames would rerun.

Probed on the GPU host (Apptainer 1.5.5): `--bind <dir>:/Users/…` mounts at a destination that
does not exist in the image, and files the container writes there are owned by the invoking user.
A non-interactive ssh session there does not set `APPTAINER_CACHEDIR`, so pulls would land in
`~/.apptainer` on a nearly full root filesystem.

## Decision

Decided by the project owner on 2026-10-25: the pipeline stays on the Mac, and the ToolRunner runs
single tools on the remote host.

1. A container tool may have a `[<tool>.remote]` table: `host` (ssh destination), `root` (absolute
   directory there) and an optional `env` table for the remote command, such as
   `APPTAINER_CACHEDIR`. It lives in the host tools file (`--tools`), which is not committed.
   Native tools cannot be remote.
2. A run creates the mirror directories, pushes every mount `/p` to `<root>/p` with
   `rsync -a --delete`, runs the container over `ssh -o BatchMode=yes` with `<root>/p` bound at
   `/p`, and pulls every mount back with `rsync -a`. The tool sees the same paths as on the Mac,
   so arguments need no translation, which keeps ADR-0003, decision 4.
3. The remote command is built from single-quoted words, so the remote login shell (zsh on the
   GPU host) does not split or expand them. rsync hands remote paths to that shell unquoted, so
   remote mounts and `root` are limited to `[A-Za-z0-9/._-+@]`.
4. The command line, the tool's output and the rsync output go to the stage's log on the Mac. The
   manifest's tool record names the host.
5. The host is not fingerprinted, as the runtime is not: the image digest pins the tool.

## Consequences

- Each remote call moves its mounts both ways. For SfM that is the frames and masks, about 125 MB
  per clip (`du` of the current `work/`), against minutes of COLMAP. Pulling back an unchanged
  input transfers nothing.
- Outputs are pulled back only after a successful run. A failed remote run leaves its partial
  outputs under `root`, and the next push with `--delete` replaces them.
- Mirrored inputs stay on the host under `root` until someone deletes them.
- A remote native tool (the Brush release binary, per ADR-0002) would need argument translation
  and is not supported. Brush runs from an image, or this ADR is extended.
- The host needs `rsync`, `ssh` key access in batch mode, and the container runtime named in the
  table.
