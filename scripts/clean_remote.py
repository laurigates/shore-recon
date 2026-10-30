#!/usr/bin/env python3
"""Clean stale mirrors and Apptainer cache entries on a GPU host (issue #42).

The local ``work/`` tree is the source of truth: any mirrored mount directory
under a tools file's ``remote.root`` whose corresponding local path no longer
exists is stale and can be removed.  SIF/cache entries whose digest is not
referenced by any tool in the tools file can optionally be pruned too.

By default this is a dry run that only lists what it would delete.  Pass
``--apply`` to actually delete.
"""

from __future__ import annotations

import argparse
import os
import shlex
import subprocess
import sys
import tomllib
from collections.abc import Iterable
from pathlib import Path


def load_tools(path: Path) -> dict:
    with path.open("rb") as fh:
        return tomllib.load(fh)


def remote_configs(tools: dict) -> list[dict]:
    """Return the list of remote tables found in a tools file."""
    out = []
    for name, spec in tools.items():
        if not isinstance(spec, dict):
            continue
        remote = spec.get("remote")
        if isinstance(remote, dict):
            out.append({"tool": name, **remote})
    return out


def referenced_digests(tools: dict) -> set[str]:
    digests: set[str] = set()
    for spec in tools.values():
        if not isinstance(spec, dict):
            continue
        image = spec.get("image")
        if isinstance(image, str) and "@" in image:
            digests.add(image.rsplit("@", 1)[-1])
    return digests


SSH_TIMEOUT = 60


