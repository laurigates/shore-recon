---
paths:
  - "justfile"
  - "spike/**"
  - ".env.example"
  - "containers/**"
  - "configs/**"
---

# Running Stages on the Remote GPU Host

Video renders need a CUDA host over ssh; SfM, splat training and dense reconstruction run
there faster, but also locally from the built-in tools (ADR-0019). This machine prepares
inputs and receives results. The host is **configuration, not code**: never write its name
or paths into committed files.

## Where the host comes from

- `SHORELINE_GPU_HOST` (ssh target) and `SHORELINE_GPU_ROOT` (working directory there), from
  the environment or a git-ignored `.env` (`.env.example` lists them). The pipeline itself
  reaches the host through the `[<tool>.remote]` tables of the host tools file
  (`SHORELINE_TOOLS`, ADR-0009); the M0 spike's `gpu-*` recipes in `spike/m0/justfile` use the
  two variables and fail with a pointer to `.env.example` when either is unset.
- If neither is set and a task needs the GPU, **ask the user for the host and root**. Do not
  guess from other repos' notes or shell history.
- Optional `SHORELINE_STOP_SERVICE`: a systemd unit (for example another GPU service) that
  `just batch` and the spike's `gpu-overnight` stop for the run and start again on any exit. It
  needs `SHORELINE_GPU_HOST` in the environment too: without it the batch exits before the
  first dataset. Stopping a shared service interrupts its users — confirm with the user, and
  check the service is idle first.

## Images and pins

- The containers workflow builds images on pull requests and **publishes only from `main`**.
  A change to an image therefore merges before the pipeline can use it.
- GPU images are pinned by digest in the **host tools file** (`configs/<host>.local.toml`,
  git-ignored), not in `configs/tools.toml`. After the `main` run finishes, take the digest
  from its log (`gh run view <id> --log | grep -oE 'shoreline-<name>@sha256:[0-9a-f]{64}'`)
  and edit the host file; there is nothing to commit.
- The local-profile images are the exception: `segment` and `openmvs` are pinned in the
  committed `configs/tools.toml`, so a new digest is a PR. `openmvs-cpu` builds per
  architecture; pin the **index** digest that the `openmvs-cpu (manifest)` job prints, not
  either per-arch digest from the same log:

  ```sh
  gh run view <id> --log | grep 'openmvs-cpu (manifest)' | grep -oE 'shoreline-openmvs-cpu@sha256:[0-9a-f]{64}' | sort -u
  ```
- A Brush bump touches two repos: `containers/brush/Containerfile` here and
  `Formula/shoreline-brush.rb` in `laurigates/homebrew-tap`, which the shoreline-recon
  formula depends on. Merge the tap change before the release that needs it.
- A new pin changes the stage's version, so the stage and everything downstream re-run.

## How to drive it

- **Pipeline runs**: `just run <config>` or `just batch` with `SHORELINE_TOOLS` set.
- **Spike scripts** (`just -f spike/m0/justfile --list`, group `gpu`): `gpu-sync`, `gpu-setup`
  (idempotent bring-up), `gpu-sfm`, `gpu-overnight` (detached batch), `gpu-status`,
  `gpu-fetch`, `gpu-videos`, `gpu-timeline`. Scripts live in `spike/m0/remote/` and all source
  `env.sh`.
- **Run remote scripts with `bash`, not the login shell.** `ssh host 'cmd'` is parsed by the
  remote user's login shell, which may be zsh: a command held in a string variable then fails
  with 127 (no word splitting). Use `ssh host env VAR=… bash script` or
  `ssh host bash -s <<'EOF' … EOF`.
- **Keep everything under `SHORELINE_GPU_ROOT`.** `env.sh` points the Apptainer, Hugging Face,
  uv and torch-extension caches there; a GPU host's root filesystem can be small, and one COLMAP
  image pull needs ~6.6 GB of layers plus SIF.
- **Long jobs run detached** (`setsid nohup … &`) and log STEP-START / STEP-OK / STEP-FAIL
  markers; read them with `just -f spike/m0/justfile gpu-status`. A step's own output goes to
  its per-step log, so the markers stay in the main log.
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
- **Headless OpenGL in an image under `apptainer --nv`**: load glvnd's `libOpenGL.so.0` and
  `libEGL.so.1`, which `--nv` binds with the NVIDIA vendor libraries
  (`moderngl.create_standalone_context(backend="egl", libgl="libOpenGL.so.0",
  libegl="libEGL.so.1")`). The default `libGL.so` needs X11 libraries that a slim image does
  not have, and fails with `libGL.so not loaded`.
