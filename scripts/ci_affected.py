#!/usr/bin/env python3
"""Scope CI test/clippy to the crates a PR affects; skip entirely when no
first-party crate code changed.

Subcommands:
  detect      Compute mode (none|all|subset) + the per-job crate lists and
              write them to $GITHUB_OUTPUT. Stdlib-only (tomllib + git), needs
              no cargo and no submodules, so the gate job never compiles.
  run-test    `cargo test` for the affected crates (changed + dependents), or
              the full workspace for `all`.
  run-clippy  `cargo clippy` for the changed crates only (frontend on the wasm
              target), or the curated host+wasm set for `all`.

Crate-set rationale:
  * test uses the dependents closure -- a library change can break a dependent's
    behaviour, so dependents must be tested.
  * clippy uses only the directly-changed crates -- clippy lints the code in a
    crate, and pulling in untouched dependents would surface their latent
    warnings (most crates have never been clippy-gated in CI).
  * Cargo.lock changes are attributed via the lock's own dependency graph
    (lock_affected_members) instead of forcing a full workspace run: only the
    members that transitively depend on a changed lock entry are seeded.
  * detect also emits disk_heavy=true|false: whether the test set pulls in a
    vendored deps/ tree (lance/datafusion etc.), so the workflow knows when
    to free runner disk before building.

Env:
  BASE_SHA, HEAD_SHA   PR base/head (or push before/after); used by `detect`.
  MODE, CRATES         consumed by run-test / run-clippy (from gate outputs).
  CI_AFFECTED_DRY_RUN  print decisions/commands without invoking cargo.
"""
import os
import subprocess
import sys
import tomllib
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
FRONTEND = "static-flow-frontend"

# Files whose change can affect every crate's build/lint, so they force a full
# run rather than a per-crate subset: the root manifest, the toolchain pin
# (rust-toolchain*), cargo config (.cargo/), and the vendored trees
# first-party crates compile through path deps / patches -- deps/ (lance,
# lancedb, pingora, ffmpeg-sidecar) and patches/ (object_store, patched in by
# the root [patch.crates-io]). CI YAML and scripts deliberately are NOT here:
# they change no Rust code, so a PR touching only them compiles nothing; the
# `detect` job itself exercises this selector.
#
# Cargo.lock is deliberately NOT here either: the lockfile records the full
# resolved dependency graph, so its diff can be attributed precisely -- see
# lock_affected_members(). A lock-only edge change (e.g. one workspace crate
# gaining a path dep) used to escalate every PR touching it to a full
# workspace build, pulling the vendored lance/datafusion tree into CI for
# changes that never compile it.
CROSS_EXACT = {"Cargo.toml"}
CROSS_PREFIX = (".cargo/", "deps/", "patches/")


def run(cmd, **kw):
    return subprocess.run(cmd, cwd=REPO, text=True, capture_output=True, **kw)


def manifest_dep_items(ct):
    """Yield (name, spec) across every dependency table of one manifest,
    including the target-specific ones (e.g. the non-wasm table that gates
    static-flow-shared's lance/lancedb deps)."""
    for sect in ("dependencies", "dev-dependencies", "build-dependencies"):
        yield from ct.get(sect, {}).items()
        for target_tables in ct.get("target", {}).values():
            yield from target_tables.get(sect, {}).items()


def workspace_graph():
    """Return (dir2name, deps) parsed from Cargo.toml manifests.

    deps[crate] = set of intra-workspace crates it depends on (any dep kind,
    any target), resolved via path deps. No cargo invocation, so no
    submodules required.
    """
    root_manifest = tomllib.load(open(REPO / "Cargo.toml", "rb"))
    members = root_manifest["workspace"]["members"]
    dir2name = {}
    for m in members:
        ct = tomllib.load(open(REPO / m / "Cargo.toml", "rb"))
        dir2name[m] = ct["package"]["name"]
    deps = defaultdict(set)
    for m in members:
        ct = tomllib.load(open(REPO / m / "Cargo.toml", "rb"))
        for _key, spec in manifest_dep_items(ct):
            if isinstance(spec, dict) and "path" in spec:
                tgt = (REPO / m / spec["path"]).resolve()
                try:
                    rel = tgt.relative_to(REPO).as_posix()
                except ValueError:
                    continue
                if rel in dir2name:
                    deps[dir2name[m]].add(dir2name[rel])
    return dir2name, deps


