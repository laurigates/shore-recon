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
6. Behaviour on an ssh drop (#44, added 2026-10-31). The container command does not run inside
   the ssh session. One ssh call creates `<root>/.shoreline-runs/<run-id>/` and an empty
   `out.log` there, and starts a wrapper under `nohup` (and `setsid` where the host has it) with
   all streams redirected, so ssh returns at once. The wrapper writes its pid to `pid`, runs the
   command with its output appended to `out.log`, and writes the exit code to `status` through
   `status.tmp` and `mv`. Every `poll_secs` seconds a short ssh call prints the state, read
   before the log, and the bytes of `out.log` past what the Mac already has; those bytes are
   appended to the stage log, which the progress display tails. The state is the exit code,
   running, starting (`out.log` exists but the wrapper has not written `pid`), dead (the
   wrapper's pid is gone without a `status`: killed, or the host restarted), or missing. A run
   that answers starting to more than `reconnect_attempts` consecutive polls fails as never
   started, which covers a launch whose connection dropped after it created `out.log` but before
   the wrapper ran. A failed call (ssh exit 255, any other non-zero exit, or output without the state
   line) is retried after `reconnect_delay_secs`, doubled per consecutive failure up to
   `reconnect_max_delay_secs`; a successful poll resets the count. After more than
   `reconnect_attempts` consecutive failures the run fails with an error that names the host and
   the run directory and says the command may still be running there; the directory is left in
   place with its `out.log` and `status`. If the launch call itself fails, the next poll decides:
   a running or finished command is not started again, a missing one is. Exit 0 removes the run
   directory and the outputs are pulled back; another exit code removes it and fails the step
   with that code; dead or a directory that disappeared mid-run fail the step with the log named.
   The rsync push and pull still run as plain ssh calls and are not resumed: a drop during a
   transfer fails that step, and rerunning the stage pushes again with `--delete`.

## Consequences

- Each remote call moves its mounts both ways. For SfM that is the frames and masks, about 125 MB
  per clip (`du` of the current `work/`), against minutes of COLMAP. Pulling back an unchanged
  input transfers nothing.
- Outputs are pulled back only after a successful run. A failed remote run leaves its partial
  outputs under `root`, and the next push with `--delete` replaces them.
- Mirrored inputs stay on the host under `root` until someone deletes them. The `just clean-remote` recipe (dry run by default) lists and removes mirror directories with no matching stage directory under the local `work/`.
- A remote native tool (the Brush release binary, per ADR-0002) would need argument translation
  and is not supported. Brush runs from an image, or this ADR is extended.
- The host needs `rsync`, `ssh` key access in batch mode, and the container runtime named in the
  table.
- A connection drop during the container run costs retries rather than the run, as long as the
  connection comes back within the reconnect budget (with the defaults the retry delays alone sum
  to 435 s, plus up to 30 s of connect timeout per call). The
  ssh calls of a detached run set `ServerAliveInterval` and `ConnectTimeout`, so a dead
  connection fails a call within about a minute instead of hanging. Unit tests cover the command
  building, the poll output parsing, the poll decisions and the backoff; integration tests in
  `crates/core/src/tools/detached.rs` drive the whole loop through a fake `ssh` that drops chosen
  calls, kills the wrapper, or fails the launch after it ran. A real drop test against the GPU
  host has not been run.
