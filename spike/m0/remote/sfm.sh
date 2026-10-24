#!/bin/bash
# Unmasked SfM baseline for one dataset (GPU SIFT + matching, incremental mapper).
# Usage: sfm.sh <dataset>     (matcher comes from env.sh's MATCHER map)
set -euo pipefail
source "$(dirname "$0")/env.sh"
DS=$1
W=$S/work/$DS/sfm/$TAG
IMG=$S/work/$DS/frames/images
mkdir -p "$W/sparse"
t() { local s=$SECONDS; "$@"; echo "TIMING $DS $2 $((SECONDS - s))s" | tee -a "$W/timings.txt"; }

t colmap feature_extractor --database_path "$W/database.db" --image_path "$IMG" \
  --ImageReader.single_camera 1 --SiftExtraction.max_num_features 16384 --FeatureExtraction.use_gpu 1
t colmap "${MATCHER[$DS]}_matcher" --database_path "$W/database.db" --FeatureMatching.use_gpu 1
t colmap mapper --database_path "$W/database.db" --image_path "$IMG" --output_path "$W/sparse"
colmap model_analyzer --path "$W/sparse/0" > "$W/model_analyzer.txt" 2>&1
grep -E "Registered images|Points:|track length|reprojection" "$W/model_analyzer.txt"
