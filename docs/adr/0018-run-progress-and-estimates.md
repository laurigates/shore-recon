# 0018. Run progress and estimates

- Status: accepted
- Date: 2026-10-30
- Amends: ADR-0004 decision 3 (a new manifest field)

## Context

A run takes 40–60 minutes across seven stages, and the CLI printed `dense: running in
<dir>` and then nothing until the stage ended: up to 26.5 minutes of silence
(reposalmi-vartiosaari dense). It gave no step, elapsed time, estimate or summary.

Two Reposalmi runs with 121 and 117 frames took 7 s for frames, about 7 min for masks,
3–6 min for sfm, 10–26 min for dense and about 17 min for splat. The frame counts are
almost the same, yet dense differs 2.5×, so a model of input size would not predict the
time. None of cargo, nextest, uv, buck2, buildx or hyperfine estimates durations from
earlier runs. indicatif's `{eta}` is a rate estimator weighted over 15 s, which is wrong
for steps that take minutes.

## Decision

Decided by the project owner on 2026-10-30:

1. **History is the cache.** Each manifest records the stage run's wall time,
   `seconds` (absent in older manifests, not part of the fingerprint). The estimate for
   a stage is the range of durations over every
   `<work root>/<dataset>/<stage>/<fingerprint>/manifest.json`, all datasets together,
   using a manifest's tool time when it has no `seconds`. It reads `usual A–B` from two
   or more runs and `last A` from one. A manifest that cannot be read is skipped:
   history is advisory and never fails a run. There is no separate store.
2. **Steps come from log files.** `Stage::steps(config)` lists a stage's tool steps by
   their log stems, and `Stage::progress(step, log, frames)` reads a fraction from a
   step's log so far: OpenMVS's last `(NN.NN%,` for dense, `num_reg_frames` over the
   kept frames for the sfm mapper, and segmented frames over kept frames for masks. Every
   tool log is a local file under `<out_dir>/logs/`, remote ones included (ssh streams
   into it), so the CLI tells the current step by the newest log. `Tool::run` and the
   stages' call sites are unchanged. `pipeline::Event::Running` carries the stage's
   position in the run and its estimate.
3. **indicatif renders, in the CLI crate only.** On a terminal (stderr), finished
   stages scroll above two live lines: the running stage with its step, percentage,
   elapsed time, estimate and the last line its tool printed, and the run's total with
   the time left (the rest of the current stage's range plus the stages expected to run
   after it). Otherwise, as in CI and `just batch` logs, plain lines: start, each step,
   a heartbeat every 60 s, and finish. There is no flag. Both modes end with a summary
   table on stderr. `run`'s tab-separated summary on stdout is unchanged.
4. **Splat progress is deferred.** Brush 0.3.0 logs nothing by default; it is said to
   log `Refine iter N` under `RUST_LOG=info`, which needs a probe on the GPU host first.

## Consequences

- Estimates improve as the cache fills, and quick mode's cache in the user cache
  directory gathers history across clips. Outputs made before this change count with
  their tool time, which leaves out in-process work such as frame scoring and post.
- Stages expected to run after the current one are those depending on a stage that
  ran; a stage rerun for its own config change shows in the total line only once it
  starts.
- A Ctrl-C leaves the last drawn live lines on the terminal: there is no signal
  handler.
- A stage that renames a log changes what `steps` must return; each stage's tests pin
  its list.
