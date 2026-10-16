"""Newline-delimited JSON-RPC 2.0 over stdio (PROTOCOL.md 1.1 and 1.2).

The reader loop runs on the calling thread; each request is dispatched on a worker
thread so long-running calls do not block others. Notifications may be emitted from
any thread (run watchers) and are serialized through one writer lock.
"""

from __future__ import annotations

import json
import logging
import sys
import threading
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from typing import Any, BinaryIO

from .errors import (
    INTERNAL_ERROR,
    INVALID_PARAMS,
    INVALID_REQUEST,
    METHOD_NOT_FOUND,
    PARSE_ERROR,
    RpcError,
)

log = logging.getLogger("analyst_kernel.rpc")

Handler = Callable[[dict[str, Any]], Any]


@dataclass
class Deferred:
    """A handler result plus a callback to run after the response has been written.

    ``run.attach`` uses this: the response must precede the replayed notifications.
    """

    result: Any
    after: Callable[[], None] | None = None


class Dispatcher:
    """Method table shared by the stdio server and the in-process test harness."""

    def __init__(self) -> None:
        self._methods: dict[str, Handler] = {}

    def register(self, name: str, handler: Handler) -> None:
        self._methods[name] = handler

    def methods(self) -> list[str]:
        return sorted(self._methods)

    def dispatch(self, method: str, params: Any) -> Any:
        """Run a method; returns the raw handler result (possibly a Deferred)."""
        handler = self._methods.get(method)
        if handler is None:
            raise RpcError(METHOD_NOT_FOUND, f"unknown method {method!r}")
        if params is None:
            params = {}
        if not isinstance(params, dict):
            raise RpcError(INVALID_PARAMS, "params must be an object")
        return handler(params)

    def call(self, method: str, params: Any = None) -> Any:
        """In-process call: runs any deferred callback synchronously after returning."""
        out = self.dispatch(method, params)
        if isinstance(out, Deferred):
            result = out.result
            if out.after is not None:
                out.after()
            return result
        return out


def _encode(obj: dict[str, Any]) -> bytes:
    # ensure_ascii keeps every line pure ASCII; separators avoid stray whitespace.
    return (json.dumps(obj, ensure_ascii=True, separators=(",", ":"), default=str) + "\n").encode(
        "utf-8"
    )


class Server:
    """Serve JSON-RPC over a byte stream pair until shutdown or EOF."""

    def __init__(
        self,
        dispatcher: Dispatcher,
        stdin: BinaryIO | None = None,
        stdout: BinaryIO | None = None,
        max_workers: int = 8,
    ) -> None:
        self.dispatcher = dispatcher
        self._in = stdin if stdin is not None else sys.stdin.buffer
        self._out = stdout if stdout is not None else sys.stdout.buffer
        self._write_lock = threading.Lock()
        self._pool = ThreadPoolExecutor(max_workers=max_workers, thread_name_prefix="rpc")
        self._stop = threading.Event()
        self.shutdown_method = "kernel.shutdown"

    # ---- output ---------------------------------------------------------
    def _write(self, obj: dict[str, Any]) -> None:
        data = _encode(obj)
        with self._write_lock:
            try:
                self._out.write(data)
                self._out.flush()
            except (BrokenPipeError, ValueError, OSError):
                self._stop.set()

    def notify(self, method: str, params: dict[str, Any]) -> None:
        self._write({"jsonrpc": "2.0", "method": method, "params": params})

    def _respond(self, req_id: Any, result: Any) -> None:
        self._write({"jsonrpc": "2.0", "id": req_id, "result": result})

    def _respond_error(self, req_id: Any, err: RpcError) -> None:
        self._write({"jsonrpc": "2.0", "id": req_id, "error": err.to_json()})

    # ---- request handling ----------------------------------------------
    def _handle_line(self, line: bytes) -> None:
        text = line.decode("utf-8", errors="replace").strip()
        if not text:
            return
        try:
            msg = json.loads(text)
        except json.JSONDecodeError as exc:
            self._respond_error(None, RpcError(PARSE_ERROR, f"invalid JSON: {exc.msg}"))
            return
        if not isinstance(msg, dict) or msg.get("jsonrpc") != "2.0" or "method" not in msg:
            self._respond_error(
                msg.get("id") if isinstance(msg, dict) else None,
                RpcError(INVALID_REQUEST, "expected a JSON-RPC 2.0 request object"),
            )
            return
        method = msg["method"]
        if not isinstance(method, str):
            self._respond_error(msg.get("id"), RpcError(INVALID_REQUEST, "method must be a string"))
            return
        if "id" not in msg:
            # Notifications are not accepted from the core; ignore silently.
            log.debug("ignoring notification %s", method)
            return
        req_id = msg["id"]
        if not isinstance(req_id, (str, int)) or isinstance(req_id, bool):
            self._respond_error(None, RpcError(INVALID_REQUEST, "id must be a string or integer"))
            return
        if method == self.shutdown_method:
            # Handle inline so the loop can stop deterministically after the response.
            self._run_request(req_id, method, msg.get("params"))
            self._stop.set()
            return
        self._pool.submit(self._run_request, req_id, method, msg.get("params"))

    def _run_request(self, req_id: Any, method: str, params: Any) -> None:
        after: Callable[[], None] | None = None
        try:
            out = self.dispatcher.dispatch(method, params)
            if isinstance(out, Deferred):
                after = out.after
                out = out.result
            self._respond(req_id, out)
        except RpcError as err:
            self._respond_error(req_id, err)
            return
        except Exception as exc:
            log.exception("internal error in %s", method)
            self._respond_error(req_id, RpcError(INTERNAL_ERROR, f"{type(exc).__name__}: {exc}"))
            return
        if after is not None:
            try:
                after()
            except Exception:
                log.exception("deferred callback for %s failed", method)

    def serve(self) -> None:
        """Read requests until shutdown or EOF; waits for in-flight requests."""
        try:
            while not self._stop.is_set():
                line = self._in.readline()
                if not line:
                    break
                self._handle_line(line)
        finally:
            self._pool.shutdown(wait=True)
