"""Sandbox wrappers (PROTOCOL.md 1.5): bwrap argv, sandbox-exec profile, resource limits."""

from __future__ import annotations

import os
import platform
import shutil
import subprocess
import sys
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

from .errors import SANDBOX_UNAVAILABLE, RpcError

KINDS = ("auto", "bwrap", "sandbox-exec", "limits", "none")
DEFAULT_RLIMIT_AS = 8 * 1024**3  # 8 GiB
DEFAULT_RLIMIT_NOFILE = 4096


@dataclass
class SandboxSpec:
    """Everything a wrapper needs to know about one run."""

    root: Path
    venv: Path | None
    run_dir: Path  # <runs_dir>/<run_id>, writable
    out_dir: Path  # the run's output directory, writable
    data_dirs: list[Path] = field(default_factory=list)
    tooling_dir: Path | None = None
    derived_dir: Path | None = None
    home: Path | None = None
    python: Path | None = None  # session interpreter; its base installation is made readable
    network: bool = False
    rlimit_as: int = DEFAULT_RLIMIT_AS
    rlimit_nofile: int = DEFAULT_RLIMIT_NOFILE

    def read_only_dirs(self) -> list[Path]:
        dirs: list[Path] = [self.root]
        if self.venv is not None:
            dirs.append(self.venv)
        if self.python is not None:
            dirs += interpreter_dirs(self.python)
        if self.tooling_dir is not None and self.tooling_dir.exists():
            dirs.append(self.tooling_dir)
        for d in self.data_dirs:
            if d.exists():
                dirs.append(d)
        return _dedupe(_with_realpaths(dirs))

    def writable_dirs(self) -> list[Path]:
        dirs = [self.out_dir]
        if self.derived_dir is not None and self.derived_dir.exists():
            dirs.append(self.derived_dir)
        dirs.append(self.run_dir)
        return _dedupe(_with_realpaths(dirs))


@dataclass
class SandboxCommand:
    kind: str
    argv: list[str]
    rlimit_as: int | None = None
    rlimit_nofile: int | None = None
    profile: str | None = None  # sandbox-exec profile text when kind == "sandbox-exec"

    def preexec(self) -> Callable[[], None] | None:
        if self.rlimit_as is None and self.rlimit_nofile is None:
            return None
        return limits_preexec(self.rlimit_as, self.rlimit_nofile)


def _with_realpaths(paths: list[Path]) -> list[Path]:
    """Each path followed by its canonical form when that differs.

    Sandbox rules are matched against canonical paths (``/tmp`` is ``/private/tmp`` on macOS,
    and a repo may live behind a symlink), so both spellings are listed.
    """
    out: list[Path] = []
    for p in paths:
        out.append(p)
        real = Path(os.path.realpath(p))
        if real != p:
            out.append(real)
    return out


def interpreter_dirs(python: Path) -> list[Path]:
    """Directories the session interpreter needs besides its venv.

    A venv's ``bin/python`` is a symlink to a base installation, typically under the home
    directory (``~/.local/share/uv/python/<build>/bin/python3.x`` for ``uv``), which the
    sandbox otherwise hides. The base installation is found two ways and both are returned:
    by resolving the symlink, and from ``home =`` in the venv's ``pyvenv.cfg``.
    """
    prefixes: list[Path] = []
    real = Path(os.path.realpath(python))
    prefixes.append(real.parent.parent if real.parent.name == "bin" else real.parent)
    cfg = python.parent.parent / "pyvenv.cfg"
    try:
        for line in cfg.read_text().splitlines():
            key, sep, value = line.partition("=")
            if sep and key.strip() == "home" and value.strip():
                base_bin = Path(os.path.realpath(value.strip()))
                prefixes.append(base_bin.parent if base_bin.name == "bin" else base_bin)
    except OSError:
        pass
    return [p for p in _dedupe(prefixes) if p.exists() and p != Path("/")]


