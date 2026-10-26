# 0013. The demo video carries chapters and a timeline bar

- Status: accepted
- Date: 2026-10-26

## Context

`just demo` joins each dataset's splat and mesh videos from the export stage into one video
(ADR-0008's showcase deliverable). The first v1 demo had MP4 chapters but nothing on screen
to show where the viewer is or what comes next, and a player that does not show chapters
gives no way to jump to a part. The M0 spike's combined video
(`spike/m0/remote/timeline.py`) drew a timeline bar under the picture, and the project owner
asked for the same on the v1 demo and for it to be part of the process.

## Decision

1. Every demo built by `just demo` has one MP4 chapter per part: the title card, then for each
   dataset a clip per processing step (frames, masks, sparse, dense, raw mesh, raw splat; the
   video stage's `step-<n>-<name>.mp4`), then the splat and the mesh along each of
   `video.views` (arc and drone by default). Parts are
   titled `<group> · <part>` (for example `Laajasalo side · mesh · drone`), and there is a
   timeline bar under the picture.
   The bar is 80 px at 1920x1080, which gives a 1920x1160 frame.
2. The bar is drawn from the chapters by `scripts/timeline_bar.py`, the spike's bar applied
   to a finished video. It has one block per chapter, grouped and labelled by the text before
   ` · `, the current chapter highlighted (teal for splats, amber for meshes), a playhead
   with progress through the chapter, and elapsed / total time. Processing steps are drawn in
   indigo. It runs under `uv` with Pillow
   and NumPy pinned to the render image's versions. It is not drawn with ffmpeg's `drawtext`,
   because Homebrew's ffmpeg is built without it.
3. `scripts/demo-video.sh` checks the full video and the share copy before it reports
   success: the frame height has to include the bar, and the chapter count has to equal the
   number of parts.

## Consequences

- Any chaptered video can get the same bar with `timeline_bar.py`; nothing in it is specific
  to the demo.
- The bar pass re-encodes the video. The joined intermediate is encoded at CRF 20, so the
  second encode (CRF 30) is not stacked on a low-quality input. `just demo` takes about 50 s
  for the 66 s v1 demo.
- The video stage's per-view videos (`<scene>-<path>.mp4`) have no bar: each is a single loop.
