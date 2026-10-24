#!/bin/bash
# Render one looping AV1 video per splat (both datasets, masked and unmasked).
# Usage: videos.sh <arc|drone> [seconds] [extra render_orbit.py args...]
# Needs cams/ (COLMAP TXT of each splat's training cameras) and meta/ (overlay JSON, from
# local/make_meta.py) under $S/render; cameras are exported here when missing.
set -euo pipefail
source "$(dirname "$0")/env.sh"
path=$1; secs=${2:-$([ "$1" = arc ] && echo 16 || echo 20)}; shift $(( $# < 2 ? $# : 2 ))
out=$S/render/videos; mkdir -p "$out" "$S/render/cams"
for ds in "${DATASETS[@]}"; do for k in masked unmasked; do
  n=$ds-$k; b=$S/work/$ds/splat-$k/$TAG
  [ -f "$S/render/cams/$n/images.txt" ] || { mkdir -p "$S/render/cams/$n"
    colmap model_converter --input_path "$b/dataset/sparse/0" --output_path "$S/render/cams/$n" --output_type TXT >/dev/null 2>&1; }
  "$RENDER_PY" "$BIN/render_orbit.py" "$b/splat_30000.ply" "$S/render/cams/$n" "$S/render/meta/$n.json" \
    "$out/$n-$path.mp4" --path "$path" --seconds "$secs" --ffmpeg "$FFMPEG" "$@" 2>&1 | tail -1
done; done
