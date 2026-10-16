from __future__ import annotations

import sys
from pathlib import Path

import pytest

from analyst_kernel import envinfo
from analyst_kernel.errors import TOOL_MISSING, RpcError


def test_env_info(kernel):
    info = kernel.call("env.info")
    assert Path(info["python"]).exists()
    assert info["version"].split(".")[0] == str(sys.version_info[0])
    assert info["venv"] == sys.prefix
    assert any(p.endswith("site-packages") for p in info["site_packages"])
    assert info["sandbox"] == "limits" and info["host"] == kernel.host and info["os"] in ("linux", "macos", "other")
    assert info["uv"] is None or Path(info["uv"]).exists()


def test_env_packages_and_lock(kernel, repo: Path):
    pk = kernel.call("env.packages")["packages"]
    names = {p["name"].lower(): p for p in pk}
    assert "numpy" in names and names["numpy"]["version"]
    assert all(p["locked"] is False for p in pk)
    (repo / "uv.lock").write_text('version = 1\n\n[[package]]\nname = "numpy"\nversion = "2.0.0"\n')
    pk = {p["name"].lower(): p for p in kernel.call("env.packages")["packages"]}
    assert pk["numpy"]["locked"] is True and pk["sympy"]["locked"] is False


def test_env_packages_without_uv_uses_pip(kernel, monkeypatch, repo):
    monkeypatch.setattr(envinfo.shutil, "which", lambda name: None)
    try:
        pk = kernel.call("env.packages")["packages"]
    except RpcError as exc:  # pip itself may be absent from a uv venv: then 1005 is the contract
        assert exc.code == TOOL_MISSING
    else:
        assert isinstance(pk, list)


def test_env_info_bad_python(kernel):
    kernel.python = "/nonexistent/python"
    with pytest.raises(RpcError) as exc:
        kernel.call("env.info")
    assert exc.value.code == TOOL_MISSING