def _dedupe(paths: list[Path]) -> list[Path]:
    seen: set[str] = set()
    out: list[Path] = []
    for p in paths:
        key = str(p)
        if key not in seen:
            seen.add(key)
            out.append(p)
    return out


def _home() -> Path:
    return Path(os.environ.get("HOME") or Path.home())


# ---- detection ---------------------------------------------------------------


def userns_usable() -> bool:
    """Best-effort check that unprivileged user namespaces work (Linux)."""
    try:
        p = Path("/proc/sys/kernel/unprivileged_userns_clone")
        if p.exists() and p.read_text().strip() == "0":
            return False
        p = Path("/proc/sys/user/max_user_namespaces")
        if p.exists() and p.read_text().strip() == "0":
            return False
    except OSError:
        return False
    return True


def bwrap_available(which: Callable[[str], str | None] = shutil.which) -> bool:
    if sys.platform != "linux" or which("bwrap") is None:
        return False
    if not userns_usable():
        return False
    try:
        proc = subprocess.run(
            ["bwrap", "--ro-bind", "/", "/", "--unshare-all", "--", "/bin/true"],
            capture_output=True,
            timeout=5,
        )
        return proc.returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


def sandbox_exec_available(which: Callable[[str], str | None] = shutil.which) -> bool:
    return sys.platform == "darwin" and which("sandbox-exec") is not None


def detect(requested: str = "auto") -> str:
    """Resolve ``--sandbox`` to the kind actually used; error 1001 if an explicit kind is missing."""
    if requested not in KINDS:
        raise RpcError(SANDBOX_UNAVAILABLE, f"unknown sandbox kind {requested!r}")
    if requested == "auto":
        if bwrap_available():
            return "bwrap"
        if sandbox_exec_available():
            return "sandbox-exec"
        return "limits"
    if requested == "bwrap" and not bwrap_available():
        raise RpcError(SANDBOX_UNAVAILABLE, "bwrap is not on PATH or user namespaces are unusable")
    if requested == "sandbox-exec" and not sandbox_exec_available():
        raise RpcError(SANDBOX_UNAVAILABLE, "sandbox-exec is only available on macOS")
    return requested


def host_os() -> str:
    system = platform.system()
    if system == "Linux":
        return "linux"
    if system == "Darwin":
        return "macos"
    return "other"


# ---- limits --------------------------------------------------------------------


def limits_preexec(rlimit_as: int | None, rlimit_nofile: int | None) -> Callable[[], None]:
    """Return a preexec_fn that applies RLIMIT_AS and RLIMIT_NOFILE in the child."""

    def _apply() -> None:
        try:
            import resource
        except ImportError:  # pragma: no cover - windows
            return
        if rlimit_as:
            try:
                _, hard = resource.getrlimit(resource.RLIMIT_AS)
                soft = rlimit_as if hard == resource.RLIM_INFINITY else min(rlimit_as, hard)
                resource.setrlimit(resource.RLIMIT_AS, (soft, hard))
            except (ValueError, OSError):
                pass
        if rlimit_nofile:
            try:
                _, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
                soft = rlimit_nofile if hard == resource.RLIM_INFINITY else min(rlimit_nofile, hard)
                resource.setrlimit(resource.RLIMIT_NOFILE, (soft, hard))
            except (ValueError, OSError):
                pass

    return _apply


def rlimit_prologue(rlimit_as: int | None, rlimit_nofile: int | None) -> str:
    """Python source that applies the limits inside the child itself.

    Used for subprocesses started from the multi-threaded kernel, where ``preexec_fn`` is unsafe.
    """
    if not rlimit_as and not rlimit_nofile:
        return ""
    return (
        "import resource as _r\n"
        f"for _k, _v in (('RLIMIT_AS', {rlimit_as or 0}), ('RLIMIT_NOFILE', {rlimit_nofile or 0})):\n"
        "    if _v:\n"
        "        try:\n"
        "            _s, _h = _r.getrlimit(getattr(_r, _k))\n"
        "            _r.setrlimit(getattr(_r, _k), (_v if _h == _r.RLIM_INFINITY else min(_v, _h), _h))\n"
        "        except (ValueError, OSError):\n"
        "            pass\n"
        "del _r, _k, _v\n"
    )


