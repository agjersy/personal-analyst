from __future__ import annotations

import ast
from pathlib import Path

import pytest

from analyst_kernel.equations import Unsupported, derive_function, simplify_with_budget


def by_name(kernel, path="analysis/models.py"):
    return {e["function"]: e for e in kernel.call("eq.extract", {"path": path})["equations"]}


def test_tiers(kernel):
    eqs = by_name(kernel)
    de = eqs["double_exp"]
    assert de["tier"] == "derived" and de["status"] == "ok" and de["reason"] is None
    assert de["source_expr"] == "A*exp(-t/tau1) + B*exp(-t/tau2)"
    assert "e^{" in de["latex"] and r"\tau_{1}" in de["latex"]
    dc = eqs["damped_cos"]
    assert dc["tier"] == "declared" and dc["status"] == "ok"
    assert dc["source_expr"] == "A*exp(-t/tau)*cos(omega*t)"
    assert dc["latex"].startswith("y = ")
    g = eqs["gaussian"]
    assert g["tier"] == "derived" and "sqrt(2)" in g["source_expr"] and "pi" in g["source_expr"]
    rs = eqs["running_sum"]
    assert rs["tier"] == "latex" and rs["latex"] == "S_n = \\sum_{i=0}^{n-1} x_i" and rs["source_expr"] is None
    pl = eqs["power_law"]
    assert pl["tier"] == "declared" and pl["status"] == "error"
    assert "Math: section" in pl["reason"]
    assert all(e["line"] > 0 and e["qualname"] == e["function"] for e in eqs.values())


def test_tier_none_reason(kernel, repo: Path):
    (repo / "analysis" / "misc.py").write_text(
        "import numpy as np\n\ndef clipped(x, lo):\n    y = x[0] + lo\n    return y\n\n"
        "def branchy(x):\n    if x > 0:\n        return x\n    return -x\n\n"
        "def unknown_call(x):\n    return np.floor(x)\n\n"
        "def two_args(x, y):\n    return np.arctan2(y, x)\n"
    )
    eqs = by_name(kernel, "analysis/misc.py")
    assert eqs["clipped"] == {
        "function": "clipped", "qualname": "clipped", "line": 3, "tier": "none", "latex": None,
        "source_expr": None, "status": "unsupported", "reason": "Subscript `x[0]` at line 4",
    }
    assert eqs["branchy"]["tier"] == "none" and "If" in eqs["branchy"]["reason"] and "line 8" in eqs["branchy"]["reason"]
    assert eqs["unknown_call"]["reason"] == "call to `floor` at line 13"
    assert "arctan2" in eqs["two_args"]["reason"]


def test_function_filter_and_methods(kernel, repo: Path):
    (repo / "analysis" / "cls.py").write_text(
        "import math\nclass M:\n    def area(self, r):\n        return math.pi * r ** 2\n"
    )
    eqs = kernel.call("eq.extract", {"path": "analysis/cls.py", "function": "M.area"})["equations"]
    assert len(eqs) == 1 and eqs[0]["qualname"] == "M.area" and eqs[0]["source_expr"] == "pi*r**2"
    assert kernel.call("eq.extract", {"path": "analysis/models.py", "function": "double_exp"})["equations"][0]["tier"] == "derived"
    assert kernel.call("eq.extract", {"path": "analysis/models.py", "function": "nothing"})["equations"] == []


def test_derivation_supported_subset():
    src = (
        "def f(x, y):\n"
        "    a = np.log10(x) + math.log2(y) + np.e\n"
        "    b: float = -abs(a) ** 2 / 3\n"
        "    return np.sqrt(b + arctan(x)) * np.sinh(y) + np.pi\n"
    )
    node = ast.parse(src).body[0]
    expr = derive_function(node, ["x", "y"])
    text = str(expr)
    assert "sqrt" in text and "asin" not in text and "pi" in text and "atan(x)" in text
    with pytest.raises(Unsupported):
        derive_function(ast.parse("def g(x):\n    return x @ x\n").body[0], ["x"])
    with pytest.raises(Unsupported):
        derive_function(ast.parse("def g(x):\n    return 'a'\n").body[0], ["x"])
    with pytest.raises(Unsupported):
        derive_function(ast.parse("def g(x):\n    y = x\n").body[0], ["x"])


