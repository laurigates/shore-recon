#!/usr/bin/env bash
# Print the datasets the batch, demo and bundle recipes cover by default: the name of every
# <dir>/*.toml with a [dataset] table whose input exists, one per line. Relative inputs resolve
# against the working directory, as the pipeline resolves them. Host tools files and
# defaults.toml have no [dataset] table and are skipped.
# Usage: datasets.sh [configs-dir]
set -euo pipefail

dir=${1:-configs}
shopt -s nullglob
for config in "$dir"/*.toml; do
    grep -Eq '^[[:space:]]*\[[[:space:]]*dataset[[:space:]]*\]' "$config" || continue
    input=$(sed -nE 's/^[[:space:]]*input[[:space:]]*=[[:space:]]*"([^"]*)".*/\1/p' "$config" | head -1)
    [[ -n $input && -e $input ]] || continue
    name=${config##*/}
    echo "${name%.toml}"
done
