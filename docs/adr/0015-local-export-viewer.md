# 0015. A local web viewer for the export outputs

- Status: accepted
- Date: 2026-10-27

## Context

The deliverables (ADR-0008) are a splat PLY, a mesh GLB with its water plane, and videos. The
videos show fixed camera paths; checking a run, or showing it to someone, needs the splat and
the mesh in an interactive view, with the raw outputs beside the post-processed ones. The
kickoff's M5 web viewer (TypeScript, for hosting) is not started. Third-party viewers take one
file at a time and do not know the model frame: `splat.ply` stays in the reconstruction frame
while `model.glb` is in the model frame (ADR-0012).

## Decision

1. `just viewer` runs `scripts/viewer.py`, a standard-library HTTP server bound to 127.0.0.1.
   It serves the page, `/index.json` and the files under `work/` at `/work/`. The index is
   rebuilt on every request from `work/<dataset>/export/<id>/`, newest first, so a finished run
   shows up on reload. Directory listings, hidden files and paths outside `work/` are refused.
2. Per export output, the page has five layers: the splat (`splat.ply`), the mesh and the water
   plane (the nodes of `model.glb`), and the splat and mesh before post. Those two are the splat
   and dense stage outputs named by the fingerprints in its `report.json`. It also plays the
   export's videos (not the share copies) and shows the post metrics.
3. Everything is drawn in the model frame. The PLY splats and the raw mesh are placed with
   `model_from_reconstruction` from `water_plane.json`.
4. Rendering: three.js 0.186.1 with Spark 2.3.1 (World Labs, MIT) for the splats, loaded from
   jsDelivr through an import map. Spark reads Brush's PLY with degree-3 SH directly, sorts
   splats together with three.js meshes, and is a single ES module. The alpha-blended water is
   drawn before the splats and writes depth, so it covers only what lies below the surface.
5. `scripts/test_viewer.py` covers the index and the path checks, and serves a temporary
   `work/`; `just check` runs it.

## Consequences

- The page needs network access to jsDelivr for the libraries; the data never leaves the
  machine. Pinned versions keep it reproducible; vendoring them is the step if it has to work
  offline.
- The full PLYs load as they are (102 and 137 MB for the Reposalmi splats). That is fine over
  localhost; a hosted viewer (M5) would load SPZ (ADR-0014) instead.
- The rendering was checked in headless Chromium with a synthetic export (a splat box on a
  water plane, a raw mesh in the reconstruction frame): the frames line up and the water no
  longer covers the splat. It has not been opened on the Reposalmi exports in this change.
- The viewer does not cover the earlier stages' intermediate outputs (frames, masks, sparse and
  dense point clouds); the step clips show those.
