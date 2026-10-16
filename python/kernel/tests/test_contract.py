from __future__ import annotations

import ast
from pathlib import Path

from analyst_kernel.contract import DEFAULT_SEVERITY, RULES
from analyst_kernel.similarity import fingerprint, similarity, statement_count, token_stream


def diags(kernel, **params):
    return kernel.call("contract.check", params)["diagnostics"]


def rules_for(ds, path):
    return {d["rule"] for d in ds if d["path"] == path}


def test_shape_and_sorting(kernel):
    ds = diags(kernel)
    for d in ds:
        assert set(d) == {"path", "line", "col", "rule", "severity", "message", "hint", "related"}
        assert d["severity"] in ("error", "warn") and d["rule"] in RULES
    assert ds == sorted(ds, key=lambda d: (d["path"], d["line"], d["col"], d["rule"]))


def test_fit_decay_is_clean(kernel):
    assert rules_for(diags(kernel), "scripts/fit_decay.py") == set()
    assert diags(kernel, paths=["scripts/fit_decay.py"]) == []


def test_every_rule_fires_on_bad_fixtures(kernel):
    ds = diags(kernel)
    bad = rules_for(ds, "scripts/bad_script.py")
    assert bad == {
        "cli.help-text", "cli.main-guard", "cli.option-for-every-input", "cli.no-absolute-paths",
        "lib.script-defines-logic", "lib.duplicate", "lib.near-duplicate", "lib.unused-twin", "out.layout", "seed.option",
    }
    assert rules_for(ds, "scripts/no_parser.py") >= {"cli.parser-required", "cli.main-function", "cli.main-guard"}
    assert rules_for(ds, "analysis/models.py") == {"doc.docstring", "math.required"}
    fired = {d["rule"] for d in ds}
    assert fired == set(RULES), set(RULES) - fired


def test_rule_details_on_bad_script(kernel):
    ds = [d for d in diags(kernel) if d["path"] == "scripts/bad_script.py"]
    by_rule: dict[str, list[dict]] = {}
    for d in ds:
        by_rule.setdefault(d["rule"], []).append(d)

    help_text = by_rule["cli.help-text"]
    assert len(help_text) == 1 and "--width" in help_text[0]["message"] and help_text[0]["line"] == 42

    inputs = by_rule["cli.option-for-every-input"]
    msgs = [d["message"] for d in inputs]
    assert any("'data/sample.csv'" in m and "load_runs" in m for m in msgs)
    assert any("3.5" in m for m in msgs)
    assert any("'results/smooth.npy'" in m for m in msgs)
    assert all("add_argument" in d["hint"] for d in inputs)

    absolute = by_rule["cli.no-absolute-paths"]
    assert len(absolute) == 1 and absolute[0]["message"].endswith("'/data/shared/runs.csv'")

    logic = {d["message"].split()[2] for d in by_rule["lib.script-defines-logic"]}
    assert logic == {"load_runs()", "load_runs_v2()", "smooth()"}
    assert all(d["hint"] == "move to analysis/bad_script.py and import it" for d in by_rule["lib.script-defines-logic"])
    smooth = next(d for d in by_rule["lib.script-defines-logic"] if "smooth" in d["message"])
    assert "numpy" in smooth["message"]

    dup = by_rule["lib.duplicate"]
    assert len(dup) == 1 and dup[0]["severity"] == "error"
    assert dup[0]["related"] == [{"path": "analysis/io.py", "line": 7}] and "analysis/io.py:7" in dup[0]["message"]

    near = by_rule["lib.near-duplicate"]
    assert len(near) >= 1 and near[0]["severity"] == "warn"
    assert any(d["related"] == [{"path": "analysis/io.py", "line": 7}] and "load_runs_v2" in d["message"] for d in near)

    twin = by_rule["lib.unused-twin"]
    assert len(twin) == 1 and twin[0]["related"] == [{"path": "analysis/io.py", "line": 7}]
    assert "import load_runs" in twin[0]["hint"]

    layout = by_rule["out.layout"]
    assert len(layout) == 1 and "'results/smooth.npy'" in layout[0]["message"] and layout[0]["severity"] == "warn"

    seed = by_rule["seed.option"]
    assert len(seed) == 1 and "np.random.normal" in seed[0]["message"]


