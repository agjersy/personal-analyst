"""analyst_mpl backend: naming, de-duplication, sidecars, savefig wrapping, fallback."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

KERNEL_DIR = Path(__file__).resolve().parent.parent

SCRIPT = """
import sys
import matplotlib
import matplotlib.pyplot as plt
assert matplotlib.get_backend().lower() == "module://analyst_mpl", matplotlib.get_backend()
import analyst_mpl
from matplotlib.backends.backend_agg import FigureCanvasAgg
assert issubclass(analyst_mpl.FigureCanvas, FigureCanvasAgg)
assert all(hasattr(analyst_mpl, n) for n in ("FigureCanvas", "FigureManager", "show"))

fig1, ax = plt.subplots()
ax.plot([0, 1], [1, 0])
fig2, ax2 = plt.subplots(2, 1)
fig2.suptitle("Residuals")
ax2[0].set_title("top"); ax2[0].set_xlabel("x"); ax2[1].set_ylabel("y")
fig3, ax3 = plt.subplots()
ax3.set_title("Residuals")  # same slug: de-duplicated
fig4, ax4 = plt.subplots(figsize=(4, 3), dpi=50)
ax4.set_title("Decay fit: run #1")
plt.show()
assert plt.get_fignums() == [], plt.get_fignums()

fig5, ax5 = plt.subplots()
ax5.set_title("explicit")
fig5.savefig(sys.argv[1])
plt.close(fig5)
print("ok")
"""


def run_backend(tmp_path: Path, env_extra: dict[str, str], cwd: Path | None = None) -> subprocess.CompletedProcess:
    env = dict(os.environ)
    env.update(
        {
            "MPLBACKEND": "module://analyst_mpl",
            "PYTHONPATH": str(KERNEL_DIR),
            "MPLCONFIGDIR": str(tmp_path / "mplcfg"),
        }
    )
    env.pop("ANALYST_OUT_DIR", None)
    env.update(env_extra)
    script = tmp_path / "plot_script.py"
    script.write_text(SCRIPT)
    explicit = tmp_path / "explicit_dir" / "custom.png"
    explicit.parent.mkdir()
    return subprocess.run(
        [sys.executable, str(script), str(explicit), "--flag", "7"],
        env=env, cwd=str(cwd or tmp_path), capture_output=True, text=True,
    )


def test_show_naming_sidecars_and_savefig(tmp_path: Path):
    out = tmp_path / "outdir"
    proc = run_backend(tmp_path, {"ANALYST_OUT_DIR": str(out), "ANALYST_RUN_ID": "20260101-000000-abcdef",
                                  "ANALYST_ROOT": str(tmp_path)})
    assert proc.returncode == 0, proc.stderr
    figs = out / "figures"
    names = sorted(p.name for p in figs.iterdir())
    assert names == sorted([
        "fig-1.png", "fig-1.svg", "fig-1.json",
        "residuals.png", "residuals.svg", "residuals.json",
        "residuals-2.png", "residuals-2.svg", "residuals-2.json",
        "decay-fit-run-1.png", "decay-fit-run-1.svg", "decay-fit-run-1.json",
    ])
    assert (figs / "fig-1.png").read_bytes()[:4] == b"\x89PNG"
    assert b"<svg" in (figs / "fig-1.svg").read_bytes()[:400]

    sc = json.loads((figs / "residuals.json").read_text())
    assert sc == {
        "run_id": "20260101-000000-abcdef",
        "script": "plot_script.py",
        "args": [str(tmp_path / "explicit_dir" / "custom.png"), "--flag", "7"],
        "title": "Residuals",
        "axes": [{"title": "top", "xlabel": "x", "ylabel": None}, {"title": None, "xlabel": None, "ylabel": "y"}],
        "created_at": sc["created_at"],
        "width_in": sc["width_in"], "height_in": sc["height_in"], "dpi": sc["dpi"],
    }
    assert sc["created_at"].endswith("Z") and sc["width_in"] > 0 and sc["dpi"] > 0
    sc1 = json.loads((figs / "fig-1.json").read_text())
    assert sc1["title"] is None and sc1["axes"] == [{"title": None, "xlabel": None, "ylabel": None}]
    sc4 = json.loads((figs / "decay-fit-run-1.json").read_text())
    assert (sc4["width_in"], sc4["height_in"], sc4["dpi"]) == (4.0, 3.0, 50.0)

    # savefig: saved where asked plus a sidecar next to it
    explicit = tmp_path / "explicit_dir" / "custom.png"
    assert explicit.exists()
    side = json.loads(explicit.with_suffix(".json").read_text())
    assert side["title"] == "explicit" and side["run_id"] == "20260101-000000-abcdef"


def test_fallback_without_out_dir(tmp_path: Path):
    cwd = tmp_path / "work"
    cwd.mkdir()
    proc = run_backend(tmp_path, {}, cwd=cwd)
    assert proc.returncode == 0, proc.stderr
    figs = cwd / "out" / "figures"
    assert (figs / "fig-1.png").exists() and (figs / "residuals-2.json").exists()
    assert json.loads((figs / "fig-1.json").read_text())["run_id"] is None


def test_module_imports_only_stdlib_and_matplotlib():
    import ast

    src = (KERNEL_DIR / "analyst_mpl" / "__init__.py").read_text()
    tops = set()
    for node in ast.walk(ast.parse(src)):
        if isinstance(node, ast.Import):
            tops.update(a.name.split(".")[0] for a in node.names)
        elif isinstance(node, ast.ImportFrom) and node.module:
            tops.add(node.module.split(".")[0])
    assert tops <= {"json", "os", "re", "sys", "datetime", "pathlib", "matplotlib", "__future__"}, tops