def vendored_heavy_crates(dir2name, deps):
    """Crates whose build pulls in a vendored deps/ tree (lance, lancedb,
    pingora, ffmpeg-sidecar, ...), directly or through other members.

    Building these from a cold cache is what exhausts runner disk, so the
    workflow only spends time freeing disk when the test set contains one.
    Vendored deps are recognized generically: root [workspace.dependencies]
    entries whose path points under deps/, plus direct member path deps into
    deps/.
    """
    root_manifest = tomllib.load(open(REPO / "Cargo.toml", "rb"))
    vendored_names = {
        name
        for name, spec in root_manifest.get("workspace", {}).get("dependencies", {}).items()
        if isinstance(spec, dict) and str(spec.get("path", "")).startswith("deps/")
    }
    direct = set()
    for m, crate in dir2name.items():
        ct = tomllib.load(open(REPO / m / "Cargo.toml", "rb"))
        for name, spec in manifest_dep_items(ct):
            if name in vendored_names and not (isinstance(spec, dict) and "path" in spec):
                direct.add(crate)
                break
            if isinstance(spec, dict) and "path" in spec:
                tgt = (REPO / m / spec["path"]).resolve()
                if tgt.is_relative_to(REPO / "deps"):
                    direct.add(crate)
                    break
    # Propagate through the member graph: a crate is heavy when any member
    # in its forward dependency closure is.
    heavy = set()
    for crate in dir2name.values():
        stack, seen = [crate], {crate}
        while stack:
            current = stack.pop()
            if current in direct:
                heavy.add(crate)
                break
            for dep in deps.get(current, ()):
                if dep not in seen:
                    seen.add(dep)
                    stack.append(dep)
    return heavy


def changed_files(base, head):
    """Files changed between base and head, or None if undeterminable."""
    if not base or not head or set(base) <= {"0"}:
        return None
    res = run(["git", "diff", "--name-only", f"{base}...{head}"])
    if res.returncode != 0:
        res = run(["git", "diff", "--name-only", base, head])
    if res.returncode != 0:
        return None
    return [f for f in res.stdout.splitlines() if f.strip()]


def is_cross_cutting(f):
    return (
        f in CROSS_EXACT
        or any(f.startswith(p) for p in CROSS_PREFIX)
        or Path(f).name.startswith("rust-toolchain")
    )


def owning_crate(f, dir2name):
    best = None
    for d in dir2name:
        if f == d or f.startswith(d + "/"):
            if best is None or len(d) > len(best):
                best = d
    return dir2name[best] if best else None


def _lock_packages(rev):
    """Parse Cargo.lock at `rev`.

    Returns (format_version, {id: fingerprint}, {id: deps_tuple}) where
    id = (name, version, source-or-None), or None when the lock cannot be
    read/parsed at that revision.
    """
    res = run(["git", "show", f"{rev}:Cargo.lock"])
    if res.returncode != 0:
        return None
    try:
        lock = tomllib.loads(res.stdout)
    except tomllib.TOMLDecodeError:
        return None
    fingerprints, deps = {}, {}
    for p in lock.get("package", []):
        pid = (p.get("name", ""), p.get("version", ""), p.get("source"))
        dep_list = tuple(p.get("dependencies", []))
        fingerprints[pid] = (p.get("checksum"), tuple(sorted(dep_list)))
        deps[pid] = dep_list
    return lock.get("version"), fingerprints, deps


def _lock_reverse_reachable(deps, start_ids):
    """All package ids that transitively depend on any id in `start_ids`
    (inclusive), following the lock graph in reverse.

    Lock dependency strings are `name`, `name version`, or
    `name version (source)` -- the longer forms appear only when the short
    ones would be ambiguous, so matching on the provided parts is exact.
    """
    by_name = defaultdict(list)
    for pid in deps:
        by_name[pid[0]].append(pid)
    rev = defaultdict(set)
    for pid, dep_list in deps.items():
        for dep in dep_list:
            parts = dep.split()
            targets = by_name.get(parts[0], ())
            if len(parts) > 1:
                targets = [t for t in targets if t[1] == parts[1]]
            if len(parts) > 2:
                source = parts[2].strip("()")
                targets = [t for t in targets if t[2] == source]
            for target in targets:
                rev[target].add(pid)
    reached = {pid for pid in start_ids if pid in deps}
    stack = list(reached)
    while stack:
        for dependent in rev.get(stack.pop(), ()):
            if dependent not in reached:
                reached.add(dependent)
                stack.append(dependent)
    return reached


def lock_affected_members(base, head, member_names):
    """Attribute a Cargo.lock diff to the workspace members it can affect.

    A lock entry change (version bump, checksum change, dependency-edge
    change, add/remove) affects exactly the workspace members that
    transitively depend on the changed package -- the lock itself records
    that graph, so no cargo invocation is needed. Returns the affected
    member-name set, or None when attribution is unsafe (unreadable lock,
    lockfile format-version change) and the caller should fall back to the
    full workspace.
    """
    mb = run(["git", "merge-base", base, head])
    old_rev = mb.stdout.strip() if mb.returncode == 0 and mb.stdout.strip() else base
    old = _lock_packages(old_rev)
    new = _lock_packages(head)
    if old is None or new is None:
        return None
    old_format, old_fingerprints, old_deps = old
    new_format, new_fingerprints, new_deps = new
    if old_format != new_format:
        return None
    changed = old_fingerprints.keys() ^ new_fingerprints.keys()
    changed |= {
        pid
        for pid in old_fingerprints.keys() & new_fingerprints.keys()
        if old_fingerprints[pid] != new_fingerprints[pid]
    }
    affected = set()
    # Removed entries matter to the old graph's dependents, added entries to
    # the new graph's; entries changed in place matter to both. Walking both
    # graphs covers all three without classifying each id.
    for graph in (old_deps, new_deps):
        for pid in _lock_reverse_reachable(graph, changed):
            if pid[2] is None and pid[0] in member_names:
                affected.add(pid[0])
    return affected


