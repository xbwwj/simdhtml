#!/usr/bin/env python3
"""Assemble a benchmark bundle directory.

Invoked by `./x bundle`.  The bundle contains the `x` entry point, the Rust
source (fallback), the prebuilt Criterion binaries, the uv environment, a small
portable CPython runtime, and either a sampled or full data tree.

Layout:

    simdhtml-bundle/
      x
      BUNDLE.env
      bin/{markup,parse,classify,select,parse_compare,extract,python_compare,cc-export}
      .venv/lib/pythonX.Y/site-packages/
      python/{bin,lib}                  # portable CPython for PyO3 embedding
      data/...                          # sample or full corpora
      bench/ crates/ scripts/ Cargo.* pyproject.toml uv.lock
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
import sysconfig
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TARGET = ROOT / "target" / "release"
BENCHES = (
    "markup",
    "parse",
    "classify",
    "select",
    "parse_compare",
    "extract",
    "python_compare",
)
IGNORE = shutil.ignore_patterns("__pycache__", "*.pyc", "*.pyo")


def copy_tree(src: Path, dst: Path, ignore=IGNORE) -> None:
    if not src.exists():
        return
    shutil.copytree(src, dst, ignore=ignore, dirs_exist_ok=True)


def newest_binary(name: str) -> Path | None:
    candidates = [
        path
        for path in TARGET.joinpath("deps").glob(f"{name}-*")
        if path.is_file() and path.suffix != ".d" and path.stat().st_mode & 0o111
    ]
    if not candidates:
        return None
    return max(candidates, key=lambda path: path.stat().st_mtime)


def copy_binaries(bundle: Path) -> None:
    bin_dir = bundle / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    for name in BENCHES:
        binary = newest_binary(name)
        if binary is None:
            continue
        shutil.copy2(binary, bin_dir / name)
        (bin_dir / name).chmod(0o755)
    cc_export = TARGET / "cc-export"
    if cc_export.is_file():
        shutil.copy2(cc_export, bin_dir / "cc-export")
        (bin_dir / "cc-export").chmod(0o755)


def copy_source(bundle: Path) -> None:
    for name in ("x", "README.md", "Cargo.toml", "Cargo.lock", "pyproject.toml", "uv.lock"):
        src = ROOT / name
        if src.is_file():
            shutil.copy2(src, bundle / name)
    for name in ("bench", "crates", "scripts"):
        copy_tree(ROOT / name, bundle / name)
    # The source fallback needs the workspace's data README, not the corpora.
    (bundle / "data").mkdir(exist_ok=True)
    shutil.copy2(ROOT / "data/README.md", bundle / "data/README.md")


def copy_python(bundle: Path) -> str:
    """Copy the venv's site-packages and a portable CPython runtime."""
    version = f"{sys.version_info.major}.{sys.version_info.minor}"
    site = ROOT / ".venv" / "lib" / f"python{version}" / "site-packages"
    if site.is_dir():
        copy_tree(site, bundle / ".venv" / "lib" / f"python{version}" / "site-packages")

    # A venv's `bin/python` is a symlink to the host interpreter, so copy the
    # real interpreter + libpython + stdlib into the bundle and point
    # PYTHONHOME at it.  The PyO3 bench binary then has a stable libpython.
    py_root = bundle / "python"
    (py_root / "bin").mkdir(parents=True, exist_ok=True)
    (py_root / "lib").mkdir(parents=True, exist_ok=True)
    shutil.copy2(sys.executable, py_root / "bin" / Path(sys.executable).name)
    libdir = Path(sysconfig.get_config_var("LIBDIR") or "/usr/lib")
    for lib in libdir.glob(f"libpython{version}.so*"):
        shutil.copy2(lib, py_root / "lib" / lib.name)

    stdlib = Path(sysconfig.get_paths()["stdlib"])
    ignore = shutil.ignore_patterns(
        "site-packages",
        "test",
        "tests",
        "tkinter",
        "idlelib",
        "ensurepip",
        "lib2to3",
        "config-*",
        "__pycache__",
        "*.pyc",
        "*.pyo",
    )
    copy_tree(stdlib, py_root / "lib" / f"python{version}", ignore=ignore)
    return version


