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
# run rather than a per-crate subset. .github/ and scripts/ are included so a
# change to this very selector is validated against the whole workspace.
CROSS_EXACT = {"Cargo.toml", "Cargo.lock"}
CROSS_PREFIX = (".github/", "scripts/")


def run(cmd, **kw):
    return subprocess.run(cmd, cwd=REPO, text=True, capture_output=True, **kw)


def workspace_graph():
    """Return (dir2name, deps) parsed from Cargo.toml manifests.

    deps[crate] = set of intra-workspace crates it depends on (any dep kind),
    resolved via path deps. No cargo invocation, so no submodules required.
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
        for sect in ("dependencies", "dev-dependencies", "build-dependencies"):
            for _key, spec in ct.get(sect, {}).items():
                if isinstance(spec, dict) and "path" in spec:
                    tgt = (REPO / m / spec["path"]).resolve()
                    try:
                        rel = str(tgt.relative_to(REPO))
                    except ValueError:
                        continue
                    if rel in dir2name:
                        deps[dir2name[m]].add(dir2name[rel])
    return dir2name, deps


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


def emit(mode, test_crates, clippy_crates):
    test_s = " ".join(sorted(test_crates))
    clippy_s = " ".join(sorted(clippy_crates))
    print(f"[detect] mode={mode}")
    print(f"[detect] test_crates=[{test_s}]")
    print(f"[detect] clippy_crates=[{clippy_s}]")
    gh_out = os.environ.get("GITHUB_OUTPUT")
    if gh_out:
        with open(gh_out, "a") as f:
            f.write(f"mode={mode}\ntest_crates={test_s}\nclippy_crates={clippy_s}\n")


def detect():
    base = os.environ.get("BASE_SHA", "").strip()
    head = os.environ.get("HEAD_SHA", "").strip()
    files = changed_files(base, head)
    if files is None:
        print("[detect] base/head unresolved; falling back to full workspace.")
        return emit("all", [], [])
    if not files:
        return emit("none", [], [])
    if any(is_cross_cutting(f) for f in files):
        hit = sorted({f for f in files if is_cross_cutting(f)})[:5]
        print(f"[detect] cross-cutting change(s): {hit} -> full workspace.")
        return emit("all", [], [])
    dir2name, deps = workspace_graph()
    seeds = {c for c in (owning_crate(f, dir2name) for f in files) if c}
    if not seeds:
        print("[detect] no first-party crate affected (docs/vendored only).")
        return emit("none", [], [])
    return emit("subset", dependents_closure(seeds, deps), seeds)


def sh(cmd):
    print("+", " ".join(cmd))
    if os.environ.get("CI_AFFECTED_DRY_RUN"):
        return 0
    return subprocess.run(cmd, cwd=REPO).returncode


def run_test():
    mode = os.environ.get("MODE", "")
    crates = os.environ.get("CRATES", "").split()
    if mode == "none" or (mode == "subset" and not crates):
        print("No affected crates; skipping tests.")
        return 0
    if mode == "all":
        return sh(["cargo", "test", "--workspace", "--locked", "--jobs", "8"])
    cmd = ["cargo", "test", "--locked", "--jobs", "8"]
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
            "cargo", "clippy", "-p", "static-flow-shared", "-p", "static-flow-backend",
            "-p", "sf-cli", "--tests", "--", "-D", "warnings",
        ])
        return rc or sh([
            "cargo", "clippy", "-p", FRONTEND,
            "--target", "wasm32-unknown-unknown", "--", "-D", "warnings",
        ])
    rc = 0
    non_frontend = [c for c in crates if c != FRONTEND]
    if non_frontend:
        cmd = ["cargo", "clippy", "--jobs", "8"]
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