def dependents_closure(seeds, deps):
    rev = defaultdict(set)
    for crate, ds in deps.items():
        for d in ds:
            rev[d].add(crate)
    affected, stack = set(seeds), list(seeds)
    while stack:
        for dep in rev.get(stack.pop(), ()):
            if dep not in affected:
                affected.add(dep)
                stack.append(dep)
    return affected


def emit(mode, test_crates, clippy_crates, disk_heavy):
    test_s = " ".join(sorted(test_crates))
    clippy_s = " ".join(sorted(clippy_crates))
    heavy_s = "true" if disk_heavy else "false"
    print(f"[detect] mode={mode}")
    print(f"[detect] test_crates=[{test_s}]")
    print(f"[detect] clippy_crates=[{clippy_s}]")
    print(f"[detect] disk_heavy={heavy_s}")
    gh_out = os.environ.get("GITHUB_OUTPUT")
    if gh_out:
        with open(gh_out, "a") as f:
            f.write(
                f"mode={mode}\ntest_crates={test_s}\n"
                f"clippy_crates={clippy_s}\ndisk_heavy={heavy_s}\n"
            )


def detect():
    base = os.environ.get("BASE_SHA", "").strip()
    head = os.environ.get("HEAD_SHA", "").strip()
    files = changed_files(base, head)
    if files is None:
        print("[detect] base/head unresolved; falling back to full workspace.")
        return emit("all", [], [], True)
    if not files:
        return emit("none", [], [], False)
    if any(is_cross_cutting(f) for f in files):
        hit = sorted({f for f in files if is_cross_cutting(f)})[:5]
        print(f"[detect] cross-cutting change(s): {hit} -> full workspace.")
        return emit("all", [], [], True)
    dir2name, deps = workspace_graph()
    seeds = {c for c in (owning_crate(f, dir2name) for f in files) if c}
    if "Cargo.lock" in files:
        lock_seeds = lock_affected_members(base, head, set(dir2name.values()))
        if lock_seeds is None:
            print("[detect] Cargo.lock change not attributable -> full workspace.")
            return emit("all", [], [], True)
        print(f"[detect] Cargo.lock changes map to crates: {sorted(lock_seeds)}")
        seeds |= lock_seeds
    if not seeds:
        print("[detect] no first-party crate affected (docs/vendored only).")
        return emit("none", [], [], False)
    test_crates = dependents_closure(seeds, deps)
    heavy = sorted(test_crates & vendored_heavy_crates(dir2name, deps))
    if heavy:
        print(f"[detect] vendored-heavy test crates: {heavy} -> free disk first.")
    return emit("subset", test_crates, seeds, bool(heavy))


def sh(cmd):
    print("+", " ".join(cmd))
    if os.environ.get("CI_AFFECTED_DRY_RUN", "").lower() in ("1", "true"):
        return 0
    return subprocess.run(cmd, cwd=REPO).returncode


def run_test():
    mode = os.environ.get("MODE", "")
    crates = os.environ.get("CRATES", "").split()
    if mode == "none" or (mode == "subset" and not crates):
        print("No affected crates; skipping tests.")
        return 0
    if mode == "all":
        return sh(["cargo", "test", "--workspace", "--locked"])
    cmd = ["cargo", "test", "--locked"]
    for c in crates:
        cmd += ["-p", c]
    return sh(cmd)


def run_clippy():
    mode = os.environ.get("MODE", "")
    crates = os.environ.get("CRATES", "").split()
    if mode == "none" or (mode == "subset" and not crates):
        print("No changed crates; skipping clippy.")
        return 0
    if mode == "all":
        rc = sh([
            "cargo", "clippy", "-p", "static-flow-shared", "-p", "static-flow-store",
            "-p", "static-flow-embedding", "-p", "static-flow-backend",
            "-p", "sf-cli", "--tests", "--", "-D", "warnings",
        ])
        return rc or sh([
            "cargo", "clippy", "-p", FRONTEND,
            "--target", "wasm32-unknown-unknown", "--", "-D", "warnings",
        ])
    rc = 0
    non_frontend = [c for c in crates if c != FRONTEND]
    if non_frontend:
        cmd = ["cargo", "clippy"]
        for c in non_frontend:
            cmd += ["-p", c]
        cmd += ["--tests", "--", "-D", "warnings"]
        rc = sh(cmd)
    if FRONTEND in crates:
        rc = rc or sh([
            "cargo", "clippy", "-p", FRONTEND,
            "--target", "wasm32-unknown-unknown", "--", "-D", "warnings",
        ])
    return rc


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "detect":
        detect()
    elif cmd == "run-test":
        sys.exit(run_test())
    elif cmd == "run-clippy":
        sys.exit(run_clippy())
    else:
        sys.exit(f"usage: {sys.argv[0]} {{detect|run-test|run-clippy}}")


if __name__ == "__main__":
    main()


