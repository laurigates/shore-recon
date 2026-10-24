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
   output) and `version()` (its implementation revision and tool versions), so no stage can leave
   its upstream lineage out.
2. Each output directory gets `manifest.json` (stage, full fingerprint, hashed config, version,
   upstream stages with fingerprints, tool records, metrics, creation time). It is written last,
   through a temporary file and a rename, and marks the output complete.
3. A stage directory without a manifest is an interrupted run. The pipeline removes it and runs the
   stage again. A manifest that does not parse, or names another fingerprint, is an error and the
   directory is left alone.
4. Stages return their metrics from `run()`; the manifest stores them. Gates are a pure check of
   stored metrics against the current `[gates]` config, evaluated on every run, cached or not. A
   failed gate stops the run but keeps the output, so relaxing a threshold does not recompute.
5. Running a stage selects it and its transitive dependencies; with no targets, all registered
   stages run.

## Consequences

- Changing a stage's config or version reruns that stage and its dependents. Earlier outputs stay
  on disk under their own fingerprints, so switching back is a cache hit; nothing deletes old
  outputs yet.
- `[gates]` is not part of any fingerprint.
- Gate thresholds that need data outside the metrics map cannot be expressed; such a stage records
  the value as a metric.
- Two runs of the same dataset at the same time are not supported: both could run a stage into the
  same directory.
- Metrics must be finite numbers. JSON has no NaN, so a non-finite metric is an error when the
  manifest is written.
