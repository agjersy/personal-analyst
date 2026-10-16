from __future__ import annotations

import sys
from pathlib import Path

import pytest

from analyst_kernel import sandbox
from analyst_kernel.errors import SANDBOX_UNAVAILABLE, RpcError
from analyst_kernel.sandbox import (
    SandboxSpec,
    bwrap_argv,
    detect,
    sandbox_exec_argv,
    sandbox_exec_profile,
    wrap,
)


def spec(tmp_path: Path, network=False) -> SandboxSpec:
    root = tmp_path / "repo"
    (root / "data" / "derived").mkdir(parents=True)
    venv = root / ".venv"
    venv.mkdir()
    data = tmp_path / "shared"
    data.mkdir()
    tooling = tmp_path / "tooling"
    tooling.mkdir()
    run_dir = tmp_path / "runs" / "r1"
    run_dir.mkdir(parents=True)
    out = root / "out" / "s" / "r1"
    out.mkdir(parents=True)
    return SandboxSpec(
        root=root, venv=venv, run_dir=run_dir, out_dir=out, data_dirs=[data, tmp_path / "missing"],
        tooling_dir=tooling, derived_dir=root / "data" / "derived", home=tmp_path / "home", network=network,
    )


def test_bwrap_argv(tmp_path: Path):
    s = spec(tmp_path)
    cmd = ["/py", "script.py", "--x", "1"]
    argv = bwrap_argv(s, cmd)
    home = str(tmp_path / "home")
    assert argv[:5] == ["bwrap", "--ro-bind", "/", "/", "--tmpfs"] and argv[5] == home
    text = " ".join(argv)
    for d in (s.root, s.venv, s.tooling_dir, tmp_path / "shared"):
        assert f"--ro-bind {d} {d}" in text
    assert f"--ro-bind {tmp_path / 'missing'}" not in text
    for d in (s.out_dir, s.derived_dir, s.run_dir):
        assert f"--bind {d} {d}" in text
    # tmpfs HOME comes before the ro-binds of things that may live under HOME
    assert argv.index("--tmpfs") < argv.index(str(s.root))
    assert "--tmpfs /tmp" in text
    assert "--unshare-net" in argv
    for flag in ("--die-with-parent", "--new-session", "--unshare-pid"):
        assert flag in argv
    assert "--proc /proc --dev /dev" in text
    assert argv[argv.index("--") + 1 :] == cmd
    assert "--unshare-net" not in bwrap_argv(spec(tmp_path / "b", network=True), cmd)


def test_sandbox_exec_profile_and_argv(tmp_path: Path):
    s = spec(tmp_path)
    prof = sandbox_exec_profile(s)
    lines = prof.splitlines()
    assert lines[0] == "(version 1)"
    assert lines[1] == "(deny default)"
    assert lines[2] == '(allow file-read* (subpath "/"))'
    assert lines[3] == f'(deny file-read* (subpath "{tmp_path / "home"}"))'
    for d in (s.root, s.venv, s.tooling_dir, tmp_path / "shared"):
        assert f'(allow file-read* (subpath "{d}"))' in lines
    for d in (s.out_dir, s.derived_dir, s.run_dir):
        assert f'(allow file-read* file-write* (subpath "{d}"))' in lines
    assert '(allow file-write* (subpath "/private/tmp"))' in lines
    assert '(allow file-write* (subpath "/private/var/folders"))' in lines
    for rule in ("(allow process*)", "(allow sysctl-read)", "(allow mach-lookup)", "(deny network*)"):
        assert rule in lines
    assert "(allow network*)" in sandbox_exec_profile(spec(tmp_path / "n", network=True))
    argv = sandbox_exec_argv(s, ["/py", "s.py"])
    assert argv[:2] == ["sandbox-exec", "-p"] and argv[2] == prof and argv[3:] == ["/py", "s.py"]


def test_wrap_kinds(tmp_path: Path):
    s = spec(tmp_path)
    cmd = ["/py", "s.py"]
    lim = wrap("limits", s, cmd)
    assert lim.argv == cmd and lim.rlimit_as == 8 * 1024**3 and lim.rlimit_nofile == 4096
    assert callable(lim.preexec())
    none = wrap("none", s, cmd)
    assert none.argv == cmd and none.preexec() is None
    bw = wrap("bwrap", s, cmd)
    assert bw.kind == "bwrap" and bw.argv[0] == "bwrap" and bw.rlimit_as
    se = wrap("sandbox-exec", s, cmd)
    assert se.profile and se.argv[0] == "sandbox-exec"
    with pytest.raises(RpcError):
        wrap("docker", s, cmd)


def test_limits_preexec_applies_rlimits():
    import json
    import resource
    import subprocess

    hard = resource.getrlimit(resource.RLIMIT_NOFILE)[1]

    code = "import resource,json;print(json.dumps([resource.getrlimit(resource.RLIMIT_AS)[0], resource.getrlimit(resource.RLIMIT_NOFILE)[0]]))"
    out = subprocess.run(
        [sys.executable, "-c", code], capture_output=True, text=True,
        preexec_fn=sandbox.limits_preexec(1 * 1024**3, min(256, hard)),
    ).stdout
    as_limit, nofile = json.loads(out)
    assert as_limit == 1 * 1024**3
    assert nofile == min(256, hard)


def test_detect_fallback_and_explicit_unavailable(monkeypatch):
    monkeypatch.setattr(sandbox, "bwrap_available", lambda *a, **k: False)
    monkeypatch.setattr(sandbox, "sandbox_exec_available", lambda *a, **k: False)
    assert detect("auto") == "limits"
    assert detect("limits") == "limits" and detect("none") == "none"
    for kind in ("bwrap", "sandbox-exec"):
        with pytest.raises(RpcError) as exc:
            detect(kind)
        assert exc.value.code == SANDBOX_UNAVAILABLE
    with pytest.raises(RpcError):
        detect("firejail")


def test_detect_order(monkeypatch):
    monkeypatch.setattr(sandbox, "bwrap_available", lambda *a, **k: True)
    monkeypatch.setattr(sandbox, "sandbox_exec_available", lambda *a, **k: True)
    assert detect("auto") == "bwrap"
    monkeypatch.setattr(sandbox, "bwrap_available", lambda *a, **k: False)
    assert detect("auto") == "sandbox-exec"


def test_bwrap_available_needs_binary(monkeypatch):
    assert sandbox.bwrap_available(which=lambda name: None) is False
    assert sandbox.sandbox_exec_available(which=lambda name: "/usr/bin/sandbox-exec") is (sys.platform == "darwin")


def test_venv_of(tmp_path: Path):
    venv = tmp_path / "v"
    (venv / "bin").mkdir(parents=True)
    (venv / "pyvenv.cfg").write_text("home = /usr\n")
    py = venv / "bin" / "python"
    py.write_text("")
    assert sandbox.venv_of(py) == venv.resolve()
    assert sandbox.venv_of(tmp_path / "nope" / "python") is None
