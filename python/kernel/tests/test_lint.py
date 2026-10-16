from __future__ import annotations

from pathlib import Path

import pytest

from analyst_kernel import lint
from analyst_kernel.errors import TOOL_MISSING, RpcError


def test_lint_run_maps_ruff(kernel, repo: Path):
    (repo / "scripts" / "lintme.py").write_text("import os\nimport sys\n\ndef main():\n    x = undefined_name\n    return x\n")
    ds = kernel.call("lint.run", {"paths": ["scripts/lintme.py"]})["diagnostics"]
    rules = {d["rule"] for d in ds}
    assert "ruff:F401" in rules and "ruff:F821" in rules
    for d in ds:
        assert d["path"] == "scripts/lintme.py" and d["line"] >= 1 and d["col"] >= 1
        assert d["severity"] in ("error", "warn") and d["related"] == []
        assert d["rule"].startswith("ruff:")
    f821 = next(d for d in ds if d["rule"] == "ruff:F821")
    assert f821["severity"] == "error" and "undefined_name" in f821["message"]
    f401 = next(d for d in ds if d["rule"] == "ruff:F401")
    assert f401["severity"] == "warn" and f401["hint"]


def test_lint_default_paths(kernel):
    ds = kernel.call("lint.run")["diagnostics"]
    assert all(d["path"].startswith(("scripts/", "analysis/")) for d in ds)


def test_lint_missing_ruff(kernel, monkeypatch):
    monkeypatch.setattr(lint, "find_ruff", lambda: None)
    with pytest.raises(RpcError) as exc:
        kernel.call("lint.run")
    assert exc.value.code == TOOL_MISSING
    assert exc.value.to_json()["data"]["tool"] == "ruff"
