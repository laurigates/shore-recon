# 0017. Installed CLI: one command per video, layered config, optional videos

- Status: accepted
- Date: 2026-10-29
- Amends: ADR-0003 decision 2, ADR-0008 decision 1

## Context

Since 0.1.0 (ADR-0016) `shoreline-recon` installs as a binary, but it still needed a
checkout: `Config::load` read `defaults.toml` and `Tools::load` read `tools.toml` from
beside the dataset config, and every run needed a dataset config naming the clip. The
owner wants `shoreline-recon <video>` to produce the models by default, with everything
else as options. `run` always ran every stage, the video stage included, which needs the
CUDA render image (ADR-0008, ADR-0011).

## Decision

Decided by the project owner on 2026-10-29:

1. `configs/defaults.toml` and `configs/tools.toml` are built into the binary. Config
   loads in layers, each deep-merged over the one before: built-in defaults, the user's
   `config.toml`, `defaults.toml` beside a dataset config, the dataset config, then
   command-line overrides. Tools load the same way with whole-table replacement (as
   ADR-0003): built-in, the user's `tools.toml`, `tools.toml` beside the dataset config,
   then `--tools` / `SHORELINE_TOOLS`.
2. The user directory is `SHORELINE_CONFIG_DIR`, else `$XDG_CONFIG_HOME/shoreline-recon`,
   else `~/.config/shoreline-recon`, on macOS too. `Config::load` reads no user layer,
   so tests and `just` recipes stay hermetic; the CLI adds it.
3. `[export] videos` (default `false`) decides whether export depends on the video
   stage. It lives under `[export]` because the video stage hashes its whole `[video]`
   table. With videos, export keeps the previous dependency order, so exports made
   before this change stay cached. The repository's dataset configs set it to `true`.
   `run` builds `export` by default instead of every stage.
4. `shoreline-recon <video>` builds the config from the layers plus its options
   (`--name`, `--fps`, `--steps`, `--matcher`, `--videos`, `--config <partial toml>`),
   runs to export with its cache in the user cache directory (`~/Library/Caches` on
   macOS, `$XDG_CACHE_HOME` or `~/.cache` elsewhere; `--work` overrides), and copies the
   deliverables to `./<name>-recon/` (`-o`). Copies, not links, so editing them leaves
   the cache intact. The dataset name is the file stem made into a plain path component.
   The subcommands (`run`, `config`, `plan`, `tools`) are unchanged and keep `./work`.

## Consequences

- An installed binary runs without a checkout: built-in defaults and tools, and a
  user `tools.toml` for anything machine-specific, such as the remote GPU host.
- Until the stages accept native tools on macOS (the local tool profile), the built-in
  tools cover frames and masks only; the other stages need tools from a user file or
  `--tools`. `--videos` without a `render` tool fails before anything runs.
- `report.json` now records `config.export.videos`. `demo-video.sh` picks the newest
  export whose report has videos; reports from before this change have no key and are
  treated as having them.
- The same video run from the subcommands and from the quick mode lands in different
  caches (`./work` and the user cache), unless `--work` points them at the same root.
