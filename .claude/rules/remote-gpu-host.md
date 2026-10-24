---
paths:
  - "justfile"
  - "spike/**"
  - ".env.example"
---

# Running Stages on the Remote GPU Host

SfM, splat training, dense reconstruction and video renders run on a CUDA host over ssh; this
machine prepares inputs and receives results. The host is **configuration, not code**: never
write its name or paths into committed files.

## Where the host comes from

- `SHORELINE_GPU_HOST` (ssh target) and `SHORELINE_GPU_ROOT` (working directory there), from
  the environment or a git-ignored `.env` (`.env.example` lists them). The `just gpu-*`
  recipes fail with a pointer to `.env.example` when either is unset.
- If neither is set and a task needs the GPU, **ask the user for the host and root**. Do not
  guess from other repos' notes or shell history.
- Optional `SHORELINE_STOP_SERVICE`: a systemd unit (for example another GPU service) that
  `just gpu-overnight` stops for the run and starts again on any exit. Stopping a shared
  service interrupts its users — confirm with the user, and check the service is idle first.

## How to drive it

- **Use the recipes** (`just --list`, group `gpu`): `gpu-sync`, `gpu-setup` (idempotent
  bring-up), `gpu-sfm`, `gpu-overnight` (detached batch), `gpu-status`, `gpu-fetch`,
  `gpu-videos`, `gpu-timeline`. Scripts live in `spike/m0/remote/` and all source `env.sh`.
- **Run remote scripts with `bash`, not the login shell.** `ssh host 'cmd'` is parsed by the
  remote user's login shell, which may be zsh: a command held in a string variable then fails
  with 127 (no word splitting). Use `ssh host env VAR=… bash script` or
  `ssh host bash -s <<'EOF' … EOF`.
- **Keep everything under `SHORELINE_GPU_ROOT`.** `env.sh` points the Apptainer, Hugging Face,
  uv and torch-extension caches there; a GPU host's root filesystem can be small, and one COLMAP
  image pull needs ~6.6 GB of layers plus SIF.
- **Long jobs run detached** (`setsid nohup … &`) and log STEP-START / STEP-OK / STEP-FAIL
  markers; read them with `just gpu-status`. A step's own output goes to its per-step log, so
  the markers stay in the main log.
- **Check the GPU before a run**: `nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv`.
  Another service holding VRAM is normal on a shared box; SIFT and matching fit beside it,
  Brush and patch-match prefer the whole card.

## Toolchain facts that bite

- **CUDA toolkit major must match torch's build** when anything compiles an extension (gsplat
  JIT-builds one). `setup.sh` picks the torch wheel index from `nvcc --version`. Running a
  prebuilt torch only needs a new enough driver.
- **Put the venv's `bin/` on `PATH`** when calling a venv python by path: torch's extension
  builder looks for `ninja` on `PATH`, not next to the interpreter.
- **Apptainer, not Docker**, for the COLMAP image: `--nv` for the GPU, `--bind` the root, pull
  by digest. See `tools-plugin:apptainer`.
- **AV1 encoding** uses the BtbN static ffmpeg (`get_ffmpeg.sh`); distro ffmpeg builds often
  lack `libsvtav1`.
