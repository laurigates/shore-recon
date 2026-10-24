#!/bin/bash
# M0 spike batch on the GPU host. Order puts the most valuable, fastest work first:
#   1. masks on frames + masked SfM          (each dataset)
#   2. undistort (+ masks on undistorted)    (masked and unmasked)
#   3. Brush splats                          (masked, then unmasked)
#   4. COLMAP MVS dense, masked @1600 px, then unmasked @1200 px
# A failed step skips only the work that depends on it. Expects the unmasked SfM from sfm.sh
# under work/<dataset>/sfm/<tag>/sparse/0. Optional: SHORELINE_STOP_SERVICE=<systemd unit>
# is stopped for the run (to free VRAM) and started again on any exit; needs passwordless sudo.
set -uo pipefail
source "$(dirname "$0")/env.sh"
LOG=$S/overnight-$TAG.log
declare -A READY=()

step() { # step <label> <log file> <cmd...>: run cmd with output to the log; markers to stdout
  local label=$1 log=$2 s=$SECONDS rc; shift 2
  echo "STEP-START $label $(date +%H:%M:%S)"
  "$@" >>"$log" 2>&1; rc=$?
  if [ $rc -eq 0 ]; then echo "STEP-OK $label $((SECONDS - s))s"; else echo "STEP-FAIL $label rc=$rc $((SECONDS - s))s (see $log)"; fi
  return $rc
}

stop_service() { [ -n "${SHORELINE_STOP_SERVICE:-}" ] && sudo -n systemctl stop "$SHORELINE_STOP_SERVICE" && echo "SERVICE stopped $SHORELINE_STOP_SERVICE"; }
start_service() { [ -n "${SHORELINE_STOP_SERVICE:-}" ] || return 0; sudo -n systemctl start "$SHORELINE_STOP_SERVICE" && echo "SERVICE restarted" || echo "SERVICE restart FAILED"; }

masked_sfm() {
  local ds=$1 img=$S/work/$1/frames/images w=$S/work/$1/sfm-masked/$TAG
  mkdir -p "$w/sparse"
  step "$ds masks-frames" "$w/masks.log" "$SEG_PY" "$BIN/masks.py" "$img" "$w/masks" &&
  step "$ds masked-features" "$w/colmap.log" colmap feature_extractor --database_path "$w/database.db" \
    --image_path "$img" --ImageReader.mask_path "$w/masks" --ImageReader.single_camera 1 \
    --SiftExtraction.max_num_features 16384 --FeatureExtraction.use_gpu 1 &&
  step "$ds masked-match" "$w/colmap.log" colmap "${MATCHER[$ds]}_matcher" --database_path "$w/database.db" --FeatureMatching.use_gpu 1 &&
  step "$ds masked-mapper" "$w/colmap.log" colmap mapper --database_path "$w/database.db" --image_path "$img" --output_path "$w/sparse" &&
  step "$ds masked-analyze" "$w/model_analyzer.txt" colmap model_analyzer --path "$w/sparse/0" &&
  grep -E "Registered images|Points:|track length|reprojection" "$w/model_analyzer.txt"
}

prepare() { # prepare <dataset> <masked|unmasked>: undistort, masks, Brush dataset layout
  local ds=$1 kind=$2 sparse d=$S/work/$1/dense-$2/$TAG bd=$S/work/$1/splat-$2/$TAG/dataset
  if [ "$kind" = masked ]; then sparse=$S/work/$ds/sfm-masked/$TAG/sparse/0; else sparse=$S/work/$ds/sfm/$TAG/sparse/0; fi
  mkdir -p "$d"
  step "$ds $kind undistort" "$d/undistort.log" colmap image_undistorter --image_path "$S/work/$ds/frames/images" \
    --input_path "$sparse" --output_path "$d" --output_type COLMAP || return 1
  if [ "$kind" = masked ]; then
    step "$ds $kind masks-undistorted" "$d/masks.log" "$SEG_PY" "$BIN/masks.py" "$d/images" "$d/masks" || return 1
  fi
  # Brush 0.3.0 matches masks by file stem (masks/00001.png for images/00001.jpg), not by
  # COLMAP's 00001.jpg.png, and reads only focal + principal point, so it gets the undistorted set.
  rm -rf "$bd"; mkdir -p "$bd/sparse/0"
  cp -al "$d/images" "$bd/images" && cp -al "$d/sparse/." "$bd/sparse/0/" || return 1
  if [ "$kind" = masked ]; then
    mkdir -p "$bd/masks"
    for m in "$d"/masks/*.jpg.png; do n=${m##*/}; cp "$m" "$bd/masks/${n%.jpg.png}.png" || return 1; done
  fi
}

splat() {
  local ds=$1 kind=$2 b=$S/work/$1/splat-$2/$TAG
  step "$ds $kind brush" "$b/brush.log" "$BRUSH" "$b/dataset" --total-steps 30000 --eval-split-every 8 \
    --eval-every 10000 --export-every 10000 --export-path "$b" --export-name "splat_{iter}.ply"
}

dense() { # dense <dataset> <kind> <max_image_size>
  local ds=$1 kind=$2 d=$S/work/$1/dense-$2/$TAG fusion_mask=()
  [ "$kind" = masked ] && fusion_mask=(--StereoFusion.mask_path "$d/masks")
  step "$ds $kind patch-match@$3" "$d/patch_match.log" colmap patch_match_stereo --workspace_path "$d" \
    --PatchMatchStereo.max_image_size "$3" --PatchMatchStereo.geom_consistency true &&
  step "$ds $kind fusion" "$d/fusion.log" colmap stereo_fusion --workspace_path "$d" --output_path "$d/fused.ply" \
    --StereoFusion.max_image_size "$3" "${fusion_mask[@]}" &&
  step "$ds $kind poisson" "$d/poisson.log" colmap poisson_mesher --input_path "$d/fused.ply" --output_path "$d/meshed-poisson.ply"
}

main() {
  echo "OVERNIGHT-START $(date -Is)"
  stop_service
  for ds in "${DATASETS[@]}"; do
    masked_sfm "$ds" && prepare "$ds" masked && READY[$ds/masked]=1 || echo "BRANCH-FAIL $ds masked prep"
    prepare "$ds" unmasked && READY[$ds/unmasked]=1 || echo "BRANCH-FAIL $ds unmasked prep"
  done
  for kind in masked unmasked; do for ds in "${DATASETS[@]}"; do
    [ -n "${READY[$ds/$kind]:-}" ] && { splat "$ds" "$kind" || echo "BRANCH-FAIL $ds $kind splat"; }
  done; done
  for ds in "${DATASETS[@]}"; do [ -n "${READY[$ds/masked]:-}" ] && { dense "$ds" masked 1600 || echo "BRANCH-FAIL $ds masked dense"; }; done
  for ds in "${DATASETS[@]}"; do [ -n "${READY[$ds/unmasked]:-}" ] && { dense "$ds" unmasked 1200 || echo "BRANCH-FAIL $ds unmasked dense"; }; done
  start_service
  df -h "$S" | tail -1
  echo "OVERNIGHT-END $(date -Is)"
}

trap 'echo "OVERNIGHT-ABORT $(date -Is)"; start_service' INT TERM HUP
main >> "$LOG" 2>&1
