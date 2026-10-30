<div align="center">

<img src="assets/icon.png" alt="shoreline-recon icon: textured mesh of the Reposalmi pier" width="128" height="128">

# shoreline-recon

**Water-aware photogrammetry and Gaussian splatting for shoreline structures**

[![CI](https://github.com/laurigates/shore-recon/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/laurigates/shore-recon/actions/workflows/ci.yml)
[![Containers](https://github.com/laurigates/shore-recon/actions/workflows/containers.yml/badge.svg?branch=main)](https://github.com/laurigates/shore-recon/actions/workflows/containers.yml)
[![Rust](https://img.shields.io/badge/rust-1.99.1-orange)](rust-toolchain.toml)
[![License](https://img.shields.io/github/license/laurigates/shore-recon)](LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/laurigates/shore-recon)](https://github.com/laurigates/shore-recon/stargazers)

<img src="assets/banner.jpg" alt="Vartiosaari pier from one camera position: Gaussian splat on the left, textured mesh with its synthetic water plane on the right" width="100%">

</div>

Water, sky and transient objects are masked out of every reconstruction stage, and a
synthetic water plane replaces the water surface afterwards. One drone or phone clip of a
pier, shore or waterfront goes in; a textured mesh with its water plane and a Gaussian
splat come out.

## Quick start

```sh
brew tap laurigates/tap
brew trust laurigates/tap      # Homebrew loads third-party taps only once trusted
brew install shoreline-recon   # brings ffmpeg, COLMAP and Brush
shoreline-recon pier.mp4
```

Docker must be running: the segmentation model and OpenMVS run in public images,
pulled on first use. Everything runs on the local machine, on the CPU apart from
Brush; a clip that keeps 117 frames took about 90 minutes on Apple Silicon.

The deliverables land in `./pier-recon/`:

| File | Contents |
|---|---|
| `model.glb` | textured mesh with the water plane (glTF binary) |
| `splat.ply` | Gaussian splat, floaters over the water culled |
| `water_plane.json` | the fitted water plane and the splat-to-mesh transform |
| `report.json` | every stage's metrics, fingerprint and tools, and the resolved config |

Other options: `shoreline-recon --help` and [docs/usage.md](docs/usage.md).

A remote CUDA host runs the heavy stages faster and also renders the showcase and
processing-step videos (`--videos`). How each tool runs, locally, in a container or on
that host, is machine configuration: see [docs/configuration.md](docs/configuration.md).

Other installs: the shell installer and archives for macOS (Apple Silicon) and Linux
(x86_64) on the [releases page](https://github.com/laurigates/shore-recon/releases), or
from source ([docs/development.md](docs/development.md)). Homebrew installs the shell
completions; for the others, see [shell completions](docs/usage.md#shell-completions).

## Documentation

| Page | For |
|---|---|
| [Usage](docs/usage.md) | the command line, dataset configs, outputs, batches, demo video, viewer, bundles |
| [Configuration](docs/configuration.md) | config and tool layers, user files, runtimes, the remote GPU host |
| [Pipeline](docs/pipeline.md) | what each stage does, backends, technology |
| [Development](docs/development.md) | building, testing, project layout, releases, contributing |
| [Kickoff](docs/kickoff.md) | design and milestones |
| [Decisions](docs/adr/) | architecture decision records |

## License

[MIT](LICENSE)