def read_rows(path: Path) -> list[dict]:
    if not path.is_file():
        return []
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def sample_rows(rows: list[dict], limit: int) -> list[dict]:
    if limit <= 0 or len(rows) <= limit:
        return rows
    step = max(1, len(rows) // limit)
    return rows[::step][:limit]


def copy_rows(bundle: Path, rows: list[dict], manifest: Path) -> None:
    for row in rows:
        src = ROOT / row["path"]
        dst = bundle / row["path"]
        if not src.is_file():
            continue
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)
        queries = row.get("queries")
        if queries:
            query_src = ROOT / queries
            query_dst = bundle / queries
            if query_src.is_file():
                query_dst.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(query_src, query_dst)
    manifest.parent.mkdir(parents=True, exist_ok=True)
    with manifest.open("w", encoding="utf-8") as out:
        for row in rows:
            out.write(json.dumps(row, ensure_ascii=False, separators=(",", ":")) + "\n")


def copy_sample_data(bundle: Path, cc_docs: int, swde_docs: int) -> None:
    manifests = ROOT / "data" / "prepared" / "manifests"
    out_manifests = bundle / "data" / "prepared" / "manifests"
    out_manifests.mkdir(parents=True, exist_ok=True)

    cc = sample_rows(read_rows(manifests / "cc.jsonl"), cc_docs)
    swde = sample_rows(read_rows(manifests / "swde.jsonl"), swde_docs)
    copy_rows(bundle, cc, out_manifests / "cc.jsonl")
    copy_rows(bundle, swde, out_manifests / "swde.jsonl")

    wpt = read_rows(manifests / "wpt.jsonl")
    copy_rows(bundle, wpt, out_manifests / "wpt.jsonl")

    # Selector workloads are tiny and the SWDE manifests point into them.
    copy_tree(
        ROOT / "data" / "prepared" / "workload",
        bundle / "data" / "prepared" / "workload",
    )
    if (ROOT / "data/prepared/MANIFEST.json").is_file():
        shutil.copy2(ROOT / "data/prepared/MANIFEST.json", bundle / "data/prepared/MANIFEST.json")


def copy_full_data(bundle: Path, with_warc: bool) -> None:
    for name in ("wpt", "prepared"):
        copy_tree(ROOT / "data" / name, bundle / "data" / name)
    copy_tree(ROOT / "data" / "commoncrawl" / "html", bundle / "data" / "commoncrawl" / "html")
    if with_warc:
        copy_tree(ROOT / "data" / "commoncrawl" / "warc", bundle / "data" / "commoncrawl" / "warc")


def write_env(bundle: Path, version: str, data_mode: str) -> None:
    env = f"""#!/usr/bin/env bash
# Sourced by `x` when this bundle is unpacked.
export BUNDLE=1
export SIMDHTML_DATA_DIR="$BUNDLE_ROOT/data"
export PYTHONHOME="$BUNDLE_ROOT/python"
export LD_LIBRARY_PATH="$BUNDLE_ROOT/python/lib${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}"
export PYTHONPATH="$BUNDLE_ROOT/.venv/lib/python{version}/site-packages${{PYTHONPATH:+:$PYTHONPATH}}"
# Use all documents present in the bundle; `./x bench-quick` overrides these.
export SIMDHTML_BENCH_LIMIT="${{SIMDHTML_BENCH_LIMIT:-500}}"
export SIMDHTML_BENCH_BYTES="${{SIMDHTML_BENCH_BYTES:-268435456}}"
export SIMDHTML_BUNDLE_DATA="{data_mode}"
"""
    (bundle / "BUNDLE.env").write_text(env, encoding="utf-8")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, required=True, help="bundle directory")
    ap.add_argument("--data", choices=("sample", "full", "none"), default="sample")
    ap.add_argument("--cc-docs", type=int, default=500)
    ap.add_argument("--swde-docs", type=int, default=500)
    ap.add_argument("--with-warc", action="store_true")
    args = ap.parse_args()

    bundle = args.out.resolve()
    if bundle.exists():
        shutil.rmtree(bundle)
    bundle.mkdir(parents=True)

    copy_source(bundle)
    copy_binaries(bundle)
    version = copy_python(bundle)
    if args.data == "sample":
        copy_sample_data(bundle, args.cc_docs, args.swde_docs)
    elif args.data == "full":
        copy_full_data(bundle, args.with_warc)
    write_env(bundle, version, args.data)

    (bundle / "x").chmod(0o755)
    total = sum(path.stat().st_size for path in bundle.rglob("*") if path.is_file())
    print(f"[bundle] {bundle} ({total / (1024 ** 3):.2f} GiB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
