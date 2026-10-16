"""``lint.run``: ruff diagnostics mapped onto the contract Diagnostic shape."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

from .errors import TOOL_MISSING, RpcError
from .paths import rel_to_root, resolve_in_root

_ERROR_CODES = {"F821", "F822", "F823", "F811", "F401"}  # F401 kept as warn below; see map


def find_ruff() -> str | None:
    candidate = Path(sys.executable).with_name("ruff")
    if candidate.is_file():
        return str(candidate)
    return shutil.which("ruff")


def _severity(code: str | None) -> str:
    if code is None:
        return "error"  # syntax error
    if code.startswith("E9") or code in ("F821", "F822", "F823"):
        return "error"
    return "warn"


def lint_run(root: Path, paths: list[str] | None, default_paths: list[Path]) -> list[dict[str, Any]]:
    ruff = find_ruff()
    if ruff is None:
        raise RpcError(TOOL_MISSING, "ruff is not installed in the tooling environment", tool="ruff")
    targets = [resolve_in_root(root, p) for p in paths] if paths else list(default_paths)
    targets = [t for t in targets if t.exists()]
    if not targets:
        return []
    try:
        proc = subprocess.run(
            [ruff, "check", "--output-format", "json", "--exit-zero", *map(str, targets)],
            capture_output=True,
            text=True,
            cwd=str(root),
            timeout=120,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise RpcError(TOOL_MISSING, f"ruff failed to run: {exc}", tool="ruff") from exc
    try:
        items = json.loads(proc.stdout or "[]")
    except ValueError as exc:
        raise RpcError(TOOL_MISSING, f"ruff produced unreadable output: {proc.stderr[:200]}", tool="ruff") from exc
    diags: list[dict[str, Any]] = []
    for item in items:
        code = item.get("code")
        loc = item.get("location") or {}
        fix = item.get("fix") or {}
        diags.append(
            {
                "path": rel_to_root(root, Path(item.get("filename", ""))),
                "line": int(loc.get("row", 1) or 1),
                "col": int(loc.get("column", 1) or 1),
                "rule": f"ruff:{code}" if code else "ruff:syntax-error",
                "severity": _severity(code),
                "message": item.get("message", ""),
                "hint": fix.get("message") or None,
                "related": [],
            }
        )
    return diags
