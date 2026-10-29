# M0 spike tooling

Scripts that ran the M0 spike end to end (frames, masks, SfM, dense, splats, preview videos)
before the Rust stages exist. Findings and decisions: [ADR-0002](../../docs/adr/0002-m0-spike-findings.md).
These are spike artefacts: the `crates/` stages replace them, and they hard-code the two
Reposalmi datasets and the `manual-2026-10-23` run tag (override with `SHORELINE_TAG`).

Drive everything through `spike/m0/justfile` (`just -f spike/m0/justfile --list`; paths are
relative to the repository root). GPU recipes need `SHORELINE_GPU_HOST` and
`SHORELINE_GPU_ROOT` (see `.env.example` and `.claude/rules/remote-gpu-host.md`).

## Flow

```sh
just -f spike/m0/justfile frames              # local: data/raw/<dataset>.mp4 -> work/<dataset>/frames/<tag>/
just -f spike/m0/justfile gpu-setup           # once per host: COLMAP image, envs, gsplat build, Brush, ffmpeg
just -f spike/m0/justfile gpu-sfm             # unmasked SfM baseline (minutes)
just -f spike/m0/justfile gpu-overnight       # detached: masks, masked SfM, Brush splats, COLMAP MVS (~4 h)
just -f spike/m0/justfile gpu-status          # step markers
just -f spike/m0/justfile gpu-fetch           # splats, dense clouds, meshes, logs -> work/
just -f spike/m0/justfile gpu-videos arc      # one looping AV1 video per splat (or: drone)
just -f spike/m0/justfile gpu-timeline        # all renders in one video with a timeline strip + chapters
just share-copy work/renders/reposalmi-splats-timeline.mp4   # < 30 MiB copy for attaching
```

## Layout

| Path | Runs on | Purpose |
|---|---|---|
| `local/frames_manual.py` | here | blur score per frame; frames below the threshold move to `rejected/` |
| `local/make_meta.py` | here | overlay panel data per splat, read from SRT, `frames.csv`, `model_analyzer.txt`, PLY headers, Brush logs |
| `remote/env.sh` | GPU host | shared paths, caches under `SHORELINE_GPU_ROOT`, the `colmap` wrapper |
| `remote/setup.sh`, `get_ffmpeg.sh` | GPU host | idempotent bring-up |
| `remote/sfm.sh` | GPU host | unmasked SfM for one dataset |
| `remote/overnight.sh` | GPU host | the batch; STEP markers in `overnight-<tag>.log` |
| `remote/masks.py` | GPU host | OneFormer ADE20K masks, COLMAP naming, overlays every 12th frame |
| `remote/render_orbit.py` | GPU host | gsplat render along an arc or the drone path, info sidebar, AV1 or raw frames |
| `remote/timeline.py`, `timeline.sh` | GPU host | concatenation with timeline strip and chapters |
| `remote/videos.sh` | GPU host | `render_orbit.py` over all four splats |