# ---- bwrap ---------------------------------------------------------------------


def bwrap_argv(spec: SandboxSpec, cmd: list[str], bwrap: str = "bwrap") -> list[str]:
    home = Path(os.path.realpath(spec.home or _home()))
    argv: list[str] = [bwrap, "--ro-bind", "/", "/", "--tmpfs", str(home)]
    for d in spec.read_only_dirs():
        argv += ["--ro-bind", str(d), str(d)]
    for d in spec.writable_dirs():
        argv += ["--bind", str(d), str(d)]
    argv += ["--tmpfs", "/tmp"]
    if not spec.network:
        argv.append("--unshare-net")
    argv += [
        "--die-with-parent",
        "--new-session",
        "--unshare-pid",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--chdir",
        str(spec.root),
        "--",
    ]
    return argv + list(cmd)


# ---- sandbox-exec ----------------------------------------------------------------


def _q(path: Path | str) -> str:
    return '"' + str(path).replace("\\", "\\\\").replace('"', '\\"') + '"'


def sandbox_exec_profile(spec: SandboxSpec) -> str:
    home = Path(os.path.realpath(spec.home or _home()))
    lines = [
        "(version 1)",
        "(deny default)",
        '(allow file-read* (subpath "/"))',
        f"(deny file-read* (subpath {_q(home)}))",
    ]
    for d in spec.read_only_dirs():
        lines.append(f"(allow file-read* (subpath {_q(d)}))")
    for d in spec.writable_dirs():
        lines.append(f"(allow file-read* file-write* (subpath {_q(d)}))")
    lines.append('(allow file-write* (subpath "/private/tmp"))')
    lines.append('(allow file-write* (subpath "/private/var/folders"))')
    lines.append('(allow file-write* (literal "/dev/null"))')
    lines.append("(allow process*)")
    lines.append("(allow sysctl-read)")
    lines.append("(allow mach-lookup)")
    if spec.network:
        lines.append("(allow network*)")
    else:
        lines.append("(deny network*)")
    return "\n".join(lines) + "\n"


def sandbox_exec_argv(spec: SandboxSpec, cmd: list[str], exe: str = "sandbox-exec") -> list[str]:
    return [exe, "-p", sandbox_exec_profile(spec), *cmd]


# ---- entry point ------------------------------------------------------------------


def wrap(kind: str, spec: SandboxSpec, cmd: list[str]) -> SandboxCommand:
    """Wrap ``cmd`` for the chosen sandbox kind."""
    if kind == "bwrap":
        return SandboxCommand(kind, bwrap_argv(spec, cmd), spec.rlimit_as, spec.rlimit_nofile)
    if kind == "sandbox-exec":
        return SandboxCommand(
            kind,
            sandbox_exec_argv(spec, cmd),
            spec.rlimit_as,
            spec.rlimit_nofile,
            profile=sandbox_exec_profile(spec),
        )
    if kind == "limits":
        return SandboxCommand(kind, list(cmd), spec.rlimit_as, spec.rlimit_nofile)
    if kind == "none":
        return SandboxCommand(kind, list(cmd))
    raise RpcError(SANDBOX_UNAVAILABLE, f"unknown sandbox kind {kind!r}")


def venv_of(python: Path) -> Path | None:
    """The virtual environment containing ``python`` (first ancestor with pyvenv.cfg)."""
    try:
        p = python.resolve()
    except OSError:
        return None
    for parent in p.parents:
        if (parent / "pyvenv.cfg").is_file():
            return parent
    return None


def tooling_dir() -> Path:
    return _home() / ".analyst" / "tooling"
