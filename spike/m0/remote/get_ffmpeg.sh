#!/bin/bash
# Fetch a checksum-verified static ffmpeg (BtbN GPL build, includes libsvtav1).
set -euo pipefail
source "$(dirname "$0")/env.sh"
D=$S/tools/ffmpeg
mkdir -p "$D" && cd "$D"
base=https://github.com/BtbN/FFmpeg-Builds/releases/download/latest
curl -fsSL -o ffmpeg-master-latest-linux64-gpl.tar.xz "$base/ffmpeg-master-latest-linux64-gpl.tar.xz"
curl -fsSL -o checksums.sha256 "$base/checksums.sha256"
grep " ffmpeg-master-latest-linux64-gpl.tar.xz$" checksums.sha256 > want.sha256
sha256sum -c want.sha256
tar xf ffmpeg-master-latest-linux64-gpl.tar.xz
v=$("$FFMPEG" -version); echo "${v%%$'\n'*}"
