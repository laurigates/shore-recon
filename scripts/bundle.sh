#!/usr/bin/env bash
# Bundle each dataset's 3D deliverables from its latest export into work/bundle/<dataset>.zip:
# mesh/model.glb (mesh + water plane), splat/splat.spz (the splat as SPZ v3, about 8x smaller than
# the PLY, via scripts/spz.py), water_plane.json, report.json and a README.txt. The videos are
# left out: `just demo` covers them. The SPZ round trip is checked before zipping (ADR-0014).
# Usage: bundle.sh <dataset>...    Optional: SHORELINE_DEMO_PLACE, SHORELINE_DEMO_CAPTURED (README).
# Needs jq, zip, and uv for the encoder.
set -euo pipefail
(($# > 0)) || { echo "usage: $0 <dataset>..." >&2; exit 1; }

here=$(dirname "$0")
commit=$(git rev-parse --short HEAD)
spz() { uv run -q --no-project --with numpy==2.5.4 python -I "$here/spz.py" "$@"; }

for ds in "$@"; do
    # shellcheck disable=SC2012 # fingerprint dirs are hex; newest run wins
    E=$(ls -td work/"$ds"/export/*/ 2>/dev/null | head -1 || true)
    [[ -n $E ]] || { echo "no export output for $ds; run the pipeline first" >&2; exit 1; }
    out=work/bundle/$ds
    rm -rf "$out" "$out.zip"
    mkdir -p "$out/mesh" "$out/splat"
    cp "${E}model.glb" "$out/mesh/"
    cp "${E}water_plane.json" "${E}report.json" "$out/"
    spz encode "${E}splat.ply" "$out/splat/splat.spz"
    spz check "${E}splat.ply" "$out/splat/splat.spz"
    faces=$(jq -r '.stages[] | select(.stage=="post") | .metrics.mesh_faces' "${E}report.json")
    gauss=$(jq -r '.stages[] | select(.stage=="post") | .metrics.gaussians' "${E}report.json")
    cat >"$out/README.txt" <<EOF
shoreline-recon v1: $ds${SHORELINE_DEMO_PLACE:+ ($SHORELINE_DEMO_PLACE)}
${SHORELINE_DEMO_CAPTURED:+${SHORELINE_DEMO_CAPTURED^}. }Pipeline commit $commit (github.com/laurigates/data-processing),
export $(basename "$E").

mesh/model.glb
  Textured mesh with the water plane (glTF 2.0 binary): $(printf "%'.0f" "$faces") faces,
  4096 px JPEG texture. Two nodes: "mesh" (unlit, textured) and "water" (an alpha-blended plane).
  Model frame: +Y up, the water surface at y = 0. Geometry below the water was removed.
  Opens in glTF viewers, for example gltf-viewer.donmccurdy.com or Blender
  (File > Import > glTF 2.0).

splat/splat.spz
  Gaussian splat after the floater cull: $(printf "%'.0f" "$gauss") Gaussians, SH degree 3.
  SPZ version 3 (Niantic's compressed splat format, gzip), about 8x smaller than the source
  splat.ply. Quantised like Niantic's ply_to_spz: positions to 1/4096 unit, rotations within
  0.24 degrees, SH coefficients with 5 bits (degree 1) and 4 bits (degrees 2-3).
  Axes as in the source PLY (the reconstruction frame), as ply_to_spz writes with its default
  options. Open it in an SPZ-capable viewer such as SuperSplat (superspl.at/editor). If the
  scene appears upside down, rotate it 180 degrees about X: PLY-style data has Y pointing down.

water_plane.json
  The fitted water plane, how it was fitted, and model_from_reconstruction: the 4x4 transform
  from the splat's frame to the mesh's frame.

report.json
  Every pipeline stage's metrics, fingerprint, version and tools, and the resolved config.

Units are the reconstruction's own (COLMAP), not metres.
EOF
    (cd work/bundle && zip -q -r -9 "$ds.zip" "$ds")
    printf '%s\t%.1f MB\n' "work/bundle/$ds.zip" "$(echo "$(stat -f %z "work/bundle/$ds.zip") / 1000000" | bc -l)"
done
