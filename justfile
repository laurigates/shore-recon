# shoreline-recon tasks. GPU-host recipes need SHORELINE_GPU_HOST (ssh target) and
# SHORELINE_GPU_ROOT (working directory there), from the environment or a git-ignored .env
# (see .env.example). Run `just --list` for the recipes.

set dotenv-load
set shell := ["bash", "-euo", "pipefail", "-c"]

host := env("SHORELINE_GPU_HOST", "")
root := env("SHORELINE_GPU_ROOT", "")
tag := env("SHORELINE_TAG", "manual-2026-10-23")
datasets := "reposalmi-laajasalo reposalmi-vartiosaari"

# List recipes
default:
    @just --list

# Format, lint and unit-test the Rust workspace
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo nextest run

# ---------- pipeline (shoreline-recon) ----------

bin := "target/release/shoreline-recon"

# Build the release binary
[group('pipeline')]
build:
    cargo build --release -p shoreline-recon

# Run one dataset through the implemented stages (SHORELINE_TOOLS selects the host tools file)
[group('pipeline')]
run dataset *args: build
    {{bin}} run configs/{{dataset}}.toml {{args}}

# Run every dataset detached, keeping the Mac awake; survives closing the terminal
[group('pipeline')]
overnight *args: build
    @test -n "${SHORELINE_TOOLS:-}" || { echo "SHORELINE_TOOLS is not set (host tools file, e.g. configs/<host>.local.toml); see .env.example" >&2; exit 1; }
    mkdir -p tmp/runs
    log=tmp/runs/overnight-$(date +%Y%m%d-%H%M%S).log; \
      nohup caffeinate -i just _batch {{args}} > "$log" 2>&1 < /dev/null & \
      echo "started (pid $!); log: $log; follow with: just overnight-status"

# Show the latest overnight run's markers and its last lines
[group('pipeline')]
overnight-status:
    @log=$(ls -t tmp/runs/overnight-*.log 2>/dev/null | head -1); test -n "$log" || { echo "no overnight log yet"; exit 0; }; \
      echo "$log"; grep -E '^(BATCH|RUN|SERVICE)-' "$log" || true; echo '---'; tail -5 "$log"

[private]
_batch *args:
    #!/usr/bin/env bash
    set -uo pipefail
    echo "BATCH-START $(date -Iseconds) tools=$SHORELINE_TOOLS"
    # Optional: stop a GPU service on the host for the batch (needs passwordless sudo
    # for exactly `/usr/bin/systemctl stop|start <unit>`); restarted on any exit.
    if [ -n "${SHORELINE_STOP_SERVICE:-}" ]; then
      gpu=${SHORELINE_GPU_HOST:?SHORELINE_STOP_SERVICE needs SHORELINE_GPU_HOST}
      ctl() { ssh -o BatchMode=yes "$gpu" sudo -n /usr/bin/systemctl "$1" "$SHORELINE_STOP_SERVICE"; }
      if ctl stop; then echo "SERVICE-STOPPED $SHORELINE_STOP_SERVICE"
        trap 'ctl start && echo "SERVICE-STARTED $SHORELINE_STOP_SERVICE" || echo "SERVICE-START-FAILED $SHORELINE_STOP_SERVICE"' EXIT
      else echo "SERVICE-STOP-FAILED $SHORELINE_STOP_SERVICE (running beside it)"; fi
    fi
    failed=()
    for ds in {{datasets}}; do
      echo "RUN-START $ds $(date -Iseconds)"
      if {{bin}} run configs/$ds.toml {{args}}; then echo "RUN-OK $ds $(date -Iseconds)"
      else echo "RUN-FAIL $ds $(date -Iseconds)"; failed+=("$ds"); fi
    done
    summary="shoreline-recon overnight: ${#failed[@]} failed${failed:+ (${failed[*]})}"
    echo "BATCH-END $(date -Iseconds) $summary"
    if command -v telegram-notify >/dev/null; then telegram-notify "$summary" || true; fi

# ---------- local (this machine) ----------

# Extract 2 fps frames (long edge 3200) and drop blurry ones, per data/raw/<dataset>.mp4
[group('local')]
frames threshold="100":
    for ds in {{datasets}}; do out=work/$ds/frames/{{tag}}; mkdir -p $out/images; \
      ffmpeg -v error -i data/raw/$ds.mp4 -vf "fps=2,scale=3200:-2:flags=lanczos" -q:v 2 -start_number 1 $out/images/%05d.jpg; \
      uv run -q --no-project --with opencv-python-headless python -I spike/m0/local/frames_manual.py $out {{threshold}}; done

# Join each dataset's splat and mesh videos (latest export) into the v1 demo, with chapters and a timeline bar
[group('local')]
demo out="work/demo/shoreline-recon-v1-demo.mp4":
    scripts/demo-video.sh {{out}} {{datasets}}

# Build the overlay metadata JSON for each splat video from the run's artefacts
[group('local')]
meta:
    python3 -I spike/m0/local/make_meta.py . work/renders/meta

