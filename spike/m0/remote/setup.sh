#!/bin/bash
# One-time GPU-host bring-up for the M0 spike. Idempotent: each step is skipped when its
# output already exists. Needs Apptainer, uv and a CUDA toolkit on the host already.
set -euo pipefail
source "$(dirname "$0")/env.sh"
mkdir -p "$S/images" "$S/envs" "$S/tools/brush" "$S/tools/ffmpeg" "$S/work"
command -v apptainer >/dev/null || { echo "apptainer missing (see tools-plugin:apptainer)" >&2; exit 1; }
nvcc --version | tail -1

# COLMAP 4.2.1 with CUDA, pinned by digest.
[ -f "$SIF" ] || apptainer pull "$SIF" "$COLMAP_IMAGE"
v=$(apptainer exec --nv "$SIF" colmap help 2>&1); echo "${v%%$'\n'*}"

# Segmentation env (OneFormer ADE20K via transformers). torch cu128 runs fine on a CUDA 13
# driver; it only needs to match the toolkit when building extensions, which seg never does.
if [ ! -x "$SEG_PY" ]; then
  uv venv -q --python 3.12 "$S/envs/seg"
  VIRTUAL_ENV=$S/envs/seg uv pip install -q --index-strategy unsafe-best-match \
    --extra-index-url https://download.pytorch.org/whl/cu128 "torch==2.8.*" "transformers>=4.56" \
    pillow opencv-python-headless scipy
fi
"$SEG_PY" -c "from huggingface_hub import snapshot_download as s; s('shi-labs/oneformer_ade20k_swin_large', allow_patterns=['*.json', '*.bin', '*.txt'])" >/dev/null

# Render env (gsplat). gsplat JIT-builds a CUDA extension, so torch's CUDA major version must
# match the host toolkit: pick the wheel index from nvcc.
if [ ! -x "$RENDER_PY" ]; then
  cuda_major=$(nvcc --version | sed -n 's/.*release \([0-9]*\)\..*/\1/p')
  case $cuda_major in
    13) idx=cu130 torch_spec="torch==2.9.*" ;;
    12) idx=cu128 torch_spec="torch==2.8.*" ;;
    *) echo "unsupported CUDA toolkit major $cuda_major" >&2; exit 1 ;;
  esac
  uv venv -q --python 3.12 "$S/envs/render"
  VIRTUAL_ENV=$S/envs/render uv pip install -q --index-strategy unsafe-best-match \
    --extra-index-url "https://download.pytorch.org/whl/$idx" "$torch_spec" "gsplat==1.5.3" \
    plyfile pillow numpy ninja packaging setuptools
fi
# First import builds gsplat's CUDA extension (~2 min) into TORCH_EXTENSIONS_DIR.
"$RENDER_PY" -c "
import torch
from gsplat import rasterization
d = 'cuda'; n = 64
img, _, _ = rasterization(torch.randn(n, 3, device=d) + torch.tensor([0, 0, 5.], device=d),
    torch.nn.functional.normalize(torch.randn(n, 4, device=d), dim=-1), torch.full((n, 3), .05, device=d),
    torch.ones(n, device=d), torch.rand(n, 3, device=d), torch.eye(4, device=d)[None],
    torch.tensor([[[100, 0, 64], [0, 100, 64], [0, 0, 1.]]], device=d), 128, 128)
print('gsplat ok', tuple(img.shape))"

# Brush 0.3.0 (Linux release binary, checksum-verified).
if [ ! -x "$BRUSH" ]; then
  (cd "$S/tools/brush"
   base=https://github.com/ArthurBrussee/brush/releases/download/v0.3.0
   curl -fsSLO "$base/brush-app-x86_64-unknown-linux-gnu.tar.xz"
   curl -fsSL "$base/brush-app-x86_64-unknown-linux-gnu.tar.xz.sha256" | awk '{print $1"  brush-app-x86_64-unknown-linux-gnu.tar.xz"}' | sha256sum -c -
   tar xf brush-app-x86_64-unknown-linux-gnu.tar.xz)
fi
"$BRUSH" --version

# ffmpeg with libsvtav1 (BtbN static GPL build, checksum-verified).
[ -x "$FFMPEG" ] || bash "$BIN/get_ffmpeg.sh"
# Capture first: `cmd | grep -q` under pipefail fails with SIGPIPE (141) once grep exits early.
encoders=$("$FFMPEG" -hide_banner -encoders)
grep -q libsvtav1 <<<"$encoders" && echo "ffmpeg libsvtav1 ok"
