# Development

## Building from source

Needs [Rust](https://rustup.rs) via rustup (`rust-toolchain.toml` pins 1.99.1, which
rustup selects inside the repository), [just](https://github.com/casey/just),
[cargo-nextest](https://nexte.st), ffmpeg and `python3`.

```sh
git clone https://github.com/laurigates/shore-recon.git
cd shore-recon
cp .env.example .env   # optional settings for the recipes
just build             # target/release/shoreline-recon
```

## Checks

```sh
just check                                # all of the following except the integration profile
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run                         # unit tests
cargo nextest run --profile integration   # tests in `integration` modules: need tools or a GPU
python3 -m unittest discover -s scripts -p 'test_*.py'
```

Integration tests that need external tools, a GPU or a remote host live in modules named
`integration`, which the default nextest profile skips. Some read their inputs from
environment variables, for example `SHORELINE_TEST_SFM_DIR` and
`SHORELINE_TEST_REMOTE_HOST`.

## Layout

```
shore-recon/
├── crates/
│   ├── cli/          # clap entrypoint (binary: shoreline-recon)
│   ├── core/         # config layers, user dirs, DAG, fingerprints, Stage trait, cache, pipeline, tool runner
│   └── frames/ masks/ sfm/ dense/ splat/ video/ post/ export/   # one crate per stage
├── configs/          # defaults.toml and tools.toml (built into the binary), dataset configs
├── containers/       # Containerfiles for the tool images, pinned versions
├── docs/             # these pages, kickoff, ADRs
├── scripts/          # demo video, bundle, SPZ conversion, export viewer, dataset discovery
├── spike/m0/         # the M0 spike's scripts and justfile (just -f spike/m0/justfile)
├── tests/fixtures/   # frame and mask fixtures
└── justfile
```

`work/`, `data/` and `models/` are git-ignored runtime directories. The M0 spike ran end
to end with the scripts in [`spike/m0/`](../spike/m0/README.md); findings in
[ADR-0002](adr/0002-m0-spike-findings.md).

## Contributing

Work follows [CLAUDE.md](../CLAUDE.md): conventional commits, a unit test with every
change (kept next to the code), `just check` before committing, external tools only
through the core `ToolRunner`, and an ADR in [`docs/adr/`](adr/) for each backend or
format decision.

## Releases

release-please keeps a release PR open from the conventional commits on `main`; merging
it tags `vX.Y.Z` and creates the GitHub Release. The tag starts dist's workflow, which
builds the binaries for macOS (Apple Silicon) and Linux (x86_64), uploads them with a
shell installer, and publishes the Homebrew formula to
[laurigates/homebrew-tap](https://github.com/laurigates/homebrew-tap)
([ADR-0016](adr/0016-releases-with-release-please-and-dist.md)). Versions, tags and
`CHANGELOG.md` change only through the release PR.