def test_default_severities_and_overrides(kernel):
    ds = diags(kernel)
    sev = {d["rule"]: d["severity"] for d in ds}
    for rule, s in sev.items():
        if rule == "doc.docstring":
            assert s == "error"  # overridden in the fixture config
        else:
            assert s == DEFAULT_SEVERITY[rule]
    assert DEFAULT_SEVERITY["lib.near-duplicate"] == "warn" and DEFAULT_SEVERITY["cli.help-text"] == "error"


def test_library_rules(kernel, repo: Path):
    ds = diags(kernel, paths=["analysis/models.py"])
    assert [(d["rule"], d["line"]) for d in ds] == [("doc.docstring", 30), ("math.required", 30), ("math.required", 34)]
    assert all(d["path"] == "analysis/models.py" for d in ds)
    # io.py is not under math_required and is documented
    assert diags(kernel, paths=["analysis/io.py"]) == []
    # a directory path expands
    assert {d["path"] for d in diags(kernel, paths=["analysis"])} == {"analysis/models.py"}


def test_option_for_every_input_scope(kernel, repo: Path):
    (repo / "scripts" / "scope.py").write_text(
        "import argparse\nfrom analysis.io import count_rows\nimport numpy as np\n"
        "LIMIT = 7\n\n"
        "def main():\n"
        "    p = argparse.ArgumentParser(description='d')\n"
        "    p.add_argument('--n', type=int, default=17, help='help with a/slash')\n"
        "    args = p.parse_args()\n"
        "    print(len('a/b'), range(3))\n"  # builtins are fine
        "    count_rows(args.n, LIMIT, 1, 0, -1, 2, 0.5, True)\n"  # allowed literals
        "    count_rows(42)\n"
        "    count_rows(x=-3)\n"
        "    np.zeros(5)\n"
        "    return 0\n\n"
        "def helper():\n    count_rows('untouched/outside/main.csv')\n\n"
        "if __name__ == '__main__':\n    count_rows('data.csv')\n    main()\n"
    )
    ds = [d for d in diags(kernel, paths=["scripts/scope.py"]) if d["rule"] == "cli.option-for-every-input"]
    found = sorted((d["line"], d["message"].split(" passed")[0]) for d in ds)
    assert found == [
        (12, "numeric literal 42"),
        (13, "numeric literal -3"),
        (14, "numeric literal 5"),
        (21, "path literal 'data.csv'"),
    ]


def test_no_absolute_paths_variants(kernel, repo: Path):
    (repo / "scripts" / "abs.py").write_text(
        '"""Docstring with /usr/share is fine."""\nimport argparse\n'
        "def main():\n    p = argparse.ArgumentParser(description='x')\n"
        "    p.add_argument('--d', default='data', help='e.g. /data/shared')\n"
        "    a = '~/stuff'\n    b = 'C:\\\\data'\n    c = 'relative/path'\n    return a, b, c\n"
        "if __name__ == '__main__':\n    main()\n"
    )
    ds = [d for d in diags(kernel, paths=["scripts/abs.py"]) if d["rule"] == "cli.no-absolute-paths"]
    assert sorted(d["line"] for d in ds) == [6, 7]


def test_script_helper_allowance(kernel, repo: Path):
    (repo / "scripts" / "helpers.py").write_text(
        "import argparse\nimport numpy as np\nfrom scipy import optimize\n"
        "def _short(x):\n    return x + 1\n"
        "def _long(x):\n" + "    x = x + 1\n" * 16 + "    return x\n"
        "def _numeric(x):\n    return np.sum(x)\n"
        "def _scipy(x):\n    return optimize.minimize(x)\n"
        "def parse_args():\n    return argparse.ArgumentParser(description='x').parse_args()\n"
        "def build_parser():\n    return argparse.ArgumentParser(description='x')\n"
        "def main():\n    parse_args()\n"
        "if __name__ == '__main__':\n    main()\n"
    )
    ds = [d for d in diags(kernel, paths=["scripts/helpers.py"]) if d["rule"] == "lib.script-defines-logic"]
    names = sorted(d["message"].split()[2] for d in ds)
    assert names == ["_long()", "_numeric()", "_scipy()"]


