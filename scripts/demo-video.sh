#!/usr/bin/env bash
# Assemble the v1 demo video from the pipeline's deliverables: a title card, then for each
# dataset a clip per processing step (`step-<n>-<name>.mp4`: frames, masks, sparse, dense, raw
# mesh, raw splat), then the splat and the mesh with its water along every view (`<scene>-<path>.mp4`, the
# views listed in its report.json) from its latest `work/<dataset>/export/<fingerprint>/`,
# joined with crossfades, with one MP4 chapter per part
# and a timeline bar under the picture for seeking (scripts/timeline_bar.py, ADR-0013).
# Also writes a share copy under the attachment limit, and fails if either file lacks the bar
# or a chapter.
# Usage: demo-video.sh <out.mp4> <dataset>...
# Needs ffmpeg with libsvtav1, jq, ImageMagick (`magick`) for the title card, and uv for the bar.
set -euo pipefail

out=$1
shift
(($# > 0)) || { echo "usage: $0 <out.mp4> <dataset>..." >&2; exit 1; }

W=1920 H=1080 FPS=30 TITLE_S=4 FADE=0.5 BAR=80
FONT=${SHORELINE_DEMO_FONT:-/System/Library/Fonts/Helvetica.ttc}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

inputs=() labels=() durations=()
side_names=()
for ds in "$@"; do
    # shellcheck disable=SC2012 # fingerprint dirs are hex; newest run wins
    dir=$(ls -td work/"$ds"/export/*/ 2>/dev/null | head -1 || true)
    [[ -n $dir ]] || { echo "no export output for $ds; run the pipeline first" >&2; exit 1; }
    side=${ds##*-}
    side_names+=("${side^} side")
    # The processing steps first, as short clips, then the results along every view.
    for step in 1-frames:frames 2-masks:masks 3-sparse:sparse 4-dense:dense 5-mesh-raw:raw\ mesh 6-splat-raw:raw\ splat; do
        clip=${dir}step-${step%%:*}.mp4
        [[ -f $clip ]] || { echo "$clip not found" >&2; exit 1; }
        inputs+=("$clip")
        labels+=("${side^} side · ${step#*:}")
        durations+=("$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$clip")")
        echo "$ds: $clip" >&2
    done
    mapfile -t paths < <(jq -r '.config.video.views[].path' "${dir}report.json")
    ((${#paths[@]} > 0)) || { echo "${dir}report.json lists no video views" >&2; exit 1; }
    for scene in splat mesh; do
        for path in "${paths[@]}"; do
            clip=$dir$scene-$path.mp4
            [[ -f $clip ]] || { echo "$clip not found" >&2; exit 1; }
            inputs+=("$clip")
            labels+=("${side^} side · $scene · $path")
            durations+=("$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$clip")")
            echo "$ds: $clip" >&2
        done
    done
done

# Title card (colours match the showcase sidebar).
sides=$(printf '%s · ' "${side_names[@]}")
magick -size ${W}x${H} xc:'rgb(10,12,16)' \
    -fill 'rgb(20,184,166)' -draw "rectangle 160,430 168,650" \
    -font "$FONT" -fill white -pointsize 84 -annotate +200+510 'shoreline-recon v1' \
    -fill 'rgb(205,210,220)' -pointsize 40 -annotate +200+580 'Water-aware splats and meshes · Reposalmi pier, Helsinki' \
    -fill 'rgb(150,156,168)' -pointsize 30 -annotate +200+640 "${sides% · } · drone capture 2026-10-23" \
    "$tmp/title.png"

# Filter graph: title, then each clip, crossfaded; all normalised to WxH@FPS yuv420p.
args=(-loop 1 -framerate "$FPS" -t "$TITLE_S" -i "$tmp/title.png")
for clip in "${inputs[@]}"; do args+=(-i "$clip"); done
norm="scale=${W}:${H}:flags=lanczos,fps=${FPS},format=yuv420p,setsar=1"
graph="[0:v]${norm}[v0];"
for i in "${!inputs[@]}"; do graph+="[$((i + 1)):v]${norm}[v$((i + 1))];"; done
prev=v0 offset=$TITLE_S starts=(0)
for i in "${!inputs[@]}"; do
    offset=$(echo "$offset - $FADE" | bc)
    starts+=("$offset")
    graph+="[$prev][v$((i + 1))]xfade=transition=fade:duration=${FADE}:offset=${offset}[x$((i + 1))];"
    prev=x$((i + 1))
    offset=$(echo "$offset + ${durations[$i]}" | bc)
done
graph+="[$prev]fade=t=out:st=$(echo "$offset - $FADE" | bc):d=${FADE}[out]"

# Chapters: title, then one per dataset, each starting where its crossfade begins.
{
    echo ';FFMETADATA1'
    echo 'title=shoreline-recon v1 demo'
    names=("Title" "${labels[@]}")
    ends=("${starts[@]:1}" "$offset")
    for i in "${!names[@]}"; do
        printf '[CHAPTER]\nTIMEBASE=1/1000\nSTART=%.0f\nEND=%.0f\ntitle=%s\n' \
            "$(echo "${starts[$i]} * 1000" | bc)" "$(echo "${ends[$i]} * 1000" | bc)" "${names[$i]}"
    done
} >"$tmp/chapters.txt"
n=$((${#inputs[@]} + 1))

mkdir -p "$(dirname "$out")"
# Intermediate at higher quality: the bar pass re-encodes it.
ffmpeg -v error -y "${args[@]}" -f ffmetadata -i "$tmp/chapters.txt" \
    -filter_complex "$graph" -map '[out]' -map_metadata $n -map_chapters $n \
    -c:v libsvtav1 -preset 6 -crf 20 -g 60 -pix_fmt yuv420p "$tmp/joined.mp4"
uv run -q --no-project --with pillow==13.0.0 --with numpy==2.5.4 python -I \
    "$(dirname "$0")/timeline_bar.py" "$tmp/joined.mp4" "$out" --bar "$BAR" \
    --font "$FONT" --mono "${SHORELINE_DEMO_MONO:-/System/Library/Fonts/Menlo.ttc}"

share=${out%.mp4}-share.mp4
ffmpeg -v error -y -i "$out" -map 0:v -map_chapters 0 -map_metadata 0 -vf "scale=1280:-2:flags=lanczos" \
    -c:v libsvtav1 -preset 6 -crf 38 -g 60 -pix_fmt yuv420p -movflags +faststart "$share"

# The format the process promises (ADR-0013): bar under the picture, one chapter per part.
for f in "$out" "$share"; do
    dims=$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$f")
    chapters=$(ffprobe -v error -show_chapters -of csv=p=0 "$f" | grep -c . || true)
    w=${dims%,*} h=${dims#*,}
    # Height in proportion to WxH+BAR, allowing the share copy's rounding to an even height.
    diff=$((h * W - w * (H + BAR)))
    ((${diff#-} <= 2 * W)) || { echo "$f is ${dims/,/x}: no timeline bar" >&2; exit 1; }
    ((chapters == n)) || { echo "$f has $chapters chapters, expected $n" >&2; exit 1; }
done
for f in "$out" "$share"; do
    printf '%s\t%.1f s\t%.1f MiB\n' "$f" \
        "$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$f")" \
        "$(echo "$(stat -f %z "$f") / 1048576" | bc -l)"
done
