"""The kernel: wires every RPC method onto the modules. Shared by the stdio server and tests."""

from __future__ import annotations

import logging
import socket
import sys
import threading
from collections.abc import Callable
from pathlib import Path
from typing import Any

from . import PROTOCOL_VERSION, __version__
from .config import ProjectConfig, load_config
from .errors import RpcError, invalid_params
from .paths import resolve_in_root
from .rpc import Dispatcher
from .runs import RunManager, resolve_data_dir
from .sandbox import SandboxSpec, detect, host_os, tooling_dir, venv_of

log = logging.getLogger("analyst_kernel")

Notify = Callable[[str, dict[str, Any]], None]


def _opt(params: dict[str, Any], key: str, types: tuple[type, ...], default: Any = None) -> Any:
    value = params.get(key, default)
    if value is None:
        return default
    if isinstance(value, bool) and bool not in types:
        raise invalid_params(f"{key} has the wrong type")
    if not isinstance(value, types):
        raise invalid_params(f"{key} has the wrong type")
    return value


def _str_list(params: dict[str, Any], key: str) -> list[str] | None:
    value = params.get(key)
    if value is None:
        return None
    if not isinstance(value, list) or not all(isinstance(v, str) for v in value):
        raise invalid_params(f"{key} must be a list of strings")
    return value


