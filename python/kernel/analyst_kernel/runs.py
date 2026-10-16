"""Run execution (PROTOCOL.md 1.4): detached runs, log tailing, artifact discovery, attach."""

from __future__ import annotations

import json
import logging
import os
import secrets
import signal
import subprocess
import sys
import threading
import time
from collections.abc import Callable
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from .config import ProjectConfig
from .errors import RUN_NOT_FOUND, SCRIPT_NOT_FOUND, RpcError, invalid_params
from .paths import mtime_iso, parse_iso, rel_to_root, resolve_in_root, utc_now_iso
from .rpc import Deferred
from .sandbox import SandboxSpec, tooling_dir, venv_of, wrap

log = logging.getLogger("analyst_kernel.runs")

POLL_S = 0.25
PASSTHROUGH_ENV = ("HOME", "PATH", "LANG", "TMPDIR", "VIRTUAL_ENV")
FIGURE_EXTS = (".png", ".svg", ".pdf")
ARTIFACT_KINDS = {
    ".png": "figure",
    ".svg": "figure",
    ".pdf": "figure",
    ".csv": "table",
    ".tsv": "table",
    ".parquet": "table",
    ".npy": "array",
    ".npz": "array",
    ".json": "json",
    ".txt": "text",
    ".md": "text",
    ".log": "text",
}
RENDERED_KINDS = {"figure", "json", "text"}
META_KEYS = (
    "run_id",
    "script",
    "args",
    "status",
    "exit_code",
    "started_at",
    "finished_at",
    "out_dir",
    "sandbox",
    "tree_hash",
    "pid",
    "host",
)

Notify = Callable[[str, dict[str, Any]], None]


def artifact_kind(path: Path) -> str:
    return ARTIFACT_KINDS.get(path.suffix.lower(), "file")


def new_run_id() -> str:
    stamp = datetime.now(UTC).strftime("%Y%m%d-%H%M%S")
    return f"{stamp}-{secrets.token_hex(3)}"


def write_atomic(path: Path, text: str) -> None:
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(text, encoding="utf-8")
    os.replace(tmp, path)


def read_status_line(run_dir: Path) -> str:
    try:
        return (run_dir / "status").read_text(encoding="utf-8").strip()
    except OSError:
        return ""


def status_from_line(line: str) -> tuple[str, int | None]:
    if line.startswith("done:"):
        try:
            return "done", int(line.split(":", 1)[1])
        except ValueError:
            return "done", None
    if line in ("running", "cancelled", "timeout", "failed"):
        return line, None
    return "failed", None


