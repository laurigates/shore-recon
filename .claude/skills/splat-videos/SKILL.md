---
name: splat-videos
description: Render looping preview videos of the Brush Gaussian splats (arc or drone-path camera, stats sidebar, AV1) and the combined timeline video. Use when asked for splat videos, orbit/flythrough renders, or a shareable preview.
allowed-tools: Bash, Read
created: 2026-10-24
modified: 2026-10-24
reviewed: 2026-10-24
---

# Splat preview videos

Renders run on the GPU host with gsplat; recipes and scripts are in `justfile` (group `gpu`) and
`spike/m0/remote/`. Host settings: `.claude/rules/remote-gpu-host.md`.

## Commands

| Want | Run |
|---|---|
| One video per splat, arc camera (16 s loop) | `just gpu-videos arc` |
| One video per splat, drone-path camera (20 s loop) | `just gpu-videos drone` |
| Custom length / framing | `just gpu-videos arc 24 --fov-scale 0.7` (extra args go to `render_orbit.py`) |
| All eight in one video, timeline strip + chapters | `just gpu-timeline` |
| Copy small enough to attach (< 30 MiB) | `just share-copy work/renders/<file>.mp4` |

Both GPU recipes rebuild the sidebar data first (`just meta`), so the panel always reflects the
artefacts in `work/`. They need the splats and cameras from `just gpu-overnight`.

## How the camera paths are built

- **Arc**: centre = least-squares point nearest to all training cameras' optical axes; up = the
  direction orthogonal to every camera's x axis (a level gimbal keeps x horizontal; averaging
  camera up vectors tilts the horizon by the gimbal pitch). Sweeps `SWEEP` (0.85) of the
  10th–90th-percentile filmed azimuth on a sine, so the loop has no seam.
- **Drone**: the recorded camera poses in frame order, Gaussian-smoothed (σ 2.5 frames), played
  forward then back with cosine easing, and pulled back along the view direction by
  `--pullback` (0.35) × the median camera-to-centre distance.
- Both widen the field of view with `--fov-scale` (0.8 = 20 % wider than the training camera).

## Knobs and what they fix

| Symptom | Change |
|---|---|
| Subject too close / cropped | lower `--fov-scale` (zoom out) or raise `--pullback` (drone) |
| Ends of the arc show unfilmed blur and floaters | lower `SWEEP` in `render_orbit.py` |
| Too fast | longer seconds argument |
| Panel covers the scene | it does not by design: the panel is a 600 px sidebar (`--sidebar`) |

## Checking a change before rendering everything

`render_orbit.py … --still N` writes frame N as PNG; `--check` renders training view 0, which
must match the source frame (camera convention check). Look at the frames at the arc extremes
(`N` = frames/4 and 3·frames/4) for horizon tilt and floaters.

## Gotchas

- Masked splats render the water dark with streaks: Brush never trained there. That is expected
  until the post stage adds the water plane.
- Raw-frame mode (`out = -`) redirects every other stdout write to stderr; anything printing to
  stdout in the renderer would otherwise corrupt the frame stream.
- The attachment limit is 30 MiB; the 1920×1160 timeline is ~75 MiB, the `share-copy` ~30 MiB.