class Kernel:
    def __init__(
        self,
        python: str,
        root: Path,
        runs_dir: Path,
        out_dir: str | None = None,
        sandbox: str = "auto",
        notify: Notify | None = None,
    ) -> None:
        self.python = python
        self.root = Path(root).resolve()
        self.runs_dir = Path(runs_dir).resolve()
        self.config: ProjectConfig = load_config(self.root)
        self.out_dir_name = (out_dir or self.config.out_dir).strip("/")
        self.sandbox = detect(sandbox)
        self.host = socket.gethostname()
        self.os_name = host_os()
        self._notify: Notify = notify or (lambda m, p: None)
        self.mpl_dir = Path(__file__).resolve().parent.parent  # directory containing analyst_mpl/
        self.runs = RunManager(
            root=self.root,
            runs_dir=self.runs_dir,
            python=self.python,
            sandbox_kind=self.sandbox,
            config=self.config,
            notify=self.notify,
            mpl_dir=self.mpl_dir,
            host=self.host,
            out_dir_name=self.out_dir_name,
        )
        self._snippets = None
        self._snippets_lock = threading.Lock()
        self.dispatcher = Dispatcher()
        self._register()

    # ---- plumbing ---------------------------------------------------------------
    def notify(self, method: str, params: dict[str, Any]) -> None:
        self._notify(method, params)

    def set_notify(self, fn: Notify) -> None:
        self._notify = fn

    def call(self, method: str, params: dict[str, Any] | None = None) -> Any:
        return self.dispatcher.call(method, params)

    def _register(self) -> None:
        reg = self.dispatcher.register
        reg("kernel.hello", self.m_hello)
        reg("kernel.shutdown", self.m_shutdown)
        reg("env.info", self.m_env_info)
        reg("env.packages", self.m_env_packages)
        reg("scripts.index", self.m_scripts_index)
        reg("lib.index", self.m_lib_index)
        reg("run.start", self.m_run_start)
        reg("run.attach", self.m_run_attach)
        reg("run.status", self.m_run_status)
        reg("run.cancel", self.m_run_cancel)
        reg("runs.list", self.m_runs_list)
        reg("exec.snippet", self.m_exec_snippet)
        reg("out.list", self.m_out_list)
        reg("out.read", self.m_out_read)
        reg("eq.extract", self.m_eq_extract)
        reg("eq.check", self.m_eq_check)
        reg("contract.check", self.m_contract_check)
        reg("contract.similar", self.m_contract_similar)
        reg("lint.run", self.m_lint_run)

    def _tool_spec(self, name: str) -> SandboxSpec:
        """Sandbox spec for kernel-internal subprocesses (REPL, eq.check)."""
        run_dir = self.runs_dir / f"_{name}"
        run_dir.mkdir(parents=True, exist_ok=True)
        out_dir = self.root / self.out_dir_name / f"_{name}"
        out_dir.mkdir(parents=True, exist_ok=True)
        derived = self.root / "data" / "derived"
        return SandboxSpec(
            root=self.root,
            venv=venv_of(Path(self.python)),
            python=Path(self.python),
            run_dir=run_dir,
            out_dir=out_dir,
            data_dirs=[resolve_data_dir(self.root, d) for d in self.config.data_dirs],
            tooling_dir=tooling_dir(),
            derived_dir=derived if derived.is_dir() else None,
            network=False,
        )

    def _tool_env(self, name: str) -> dict[str, str]:
        run_dir = self.runs_dir / f"_{name}"
        out_dir = self.root / self.out_dir_name / f"_{name}"
        env = self.runs._build_env(f"_{name}", run_dir, out_dir, {})
        (run_dir / "mplconfig").mkdir(parents=True, exist_ok=True)
        return env

    # ---- lifecycle ----------------------------------------------------------------
    def m_hello(self, params: dict[str, Any]) -> dict[str, Any]:
        pv = params.get("protocol_version")
        if pv is not None and pv != PROTOCOL_VERSION:
            raise invalid_params(f"protocol_version {pv!r} is not supported (kernel speaks {PROTOCOL_VERSION})")
        return {
            "kernel_version": __version__,
            "protocol_version": PROTOCOL_VERSION,
            "host": self.host,
            "os": self.os_name,
            "sandbox": self.sandbox,
            "python": self.python,
            "root": str(self.root),
            "runs_dir": str(self.runs_dir),
        }

    def m_shutdown(self, params: dict[str, Any]) -> dict[str, Any]:
        self.close()
        return {"ok": True}

    def close(self) -> None:
        with self._snippets_lock:
            if self._snippets is not None:
                self._snippets.shutdown()
                self._snippets = None
        self.runs.shutdown()

    # ---- environment -------------------------------------------------------------------
    def m_env_info(self, params: dict[str, Any]) -> dict[str, Any]:
        from .envinfo import env_info

        return env_info(self.python, self.sandbox, self.host, self.os_name)

    def m_env_packages(self, params: dict[str, Any]) -> dict[str, Any]:
        from .envinfo import env_packages

        return {"packages": env_packages(self.python, self.root)}

    # ---- indexes --------------------------------------------------------------------
    def m_scripts_index(self, params: dict[str, Any]) -> dict[str, Any]:
        from .index_scripts import index_scripts, public_script

        globs = _str_list(params, "globs") or self.config.scripts
        return {"scripts": [public_script(s) for s in index_scripts(self.root, globs)]}

    def m_lib_index(self, params: dict[str, Any]) -> dict[str, Any]:
        from .index_lib import index_lib

        package = _opt(params, "package", (str,), self.config.library)
        resolve_in_root(self.root, package.replace(".", "/"))
        return {"functions": index_lib(self.root, package, self.config.scripts)}

    # ---- runs -------------------------------------------------------------------------
    def m_run_start(self, params: dict[str, Any]) -> dict[str, Any]:
        script = _opt(params, "script", (str,))
        if not script:
            raise invalid_params("script is required")
        args = _str_list(params, "args") or []
        env = params.get("env") or {}
        if not isinstance(env, dict) or not all(isinstance(k, str) and isinstance(v, str) for k, v in env.items()):
            raise invalid_params("env must be an object of strings")
        timeout_s = _opt(params, "timeout_s", (int, float))
        network = bool(_opt(params, "network", (bool,), False))
        tree_hash = _opt(params, "tree_hash", (str,))
        return self.runs.start(script, args, env, timeout_s, network, tree_hash)

    def _run_id(self, params: dict[str, Any]) -> str:
        rid = params.get("run_id")
        if not isinstance(rid, str) or not rid:
            raise invalid_params("run_id must be a non-empty string")
        return rid

    def m_run_attach(self, params: dict[str, Any]):
        return self.runs.attach(
            self._run_id(params),
            int(_opt(params, "stdout_offset", (int,), 0)),
            int(_opt(params, "stderr_offset", (int,), 0)),
        )

    def m_run_status(self, params: dict[str, Any]) -> dict[str, Any]:
        return self.runs.status(self._run_id(params))

    def m_run_cancel(self, params: dict[str, Any]) -> dict[str, Any]:
        return {"ok": self.runs.cancel(self._run_id(params))}

    def m_runs_list(self, params: dict[str, Any]) -> dict[str, Any]:
        return {"runs": self.runs.list(_opt(params, "limit", (int,)))}

    # ---- snippets ---------------------------------------------------------------------
    def m_exec_snippet(self, params: dict[str, Any]) -> dict[str, Any]:
        from .snippet import SnippetRunner

        code = _opt(params, "code", (str,))
        if code is None:
            raise invalid_params("code is required")
        timeout_s = _opt(params, "timeout_s", (int, float))
        with self._snippets_lock:
            if self._snippets is None:
                self._snippets = SnippetRunner(
                    self.python,
                    self.root,
                    self.sandbox,
                    lambda: self._tool_spec("repl"),
                    self._tool_env("repl"),
                )
            runner = self._snippets
        return runner.run(code, timeout_s)

    # ---- outputs ------------------------------------------------------------------------
    def m_out_list(self, params: dict[str, Any]) -> dict[str, Any]:
        from .outputs import out_list

        path = _opt(params, "path", (str,))
        run_id = _opt(params, "run_id", (str,))
        run_out = self.runs.status(run_id)["out_dir"] if run_id else None
        return {"entries": out_list(self.root, self.out_dir_name, path, run_out)}

    def m_out_read(self, params: dict[str, Any]) -> dict[str, Any]:
        from .outputs import out_read

        return out_read(self.root, self.python, _opt(params, "path", (str,), ""), _opt(params, "max_bytes", (int,)))

    # ---- equations ---------------------------------------------------------------------
    def m_eq_extract(self, params: dict[str, Any]) -> dict[str, Any]:
        from .equations import eq_extract

        return {"equations": eq_extract(self.root, _opt(params, "path", (str,), ""), _opt(params, "function", (str,)))}

    def m_eq_check(self, params: dict[str, Any]) -> dict[str, Any]:
        from .equations import eq_check

        ranges = params.get("ranges")
        if ranges is not None and not isinstance(ranges, dict):
            raise invalid_params("ranges must be an object")
        return eq_check(
            self.root,
            self.python,
            self.sandbox,
            lambda: self._tool_spec("eqcheck"),
            self._tool_env("eqcheck"),
            _opt(params, "path", (str,), ""),
            _opt(params, "function", (str,), ""),
            _opt(params, "trials", (int,)),
            _opt(params, "tol", (int, float)),
            ranges,
        )

    # ---- contract and lint -----------------------------------------------------------------
    def m_contract_check(self, params: dict[str, Any]) -> dict[str, Any]:
        from .contract import contract_check

        return {"diagnostics": contract_check(self.root, self.config, _str_list(params, "paths"))}

    def m_contract_similar(self, params: dict[str, Any]) -> dict[str, Any]:
        from .similarity import similar_pairs

        return {"pairs": similar_pairs(self.root, self.config, _opt(params, "threshold", (int, float)))}

    def m_lint_run(self, params: dict[str, Any]) -> dict[str, Any]:
        from .astutil import library_files, script_files
        from .lint import lint_run

        defaults = script_files(self.root, self.config.scripts) + library_files(self.root, self.config.library)
        return {"diagnostics": lint_run(self.root, _str_list(params, "paths"), defaults)}


__all__ = ["Kernel", "RpcError", "sys"]
