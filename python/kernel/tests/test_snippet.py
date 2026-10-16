from __future__ import annotations

import pytest

from analyst_kernel.errors import TIMEOUT, RpcError


def test_snippet_persistent_state(kernel):
    r = kernel.call("exec.snippet", {"code": "x = 21\nx * 2"})
    assert r == {"ok": True, "stdout": "", "stderr": "", "value_repr": "42"}
    r = kernel.call("exec.snippet", {"code": "print('hi'); x + 1"})
    assert r["ok"] and r["stdout"] == "hi\n" and r["value_repr"] == "22"
    r = kernel.call("exec.snippet", {"code": "import sys; sys.stderr.write('warn'); None"})
    assert r["ok"] and r["stderr"] == "warn" and r["value_repr"] is None
    r = kernel.call("exec.snippet", {"code": "y = 1"})
    assert r["ok"] and r["value_repr"] is None


def test_snippet_error(kernel):
    r = kernel.call("exec.snippet", {"code": "1/0"})
    assert r["ok"] is False and "ZeroDivisionError" in r["stderr"] and r["value_repr"] is None
    r = kernel.call("exec.snippet", {"code": "def (:"})
    assert r["ok"] is False and "SyntaxError" in r["stderr"]


def test_snippet_runs_in_session_python_with_env(kernel, repo):
    r = kernel.call("exec.snippet", {"code": "import os, sys; (os.getcwd(), os.environ['MPLBACKEND'], sys.executable)"})
    assert r["ok"]
    assert str(repo.resolve()) in r["value_repr"] and "module://analyst_mpl" in r["value_repr"]
    assert kernel.python in r["value_repr"]
    assert kernel.call("exec.snippet", {"code": "from analysis.models import double_exp; double_exp(0, 1, 1, 2, 1)"})["value_repr"].endswith("3.0)")


def test_snippet_timeout_restarts_repl(kernel):
    kernel.call("exec.snippet", {"code": "z = 5"})
    with pytest.raises(RpcError) as exc:
        kernel.call("exec.snippet", {"code": "import time; time.sleep(30)", "timeout_s": 0.5})
    assert exc.value.code == TIMEOUT
    r = kernel.call("exec.snippet", {"code": "'z' in dir()"})
    assert r["ok"] and r["value_repr"] == "False"  # fresh REPL after the kill


def test_snippet_has_resource_limits(kernel):
    r = kernel.call("exec.snippet", {"code": "import resource; resource.getrlimit(resource.RLIMIT_AS)[0]"})
    assert r["ok"] and int(r["value_repr"]) == 8 * 1024**3


def test_snippet_params(kernel):
    with pytest.raises(RpcError):
        kernel.call("exec.snippet", {})
