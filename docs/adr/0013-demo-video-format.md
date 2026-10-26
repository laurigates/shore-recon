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
2. The bar is drawn from the chapters by `scripts/timeline_bar.py`. It has three rows:
   - the groups (the text before the first ` · ` of each chapter title: the dataset side) over
     their spans, the current one highlighted;
   - a slim track with one block per chapter and no text in it, coloured by kind (indigo for
     processing steps, teal for splats, amber for meshes), played chapters brighter than
     upcoming ones, with a playhead and progress through the current chapter;
   - a caption with the current chapter in full and its position in the group
     (`Laajasalo side · 3/10 · sparse`), the next chapter, and elapsed / total time.

   No label has to fit inside a block. The first design labelled every block, and on the
   234 s demo with 21 chapters an 8 s block was about 55 px wide, so labels were cut to
   "fra…". The bar runs under `uv` with Pillow and NumPy pinned to the render image's
   versions. It does not use ffmpeg's `drawtext`, because Homebrew's ffmpeg is built without
   it.
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
