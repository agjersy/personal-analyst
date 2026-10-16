"""Path helpers: every path parameter is relative to the repo root and must not escape it."""

from __future__ import annotations

import os
from datetime import UTC
from pathlib import Path

from .errors import PATH_OUTSIDE_ROOT, RpcError


def resolve_in_root(root: Path, rel: str | os.PathLike[str]) -> Path:
    """Resolve ``rel`` against ``root`` and raise error 1002 if it escapes the root.

    Absolute paths are accepted only when they point inside the root.
    """
    root_r = root.resolve()
    candidate = Path(rel)
    full = candidate if candidate.is_absolute() else root_r / candidate
    try:
        full_r = full.resolve()
    except OSError as exc:  # pragma: no cover - exotic filesystems
        raise RpcError(PATH_OUTSIDE_ROOT, f"cannot resolve {rel!s}: {exc}") from exc
    if full_r != root_r and root_r not in full_r.parents:
        raise RpcError(PATH_OUTSIDE_ROOT, f"{rel!s} is outside the repo root")
    return full_r


def rel_to_root(root: Path, path: Path) -> str:
    """Path relative to root with forward slashes."""
    root_r = root.resolve()
    try:
        return path.resolve().relative_to(root_r).as_posix()
    except ValueError:
        return path.as_posix()


def utc_now_iso() -> str:
    """RFC 3339 UTC with millisecond precision, e.g. ``2026-10-16T18:15:02.123Z``."""
    from datetime import datetime

    return datetime.now(UTC).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def parse_iso(text: str | None) -> float | None:
    """Seconds since the epoch for an RFC 3339 string produced by :func:`utc_now_iso`."""
    if not text:
        return None
    from datetime import datetime

    try:
        dt = datetime.fromisoformat(text.replace("Z", "+00:00"))
    except ValueError:
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=UTC)
    return dt.timestamp()


def mtime_iso(ts: float) -> str:
    from datetime import datetime

    return (
        datetime.fromtimestamp(ts, tz=UTC)
        .isoformat(timespec="milliseconds")
        .replace("+00:00", "Z")
    )
