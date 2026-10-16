"""JSON-RPC framing, error codes, and concurrency through the Server over in-memory pipes."""

from __future__ import annotations

import io
import json
import os
import threading
from pathlib import Path

import pytest

from analyst_kernel.errors import (
    INVALID_PARAMS,
    INVALID_REQUEST,
    METHOD_NOT_FOUND,
    PARSE_ERROR,
    PATH_OUTSIDE_ROOT,
    RUN_NOT_FOUND,
    SCRIPT_NOT_FOUND,
    RpcError,
)
from analyst_kernel.rpc import Deferred, Dispatcher, Server


def run_server(dispatcher: Dispatcher, lines: list[str]) -> list[dict]:
    stdin = io.BytesIO(("\n".join(lines) + "\n").encode())
    stdout = io.BytesIO()
    server = Server(dispatcher, stdin=stdin, stdout=stdout)
    server.serve()
    out = [json.loads(ln) for ln in stdout.getvalue().decode().splitlines() if ln.strip()]
    assert all("\n" not in ln for ln in stdout.getvalue().decode().splitlines())
    return out


def req(id_, method, params=None):
    msg = {"jsonrpc": "2.0", "id": id_, "method": method}
    if params is not None:
        msg["params"] = params
    return json.dumps(msg)


def test_standard_errors_and_framing():
    d = Dispatcher()
    d.register("echo", lambda p: p)
    d.register("kernel.shutdown", lambda p: {"ok": True})
    out = run_server(
        d,
        [
            "{not json",
            json.dumps({"jsonrpc": "1.0", "id": 1, "method": "echo"}),
            json.dumps({"jsonrpc": "2.0", "id": 2, "method": 5}),
            req(3, "nope"),
            req(4, "echo", [1, 2]),
            req("five", "echo", {"a": "é"}),
            json.dumps({"jsonrpc": "2.0", "method": "echo", "params": {}}),  # notification: ignored
            req(6, "kernel.shutdown"),
            req(7, "echo"),  # after shutdown: never answered
        ],
    )
    by_id = {o.get("id"): o for o in out}
    assert by_id[None]["error"]["code"] == PARSE_ERROR
    assert by_id[1]["error"]["code"] == INVALID_REQUEST
    assert by_id[2]["error"]["code"] == INVALID_REQUEST
    assert by_id[3]["error"]["code"] == METHOD_NOT_FOUND
    assert by_id[4]["error"]["code"] == INVALID_PARAMS
    assert by_id["five"]["result"] == {"a": "é"}
    assert by_id[6]["result"] == {"ok": True}
    assert 7 not in by_id
    for o in out:
        if "error" in o:
            assert isinstance(o["error"]["data"]["detail"], str)


def test_internal_error_and_app_error():
    d = Dispatcher()

    def boom(p):
        raise ValueError("bad")

    def app(p):
        raise RpcError(RUN_NOT_FOUND, "no such run", extra=1)

    d.register("boom", boom)
    d.register("app", app)
    d.register("kernel.shutdown", lambda p: {"ok": True})
    out = run_server(d, [req(1, "boom"), req(2, "app"), req(3, "kernel.shutdown")])
    by_id = {o["id"]: o for o in out}
    assert by_id[1]["error"]["code"] == -32603
    assert by_id[2]["error"] == {"code": 1000, "message": "run not found", "data": {"detail": "no such run", "extra": 1}}


def test_concurrent_requests_and_notifications_from_background_thread():
    d = Dispatcher()
    gate = threading.Event()
    server_box: list[Server] = []

    def slow(p):
        gate.wait(5)
        return "slow"

    def fast(p):
        server_box[0].notify("tick", {"n": 1})
        gate.set()
        return "fast"

    d.register("slow", slow)
    d.register("fast", fast)
    d.register("kernel.shutdown", lambda p: {"ok": True})
    stdin = io.BytesIO((req(1, "slow") + "\n" + req(2, "fast") + "\n" + req(3, "kernel.shutdown") + "\n").encode())
    stdout = io.BytesIO()
    server = Server(d, stdin=stdin, stdout=stdout)
    server_box.append(server)
    server.serve()
    msgs = [json.loads(ln) for ln in stdout.getvalue().decode().splitlines()]
    ids = [m.get("id") for m in msgs if "id" in m]
    assert ids.index(2) < ids.index(1)  # answered out of order
    assert {"jsonrpc": "2.0", "method": "tick", "params": {"n": 1}} in msgs


