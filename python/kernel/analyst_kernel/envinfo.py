"""``env.info`` and ``env.packages``."""

from __future__ import annotations

import json
import shutil
import subprocess
import tomllib
from pathlib import Path
from typing import Any

from .errors import TOOL_MISSING, RpcError

_PROBE = (
    "import json,sys,sysconfig;"
    "print(json.dumps({'executable':sys.executable,'version':sys.version.split()[0],"
    "'prefix':sys.prefix,'base_prefix':sys.base_prefix,"
    "'site':[p for p in sys.path if p.endswith('site-packages') or p.endswith('dist-packages')]}))"
)


def probe_python(python: str, timeout: float = 30.0) -> dict[str, Any]:
    try:
        proc = subprocess.run([python, "-I", "-c", _PROBE], capture_output=True, text=True, timeout=timeout)
        data = json.loads(proc.stdout.strip().splitlines()[-1])
    except (OSError, subprocess.SubprocessError, ValueError, IndexError) as exc:
        raise RpcError(TOOL_MISSING, f"cannot run session python {python}: {exc}", tool="python") from exc
    return data


def env_info(python: str, sandbox: str, host: str, os_name: str) -> dict[str, Any]:
    data = probe_python(python)
    venv = data["prefix"] if data["prefix"] != data["base_prefix"] else None
    return {
        "python": data["executable"],
        "version": data["version"],
        "venv": venv,
        "site_packages": data["site"],
        "sandbox": sandbox,
        "host": host,
        "os": os_name,
        "uv": shutil.which("uv"),
    }


def locked_packages(root: Path) -> set[str]:
    lock = root / "uv.lock"
    if not lock.is_file():
        return set()
    try:
        data = tomllib.loads(lock.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return set()
    return {str(p.get("name", "")).lower().replace("_", "-") for p in data.get("package", []) if isinstance(p, dict)}


def env_packages(python: str, root: Path) -> list[dict[str, Any]]:
    uv = shutil.which("uv")
    if uv:
        cmd = [uv, "pip", "list", "--format", "json", "--python", python]
    else:
        cmd = [python, "-m", "pip", "list", "--format", "json"]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=120, cwd=str(root))
    except (OSError, subprocess.SubprocessError) as exc:
        raise RpcError(TOOL_MISSING, f"package listing failed: {exc}", tool="uv" if uv else "pip") from exc
    if proc.returncode != 0:
        raise RpcError(
            TOOL_MISSING,
            f"package listing failed: {proc.stderr.strip()[:300]}",
            tool="uv" if uv else "pip",
        )
    try:
        items = json.loads(proc.stdout or "[]")
    except ValueError as exc:
        raise RpcError(TOOL_MISSING, "package listing produced unreadable output", tool="uv" if uv else "pip") from exc
    locked = locked_packages(root)
    out = []
    for item in items:
        name = str(item.get("name", ""))
        out.append(
            {
                "name": name,
                "version": str(item.get("version", "")),
                "locked": name.lower().replace("_", "-") in locked,
            }
        )
    return out
