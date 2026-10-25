# 0004. Stage cache layout, manifest and gate evaluation

- Status: accepted
- Date: 2026-10-24

## Context

The kickoff asks for content-addressed stage caching under `work/<dataset>/<stage>/<fingerprint>/`,
a manifest recording each output's lineage, quality gates that fail a run, and a second run with an
unchanged config that is a full cache hit. Its `Stage` sketch had each stage compute its own
fingerprint and check its own outputs. Stages will run tools for tens of minutes (patch-match took
32–44 min per dataset in M0), so an interrupted run must not be mistaken for a finished one.

## Decision

1. The pipeline computes every fingerprint, not the stage: `blake3(name, stage config, upstream
   fingerprints, stage version)`. A stage supplies only `config()` (the config that determines its
   output) and `version()` (its implementation revision, tool versions, and content hashes of other
   files it reads by path, such as model weights), so no stage can leave its upstream lineage out.
2. Stages without dependencies take the content fingerprint of `dataset.input` as their upstream:
   blake3 of the file, or for a directory, of every file below it with its relative path, skipping
   names that start with `.`. A changed or re-exported capture therefore reruns the whole
   pipeline, and restoring the old file is a cache hit.
3. Each output directory gets `manifest.json` (stage, full fingerprint, hashed config, version,
   upstream stages with fingerprints, the source path and fingerprint for root stages, tool
   records with log paths relative to the output directory, metrics, creation time). It is written
   last and marks the output complete: the outputs are flushed to disk, the manifest is written to
   a flushed temporary file and renamed, and the directory entries are flushed.
4. A stage directory without a manifest is an interrupted run. The pipeline removes it and runs the
   stage again. A manifest that does not parse, or names another fingerprint, is an error and the
   directory is left alone.
5. Stages return their metrics from `run()`; the manifest stores them. Gates are a pure check of
   stored metrics against the current `[gates]` config, evaluated on every run, cached or not. A
   failed gate stops the run but keeps the output, so relaxing a threshold does not recompute. A
   passing gate can return warnings (the masks stage's masked-fraction check is warn-only); they
   are reported as events and in the run results, never stored.
6. Running a stage selects it and its transitive dependencies; with no targets, all registered
   stages run.
7. The work directory is created and made canonical (absolute, no `..`, no symlinks), so stage
   directories can be bind-mounted into containers as they are: Docker cleans a mount's target but
   not the arguments that name it, so a `..` would point at an unmounted directory. The ToolRunner
   rejects mounts containing `..`. `dataset.name` must be a single path component of letters, digits, `.`, `_` and
   `-`, starting with a letter or digit.

## Consequences

- Changing a stage's config or version reruns that stage and its dependents. Earlier outputs stay
  on disk under their own fingerprints, so switching back is a cache hit; nothing deletes old
  outputs yet.
- `[gates]` is not part of any fingerprint.
- Every run hashes the dataset input, including fully cached runs (0.4 s for each 750 MB
  Reposalmi clip on the M-series Mac), and fails if the input is missing.
- Config floats must be finite: NaN and the infinities all serialise to JSON `null` and would
  share a fingerprint, so config validation rejects them.
- Gate thresholds that need data outside the metrics map cannot be expressed; such a stage records
  the value as a metric.
- Two runs of the same dataset at the same time are not supported: both could run a stage into the
  same directory.
- Metrics must be finite numbers. JSON has no NaN, so a non-finite metric is an error when the
  manifest is written.