def test_deferred_runs_after_response():
    d = Dispatcher()
    server_box: list[Server] = []

    def attach(p):
        return Deferred({"status": "done"}, lambda: server_box[0].notify("run.done", {"run_id": "x"}))

    d.register("run.attach", attach)
    d.register("kernel.shutdown", lambda p: {"ok": True})
    stdin = io.BytesIO((req(1, "run.attach") + "\n" + req(2, "kernel.shutdown") + "\n").encode())
    stdout = io.BytesIO()
    server = Server(d, stdin=stdin, stdout=stdout)
    server_box.append(server)
    server.serve()
    msgs = [json.loads(ln) for ln in stdout.getvalue().decode().splitlines()]
    assert msgs[0] == {"jsonrpc": "2.0", "id": 1, "result": {"status": "done"}}
    assert msgs[1]["method"] == "run.done"
    # in-process harness runs the deferred callback too
    assert d.call("run.attach", {}) == {"status": "done"}


@pytest.mark.parametrize(
    "method,params",
    [
        ("eq.extract", {"path": "../outside.py"}),
        ("out.read", {"path": "/etc/passwd"}),
        ("out.list", {"path": "../"}),
        ("contract.check", {"paths": ["../../x.py"]}),
        ("lint.run", {"paths": ["../x.py"]}),
        ("run.start", {"script": "../escape.py"}),
        ("lib.index", {"package": "../analysis"}),
    ],
)
def test_path_outside_root_is_1002(kernel, repo: Path, method, params):
    (repo.parent / "outside.py").write_text("x = 1\n")
    with pytest.raises(RpcError) as exc:
        kernel.call(method, params)
    assert exc.value.code == PATH_OUTSIDE_ROOT


def test_symlink_escape_is_1002(kernel, repo: Path):
    outside = repo.parent / "secret.txt"
    outside.write_text("s")
    os.symlink(outside, repo / "link.txt")
    with pytest.raises(RpcError) as exc:
        kernel.call("out.read", {"path": "link.txt"})
    assert exc.value.code == PATH_OUTSIDE_ROOT


def test_run_and_script_not_found(kernel):
    with pytest.raises(RpcError) as exc:
        kernel.call("run.status", {"run_id": "20260101-000000-abcdef"})
    assert exc.value.code == RUN_NOT_FOUND
    with pytest.raises(RpcError) as exc:
        kernel.call("run.start", {"script": "scripts/missing.py"})
    assert exc.value.code == SCRIPT_NOT_FOUND
    with pytest.raises(RpcError) as exc:
        kernel.call("run.start", {"script": "data/sample.csv"})
    assert exc.value.code == SCRIPT_NOT_FOUND
    with pytest.raises(RpcError) as exc:
        kernel.call("run.start", {"script": 5})
    assert exc.value.code == INVALID_PARAMS


def test_hello_and_shutdown(kernel, repo):
    hello = kernel.call("kernel.hello", {"core_version": "0.0.1", "protocol_version": 1})
    assert hello["protocol_version"] == 1
    assert hello["sandbox"] == "limits"
    assert hello["os"] in ("linux", "macos", "other")
    assert hello["root"] == str(repo.resolve())
    assert Path(hello["runs_dir"]).is_dir()
    with pytest.raises(RpcError):
        kernel.call("kernel.hello", {"core_version": "0", "protocol_version": 2})
    assert kernel.call("kernel.shutdown", {}) == {"ok": True}
    assert set(kernel.dispatcher.methods()) >= {
        "kernel.hello", "kernel.shutdown", "env.info", "env.packages", "scripts.index", "lib.index",
        "run.start", "run.attach", "run.status", "run.cancel", "runs.list", "exec.snippet",
        "out.list", "out.read", "eq.extract", "eq.check", "contract.check", "contract.similar", "lint.run",
    }
