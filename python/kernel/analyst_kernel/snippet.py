"""``exec.snippet``: a persistent REPL subprocess in the session interpreter."""

from __future__ import annotations

import json
import logging
import os
import subprocess
import threading
import time
from pathlib import Path
from typing import Any

from .errors import TIMEOUT, RpcError
from .sandbox import SandboxCommand, SandboxSpec, rlimit_prologue, wrap

log = logging.getLogger("analyst_kernel.snippet")

DEFAULT_TIMEOUT_S = 30.0

# Runs inside the session python. Reads one JSON object per line on stdin and answers
# with one JSON object per line on stdout; the user's own stdout/stderr are captured.
REPL_DRIVER = r'''
import ast, io, json, sys, traceback, contextlib
_out = sys.stdout
sys.stdout = sys.__stdout__ = io.StringIO()  # user prints never reach the protocol stream
ns = {"__name__": "__analyst_snippet__"}
def run(code):
    so, se = io.StringIO(), io.StringIO()
    value = None
    ok = True
    with contextlib.redirect_stdout(so), contextlib.redirect_stderr(se):
        try:
            tree = ast.parse(code, mode="exec")
            last = None
            if tree.body and isinstance(tree.body[-1], ast.Expr):
                last = ast.Expression(tree.body.pop().value)
            if tree.body:
                exec(compile(tree, "<snippet>", "exec"), ns)
            if last is not None:
                value = eval(compile(last, "<snippet>", "eval"), ns)
                ns["_"] = value
        except SystemExit:
            ok = False
        except BaseException:
            ok = False
            traceback.print_exc(file=se)
    rep = None
    if value is not None:
        try:
            rep = repr(value)
        except Exception as exc:
            rep = "<repr failed: %s>" % exc
    return {"ok": ok, "stdout": so.getvalue(), "stderr": se.getvalue(), "value_repr": rep}
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        req = json.loads(line)
    except ValueError:
        continue
    res = run(req.get("code", ""))
    res["id"] = req.get("id")
    _out.write(json.dumps(res) + "\n")
    _out.flush()
'''


class SnippetRunner:
    def __init__(
        self,
        python: str,
        root: Path,
        sandbox_kind: str,
        spec_factory,
        env: dict[str, str],
    ) -> None:
        self.python = python
        self.root = root
        self.sandbox_kind = sandbox_kind
        self.spec_factory = spec_factory  # () -> SandboxSpec
        self.env = env
        self._proc: subprocess.Popen | None = None
        self._lock = threading.Lock()
        self._seq = 0

    def _command(self) -> SandboxCommand:
        spec: SandboxSpec = self.spec_factory()
        probe = wrap(self.sandbox_kind, spec, [])
        driver = rlimit_prologue(probe.rlimit_as, probe.rlimit_nofile) + REPL_DRIVER
        return wrap(self.sandbox_kind, spec, [self.python, "-u", "-c", driver])

    def _ensure(self) -> subprocess.Popen:
        if self._proc is not None and self._proc.poll() is None:
            return self._proc
        cmd = self._command()
        self._proc = subprocess.Popen(
            cmd.argv,
            cwd=str(self.root),
            env=self.env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
            bufsize=1,
            start_new_session=True,
        )
        return self._proc

    def _kill(self) -> None:
        proc = self._proc
        self._proc = None
        if proc is None:
            return
        try:
            os.killpg(proc.pid, 9)
        except OSError:
            try:
                proc.kill()
            except OSError:
                pass
        try:
            proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            pass

    def run(self, code: str, timeout_s: float | None = None) -> dict[str, Any]:
        timeout = float(timeout_s) if timeout_s is not None else DEFAULT_TIMEOUT_S
        with self._lock:
            proc = self._ensure()
            self._seq += 1
            req_id = self._seq
            assert proc.stdin is not None and proc.stdout is not None
            try:
                proc.stdin.write(json.dumps({"id": req_id, "code": code}) + "\n")
                proc.stdin.flush()
            except (OSError, ValueError) as exc:
                self._kill()
                return {"ok": False, "stdout": "", "stderr": f"REPL unavailable: {exc}", "value_repr": None}
            result: dict[str, Any] | None = None
            box: list[str | None] = []

            def reader() -> None:
                try:
                    box.append(proc.stdout.readline())
                except (OSError, ValueError):
                    box.append(None)

            t = threading.Thread(target=reader, daemon=True)
            t.start()
            t.join(timeout)
            if t.is_alive():
                self._kill()
                raise RpcError(TIMEOUT, f"snippet exceeded {timeout:g}s; REPL restarted")
            line = box[0] if box else None
            if not line:
                self._kill()
                return {"ok": False, "stdout": "", "stderr": "REPL process exited", "value_repr": None}
            try:
                result = json.loads(line)
            except ValueError:
                self._kill()
                return {"ok": False, "stdout": "", "stderr": "REPL protocol error", "value_repr": None}
            result.pop("id", None)
            return result

    def shutdown(self) -> None:
        with self._lock:
            proc = self._proc
            if proc is not None and proc.poll() is None:
                try:
                    proc.stdin.close()  # type: ignore[union-attr]
                except OSError:
                    pass
                deadline = time.monotonic() + 1.0
                while time.monotonic() < deadline and proc.poll() is None:
                    time.sleep(0.05)
            self._kill()
