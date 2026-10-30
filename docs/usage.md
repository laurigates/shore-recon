# Usage

## One video

```sh
shoreline-recon pier.mp4
```

Runs every stage up to `export` and copies the deliverables to `./pier-recon/`
([ADR-0017](adr/0017-installed-cli-and-config-layers.md)).

| Option | Effect | Config key |
|---|---|---|
| `-o, --out <dir>` | output directory (default `./<name>-recon`) | |
| `--name <name>` | dataset name: cache directory and report (default: the file stem) | `dataset.name` |
| `--fps <n>` | frames extracted per second of video (default 2.0) | `frames.fps` |
| `--steps <n>` | splat training steps (default 30000) | `splat.steps` |
| `--matcher sequential\|exhaustive` | COLMAP feature matcher; exhaustive for short clips | `sfm.matcher` |
| `--videos` | also render the showcase and processing-step videos | `export.videos` |
| `--config <file>` | partial config, any keys of [`configs/defaults.toml`](../configs/defaults.toml) | |
| `--tools <file>` | tools file replacing whole tool tables (`SHORELINE_TOOLS`) | |
| `--work <dir>` | stage cache root (`SHORELINE_WORK`) | |

The stage cache lives in the user cache directory: `~/Library/Caches/shoreline-recon` on
macOS, `$XDG_CACHE_HOME/shoreline-recon` or `~/.cache/shoreline-recon` elsewhere. A rerun
with nothing changed is a cache hit; changing an option reruns the stage it affects and
everything after it ([ADR-0004](adr/0004-stage-cache-and-manifest.md)).

### Deliverables

| File | Contents |
|---|---|
| `model.glb` | textured mesh with the water plane: two nodes, `mesh` and `water`; +Y up, water at y = 0 |
| `splat.ply` | Gaussian splat after the floater cull, SH degree 3 |
| `water_plane.json` | the fitted plane, how it was fitted, and the splat-to-mesh transform |
| `report.json` | every stage's fingerprint, version, metrics and tools, and the resolved config |

With `--videos`, also:

- one video per scene and view (`splat-arc.mp4`, `splat-drone.mp4`, `mesh-arc.mp4`,
  `mesh-drone.mp4`, from `video.views`), each with a `-share.mp4` copy under
  `video.share_max_mib`;
- a clip per processing step, `step-1-frames.mp4` to `step-6-splat-raw.mp4`: frames,
  masks, sparse points and cameras, dense points, and the mesh and splat before post.

Units are the reconstruction's own (COLMAP), not metres.

### What a run prints

Progress goes to stderr ([ADR-0018](adr/0018-run-progress-and-estimates.md)). On a
terminal, finished stages scroll above two live lines:

```
✓ frames   7s
✓ masks    7m02s   (usual 7m)
⠙ [4/7] dense  DensifyPointCloud 3/5  38%  4m12s · usual 10m–26m  Estimated depth-maps 47 ...
  total 18m40s · ~20m–35m left
```

Otherwise (CI, `just batch` logs), one line per event, with a heartbeat every minute:

```
[4/7] dense: running (usual 10m–26m) in <dir>
[4/7] dense: step 3/5 DensifyPointCloud
[4/7] dense: still running, 5m00s, 38% — Estimated depth-maps 47 (38.84%, 9s, ETA 14s)...
[4/7] dense: done in 10m24s
```

Estimates are the range of earlier runs of the stage in the same cache root, across
datasets; a stage with no earlier run shows its elapsed time only. Both modes end with a
table of each stage, whether it ran or was cached, and its time.

## Dataset configs

For repeated work on a capture, a dataset config keeps its settings:

```toml
# configs/pier.toml
[dataset]
name  = "pier"
input = "data/raw/pier.mp4"   # relative to the working directory

[sfm]
matcher = "exhaustive"

[export]
videos = true
```

```sh
shoreline-recon run configs/pier.toml                # every stage up to export
shoreline-recon run --stage sfm configs/pier.toml    # sfm and what it needs
shoreline-recon config configs/pier.toml             # the resolved config
shoreline-recon plan configs/pier.toml               # the stages run would run
shoreline-recon tools configs/pier.toml              # the resolved tools
```

`run` caches under `./work/<dataset>/<stage>/<fingerprint>/` (`--work` moves it), and the
deliverables are in `work/<dataset>/export/<fingerprint>/`. Unknown keys are rejected.
How configs and tools are layered: [configuration.md](configuration.md).

## Shell completions

The Homebrew formula installs completions for bash, zsh and fish. With any other install,
load them from the binary in the shell's startup file:

```sh
eval "$(shoreline-recon completions bash)"     # ~/.bashrc
eval "$(shoreline-recon completions zsh)"      # ~/.zshrc, after compinit
shoreline-recon completions fish | source      # ~/.config/fish/config.fish
```

`completions` also takes `elvish` and `powershell`. The release archives carry the
bash, zsh and fish scripts in `completions/`, for copying into a completion directory.

## Recipes in a checkout

`just --list` shows every recipe. The main ones:

| Recipe | Does |
|---|---|
| `just run <config>` | `run` for a config path or a name under `configs/` |
| `just batch` | every dataset, detached, logging to `tmp/runs/`; survives closing the terminal and keeps the machine awake |
| `just batch-status` | the latest batch's progress |
| `just demo` | joins each dataset's step clips and videos behind a title card, with chapters and a timeline bar ([ADR-0013](adr/0013-demo-video-format.md)) |
| `just viewer` | local web viewer for every export under `work/` ([ADR-0015](adr/0015-local-export-viewer.md)) |
| `just bundle` | per dataset, a zip with the mesh, the splat as SPZ, the water plane, the report and a README ([ADR-0014](adr/0014-spz-splats-in-bundles.md)) |
| `just share <file> [caption]` | sends a file with `SHORELINE_SHARE_CMD` |

The batch, demo and bundle recipes cover every `configs/*.toml` with a `[dataset]`
table whose input exists, or the names in `SHORELINE_DATASETS`. Settings come from the
environment or a git-ignored `.env`; [`.env.example`](../.env.example) lists them.

### Processing a new capture

1. Check the footage against the kickoff's capture guidelines. The camera must be
   level: the water plane's normal comes from the cameras, and `post` fails when one is
   tilted more than 5° ([ADR-0012](adr/0012-post-stage.md)).
2. Copy the clip to `data/raw/<name>.mp4` and write `configs/<name>.toml` with
   `[dataset] name` and `input`.
3. `just batch`, then check the gates, the water plane and the videos.
4. `just demo` and `just bundle` for delivery.

The `process-capture` project skill
([`.claude/skills/process-capture/SKILL.md`](../.claude/skills/process-capture/SKILL.md))
has the full runbook with timings and result checks.
