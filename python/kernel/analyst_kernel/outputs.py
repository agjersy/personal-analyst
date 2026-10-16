"""``out.list`` and ``out.read`` (PROTOCOL.md 1.3): bounded previews, nothing transferred."""

from __future__ import annotations

import csv
import io
import json
import subprocess
from pathlib import Path
from typing import Any

from .errors import PATH_OUTSIDE_ROOT, RpcError, invalid_params
from .paths import mtime_iso, rel_to_root, resolve_in_root
from .runs import artifact_kind

DEFAULT_MAX_BYTES = 16384
TEXT_KINDS = {"text", "json", "table"}
TEXT_EXTS = {".txt", ".md", ".log", ".json", ".csv", ".tsv", ".py", ".toml", ".yaml", ".yml", ".jsonl"}

_NPY_SUMMARY = r"""
import json, sys
try:
    import numpy as np
except Exception as exc:  # numpy missing in the session environment
    print(json.dumps({"error": "numpy unavailable: %s" % exc}))
    sys.exit(0)
path = sys.argv[1]

def summarize(a):
    a = np.asarray(a)
    out = {"shape": list(a.shape), "dtype": str(a.dtype), "min": None, "max": None, "mean": None}
    if a.size and np.issubdtype(a.dtype, np.number):
        f = a.astype(float, copy=False)
        if np.iscomplexobj(a):
            f = np.abs(a).astype(float)
        out.update({"min": float(np.nanmin(f)), "max": float(np.nanmax(f)), "mean": float(np.nanmean(f))})
    return out

try:
    if path.endswith(".npz"):
        with np.load(path, allow_pickle=False) as z:
            print(json.dumps({"arrays": {k: summarize(z[k]) for k in z.files}}))
    else:
        print(json.dumps(summarize(np.load(path, allow_pickle=False, mmap_mode="r"))))
except Exception as exc:
    print(json.dumps({"error": "%s: %s" % (type(exc).__name__, exc)}))
"""


def out_list(root: Path, out_dir_name: str, path: str | None, run_out_dir: str | None) -> list[dict[str, Any]]:
    target_rel = path if path is not None else (run_out_dir if run_out_dir is not None else out_dir_name)
    target = resolve_in_root(root, target_rel)
    if not target.exists():
        return []
    entries: list[dict[str, Any]] = []
    files = [target] if target.is_file() else sorted(p for p in target.rglob("*") if p.is_file())
    for f in files:
        try:
            st = f.stat()
        except OSError:
            continue
        entries.append(
            {
                "path": rel_to_root(root, f),
                "size": st.st_size,
                "kind": artifact_kind(f),
                "mtime": mtime_iso(st.st_mtime),
            }
        )
    return entries


def _csv_summary(head: str, delimiter: str) -> dict[str, Any]:
    reader = csv.reader(io.StringIO(head), delimiter=delimiter)
    rows = list(reader)
    columns = rows[0] if rows else []
    # the last line may be cut by max_bytes; do not count it if it looks partial
    data_rows = rows[1:]
    if data_rows and not head.endswith("\n"):
        data_rows = data_rows[:-1]
    return {"rows_sampled": len(data_rows), "columns": columns}


def _array_summary(python: str, path: Path, root: Path, timeout: float = 60.0) -> dict[str, Any] | None:
    try:
        proc = subprocess.run(
            [python, "-I", "-c", _NPY_SUMMARY, str(path)],
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=str(root),
        )
    except (OSError, subprocess.SubprocessError):
        return None
    try:
        data = json.loads(proc.stdout.strip().splitlines()[-1])
    except (ValueError, IndexError):
        return None
    if "error" in data:
        return None
    return data


def out_read(root: Path, python: str, path: str, max_bytes: int | None) -> dict[str, Any]:
    if not isinstance(path, str) or not path:
        raise invalid_params("path must be a non-empty string")
    limit = int(max_bytes) if max_bytes is not None else DEFAULT_MAX_BYTES
    if limit <= 0:
        raise invalid_params("max_bytes must be positive")
    file = resolve_in_root(root, path)
    if not file.is_file():
        raise RpcError(PATH_OUTSIDE_ROOT, f"{path} is not a file", message="file not found")
    size = file.stat().st_size
    kind = artifact_kind(file)
    ext = file.suffix.lower()
    if kind == "array":
        summary = _array_summary(python, file, root)
        if summary is None:
            return {"kind": "file", "preview": "", "truncated": size > 0, "summary": None}
        return {"kind": "array", "preview": "", "truncated": True, "summary": summary}
    if ext in TEXT_EXTS or (kind in TEXT_KINDS and ext not in (".parquet",)):
        with file.open("rb") as fh:
            raw = fh.read(limit)
        head = raw.decode("utf-8", errors="replace")
        summary = None
        if ext in (".csv", ".tsv"):
            summary = _csv_summary(head, "\t" if ext == ".tsv" else ",")
        return {"kind": kind, "preview": head, "truncated": size > limit, "summary": summary}
    return {"kind": kind, "preview": "", "truncated": True, "summary": None}
