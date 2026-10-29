---
name: splat-videos
description: The pipeline's showcase and processing-step videos — which clips the video stage renders (splat/mesh x arc/drone, six step clips), how to change views or framing, how render_video's camera paths work, and checking renders. Use when asked for splat or mesh videos, orbit/flythrough renders, step-by-step clips, or a different camera path.
allowed-tools: Bash, Read
created: 2026-10-24
modified: 2026-10-29
reviewed: 2026-10-26
---

# Showcase and step videos

The `video` stage (`crates/video`) runs the render image's `render_video`
(`containers/render/render_video.py`) on the GPU host. For the whole run, from a new clip to
the deliverables, use the `process-capture` skill.

## What it renders, per dataset

| File | Source | Camera |
|---|---|---|
| `splat-<path>.mp4` (+ `-share`) | post's culled `splat.ply` | each of `[video] views` |
| `mesh-<path>.mp4` (+ `-share`) | post's `model.glb` + `water_plane.json` | each of `[video] views` |
| `step-1-frames.mp4` | frames `images/` | flipbook |
| `step-2-masks.mp4` | masks `overlays/` | slideshow |
| `step-3-sparse.mp4` | sfm `points3D.bin` (+ camera frusta) | first view's path |
| `step-4-dense.mp4` | dense `scene_dense.ply` | first view's path |
| `step-5-mesh-raw.mp4` | dense `mesh_textured.glb` | first view's path |
| `step-6-splat-raw.mp4` | splat `splat.ply` (before the cull) | first view's path |

Config (`configs/defaults.toml` `[video]`): `views = [{ path = "arc", seconds = 16.0 }, { path =
"drone", seconds = 20.0 }]`, `step_seconds = 8.0`, size, fps, `share_max_mib`. Export copies all
of them; `just demo` joins them.

## render_video scene modes

The first argument picks the mode: a `.ply` with `f_dc_0` is a splat (gsplat, CUDA); any other
`.ply` or a `points3D.bin` is a point cloud (OpenGL points, frusta for `points3D.bin`); a `.glb`
is a mesh (OpenGL over headless EGL; `--frame water_plane.json` for post's model, none for the
raw mesh); a directory is an image slideshow. The second argument is always the splat stage's
`dataset/sparse/0` (cameras and intrinsics).

## Camera paths

- **arc**: centre = the point nearest all training cameras' optical axes; up = orthogonal to
  every camera's x axis (level gimbal). Sweeps 85 % of the filmed azimuth (10th–90th
  percentile) on a sine, so the loop has no seam.
- **drone**: the recorded poses in frame order, smoothed (σ 2.5 frames), played forward then
  back, pulled back by `--pullback` (0.35) × the median camera-to-centre distance.
- `--fov-scale` (0.8) widens the view relative to the training camera.

## Checking renders without the pipeline

Copy `render_video.py` and inputs to a scratch dir on the GPU host and run it in the pinned
render image with a short clip (`--seconds 3 --fps 4`), then pull frames into a contact sheet:

```sh
apptainer exec --nv --bind <root> docker://<render image@digest> python <scratch>/render_video.py \
  <scene> <splat>/dataset/sparse/0 <meta.json> <scratch>/test.mp4 --seconds 3 --fps 4
```

Use `python <script>` so an edited copy runs instead of the image's `render_video`. Headless GL
needs `libOpenGL.so.0` + `libEGL.so.1` (the GPU rule has why).

## Gotchas

- The splat's drone view shows bright artefacts at the frame edges where few training views
  reached; the culled splat removes floaters over water, not unconstrained background.
- A new render image means a ~15 min SIF pull on the first run that uses it.
- The M0 spike's `gpu-videos` / `gpu-timeline` recipes (`spike/m0/justfile`) render the spike's splats, not the
  pipeline's; prefer the video stage.
