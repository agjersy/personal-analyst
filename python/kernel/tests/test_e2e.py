"""Spawn ``python -m analyst_kernel`` and talk JSON-RPC over its stdio."""

from __future__ import annotations

import json
import os
import queue
import subprocess
import sys
import threading
import time
from pathlib import Path

KERNEL_DIR = Path(__file__).resolve().parent.parent


class KernelProc:
    def __init__(self, repo: Path, runs_dir: Path):
        env = dict(os.environ)
        env.pop("PYTHONPATH", None)
        self.proc = subprocess.Popen(
            [sys.executable, "-m", "analyst_kernel", "--python", sys.executable, "--root", str(repo),
             "--runs-dir", str(runs_dir), "--sandbox", "limits"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1,
            cwd=str(KERNEL_DIR), env=env,
        )
        self.q: queue.Queue = queue.Queue()
        self.notes: list[dict] = []
        self._id = 0
        threading.Thread(target=self._reader, daemon=True).start()

    def _reader(self):
        for line in self.proc.stdout:
            msg = json.loads(line)
            self.q.put(msg)

    def request(self, method, params=None, timeout=60):
        self._id += 1
        rid = self._id
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}}) + "\n")
        self.proc.stdin.flush()
        deadline = time.monotonic() + timeout
        while True:
            msg = self.q.get(timeout=max(0.1, deadline - time.monotonic()))
            if msg.get("id") == rid:
                return msg
            if "id" not in msg:
                self.notes.append(msg)

    def wait_note(self, method, run_id, timeout=90):
        deadline = time.monotonic() + timeout
        while True:
            for n in self.notes:
                if n["method"] == method and n["params"].get("run_id") == run_id:
                    return n["params"]
            msg = self.q.get(timeout=max(0.1, deadline - time.monotonic()))
            if "id" not in msg:
                self.notes.append(msg)

    def notes_of(self, method, run_id):
        return [n["params"] for n in self.notes if n["method"] == method and n["params"].get("run_id") == run_id]


def test_end_to_end(repo: Path, tmp_path: Path):
    runs_dir = tmp_path / "runs"
    k = KernelProc(repo, runs_dir)
    try:
        hello = k.request("kernel.hello", {"core_version": "0.0.1", "protocol_version": 1})["result"]
        assert hello["protocol_version"] == 1 and hello["sandbox"] == "limits" and hello["kernel_version"]

        scripts = k.request("scripts.index")["result"]["scripts"]
        assert {s["name"] for s in scripts} == {"bad_script", "fit_decay", "no_parser"}

        err = k.request("eq.extract", {"path": "../x.py"})["error"]
        assert err["code"] == 1002 and err["data"]["detail"]

        res = k.request("run.start", {"script": "scripts/fit_decay.py", "args": ["--seed", "3"]})["result"]
        run_id = res["run_id"]
        done = k.wait_note("run.done", run_id)
        assert done["status"] == "done" and done["exit_code"] == 0
        arts = k.notes_of("run.artifact", run_id)
        fig = next(a["artifact"] for a in arts if a["artifact"]["path"].endswith("decay-fit.png"))
        assert fig["kind"] == "figure" and fig["sidecar"]["args"] == ["--seed", "3"]
        assert fig["sidecar"]["title"] == "Decay fit"
        assert "done 50" in "".join(p["chunk"] for p in k.notes_of("run.stdout", run_id))

        eq = k.request("eq.check", {"path": "analysis/models.py", "function": "double_exp"})["result"]
        assert eq["status"] == "agree"
        snippet = k.request("exec.snippet", {"code": "1 + 1"})["result"]
        assert snippet["value_repr"] == "2"
        assert k.request("kernel.shutdown")["result"] == {"ok": True}
        assert k.proc.wait(timeout=20) == 0
    finally:
        if k.proc.poll() is None:
            k.proc.kill()
    stderr = k.proc.stderr.read()
    assert "Traceback" not in stderr, stderr

    # attach from a fresh kernel process: everything comes from the run directory
    k2 = KernelProc(repo, runs_dir)
    try:
        k2.request("kernel.hello", {"core_version": "0.0.1", "protocol_version": 1})
        resp = k2.request("run.attach", {"run_id": run_id, "stdout_offset": 0})
        assert resp["result"] == {"status": "done", "exit_code": 0}
        done = k2.wait_note("run.done", run_id, timeout=30)
        assert done["status"] == "done"
        assert "done 50" in "".join(p["chunk"] for p in k2.notes_of("run.stdout", run_id))
        assert any(a["artifact"]["path"].endswith("decay-fit.png") for a in k2.notes_of("run.artifact", run_id))
        # the response to run.attach precedes its replayed notifications
        listed = k2.request("runs.list")["result"]["runs"]
        assert listed[0]["run_id"] == run_id and listed[0]["status"] == "done"
        assert k2.request("kernel.shutdown")["result"] == {"ok": True}
        assert k2.proc.wait(timeout=20) == 0
    finally:
        if k2.proc.poll() is None:
            k2.proc.kill()


def test_cli_usage_errors():
    proc = subprocess.run([sys.executable, "-m", "analyst_kernel"], capture_output=True, text=True, cwd=str(KERNEL_DIR))
    assert proc.returncode == 2 and "--python" in proc.stderr
    proc = subprocess.run(
        [sys.executable, "-m", "analyst_kernel", "--python", sys.executable, "--root", "/nonexistent/dir",
         "--runs-dir", "/tmp/x", "--sandbox", "limits"],
        capture_output=True, text=True, cwd=str(KERNEL_DIR),
    )
    assert proc.returncode == 2
    proc = subprocess.run(
        [sys.executable, "-m", "analyst_kernel", "--python", sys.executable, "--root", str(KERNEL_DIR),
         "--runs-dir", "/tmp/x", "--sandbox", "bwrap"],
        capture_output=True, text=True, cwd=str(KERNEL_DIR), input="",
    )
    # bwrap is not available in this container: explicit request fails cleanly
    assert proc.returncode in (0, 3)