def test_out_layout_and_seed_variants(kernel, repo: Path):
    (repo / "scripts" / "io.py").write_text(
        "import argparse, random\nimport numpy as np\nfrom numpy.random import default_rng\n"
        "def main():\n"
        "    p = argparse.ArgumentParser(description='x')\n"
        "    p.add_argument('--seed', type=int, default=0, help='s')\n"
        "    args = p.parse_args()\n"
        "    df = None\n"
        "    df.to_csv('out/io/x.csv')\n"
        "    df.to_parquet('elsewhere/x.parquet')\n"
        "    np.savez('tmp/x.npz')\n"
        "    open('out/x.txt', 'w')\n"
        "    open('scratch.txt', 'wb')\n"
        "    open('scratch.txt', 'r')\n"
        "    open('scratch.txt', mode='w')\n"
        "    print(random.random(), default_rng(args.seed))\n"
        "if __name__ == '__main__':\n    main()\n"
    )
    ds = diags(kernel, paths=["scripts/io.py"])
    layout = sorted(d["line"] for d in ds if d["rule"] == "out.layout")
    assert layout == [10, 11, 13, 15]
    assert not any(d["rule"] == "seed.option" for d in ds)
    (repo / "scripts" / "noseed.py").write_text(
        "import argparse\nfrom numpy.random import default_rng\n"
        "def main():\n    argparse.ArgumentParser(description='x').parse_args()\n    default_rng(0)\n"
        "if __name__ == '__main__':\n    main()\n"
    )
    ds = diags(kernel, paths=["scripts/noseed.py"])
    assert [d["rule"] for d in ds if d["rule"] == "seed.option"] == ["seed.option"]


def test_contract_similar(kernel):
    pairs = kernel.call("contract.similar")["pairs"]
    kinds = {(p["kind"], p["a"]["qualname"], p["b"]["qualname"]) for p in pairs}
    assert ("duplicate", "load_runs", "load_runs") in kinds
    assert ("near-duplicate", "load_runs_v2", "load_runs") in kinds or ("near-duplicate", "load_runs", "load_runs_v2") in kinds
    assert ("unused-twin", "load_runs", "load_runs") in kinds
    dup = next(p for p in pairs if p["kind"] == "duplicate")
    assert dup["score"] == 1.0 and dup["a"] == {"path": "scripts/bad_script.py", "qualname": "load_runs", "line": 12}
    assert dup["b"] == {"path": "analysis/io.py", "qualname": "load_runs", "line": 7}
    near = [p for p in pairs if p["kind"] == "near-duplicate"]
    assert all(0.8 <= p["score"] < 1.0 for p in near)
    # a stricter threshold hides the near-duplicate; duplicates stay
    strict = kernel.call("contract.similar", {"threshold": 0.999})["pairs"]
    assert not [p for p in strict if p["kind"] == "near-duplicate"]
    assert [p for p in strict if p["kind"] == "duplicate"]


def test_fingerprint_normalization():
    a = ast.parse("def f(a, b):\n    'doc'\n    x = a + 1\n    y = x * b\n    return y\n").body[0]
    b = ast.parse("def g(p, q):\n    u = p + 7\n    v = u * q\n    return v\n").body[0]
    c = ast.parse("def h(p, q):\n    u = q + 7\n    v = u * p\n    return v\n").body[0]
    d = ast.parse("def k(p, q):\n    u = p + 'x'\n    v = u * q\n    return v\n").body[0]
    assert fingerprint(a) == fingerprint(b)
    assert fingerprint(a) != fingerprint(c)  # argument positions matter
    assert fingerprint(a) != fingerprint(d)  # constant types matter
    assert statement_count(a) == 3
    assert statement_count(ast.parse("def f():\n    for i in x:\n        y = i\n    return y\n").body[0]) == 3


def test_token_stream_rules():
    toks = token_stream("def f(a):\n    # comment\n    return obj.attr(a, 12, 'str')  \n")
    assert "N" in toks and "attr" in toks and "0" in toks and "S" in toks
    assert "def" in toks and "return" in toks
    assert not any(t.startswith("#") for t in toks)
    assert "a" not in toks and "f" not in toks and "obj" not in toks and "12" not in toks
    assert similarity(toks, toks) == 1.0 and similarity([], toks) == 0.0