def test_simplify_budget_falls_back(monkeypatch):
    import sympy

    x = sympy.Symbol("x")
    expr = (x + 1) ** 2 - x**2 - 2 * x

    def slow(e):
        import time

        time.sleep(1.0)
        return sympy.Integer(1)

    monkeypatch.setattr(sympy, "simplify", slow)
    assert simplify_with_budget(expr, seconds=0.05) == expr
    assert simplify_with_budget(expr, seconds=5.0) == 1


def test_declared_section_rules(kernel, repo: Path):
    (repo / "analysis" / "decl.py").write_text(
        'def chain(x, k):\n    """Doc.\n\n    Math: intermediate lines\n        u = k*x\n        y = exp(-u) + x^2\n    """\n'
        "    return [x]\n\n"
        'def stray(x):\n    """Doc.\n\n    Math:\n        y = x + unknown_q\n    """\n    return [x]\n\n'
        'def evil(x):\n    """Doc.\n\n    Math:\n        y = __import__("os").getcwd()\n    """\n    return [x]\n'
    )
    eqs = by_name(kernel, "analysis/decl.py")
    assert eqs["chain"]["status"] == "ok" and eqs["chain"]["source_expr"] == "x**2 + exp(-k*x)"
    assert eqs["stray"]["status"] == "error" and "unknown_q" in eqs["stray"]["reason"]
    assert eqs["evil"]["status"] == "error"


def test_eq_check_agree(kernel):
    res = kernel.call("eq.check", {"path": "analysis/models.py", "function": "double_exp"})
    assert res["status"] == "agree" and res["trials"] == 32 and res["counterexample"] is None
    assert res["max_abs_err"] is not None and res["max_abs_err"] < 1e-9 and res["reason"] is None
    res = kernel.call("eq.check", {"path": "analysis/models.py", "function": "gaussian", "trials": 5, "tol": 1e-3})
    assert res["status"] == "agree" and res["trials"] == 5


def test_eq_check_mismatch_with_counterexample(kernel):
    res = kernel.call("eq.check", {"path": "analysis/models.py", "function": "damped_cos", "ranges": {"omega": [1, 2]}})
    assert res["status"] == "mismatch"
    ce = res["counterexample"]
    assert set(ce["inputs"]) == {"t", "A", "tau", "omega"} and 1 <= ce["inputs"]["omega"] <= 2
    assert all(0.1 <= v <= 10 for k, v in ce["inputs"].items() if k != "omega")
    assert abs(ce["expected"] - ce["actual"]) > 1e-6
    assert res["max_rel_err"] > 1e-6
    # fixed seed: deterministic
    again = kernel.call("eq.check", {"path": "analysis/models.py", "function": "damped_cos", "ranges": {"omega": [1, 2]}})
    assert again["counterexample"] == ce


def test_eq_check_unavailable(kernel, repo: Path):
    res = kernel.call("eq.check", {"path": "analysis/models.py", "function": "running_sum"})
    assert res["status"] == "unavailable" and res["trials"] == 0 and res["reason"]
    res = kernel.call("eq.check", {"path": "analysis/models.py", "function": "power_law"})
    assert res["status"] == "unavailable" and "Math: section" in res["reason"]
    res = kernel.call("eq.check", {"path": "analysis/models.py", "function": "missing"})
    assert res["status"] == "unavailable"
    (repo / "analysis" / "broken.py").write_text(
        'import nonexistent_module\n\ndef f(x):\n    """Doc.\n\n    Math:\n        y = x\n    """\n    return x\n'
    )
    res = kernel.call("eq.check", {"path": "analysis/broken.py", "function": "f"})
    assert res["status"] == "unavailable" and "nonexistent_module" in res["reason"]


def test_sympy_not_imported_at_startup():
    import subprocess
    import sys

    code = "import sys, analyst_kernel.kernel, analyst_kernel.__main__; print('sympy' in sys.modules)"
    out = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, check=True).stdout
    assert out.strip() == "False"
