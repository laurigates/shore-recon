# Sourced by every script that runs on the GPU host. Everything lives under SHORELINE_GPU_ROOT,
# caches included, so a small root filesystem never fills.
: "${SHORELINE_GPU_ROOT:?set SHORELINE_GPU_ROOT to the working directory on the GPU host}"
S=$SHORELINE_GPU_ROOT
TAG=${SHORELINE_TAG:-manual-2026-10-23}
export APPTAINER_CACHEDIR=$S/cache/apptainer APPTAINER_TMPDIR=$S/cache/tmp
export HF_HOME=$S/cache/hf UV_CACHE_DIR=$S/cache/uv TORCH_EXTENSIONS_DIR=$S/cache/torch_extensions
export CUDA_HOME=${CUDA_HOME:-/usr/local/cuda}
# The render venv's bin/ must be on PATH: torch's extension builder looks for `ninja` there.
export PATH="$S/envs/render/bin:$CUDA_HOME/bin:$HOME/.local/share/mise/shims:$PATH"

COLMAP_IMAGE=docker://colmap/colmap@sha256:c1de51615dff33066cd305071226b6343c1168324ef3bd622f0825e55feb5efe
SIF=$S/images/colmap-20260929.8466.sif
BRUSH=$S/tools/brush/brush-app-x86_64-unknown-linux-gnu/brush_app
FFMPEG=${FFMPEG:-$S/tools/ffmpeg/ffmpeg-master-latest-linux64-gpl/bin/ffmpeg}
SEG_PY=$S/envs/seg/bin/python
RENDER_PY=$S/envs/render/bin/python
BIN=$S/tools/bin
DATASETS=(reposalmi-laajasalo reposalmi-vartiosaari)
declare -A MATCHER=([reposalmi-laajasalo]=sequential [reposalmi-vartiosaari]=exhaustive)

colmap() { apptainer exec --nv --bind "$S" "$SIF" colmap "$@"; }
mkdir -p "$APPTAINER_CACHEDIR" "$APPTAINER_TMPDIR" "$HF_HOME" "$UV_CACHE_DIR" "$TORCH_EXTENSIONS_DIR"