def ssh(host: str, command: str, timeout: int = SSH_TIMEOUT) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(
            ["ssh", "-o", "BatchMode=yes", host, command],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        raise RuntimeError(
            f"ssh to {host} timed out after {timeout}s running: {command}"
        ) from exc


def list_remote_dirs(host: str, root: str) -> list[str]:
    """Return absolute remote directory paths beneath ``root``."""
    cmd = f"find {shlex.quote(root)} -mindepth 1 -type d -print 2>/dev/null"
    res = ssh(host, cmd)
    if res.returncode != 0:
        raise RuntimeError(f"ssh find failed: {res.stderr.strip()}")
    return [line for line in res.stdout.splitlines() if line.strip()]


def rel_to_local(remote_path: str, root: str) -> Path | None:
    """Map a remote mirror path back to its local absolute path."""
    root = root.rstrip("/")
    if not remote_path.startswith(root + "/"):
        return None
    rel = remote_path[len(root) + 1:]
    if not rel:
        return None
    return Path("/") / rel


def topmost_missing(remote_dirs: Iterable[str], root: str, work_root: Path) -> list[str]:
    """Remote dirs whose local counterpart is missing, pruned to topmost ones."""
    work_root = work_root.resolve()
    candidates: list[str] = []
    for rd in remote_dirs:
        local = rel_to_local(rd, root)
        if local is None:
            continue
        try:
            local_resolved = local.resolve(strict=False)
        except (OSError, RuntimeError):
            local_resolved = local
        # Only consider mirrors of paths under the local work tree.
        try:
            local_resolved.relative_to(work_root)
        except ValueError:
            continue
        if not local.exists():
            candidates.append(rd)
    # Keep only those without a candidate ancestor.
    cand_set = set(candidates)
    result = []
    for c in candidates:
        # if any ancestor (up to root) is also a candidate, skip
        skip = False
        p = Path(c).parent
        while str(p) != root.rstrip("/") and str(p) != "/":
            if str(p) in cand_set:
                skip = True
                break
            p = p.parent
        if not skip:
            result.append(c)
    return sorted(result)


def list_remote_cache_files(host: str, cache_dir: str) -> list[str]:
    cmd = f"find {shlex.quote(cache_dir)} -type f -print 2>/dev/null"
    res = ssh(host, cmd)
    if res.returncode != 0:
        return []
    return [line for line in res.stdout.splitlines() if line.strip()]


def prune_cache_candidates(cache_files: Iterable[str], digests: set[str]) -> list[str]:
    """Cache files not referencing any digest used by the tools file.

    A cache entry is retained when its path contains (or contains a short
    prefix of) a digest referenced by some tool; every other entry is a
    candidate for pruning.  The match is deliberately by substring so that
    both full digests and Apptainer's truncated cache names are handled.
    """
    short = {d.split(":")[-1][:12] for d in digests if d}
    out = []
    for f in cache_files:
        if any(d in f or s in f for d in digests for s in short):
            continue
        out.append(f)
    return sorted(out)


def run_cleanup(
    host: str,
    root: str,
    work_root: Path,
    cache_dir: str | None,
    digests: set[str],
    prune_cache: bool,
    apply: bool,
    dry_run: bool,
) -> int:
    print(f"host: {host}")
    print(f"remote root: {root}")
    print(f"local work root: {work_root}")
    print()

    remote_dirs = list_remote_dirs(host, root)
    to_delete = topmost_missing(remote_dirs, root, work_root)

    print("Stale mirror directories:")
    if not to_delete:
        print("  (none)")
    for d in to_delete:
        print(f"  {d}")
    print()

    cache_to_delete: list[str] = []
    if prune_cache and cache_dir:
        print(f"Apptainer cache: {cache_dir}")
        files = list_remote_cache_files(host, cache_dir)
        cache_to_delete = prune_cache_candidates(files, digests)
        print("Stale cache entries:")
        if not cache_to_delete:
            print("  (none)")
        for f in cache_to_delete:
            print(f"  {f}")
        print()

    if dry_run or not apply:
        print("Dry run: no changes made. Pass --apply to delete.")
        return 0

    # Apply deletions.
    all_targets = to_delete + cache_to_delete
    if not all_targets:
        print("Nothing to delete.")
        return 0
    # Delete in batches to keep command lines sane.
    batch_size = 50
    for i in range(0, len(all_targets), batch_size):
        batch = all_targets[i:i + batch_size]
        cmd = "rm -rf -- " + " ".join(shlex.quote(t) for t in batch)
        res = ssh(host, cmd)
        if res.returncode != 0:
            print(f"delete failed: {res.stderr.strip()}", file=sys.stderr)
            return 1
    print(f"Deleted {len(all_targets)} entries.")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tools", default=os.environ.get("SHORELINE_TOOLS", "configs/tools.toml"))
    parser.add_argument("--work", default="work", help="local work directory (source of truth)")
    parser.add_argument("--host", help="override ssh host")
    parser.add_argument("--root", help="override remote root")
    parser.add_argument("--apply", action="store_true", help="actually delete (default is dry run)")
    parser.add_argument("--dry-run", action="store_true", help="list only (default)")
    args = parser.parse_args(argv)

    tools_path = Path(args.tools)
    if not tools_path.is_file():
        print(f"tools file not found: {tools_path}", file=sys.stderr)
        return 2
    tools = load_tools(tools_path)
    remotes = remote_configs(tools)
    if not remotes:
        print("no remote tools configured in tools file", file=sys.stderr)
        return 2

    digests = referenced_digests(tools)
    work_root = Path(args.work)
    dry_run = args.dry_run or not args.apply

    # Collect every distinct (host, root) so a tools file with several remote
    # tools on different roots/hosts cleans all of them (a repeated root is
    # cleaned only once).
    seen: set[tuple[str | None, str | None]] = set()
    distinct: list[tuple[str, str]] = []
    for remote in remotes:
        host = args.host or remote.get("host")
        root = args.root or remote.get("root")
        if not host or not root:
            print("remote host/root missing", file=sys.stderr)
            return 2
        key = (host, root)
        if key in seen:
            continue
        seen.add(key)
        distinct.append((host, root))

    rc = 0
    for host, root in distinct:
        result = run_cleanup(
            host=host,
            root=str(root),
            work_root=work_root,
            cache_dir=None,
            digests=digests,
            prune_cache=False,
            apply=args.apply,
            dry_run=dry_run,
        )
        if result != 0:
            rc = result
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
