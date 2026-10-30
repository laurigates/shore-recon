# 0005. Frames stage: extraction, blur metric and output layout

- Status: accepted
- Date: 2026-10-25

## Context

The kickoff's frames stage extracts frames from video at a configurable rate, caps the long edge,
and drops blurry frames by the variance of the Laplacian. The M0 spike did this with ffmpeg
(`fps=2,scale=3200:-2:flags=lanczos`, `-q:v 2`) and a Python/OpenCV script, and calibrated
`blur_threshold = 100` on OpenCV's numbers (ADR-0002). M1 stages run external tools only through
the `ToolRunner` (ADR-0003), and the pipeline owns caching (ADR-0004).

## Decision

1. ffmpeg, as the tool named `ffmpeg` in `tools.toml`, extracts frames with `fps=<fps>` followed by
   a scale that caps the long edge in either orientation and never enlarges:
   `scale='if(gte(iw,ih),min(L,iw),-2)':'if(gte(iw,ih),-2,min(L,ih))':flags=lanczos`, written as
   `-q:v 2` JPEGs numbered from `00001.jpg`. For the landscape 4K captures this is byte-identical to
   the spike's filter.
2. The blur metric is computed in Rust as OpenCV computes it: the JPEG's Y channel decoded directly
   by `zune-jpeg` 0.5.15 (libjpeg does the same for OpenCV's grayscale read), the 4-neighbour
   Laplacian with reflect-101 borders, and the population variance. This holds for the YCbCr and
   grayscale JPEGs ffmpeg writes; CMYK and Adobe RGB-transform JPEGs, which photo input (#23) may
   meet, are not decoded as OpenCV does. The `image` crate's grayscale
   conversion is not used: it applies Rec. 709 weights, which miss OpenCV by up to 65 % on the
   blurry fixture.
3. Output: `images/` (kept frames), `rejected/` (frames below the threshold, kept for review),
   `frames.csv` (every frame's score at full precision) and `logs/ffmpeg.log`. The input is
   resolved through symlinks before it is mounted, and a work path containing `%` is refused,
   since ffmpeg would read it as part of the frame-number pattern. Metrics: `extracted`, `kept`,
   `kept_ratio`. A run that keeps no frame fails the stage.
4. The stage version is `frames/<revision> <ffmpeg version>`: the first line of `ffmpeg -version`
   for a native ffmpeg, the pinned image reference for a container (ADR-0003).
5. Photo-directory input is supported (issue #23): each image is decoded with the `image`
   crate, its EXIF orientation (read via `kamadak-exif`) is applied to the pixels, the long
   edge is capped by `max_long_edge`, and the result is written as a numbered JPEG with the
   EXIF focal length preserved (written via `little_exif`). The blur filter then runs exactly
   as for video frames. ffmpeg is not invoked for a photo directory, so `Frames::version`
   omits the ffmpeg version string in that case and `steps()` is empty. Video input continues
   to use ffmpeg as in decision 1. Near-duplicate dropping is #10.

## Consequences

Measured against the M0 spike on both Reposalmi clips: 121 of 129 and 117 of 132 frames kept, the
same counts; all 261 JPEGs byte-identical; no frame changed between kept and rejected; scores
within 0.03 of OpenCV's. A release build takes about 10.5 s per clip on the M-series Mac, about
7 s of it ffmpeg, and 0.4 s for a cached run, most of it hashing the input; a debug build is about
8 times slower.

- Synthetic fixtures in `tests/fixtures/frames/`, with OpenCV's values (OpenCV 5.0.0,
  `make_fixtures.py`), pin the metric within 1e-3 relative plus 0.02 absolute. Two tests check
  that the fixtures reject the plausible mistakes (replicate borders, Rec. 709 luma).
- A new ffmpeg version string, or a new image, changes the stage version, so frames and every later
  stage rerun. A rebuild that keeps the first `-version` line does not.
- Frame numbers have gaps where frames were rejected.
- Without `max_long_edge`, frames keep the video's resolution.
