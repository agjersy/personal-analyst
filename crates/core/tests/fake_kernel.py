#!/usr/bin/env python3
"""A stdlib-only fake of `analyst_kernel` for Rust integration tests.

Speaks the JSON-RPC framing of docs/PROTOCOL.md section 1 over stdio and implements:
kernel.hello, kernel.shutdown, env.info, env.packages, scripts.index (static), lib.index
(static), run.start (real detached script execution with run.stdout / run.stderr /
run.artifact / run.done notifications), run.attach (replays from the run directory, so it
works after a kernel restart), run.status, runs.list, run.cancel, exec.snippet, out.list,
out.read, eq.extract, eq.check, contract.check, contract.similar, lint.run.

Usage: python3 fake_kernel.py --python <py> --root <root> --runs-dir <dir> [--out-dir out] [--sandbox x]
"""
import argparse
import datetime
import json
import os
import random
import subprocess
import sys
import threading
import time

LOCK = threading.Lock()
RUNS = {}  # run_id -> dict(meta)


def now():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def send(obj):
    line = json.dumps(obj, separators=(",", ":"))
    with LOCK:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})


def artifact_kind(path):
    ext = path.rsplit(".", 1)[-1].lower() if "." in path else ""
    if ext in ("png", "svg", "pdf"):
        return "figure"
    if ext in ("csv", "tsv", "parquet"):
        return "table"
    if ext in ("npy", "npz"):
        return "array"
    if ext == "json":
        return "json"
    if ext in ("txt", "md", "log"):
        return "text"
    return "file"