# Make a copy under the 30 MiB attachment limit (1280 px wide, CRF 38, chapters kept)
[group('local')]
share-copy file:
    ffmpeg -v error -y -i {{file}} -map 0:v -map_chapters 0 -map_metadata 0 -vf "scale=1280:-2:flags=lanczos" \
      -c:v libsvtav1 -preset 6 -crf 38 -g 60 -pix_fmt yuv420p -movflags +faststart {{without_extension(file)}}-1280.mp4
    @ls -l {{without_extension(file)}}-1280.mp4 | awk '{printf "%.1f MiB\n", $5/1048576}'

# ---------- GPU host ----------

[private]
_gpu:
    @test -n "{{host}}" || { echo "SHORELINE_GPU_HOST is not set (ssh target of the GPU host); see .env.example" >&2; exit 1; }
    @test -n "{{root}}" || { echo "SHORELINE_GPU_ROOT is not set (working directory on the GPU host); see .env.example" >&2; exit 1; }

# Run a remote script with the environment the scripts expect (bash, whatever the login shell is)
[private]
_remote script *args: _gpu
    ssh -o BatchMode=yes {{host}} env SHORELINE_GPU_ROOT={{root}} SHORELINE_TAG={{tag}} bash {{root}}/tools/bin/{{script}} {{args}}

# Copy the spike scripts and the kept frames to the GPU host
[group('gpu')]
gpu-sync: _gpu
    ssh -o BatchMode=yes {{host}} mkdir -p {{root}}/tools/bin
    rsync -a spike/m0/remote/ {{host}}:{{root}}/tools/bin/
    for ds in {{datasets}}; do ssh -o BatchMode=yes {{host}} mkdir -p {{root}}/work/$ds/frames; \
      rsync -a work/$ds/frames/{{tag}}/images {{host}}:{{root}}/work/$ds/frames/; done

# One-time bring-up: COLMAP image, Python envs, gsplat build, Brush, ffmpeg (idempotent)
[group('gpu')]
gpu-setup: gpu-sync
    just _remote setup.sh

# Unmasked SfM baseline for every dataset (minutes)
[group('gpu')]
gpu-sfm: gpu-sync
    for ds in {{datasets}}; do just _remote sfm.sh $ds; done

# Start the overnight batch detached (masks, masked SfM, splats, dense); add SHORELINE_STOP_SERVICE=<unit> to free VRAM
[group('gpu')]
gpu-overnight: gpu-sync
    ssh -o BatchMode=yes {{host}} env SHORELINE_GPU_ROOT={{root}} SHORELINE_TAG={{tag}} SHORELINE_STOP_SERVICE=${SHORELINE_STOP_SERVICE:-} \
      setsid nohup bash {{root}}/tools/bin/overnight.sh '</dev/null' '>/dev/null' '2>&1' '&'
    @echo "started; follow with: just gpu-status"

# Show the batch's step markers (STEP-OK/FAIL, BRANCH-FAIL, start/end)
[group('gpu')]
gpu-status: _gpu
    ssh -o BatchMode=yes {{host}} "grep -E 'STEP-(OK|FAIL)|BRANCH-FAIL|SERVICE|OVERNIGHT|Registered images|reprojection' {{root}}/overnight-{{tag}}.log || echo 'no log yet'"

# Copy final splats, dense clouds, meshes and logs back (skips databases and depth maps)
[group('gpu')]
gpu-fetch: _gpu
    for ds in {{datasets}}; do for k in masked unmasked; do \
      mkdir -p work/$ds/splat-$k/{{tag}} work/$ds/dense-$k/{{tag}}; \
      rsync -a {{host}}:{{root}}/work/$ds/splat-$k/{{tag}}/splat_30000.ply {{host}}:{{root}}/work/$ds/splat-$k/{{tag}}/brush.log work/$ds/splat-$k/{{tag}}/; \
      rsync -a --include='*.ply' --include='*.json' --include='*.log' --exclude='*' {{host}}:{{root}}/work/$ds/dense-$k/{{tag}}/ work/$ds/dense-$k/{{tag}}/; done; \
      rsync -a --exclude database.db {{host}}:{{root}}/work/$ds/sfm {{host}}:{{root}}/work/$ds/sfm-masked work/$ds/; done

# Render one looping AV1 video per splat (path: arc | drone) and fetch them
[group('gpu')]
gpu-videos path="arc" *args: meta gpu-sync
    ssh -o BatchMode=yes {{host}} mkdir -p {{root}}/render/meta
    rsync -a work/renders/meta/ {{host}}:{{root}}/render/meta/
    just _remote videos.sh {{path}} {{args}}
    mkdir -p work/renders && rsync -a {{host}}:{{root}}/render/videos/ work/renders/

# Concatenate all renders into one video with a timeline strip and chapters, then fetch it
[group('gpu')]
gpu-timeline: meta gpu-sync
    ssh -o BatchMode=yes {{host}} mkdir -p {{root}}/render/meta
    rsync -a work/renders/meta/ {{host}}:{{root}}/render/meta/
    just _remote timeline.sh
    mkdir -p work/renders && rsync -a {{host}}:{{root}}/render/reposalmi-splats-timeline.mp4 work/renders/
