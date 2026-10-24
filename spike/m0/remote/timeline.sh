#!/bin/bash
# Concatenate all eight renders (2 sides x arc/drone x masked/unmasked) into one AV1 video with
# a timeline strip and MP4 chapters. Masked and unmasked of each camera style sit side by side.
# Usage: timeline.sh [out.mp4]
set -euo pipefail
source "$(dirname "$0")/env.sh"
out=${1:-$S/render/reposalmi-splats-timeline.mp4}
spec=$S/render/segments.json
"$RENDER_PY" - "$S" "$TAG" "$spec" <<'PY'
import json, sys
S, tag, spec = sys.argv[1:]
groups = []
for ds, side in [("reposalmi-laajasalo", "Laajasalo side"), ("reposalmi-vartiosaari", "Vartiosaari side")]:
    segs = []
    for path, secs in [("arc", 16), ("drone", 20)]:
        for kind in ("masked", "unmasked"):
            n = f"{ds}-{kind}"
            segs.append(dict(label=f"{path} · {'water masked' if kind == 'masked' else 'unmasked'}", kind=kind,
                             seconds=secs, args=[f"{S}/work/{ds}/splat-{kind}/{tag}/splat_30000.ply",
                                                 f"{S}/render/cams/{n}", f"{S}/render/meta/{n}.json", "--path", path]))
    groups.append(dict(label=side, segments=segs))
json.dump(dict(title="Reposalmi pier — Gaussian splats (shoreline-recon M0)", groups=groups),
          open(spec, "w"), indent=1, ensure_ascii=False)
PY
"$RENDER_PY" "$BIN/timeline.py" "$spec" "$out" --ffmpeg "$FFMPEG" 2>&1 | grep -vE "^(raw |Svt\[)"
