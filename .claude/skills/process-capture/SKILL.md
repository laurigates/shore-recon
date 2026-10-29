---
name: process-capture
description: Run a new shoreline video through the whole pipeline to the v1 deliverables — config, GPU batch, checks, demo video and download bundle. Use when given a new drone/phone clip of a pier, shore or waterfront to reconstruct, or asked to repeat the Reposalmi process on other footage.
allowed-tools: Bash, Read, Edit, Write
created: 2026-10-26
modified: 2026-10-29
reviewed: 2026-10-26
---

# Processing a new capture

The pipeline turns one video into a water-masked splat, a mesh with a water plane, step-by-step
and final videos, a demo and a download bundle. Stages and decisions: README § Status and
`docs/adr/`. GPU host setup and image pins: `.claude/rules/remote-gpu-host.md`.

## 1. Before running: is the footage usable?

- Shot per `docs/kickoff.md` § Capture guidelines: overcast, slow arcs and lines, 60–80 %
  overlap, several heights, textured surroundings in frame.
- **The camera must be level** (a drone gimbal, horizon locked). The water plane's normal comes
  from the cameras' x axes, and `post` fails when any is more than 5° from level (ADR-0012). For
  handheld footage, set a fixed height instead (step 4).
- Name the dataset `<place>-<part>`, e.g. `reposalmi-laajasalo`. The demo labels each dataset
  "<Part> side" from the last segment.

## 2. Add the dataset

```sh
cp <clip>.mp4 data/raw/<name>.mp4
printf '[dataset]\nname  = "<name>"\ninput = "data/raw/<name>.mp4"\n' > configs/<name>.toml
```

Anything in `configs/defaults.toml` can be overridden in that file (unknown keys are rejected).
Usual knobs: `[frames] fps` (2.0; raise for a short clip), `[masks] classes` (ADE20K names to
ignore), `[video] views` and `step_seconds`.

## 3. Run

```sh
export SHORELINE_DATASETS="<name> [<name2> ...]"       # optional; default: every dataset config
SHORELINE_TOOLS=configs/<host>.local.toml just batch   # detached; logs in tmp/runs/
just batch-status
```

`SHORELINE_GPU_HOST` (and `SHORELINE_STOP_SERVICE` to free VRAM, confirmed with the user) as in
the GPU rule. One dataset in the foreground: `just run <name>`. Timings on the 2026-10 captures
(~120 frames each):

| Stage | Where | Time |
|---|---|---|
| frames, masks | this Mac (masks in local Docker, needs `docker login ghcr.io`) | minutes |
| sfm | GPU host | 3–6 min |
| dense | GPU host | 10–26 min (mesh size) |
| splat | GPU host | ~17 min |
| post, export | this Mac | ~15 s |
| video | GPU host | ~3 min; +15 min once per new render image (SIF pull) |

Every stage caches by fingerprint (ADR-0004): a rerun only redoes what changed.

## 4. Check the results

- **Gates** stop the run: registered ratio, reprojection error and track length (sfm); geometry
  below the water and camera tilt (post).
- **`post: warning: mesh has N faces, over post.target_faces`**: texture seams stopped the
  decimation. The mesh is fine; #55 tracks decimating before texturing.
- **Water plane**: read `work/<name>/post/*/water_plane.json`. If the fit failed (fewer than 20
  waterline points) or the plane sits wrong in `mesh-arc.mp4`, set `[post] water_plane = <height>`
  in the dataset config: a height along the same normal, near the fitted `plane.height`.
- **Look at the videos**: a contact sheet beats watching them.
  `ffmpeg -v error -i <video>.mp4 -vf "select='not(mod(n\,90))',scale=480:-1,tile=4x3" -frames:v 1 sheet.png`
  The step clips show where things went wrong: masks missing water (`step-2-masks`), a broken
  camera path (`step-3-sparse`), holes (`step-4-dense`).

## 5. Deliver

```sh
SHORELINE_DEMO_PLACE="<Pier, City>" SHORELINE_DEMO_CAPTURED="drone capture <date>" just demo
SHORELINE_DEMO_PLACE="<Pier, City>" SHORELINE_DEMO_CAPTURED="drone capture <date>" just bundle
just share work/demo/shoreline-recon-v1-demo-share.mp4 "<caption>"   # SHORELINE_SHARE_CMD
just share work/bundle/<name>.zip "<caption>"
```

- `just demo` (ADR-0013): title, then per dataset six step clips and the splat and mesh along
  every view, with chapters and the timeline bar. About 3 min for two datasets.
- `just bundle`: per dataset, `mesh/model.glb`, `splat/splat.spz` (SPZ v3, ~8x smaller than the
  PLY; `scripts/spz.py` fails the bundle if its round trip passes the quantisation limits;
  ADR-0014),
  `water_plane.json`, `report.json` and a README.
- Size limits: `SendUserFile` 30 MiB; the share command's channel may have its own limit.
  The full 1920 px demo of two datasets is ~150 MB; its 1280 px share copy is ~46 MiB. For the
  chat panel, re-encode smaller (`-vf scale=960:-2 -crf 44`, ~20 MiB).

## When a container image changes

Images publish only from `main`; pin the new digest in the host tools file and the stages that
use it re-run. Details: `.claude/rules/remote-gpu-host.md` § Images and pins.
