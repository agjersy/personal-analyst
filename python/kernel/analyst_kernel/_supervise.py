"""Detached run supervisor.

Started by the kernel with ``start_new_session=True`` so it becomes the process-group
leader of the run. It launches the (sandbox-wrapped) script with stdout/stderr on the
run's log files, waits for it, and writes the final ``status`` line and ``meta.json``
in the run directory. Because it is independent of the kernel, a run keeps its
bookkeeping even when the kernel restarts (PROTOCOL.md 1.4 step 7).

Usage:
    python -m analyst_kernel._supervise --run-dir DIR [--timeout S]
        [--rlimit-as BYTES] [--rlimit-nofile N] -- ARGV...

Everything printed goes to stderr.log; this process never touches the kernel's stdio.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

from .paths import utc_now_iso
from .sandbox import limits_preexec


def write_atomic(path: Path, text: str) -> None:
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(text, encoding="utf-8")
    os.replace(tmp, path)


def read_status(run_dir: Path) -> str:
    try:
        return (run_dir / "status").read_text(encoding="utf-8").strip()
    except OSError:
        return ""


def finish(run_dir: Path, status_line: str, exit_code: int | None) -> None:
    """Record the final state unless the kernel already recorded a cancel."""
    current = read_status(run_dir)
    if current and current != "running":
        status_line = current
    write_atomic(run_dir / "status", status_line + "\n")
    meta_path = run_dir / "meta.json"
    try:
        meta = json.loads(meta_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        meta = {}
    meta["status"] = status_line.split(":", 1)[0]
    meta["exit_code"] = exit_code
    meta["finished_at"] = utc_now_iso()
    write_atomic(meta_path, json.dumps(meta, indent=2) + "\n")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="analyst_kernel._supervise")
    parser.add_argument("--run-dir", required=True)
    parser.add_argument("--timeout", type=float, default=None)
    parser.add_argument("--rlimit-as", type=int, default=None)
    parser.add_argument("--rlimit-nofile", type=int, default=None)
    parser.add_argument("cmd", nargs=argparse.REMAINDER)
    ns = parser.parse_args(argv)
    cmd = ns.cmd[1:] if ns.cmd and ns.cmd[0] == "--" else ns.cmd
    run_dir = Path(ns.run_dir)

    # The supervisor must outlive a SIGTERM aimed at the group so it can record the result.
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, signal.SIG_IGN)

    preexec = None
    if ns.rlimit_as or ns.rlimit_nofile:
        preexec = limits_preexec(ns.rlimit_as, ns.rlimit_nofile)

    stdout = open(run_dir / "stdout.log", "ab", buffering=0)  # noqa: SIM115
    stderr = open(run_dir / "stderr.log", "ab", buffering=0)  # noqa: SIM115
    try:
        proc = subprocess.Popen(cmd, stdout=stdout, stderr=stderr, stdin=subprocess.DEVNULL, preexec_fn=preexec)
    except OSError as exc:
        stderr.write(f"analyst: could not start {cmd[0] if cmd else '?'}: {exc}\n".encode())
        finish(run_dir, "failed", None)
        return 1
    _write_child_pid(run_dir, proc.pid)
    status_line: str
    try:
        rc = proc.wait(timeout=ns.timeout)
    except subprocess.TimeoutExpired:
        finish(run_dir, "timeout", None)
        _kill_tree(proc)
        return 0
    if rc >= 0:
        status_line = f"done:{rc}"
        finish(run_dir, status_line, rc)
    else:
        finish(run_dir, "failed", rc)
    return 0


def _write_child_pid(run_dir: Path, pid: int) -> None:
    try:
        write_atomic(run_dir / "child.pid", f"{pid}\n")
    except OSError:
        pass


def _kill_tree(proc: subprocess.Popen) -> None:
    try:
        os.killpg(os.getpgid(0), signal.SIGTERM)  # ignored by this process
    except OSError:
        pass
    deadline = time.monotonic() + 5.0
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            return
        time.sleep(0.1)
    try:
        proc.kill()
    except OSError:
        pass
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        pass


if __name__ == "__main__":
    sys.exit(main())