def read_meta(run_dir: Path) -> dict[str, Any]:
    try:
        meta = json.loads((run_dir / "meta.json").read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise RpcError(RUN_NOT_FOUND, f"run {run_dir.name} has no readable meta.json") from exc
    # The status file is the source of truth for the state; meta may lag behind.
    status, code = status_from_line(read_status_line(run_dir))
    if status != "running" or meta.get("status") == "running":
        meta["status"] = status
        if status == "done":
            meta["exit_code"] = code
        elif meta.get("exit_code") is None:
            meta["exit_code"] = None
    return {k: meta.get(k) for k in META_KEYS}


def pid_alive(pid: int | None) -> bool:
    if not pid:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return False
    return True


class RunManager:
    """Owns the runs directory. Every query reads disk; memory holds only live watchers."""

    def __init__(
        self,
        root: Path,
        runs_dir: Path,
        python: str,
        sandbox_kind: str,
        config: ProjectConfig,
        notify: Notify,
        mpl_dir: Path,
        host: str,
        out_dir_name: str | None = None,
        supervisor_python: str | None = None,
        poll_s: float = POLL_S,
    ) -> None:
        self.root = root.resolve()
        self.runs_dir = runs_dir.resolve()
        self.python = python
        self.sandbox_kind = sandbox_kind
        self.config = config
        self.notify = notify
        self.mpl_dir = mpl_dir
        self.host = host
        self.out_dir_name = (out_dir_name or config.out_dir).strip("/")
        self.supervisor_python = supervisor_python or sys.executable
        self.poll_s = poll_s
        self._watchers: dict[str, _Watcher] = {}
        self._lock = threading.Lock()
        self.runs_dir.mkdir(parents=True, exist_ok=True)

    # ---- helpers ------------------------------------------------------------
    def run_dir(self, run_id: str) -> Path:
        if not run_id or "/" in run_id or run_id in (".", "..") or run_id.startswith("_"):
            raise RpcError(RUN_NOT_FOUND, f"invalid run id {run_id!r}")
        d = self.runs_dir / run_id
        if not (d / "meta.json").is_file():
            raise RpcError(RUN_NOT_FOUND, f"run {run_id} not found")
        return d

    def _build_env(self, run_id: str, run_dir: Path, out_abs: Path, extra: dict[str, str]) -> dict[str, str]:
        env: dict[str, str] = {}
        for key in PASSTHROUGH_ENV:
            if key in os.environ:
                env[key] = os.environ[key]
        env.update({k: str(v) for k, v in extra.items()})
        # analyst_mpl first (figure capture), then whatever the caller set, then the repo root so
        # ``python scripts/x.py`` can import the library package without installing it.
        parts = [str(self.mpl_dir)]
        if env.get("PYTHONPATH"):
            parts.append(env["PYTHONPATH"])
        parts.append(str(self.root))
        pythonpath = os.pathsep.join(parts)
        env.update(
            {
                "ANALYST_RUN_ID": run_id,
                "ANALYST_OUT_DIR": str(out_abs),
                "ANALYST_ROOT": str(self.root),
                "MPLBACKEND": "module://analyst_mpl",
                "MPLCONFIGDIR": str(run_dir / "mplconfig"),
                "PYTHONPATH": pythonpath,
                "PYTHONUNBUFFERED": "1",
            }
        )
        return env

    def sandbox_spec(self, run_dir: Path, out_abs: Path, network: bool) -> SandboxSpec:
        derived = self.root / "data" / "derived"
        return SandboxSpec(
            root=self.root,
            venv=venv_of(Path(self.python)),
            run_dir=run_dir,
            out_dir=out_abs,
            data_dirs=[resolve_data_dir(self.root, d) for d in self.config.data_dirs],
            tooling_dir=tooling_dir(),
            derived_dir=derived if derived.is_dir() else None,
            network=network,
        )

    # ---- run.start ------------------------------------------------------------
    def start(
        self,
        script: str,
        args: list[str] | None = None,
        env: dict[str, str] | None = None,
        timeout_s: float | None = None,
        network: bool = False,
        tree_hash: str | None = None,
    ) -> dict[str, Any]:
        args = [str(a) for a in (args or [])]
        script_abs = resolve_in_root(self.root, script)
        if not script_abs.is_file() or script_abs.suffix != ".py":
            raise RpcError(SCRIPT_NOT_FOUND, f"{script} is not an existing Python file")
        script_rel = rel_to_root(self.root, script_abs)
        timeout = float(timeout_s) if timeout_s is not None else float(self.config.timeout_s)

        run_id = new_run_id()
        run_dir = self.runs_dir / run_id
        while run_dir.exists():  # pragma: no cover - 6 hex digits per second
            run_id = new_run_id()
            run_dir = self.runs_dir / run_id
        run_dir.mkdir(parents=True)
        (run_dir / "mplconfig").mkdir()
        out_rel = f"{self.out_dir_name}/{script_abs.stem}/{run_id}"
        out_abs = self.root / out_rel
        out_abs.mkdir(parents=True, exist_ok=True)

        started_at = utc_now_iso()
        meta: dict[str, Any] = {
            "run_id": run_id,
            "script": script_rel,
            "args": args,
            "status": "running",
            "exit_code": None,
            "started_at": started_at,
            "finished_at": None,
            "out_dir": out_rel,
            "sandbox": self.sandbox_kind,
            "tree_hash": tree_hash,
            "pid": None,
            "host": self.host,
        }
        (run_dir / "stdout.log").touch()
        (run_dir / "stderr.log").touch()
        (run_dir / "artifacts.jsonl").touch()
        write_atomic(run_dir / "status", "running\n")
        write_atomic(run_dir / "launch.json", json.dumps({"timeout_s": timeout, "network": network}))
        write_atomic(
            out_abs / "run.json",
            json.dumps(
                {
                    "run_id": run_id,
                    "script": script_rel,
                    "args": args,
                    "started_at": started_at,
                    "host": self.host,
                    "tree_hash": tree_hash,
                },
                indent=2,
            )
            + "\n",
        )

        cmd = [self.python, str(script_abs), *args]
        wrapped = wrap(self.sandbox_kind, self.sandbox_spec(run_dir, out_abs, network), cmd)
        sup = [
            self.supervisor_python,
            "-m",
            "analyst_kernel._supervise",
            "--run-dir",
            str(run_dir),
            "--timeout",
            str(timeout),
        ]
        if wrapped.rlimit_as:
            sup += ["--rlimit-as", str(wrapped.rlimit_as)]
        if wrapped.rlimit_nofile:
            sup += ["--rlimit-nofile", str(wrapped.rlimit_nofile)]
        sup += ["--", *wrapped.argv]
        child_env = self._build_env(run_id, run_dir, out_abs, env or {})
        try:
            proc = subprocess.Popen(
                sup,
                cwd=str(self.root),
                env=child_env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
        except OSError as exc:
            meta["status"] = "failed"
            meta["finished_at"] = utc_now_iso()
            write_atomic(run_dir / "meta.json", json.dumps(meta, indent=2) + "\n")
            write_atomic(run_dir / "status", "failed\n")
            (run_dir / "stderr.log").write_text(f"analyst: could not launch supervisor: {exc}\n")
            proc = None
        else:
            meta["pid"] = proc.pid
            write_atomic(run_dir / "meta.json", json.dumps(meta, indent=2) + "\n")
        watcher = _Watcher(self, run_id, run_dir, out_abs, proc, timeout)
        with self._lock:
            self._watchers[run_id] = watcher
        watcher.start()
        return {"run_id": run_id, "out_dir": out_rel}

    # ---- run.status / runs.list ---------------------------------------------
    def status(self, run_id: str) -> dict[str, Any]:
        return read_meta(self.run_dir(run_id))

    def list(self, limit: int | None = None) -> list[dict[str, Any]]:
        limit = int(limit) if limit is not None else 50
        out: list[dict[str, Any]] = []
        try:
            names = sorted((p.name for p in self.runs_dir.iterdir() if p.is_dir()), reverse=True)
        except OSError:
            names = []
        for name in names:
            if len(out) >= limit:
                break
            if name.startswith("_") or not (self.runs_dir / name / "meta.json").is_file():
                continue
            try:
                out.append(read_meta(self.runs_dir / name))
            except RpcError:
                continue
        return out

    # ---- run.cancel ------------------------------------------------------------
    def cancel(self, run_id: str) -> bool:
        run_dir = self.run_dir(run_id)
        line = read_status_line(run_dir)
        if line != "running":
            return False
        meta = read_meta(run_dir)
        write_atomic(run_dir / "status", "cancelled\n")
        pid = meta.get("pid")
        if pid:
            _signal_group(int(pid), signal.SIGTERM)
            threading.Thread(target=_kill_later, args=(int(pid), 5.0), daemon=True).start()
        return True

    # ---- run.attach -------------------------------------------------------------
    def attach(self, run_id: str, stdout_offset: int = 0, stderr_offset: int = 0) -> Deferred:
        run_dir = self.run_dir(run_id)
        meta = read_meta(run_dir)
        out_abs = self.root / meta["out_dir"]
        with self._lock:
            watcher = self._watchers.get(run_id)
            if watcher is None and meta["status"] == "running":
                timeout = _launch_timeout(run_dir, self.config.timeout_s)
                watcher = _Watcher(self, run_id, run_dir, out_abs, None, timeout, adopt=True)
                self._watchers[run_id] = watcher
                watcher.start()
        result = {"status": meta["status"], "exit_code": meta["exit_code"]}

        def replay() -> None:
            self._replay(run_id, run_dir, out_abs, meta, watcher, stdout_offset, stderr_offset)

        return Deferred(result, replay)

    def _replay(
        self,
        run_id: str,
        run_dir: Path,
        out_abs: Path,
        meta: dict[str, Any],
        watcher: _Watcher | None,
        stdout_offset: int,
        stderr_offset: int,
    ) -> None:
        if watcher is not None:
            # Replay only what the live watcher has already emitted; it continues from there.
            with watcher.lock:
                _replay_log(self.notify, run_id, "run.stdout", run_dir / "stdout.log", stdout_offset, watcher.offsets["stdout"])
                _replay_log(self.notify, run_id, "run.stderr", run_dir / "stderr.log", stderr_offset, watcher.offsets["stderr"])
                for art in watcher.known_artifacts():
                    self.notify("run.artifact", {"run_id": run_id, "artifact": art})
            return
        # Finished run with no watcher: make sure late artifacts are on record, then replay.
        scanner = _ArtifactScanner(self.root, run_dir, out_abs)
        scanner.load_recorded()
        for art in scanner.scan(final=True):
            scanner.record(art)
        _replay_log(self.notify, run_id, "run.stdout", run_dir / "stdout.log", stdout_offset, None)
        _replay_log(self.notify, run_id, "run.stderr", run_dir / "stderr.log", stderr_offset, None)
        for art in scanner.recorded:
            self.notify("run.artifact", {"run_id": run_id, "artifact": art})
        if meta["status"] != "running":
            self.notify(
                "run.done",
                {
                    "run_id": run_id,
                    "status": meta["status"],
                    "exit_code": meta["exit_code"],
                    "duration_s": _duration(meta),
                },
            )

    def _watcher_done(self, run_id: str) -> None:
        with self._lock:
            self._watchers.pop(run_id, None)

    def shutdown(self) -> None:
        with self._lock:
            watchers = list(self._watchers.values())
        for w in watchers:
            w.stop.set()
        for w in watchers:
            w.join(timeout=2.0)


def resolve_data_dir(root: Path, d: str) -> Path:
    p = Path(os.path.expanduser(d))
    return p if p.is_absolute() else root / p


def _launch_timeout(run_dir: Path, default: float) -> float:
    try:
        return float(json.loads((run_dir / "launch.json").read_text())["timeout_s"])
    except (OSError, ValueError, KeyError, TypeError):
        return float(default)


def _duration(meta: dict[str, Any]) -> float:
    start = parse_iso(meta.get("started_at"))
    end = parse_iso(meta.get("finished_at"))
    if start is None:
        return 0.0
    if end is None:
        end = time.time()
    return round(max(0.0, end - start), 3)


def _signal_group(pid: int, sig: int) -> None:
    try:
        os.killpg(pid, sig)
    except ProcessLookupError:
        pass
    except OSError:
        try:
            os.kill(pid, sig)
        except OSError:
            pass


def _kill_later(pid: int, delay: float) -> None:
    deadline = time.monotonic() + delay
    while time.monotonic() < deadline:
        time.sleep(0.2)
        try:
            os.killpg(pid, 0)
        except OSError:
            return
    _signal_group(pid, signal.SIGKILL)


def _replay_log(
    notify: Notify, run_id: str, method: str, path: Path, start: int, end: int | None
) -> None:
    try:
        with path.open("rb") as fh:
            fh.seek(max(0, int(start)))
            data = fh.read() if end is None else fh.read(max(0, end - int(start)))
    except OSError:
        return
    if data:
        notify(method, {"run_id": run_id, "chunk": data.decode("utf-8", errors="replace"), "offset": int(start)})


class _ArtifactScanner:
    """Discover artifacts under a run's output directory with the size-stable rule."""

    def __init__(self, root: Path, run_dir: Path, out_abs: Path) -> None:
        self.root = root
        self.run_dir = run_dir
        self.out_abs = out_abs
        self.sizes: dict[Path, int] = {}
        self.sidecar_waits: dict[Path, int] = {}
        self.emitted: set[str] = set()
        self.recorded: list[dict[str, Any]] = []

    def load_recorded(self) -> None:
        try:
            lines = (self.run_dir / "artifacts.jsonl").read_text(encoding="utf-8").splitlines()
        except OSError:
            return
        for line in lines:
            try:
                art = json.loads(line)
            except ValueError:
                continue
            self.recorded.append(art)
            self.emitted.add(art["path"])

    def record(self, art: dict[str, Any]) -> None:
        self.recorded.append(art)
        with (self.run_dir / "artifacts.jsonl").open("a", encoding="utf-8") as fh:
            fh.write(json.dumps(art, separators=(",", ":")) + "\n")

    def scan(self, final: bool = False) -> list[dict[str, Any]]:
        found: list[dict[str, Any]] = []
        if not self.out_abs.is_dir():
            return found
        for file in sorted(p for p in self.out_abs.rglob("*") if p.is_file()):
            if file.name.endswith(".tmp"):
                continue
            if file.parent == self.out_abs and file.name == "run.json":
                continue
            rel = rel_to_root(self.root, file)
            if rel in self.emitted:
                continue
            try:
                st = file.stat()
            except OSError:
                continue
            prev = self.sizes.get(file)
            self.sizes[file] = st.st_size
            if not final and (prev is None or prev != st.st_size):
                continue
            ext = file.suffix.lower()
            if ext == ".json" and any(file.with_suffix(e).exists() for e in FIGURE_EXTS):
                continue  # sidecar of a figure, not an artifact
            kind = artifact_kind(file)
            sidecar = None
            if kind == "figure":
                sc = file.with_suffix(".json")
                if sc.exists():
                    try:
                        sidecar = json.loads(sc.read_text(encoding="utf-8"))
                    except (OSError, ValueError):
                        if not final:
                            continue
                        sidecar = None
                elif not final:
                    waits = self.sidecar_waits.get(file, 0) + 1
                    self.sidecar_waits[file] = waits
                    if waits <= 2:
                        continue
            art: dict[str, Any] = {
                "kind": kind,
                "path": rel,
                "size": st.st_size,
                "created_at": mtime_iso(st.st_mtime),
                "rendered": kind in RENDERED_KINDS,
            }
            if isinstance(sidecar, dict):
                art["sidecar"] = sidecar
            self.emitted.add(rel)
            found.append(art)
        return found


class _Watcher(threading.Thread):
    """Per-run tailer: logs, artifacts, completion, and the timeout backstop."""

    def __init__(
        self,
        manager: RunManager,
        run_id: str,
        run_dir: Path,
        out_abs: Path,
        proc: subprocess.Popen | None,
        timeout_s: float,
        adopt: bool = False,
    ) -> None:
        super().__init__(name=f"watch-{run_id}", daemon=True)
        self.manager = manager
        self.run_id = run_id
        self.run_dir = run_dir
        self.out_abs = out_abs
        self.proc = proc
        self.timeout_s = timeout_s
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.scanner = _ArtifactScanner(manager.root, run_dir, out_abs)
        self.offsets = {"stdout": 0, "stderr": 0}
        if adopt:
            # Attach after a restart: replay covers the past; continue from the current end.
            self.scanner.load_recorded()
            for name in ("stdout", "stderr"):
                try:
                    self.offsets[name] = (run_dir / f"{name}.log").stat().st_size
                except OSError:
                    self.offsets[name] = 0

    def known_artifacts(self) -> list[dict[str, Any]]:
        return list(self.scanner.recorded)

    def _notify(self, method: str, params: dict[str, Any]) -> None:
        try:
            self.manager.notify(method, params)
        except Exception:
            log.exception("notification %s failed", method)

    def _tail(self, name: str) -> None:
        path = self.run_dir / f"{name}.log"
        try:
            with path.open("rb") as fh:
                fh.seek(self.offsets[name])
                data = fh.read()
        except OSError:
            return
        if not data:
            return
        # Keep partial UTF-8 sequences for the next poll.
        cut = len(data)
        while cut > max(0, len(data) - 4):
            try:
                data[:cut].decode("utf-8")
                break
            except UnicodeDecodeError:
                cut -= 1
        if cut <= 0:
            return
        chunk = data[:cut].decode("utf-8", errors="replace")
        offset = self.offsets[name]
        self.offsets[name] = offset + cut
        self._notify(f"run.{name}", {"run_id": self.run_id, "chunk": chunk, "offset": offset})

    def _artifacts(self, final: bool) -> None:
        for art in self.scanner.scan(final=final):
            self.scanner.record(art)
            self._notify("run.artifact", {"run_id": self.run_id, "artifact": art})

    def _supervisor_alive(self) -> bool:
        if self.proc is not None:
            return self.proc.poll() is None
        try:
            pid = json.loads((self.run_dir / "meta.json").read_text()).get("pid")
        except (OSError, ValueError):
            return False
        return pid_alive(pid)

    def run(self) -> None:
        t0 = time.monotonic()
        started = parse_iso(_read_meta_field(self.run_dir, "started_at")) or time.time()
        try:
            while not self.stop.is_set():
                with self.lock:
                    self._tail("stdout")
                    self._tail("stderr")
                    self._artifacts(final=False)
                line = read_status_line(self.run_dir)
                if line != "running":
                    break
                if not self._supervisor_alive():
                    # Supervisor vanished without recording a result.
                    _finish_failed(self.run_dir)
                    break
                if self.timeout_s and time.time() - started > self.timeout_s + 10.0:
                    # Backstop: the supervisor should have done this already.
                    write_atomic(self.run_dir / "status", "timeout\n")
                    pid = _read_meta_field(self.run_dir, "pid")
                    if pid:
                        _signal_group(int(pid), signal.SIGKILL)
                    break
                self.stop.wait(self.manager.poll_s)
            if self.stop.is_set():
                return
            # Let the supervisor finish writing meta.json, then do the final flush.
            deadline = time.monotonic() + 2.0
            while time.monotonic() < deadline:
                if self.proc is not None and self.proc.poll() is not None:
                    break
                if self.proc is None and not self._supervisor_alive():
                    break
                time.sleep(0.05)
            with self.lock:
                self._tail("stdout")
                self._tail("stderr")
                self._artifacts(final=False)
                self._artifacts(final=True)
            meta = read_meta(self.run_dir)
            if meta.get("finished_at") is None:
                _set_finished(self.run_dir, meta)
            duration = _duration(meta) if meta.get("finished_at") else round(time.monotonic() - t0, 3)
            self._notify(
                "run.done",
                {
                    "run_id": self.run_id,
                    "status": meta["status"],
                    "exit_code": meta["exit_code"],
                    "duration_s": duration,
                },
            )
        except Exception:
            log.exception("watcher for %s crashed", self.run_id)
        finally:
            self.manager._watcher_done(self.run_id)


def _read_meta_field(run_dir: Path, key: str) -> Any:
    try:
        return json.loads((run_dir / "meta.json").read_text(encoding="utf-8")).get(key)
    except (OSError, ValueError):
        return None


def _finish_failed(run_dir: Path) -> None:
    write_atomic(run_dir / "status", "failed\n")
    try:
        meta = json.loads((run_dir / "meta.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return
    meta["status"] = "failed"
    meta["finished_at"] = utc_now_iso()
    write_atomic(run_dir / "meta.json", json.dumps(meta, indent=2) + "\n")


def _set_finished(run_dir: Path, meta: dict[str, Any]) -> None:
    meta["finished_at"] = utc_now_iso()
    try:
        full = json.loads((run_dir / "meta.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        full = {}
    full.update({k: meta[k] for k in ("status", "exit_code", "finished_at")})
    write_atomic(run_dir / "meta.json", json.dumps(full, indent=2) + "\n")


def validate_run_params(params: dict[str, Any]) -> None:
    rid = params.get("run_id")
    if not isinstance(rid, str) or not rid:
        raise invalid_params("run_id must be a non-empty string")
