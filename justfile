# shoreline-recon tasks. Settings come from the environment or a git-ignored .env (see
# .env.example). Run `just --list` for the recipes. The M0 spike's recipes are in
# spike/m0/justfile.

set dotenv-load
set shell := ["bash", "-euo", "pipefail", "-c"]

# Datasets the batch, demo and bundle recipes cover: names of configs/<name>.toml. By default
# every config there with a [dataset] table whose input exists (scripts/datasets.sh).
datasets := if env("SHORELINE_DATASETS", "") == "" { `scripts/datasets.sh configs | tr '\n' ' '` } else { env("SHORELINE_DATASETS") }

# List recipes
default:
    @just --list

# Format, lint and unit-test the Rust workspace
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo nextest run
    python3 -m unittest discover -s scripts -p 'test_*.py'

# Regenerate the shell completion scripts shipped in the release archives
completions:
    cargo build -q -p shoreline-recon
    mkdir -p crates/cli/completions
    target/debug/shoreline-recon completions bash > crates/cli/completions/shoreline-recon.bash
    target/debug/shoreline-recon completions zsh > crates/cli/completions/_shoreline-recon
    target/debug/shoreline-recon completions fish > crates/cli/completions/shoreline-recon.fish

# ---------- pipeline (shoreline-recon) ----------

bin := "target/release/shoreline-recon"

# Build the release binary
[group('pipeline')]
build:
    cargo build --release -p shoreline-recon

# Run one dataset config (a path ending in .toml, or a name under configs/) through the implemented stages
[group('pipeline')]
run config *args: build
    {{bin}} run {{quote(if config =~ '\.toml$' { join(invocation_directory(), config) } else { "configs/" + config + ".toml" })}} {{args}}

# Run every dataset detached, keeping the machine awake; survives closing the terminal
[group('pipeline')]
batch *args: build
    @test -n "${SHORELINE_TOOLS:-}" || { echo "SHORELINE_TOOLS is not set (host tools file, e.g. configs/<host>.local.toml); see .env.example" >&2; exit 1; }
    @test -n "{{datasets}}" || { echo "no datasets: no configs/*.toml with an existing input; set SHORELINE_DATASETS" >&2; exit 1; }
    mkdir -p tmp/runs
    log=tmp/runs/batch-$(date +%Y%m%d-%H%M%S).log; \
      keep=(); if command -v caffeinate >/dev/null; then keep=(caffeinate -i); \
      elif systemd-inhibit --what=idle:sleep --why=probe true >/dev/null 2>&1; then \
        keep=(systemd-inhibit --what=idle:sleep --why=shoreline-recon); fi; \
      nohup ${keep[@]+"${keep[@]}"} just _batch {{args}} > "$log" 2>&1 < /dev/null & \
      echo "started (pid $!); log: $log; follow with: just batch-status"

# Show the latest batch run's markers and its last lines
[group('pipeline')]
batch-status:
    @log=$(ls -t tmp/runs/batch-*.log 2>/dev/null | head -1); test -n "$log" || { echo "no batch log yet"; exit 0; }; \
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
    summary="shoreline-recon batch: ${#failed[@]} failed${failed:+ (${failed[*]})}"
    echo "BATCH-END $(date -Iseconds) $summary"
    # Optional: a command that takes the summary as its last argument, e.g. a chat notifier.
    if [ -n "${SHORELINE_NOTIFY_CMD:-}" ]; then bash -c "$SHORELINE_NOTIFY_CMD \"\$1\"" _ "$summary" || true; fi

# ---------- local (this machine) ----------

# Join each dataset's splat and mesh videos (latest export) into the v1 demo, with chapters and a timeline bar
[group('local')]
demo out="work/demo/shoreline-recon-v1-demo.mp4":
    scripts/demo-video.sh {{out}} {{datasets}}

# Zip each dataset's mesh (+ water), splat (as SPZ), water plane, report and README into work/bundle/
[group('local')]
bundle:
    scripts/bundle.sh {{datasets}}

# Send a file with SHORELINE_SHARE_CMD, which takes the file and the caption as its last arguments
[group('local')]
share file caption="":
    @test -n "${SHORELINE_SHARE_CMD:-}" || { echo "SHORELINE_SHARE_CMD is not set (a command taking <file> <caption>); see .env.example" >&2; exit 1; }
    bash -c "$SHORELINE_SHARE_CMD \"\$@\"" _ {{quote(join(invocation_directory(), file))}} {{quote(caption)}}

# Browse every dataset's export outputs in work/ (splat, mesh, water, raw layers, videos) at http://127.0.0.1:<port>/
[group('local')]
viewer port="8765":
    python3 -I scripts/viewer.py --work work --port {{port}} --open

# Make a smaller copy for attaching (1280 px wide, CRF 38, chapters kept)
[group('local')]
share-copy file:
    ffmpeg -v error -y -i {{file}} -map 0:v -map_chapters 0 -map_metadata 0 -vf "scale=1280:-2:flags=lanczos" \
      -c:v libsvtav1 -preset 6 -crf 38 -g 60 -pix_fmt yuv420p -movflags +faststart {{without_extension(file)}}-1280.mp4
    @ls -l {{without_extension(file)}}-1280.mp4 | awk '{printf "%.1f MiB\n", $5/1048576}'

# ---------- gpu host ----------

# Clean stale mirrored mounts on the GPU host.
# Dry run by default; pass --apply to delete. See scripts/clean_remote.py.
[group('gpu')]
clean-remote *args:
    python3 scripts/clean_remote.py --tools "${SHORELINE_TOOLS:-configs/tools.toml}" --work work {{args}}