class Kernel:
    def __init__(self, args):
        self.python = args.python
        self.root = os.path.abspath(args.root)
        self.runs_dir = os.path.abspath(args.runs_dir)
        self.out_dir = args.out_dir
        self.sandbox = "none"
        os.makedirs(self.runs_dir, exist_ok=True)

    # ---- helpers ----
    def run_dir(self, run_id):
        return os.path.join(self.runs_dir, run_id)

    def read_meta(self, run_id):
        p = os.path.join(self.run_dir(run_id), "meta.json")
        if not os.path.isfile(p):
            return None
        with open(p) as f:
            meta = json.load(f)
        status_path = os.path.join(self.run_dir(run_id), "status")
        status = "running"
        if os.path.isfile(status_path):
            with open(status_path) as f:
                status = f.read().strip() or "running"
        if status.startswith("done:"):
            meta["status"] = "done"
            meta["exit_code"] = int(status.split(":", 1)[1])
            if meta["exit_code"] != 0:
                meta["status"] = "failed"
        elif status in ("cancelled", "timeout", "failed"):
            meta["status"] = status
        else:
            meta["status"] = "running"
        return meta

    def write_meta(self, meta):
        with open(os.path.join(self.run_dir(meta["run_id"]), "meta.json"), "w") as f:
            json.dump(meta, f)

    # ---- methods ----
    def kernel_hello(self, p):
        return {
            "kernel_version": "fake-0.0.1",
            "protocol_version": 1,
            "host": "fake-host",
            "os": "linux",
            "sandbox": self.sandbox,
            "python": self.python,
            "root": self.root,
            "runs_dir": self.runs_dir,
        }

    def env_info(self, p):
        return {
            "python": self.python,
            "version": "3.13.0",
            "venv": None,
            "site_packages": [],
            "sandbox": self.sandbox,
            "host": "fake-host",
            "os": "linux",
            "uv": None,
        }

    def env_packages(self, p):
        return {"packages": [{"name": "numpy", "version": "2.0.0", "locked": False}]}

    def scripts_index(self, p):
        return {
            "scripts": [
                {
                    "path": "scripts/fit_decay.py",
                    "name": "fit_decay",
                    "description": "Fit a decay curve",
                    "has_main": True,
                    "has_main_guard": True,
                    "options": [
                        {"flags": ["--data"], "dest": "data", "type": "Path", "default": "data/decay.csv",
                         "help": "input CSV", "required": False, "choices": None, "action": None},
                        {"flags": ["--seed"], "dest": "seed", "type": "int", "default": 0,
                         "help": "random seed", "required": False, "choices": None, "action": None},
                    ],
                }
            ]
        }

    def lib_index(self, p):
        return {
            "functions": [
                {"module": "analysis.fitting", "qualname": "double_exp", "path": "analysis/fitting.py",
                 "line": 1, "signature": "(t, A, tau1, B, tau2)", "doc_summary": "Sum of two exponentials.",
                 "has_math": True, "callers": [{"path": "scripts/fit_decay.py", "line": 12}]}
            ]
        }

    def run_start(self, p):
        script = p["script"]
        full = os.path.join(self.root, script)
        if not os.path.isfile(full) or not script.endswith(".py"):
            raise RpcError(1004, "script not found", {"detail": script})
        args = p.get("args") or []
        run_id = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%d-%H%M%S") + "-" + "%06x" % random.randrange(16**6)
        stem = os.path.splitext(os.path.basename(script))[0]
        out_rel = f"{self.out_dir}/{stem}/{run_id}"
        out_abs = os.path.join(self.root, out_rel)
        rd = self.run_dir(run_id)
        os.makedirs(rd, exist_ok=True)
        os.makedirs(out_abs, exist_ok=True)
        meta = {
            "run_id": run_id, "script": script, "args": args, "status": "running", "exit_code": None,
            "started_at": now(), "finished_at": None, "out_dir": out_rel, "sandbox": self.sandbox,
            "tree_hash": p.get("tree_hash"), "pid": None, "host": "fake-host",
        }
        with open(os.path.join(out_abs, "run.json"), "w") as f:
            json.dump({"run_id": run_id, "script": script, "args": args, "started_at": meta["started_at"],
                       "host": "fake-host", "tree_hash": p.get("tree_hash")}, f)
        with open(os.path.join(rd, "status"), "w") as f:
            f.write("running\n")
        for name in ("stdout.log", "stderr.log", "artifacts.jsonl"):
            open(os.path.join(rd, name), "a").close()
        env = {
            "ANALYST_RUN_ID": run_id, "ANALYST_OUT_DIR": out_abs, "ANALYST_ROOT": self.root,
            "PYTHONUNBUFFERED": "1", "PATH": os.environ.get("PATH", ""), "HOME": os.environ.get("HOME", "/"),
        }
        wrapper = (
            "import subprocess,sys\n"
            "rd=sys.argv[1]; cmd=sys.argv[2:]\n"
            "out=open(rd+'/stdout.log','ab'); err=open(rd+'/stderr.log','ab')\n"
            "r=subprocess.run(cmd, stdout=out, stderr=err)\n"
            "open(rd+'/status','w').write('done:%d\\n' % r.returncode)\n"
        )
        proc = subprocess.Popen(
            [sys.executable, "-c", wrapper, rd, self.python, full] + list(args),
            cwd=self.root, env=env, start_new_session=True,
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        meta["pid"] = proc.pid
        self.write_meta(meta)
        RUNS[run_id] = meta
        threading.Thread(target=self.tail, args=(run_id, 0, 0, set()), daemon=True).start()
        return {"run_id": run_id, "out_dir": out_rel}

    def tail(self, run_id, so, se, seen):
        """Poll logs, output dir, and status; emit notifications; stop after run.done."""
        rd = self.run_dir(run_id)
        meta = self.read_meta(run_id)
        out_abs = os.path.join(self.root, meta["out_dir"])
        started = time.time()
        stable = {}
        while True:
            for name, method in (("stdout.log", "run.stdout"), ("stderr.log", "run.stderr")):
                off = so if name == "stdout.log" else se
                with open(os.path.join(rd, name), "rb") as f:
                    f.seek(off)
                    data = f.read()
                if data:
                    notify(method, {"run_id": run_id, "chunk": data.decode("utf-8", "replace"), "offset": off})
                    if name == "stdout.log":
                        so += len(data)
                    else:
                        se += len(data)
            self.scan_artifacts(run_id, out_abs, seen, stable, final=False)
            meta = self.read_meta(run_id)
            if meta["status"] != "running":
                # one last drain happens on the next loop iteration; do it now
                for name, method in (("stdout.log", "run.stdout"), ("stderr.log", "run.stderr")):
                    off = so if name == "stdout.log" else se
                    with open(os.path.join(rd, name), "rb") as f:
                        f.seek(off)
                        data = f.read()
                    if data:
                        notify(method, {"run_id": run_id, "chunk": data.decode("utf-8", "replace"), "offset": off})
                self.scan_artifacts(run_id, out_abs, seen, stable, final=True)
                meta["finished_at"] = now()
                self.write_meta(meta)
                notify("run.done", {"run_id": run_id, "status": meta["status"], "exit_code": meta["exit_code"],
                                    "duration_s": round(time.time() - started, 3)})
                return
            time.sleep(0.05)

    def scan_artifacts(self, run_id, out_abs, seen, stable, final):
        """Emit run.artifact for files whose size was stable for one poll (or all, when final)."""
        rd = self.run_dir(run_id)
        for dirpath, _, files in os.walk(out_abs):
            for fn in files:
                full = os.path.join(dirpath, fn)
                rel = os.path.relpath(full, self.root).replace(os.sep, "/")
                if rel in seen or fn == "run.json":
                    continue
                if fn.endswith(".json") and os.path.isfile(full[:-5] + ".png"):
                    continue  # sidecar
                size = os.path.getsize(full)
                if final or stable.get(rel) == size:
                    kind = artifact_kind(rel)
                    art = {"kind": kind, "path": rel, "size": size, "created_at": now(),
                           "rendered": kind in ("figure", "json", "text")}
                    seen.add(rel)
                    with open(os.path.join(rd, "artifacts.jsonl"), "a") as f:
                        f.write(json.dumps(art) + "\n")
                    notify("run.artifact", {"run_id": run_id, "artifact": art})
                stable[rel] = size

    def run_attach(self, p):
        run_id = p["run_id"]
        meta = self.read_meta(run_id)
        if meta is None:
            raise RpcError(1000, "run not found", {"detail": run_id})
        so = int(p.get("stdout_offset") or 0)
        se = int(p.get("stderr_offset") or 0)
        seen = set()
        threading.Thread(target=self.tail, args=(run_id, so, se, seen), daemon=True).start()
        return {"status": meta["status"], "exit_code": meta["exit_code"]}

    def run_status(self, p):
        meta = self.read_meta(p["run_id"])
        if meta is None:
            raise RpcError(1000, "run not found", {"detail": p["run_id"]})
        return meta

    def runs_list(self, p):
        ids = sorted(os.listdir(self.runs_dir), reverse=True)
        runs = [m for m in (self.read_meta(i) for i in ids) if m]
        return {"runs": runs[: int(p.get("limit") or 50)]}

    def run_cancel(self, p):
        meta = self.read_meta(p["run_id"])
        if meta is None:
            raise RpcError(1000, "run not found", {"detail": p["run_id"]})
        if meta["status"] == "running" and meta.get("pid"):
            try:
                os.killpg(meta["pid"], 15)
            except OSError:
                pass
            with open(os.path.join(self.run_dir(p["run_id"]), "status"), "w") as f:
                f.write("cancelled\n")
        return {"ok": True}

    def exec_snippet(self, p):
        r = subprocess.run([self.python, "-c", p["code"]], capture_output=True, text=True,
                           timeout=float(p.get("timeout_s") or 60), cwd=self.root)
        return {"ok": r.returncode == 0, "stdout": r.stdout, "stderr": r.stderr, "value_repr": None}

    def out_list(self, p):
        base = os.path.join(self.root, p.get("path") or self.out_dir)
        entries = []
        for dirpath, _, files in os.walk(base):
            for fn in files:
                full = os.path.join(dirpath, fn)
                rel = os.path.relpath(full, self.root).replace(os.sep, "/")
                st = os.stat(full)
                entries.append({"path": rel, "size": st.st_size, "kind": artifact_kind(rel),
                                "mtime": datetime.datetime.fromtimestamp(st.st_mtime, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")})
        return {"entries": sorted(entries, key=lambda e: e["path"])}

    def out_read(self, p):
        full = os.path.join(self.root, p["path"])
        if not os.path.abspath(full).startswith(self.root):
            raise RpcError(1002, "path outside root", {"detail": p["path"]})
        n = int(p.get("max_bytes") or 16384)
        with open(full, "rb") as f:
            data = f.read(n + 1)
        kind = artifact_kind(p["path"])
        try:
            preview = data[:n].decode("utf-8")
        except UnicodeDecodeError:
            return {"kind": kind, "preview": "", "truncated": True, "summary": None}
        return {"kind": kind, "preview": preview, "truncated": len(data) > n, "summary": None}

    def eq_extract(self, p):
        return {"equations": [{"function": p.get("function") or "double_exp", "qualname": p.get("function") or "double_exp",
                               "line": 1, "tier": "declared", "latex": "A e^{-t/\\tau_1} + B e^{-t/\\tau_2}",
                               "source_expr": "A*exp(-t/tau1) + B*exp(-t/tau2)", "status": "ok", "reason": None}]}

    def eq_check(self, p):
        return {"status": "agree", "trials": 32, "max_abs_err": 0.0, "max_rel_err": 0.0, "counterexample": None, "reason": None}

    def contract_check(self, p):
        return {"diagnostics": []}

    def contract_similar(self, p):
        return {"pairs": []}

    def lint_run(self, p):
        return {"diagnostics": []}


class RpcError(Exception):
    def __init__(self, code, message, data=None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.data = data or {"detail": message}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--python", required=True)
    ap.add_argument("--root", required=True)
    ap.add_argument("--runs-dir", required=True)
    ap.add_argument("--out-dir", default="out")
    ap.add_argument("--sandbox", default="auto")
    args = ap.parse_args()
    k = Kernel(args)
    methods = {
        "kernel.hello": k.kernel_hello, "env.info": k.env_info, "env.packages": k.env_packages,
        "scripts.index": k.scripts_index, "lib.index": k.lib_index, "run.start": k.run_start,
        "run.attach": k.run_attach, "run.status": k.run_status, "runs.list": k.runs_list,
        "run.cancel": k.run_cancel, "exec.snippet": k.exec_snippet, "out.list": k.out_list,
        "out.read": k.out_read, "eq.extract": k.eq_extract, "eq.check": k.eq_check,
        "contract.check": k.contract_check, "contract.similar": k.contract_similar, "lint.run": k.lint_run,
    }
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except ValueError:
            send({"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": "parse error", "data": {"detail": line[:100]}}})
            continue
        rid = req.get("id")
        method = req.get("method")
        params = req.get("params") or {}
        if method == "kernel.shutdown":
            send({"jsonrpc": "2.0", "id": rid, "result": {"ok": True}})
            return 0
        fn = methods.get(method)
        if fn is None:
            send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "method not found", "data": {"detail": method}}})
            continue
        try:
            result = fn(params)
            send({"jsonrpc": "2.0", "id": rid, "result": result})
        except RpcError as e:
            send({"jsonrpc": "2.0", "id": rid, "error": {"code": e.code, "message": e.message, "data": e.data}})
        except Exception as e:  # noqa: BLE001
            send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32603, "message": "internal error", "data": {"detail": repr(e)}}})
    return 0


if __name__ == "__main__":
    sys.exit(main())
