from __future__ import annotations

from pathlib import Path

from analyst_kernel.index_scripts import infer_dest


def test_scripts_index(kernel):
    scripts = {s["name"]: s for s in kernel.call("scripts.index")["scripts"]}
    assert set(scripts) == {"bad_script", "fit_decay", "no_parser"}
    fd = scripts["fit_decay"]
    assert fd["path"] == "scripts/fit_decay.py"
    assert fd["description"] == "Fit a double exponential decay"
    assert fd["has_main"] and fd["has_main_guard"]
    opts = {o["dest"]: o for o in fd["options"]}
    assert opts["input"]["type"] == "Path" and opts["input"]["default"] == "data/sample.csv"
    assert opts["tau1"] == {
        "flags": ["--tau1"], "dest": "tau1", "type": "float", "default": 1.0,
        "help": "initial guess for the fast time constant (s)", "required": False, "choices": None, "action": None,
    }
    assert opts["mode"]["choices"] == ["fast", "full"]
    assert opts["verbose"]["action"] == "store_true" and opts["verbose"]["default"] is False
    assert all(o["help"] for o in fd["options"])
    bad = scripts["bad_script"]
    assert bad["description"] is None and not bad["has_main_guard"]
    assert bad["options"][0]["help"] is None
    npx = scripts["no_parser"]
    assert npx["options"] == [] and not npx["has_main"]


def test_scripts_index_custom_globs_and_forms(kernel, repo: Path):
    (repo / "tools").mkdir()
    (repo / "tools" / "t.py").write_text(
        "import argparse\n"
        "from pathlib import Path\n"
        "def build_parser():\n"
        "    p = argparse.ArgumentParser(description='Tool')\n"
        "    p.add_argument('-n', '--num-steps', dest='steps', type=int, required=True, help='n')\n"
        "    p.add_argument('--out-file', type=Path, default=Path('out/x.csv'), help='o')\n"
        "    p.add_argument('positional', help='p')\n"
        "    p.add_argument('--flag', action='store_false', help='f')\n"
        "    p.add_argument('--weird', default=compute(), help='w')\n"
        "    return p\n"
        "def main(argv=None):\n    build_parser().parse_args(argv)\n"
        "if '__main__' == __name__:\n    main()\n"
    )
    res = kernel.call("scripts.index", {"globs": ["tools/*.py"]})["scripts"]
    assert len(res) == 1
    s = res[0]
    assert s["description"] == "Tool" and s["has_main_guard"]
    o = {x["dest"]: x for x in s["options"]}
    assert o["steps"]["flags"] == ["-n", "--num-steps"] and o["steps"]["required"] is True
    assert o["out_file"]["default"] == "out/x.csv" and o["out_file"]["type"] == "Path"
    assert o["positional"]["required"] is True and o["positional"]["flags"] == ["positional"]
    assert o["flag"]["default"] is True
    assert o["weird"]["default"] is None


def test_infer_dest():
    assert infer_dest(["--foo-bar"], None) == "foo_bar"
    assert infer_dest(["-f", "--foo"], None) == "foo"
    assert infer_dest(["-f"], None) == "f"
    assert infer_dest(["pos-arg"], None) == "pos_arg"
    assert infer_dest(["--foo"], "bar") == "bar"


def test_lib_index(kernel):
    funcs = {f["qualname"]: f for f in kernel.call("lib.index")["functions"]}
    assert set(funcs) == {"load_runs", "count_rows", "double_exp", "damped_cos", "gaussian", "running_sum", "power_law"}
    de = funcs["double_exp"]
    assert de["module"] == "analysis.models" and de["path"] == "analysis/models.py"
    assert de["signature"] == "(t, A, tau1, B, tau2)"
    assert de["doc_summary"] == "Two-component exponential decay."
    assert de["has_math"] is True
    assert de["callers"] == [{"path": "scripts/fit_decay.py", "line": 35}]
    lr = funcs["load_runs"]
    assert lr["has_math"] is False
    assert {c["path"] for c in lr["callers"]} == {"scripts/bad_script.py", "scripts/fit_decay.py"}
    assert funcs["gaussian"]["doc_summary"] is None
    assert funcs["count_rows"]["callers"] == []


def test_lib_index_other_package_recursive(kernel, repo: Path):
    pkg = repo / "mylib" / "sub"
    pkg.mkdir(parents=True)
    (repo / "mylib" / "__init__.py").write_text("")
    (pkg / "__init__.py").write_text("")
    (pkg / "deep.py").write_text("def f(x: int) -> int:\n    'Doc.'\n    return x\n\ndef _hidden():\n    pass\n")
    res = kernel.call("lib.index", {"package": "mylib"})["functions"]
    assert [f["qualname"] for f in res] == ["f"]
    assert res[0]["module"] == "mylib.sub.deep" and res[0]["signature"] == "(x: int) -> int"
