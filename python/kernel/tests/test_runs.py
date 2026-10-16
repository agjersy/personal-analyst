"""run.start / run.attach / run.status / run.cancel / runs.list per PROTOCOL.md 1.4."""

from __future__ import annotations

import json
import os
import re
import sys
import time
from pathlib import Path

from conftest import Notes, wait_until

from analyst_kernel.kernel import Kernel

RUN_ID_RE = re.compile(r"^\d{8}-\d{6}-[0-9a-f]{6}$")


def test_run_fit_decay_end_to_end(kernel: Kernel, repo: Path, notes: Notes, tmp_path: Path):
    res = kernel.call("run.start", {"script": "scripts/fit_decay.py", "args": ["--tau1", "1.5"], "tree_hash": "abc"})
    run_id = res["run_id"]
    assert RUN_ID_RE.match(run_id)
    assert res["out_dir"] == f"out/fit_decay/{run_id}"

    run_dir = tmp_path / "runs" / run_id
    for name in ("meta.json", "stdout.log", "stderr.log", "status", "artifacts.jsonl"):
        assert (run_dir / name).exists()
    run_json = json.loads((repo / res["out_dir"] / "run.json").read_text())
    assert run_json == {
        "run_id": run_id, "script": "scripts/fit_decay.py", "args": ["--tau1", "1.5"],
        "started_at": run_json["started_at"], "host": kernel.host, "tree_hash": "abc",
    }

    done = notes.wait_for("run.done", run_id)[0]
    assert done["status"] == "done" and done["exit_code"] == 0 and done["duration_s"] >= 0
    assert (run_dir / "status").read_text().strip() == "done:0"

    stdout = "".join(p["chunk"] for p in notes.of("run.stdout", run_id))
    assert "done 50" in stdout
    offsets = [p["offset"] for p in notes.of("run.stdout", run_id)]
    assert offsets == sorted(offsets) and offsets[0] == 0
    assert (run_dir / "stdout.log").read_text() == stdout

    arts = {a["artifact"]["path"].rsplit("/", 1)[-1]: a["artifact"] for a in notes.of("run.artifact", run_id)}
    assert "run.json" not in arts
    fig = arts["decay-fit.png"]
    assert fig["kind"] == "figure" and fig["rendered"] is True and fig["size"] > 0
    assert fig["path"] == f"{res['out_dir']}/figures/decay-fit.png"
    assert fig["sidecar"]["run_id"] == run_id
    assert fig["sidecar"]["script"] == "scripts/fit_decay.py"
    assert fig["sidecar"]["args"] == ["--tau1", "1.5"]
    assert fig["sidecar"]["title"] == "Decay fit"
    assert fig["sidecar"]["axes"] == [{"title": "Decay fit", "xlabel": "t (s)", "ylabel": "signal"}]
    assert arts["decay-fit.svg"]["kind"] == "figure"
    assert "decay-fit.json" not in arts  # sidecar is not an artifact
    assert arts["fit.json"]["kind"] == "json" and arts["fit.json"]["rendered"] is True
    recorded = [json.loads(ln) for ln in (run_dir / "artifacts.jsonl").read_text().splitlines()]
    assert {a["path"] for a in recorded} == {a["path"] for a in arts.values()}

    meta = kernel.call("run.status", {"run_id": run_id})
    assert set(meta) == {"run_id", "script", "args", "status", "exit_code", "started_at", "finished_at",
                         "out_dir", "sandbox", "tree_hash", "pid", "host"}
    assert meta["status"] == "done" and meta["exit_code"] == 0 and meta["sandbox"] == "limits"
    assert meta["tree_hash"] == "abc" and meta["finished_at"] and isinstance(meta["pid"], int)

    runs = kernel.call("runs.list")["runs"]
    assert runs[0]["run_id"] == run_id
    assert kernel.call("runs.list", {"limit": 0})["runs"] == []

    # ---- attach from a fresh manager over the same runs dir (kernel restart) ----
    notes2 = Notes()
    k2 = Kernel(sys.executable, repo, tmp_path / "runs", sandbox="limits", notify=notes2)
    try:
        att = k2.call("run.attach", {"run_id": run_id, "stdout_offset": 5})
        assert att == {"status": "done", "exit_code": 0}
        replay = notes2.of("run.stdout", run_id)
        assert len(replay) == 1 and replay[0]["offset"] == 5 and replay[0]["chunk"] == stdout[5:]
        assert {a["artifact"]["path"] for a in notes2.of("run.artifact", run_id)} == {a["path"] for a in arts.values()}
        assert notes2.of("run.done", run_id)[0]["status"] == "done"
        assert k2.call("runs.list")["runs"][0]["run_id"] == run_id
    finally:
        k2.close()


def write_script(repo: Path, name: str, body: str) -> str:
    p = repo / "scripts" / name
    p.write_text(body)
    return f"scripts/{name}"


ENV_DUMP = """
import json, os, sys
out = os.environ["ANALYST_OUT_DIR"]
with open(os.path.join(out, "env.json"), "w") as fh:
    json.dump(dict(os.environ), fh)
print("cwd", os.getcwd())
print("mpl", sys.modules.get("matplotlib") is None)
"""


def test_environment_table_and_cwd(kernel: Kernel, repo: Path, notes: Notes, tmp_path: Path, monkeypatch):
    monkeypatch.setenv("SECRET_TOKEN", "nope")
    monkeypatch.setenv("LANG", "C.UTF-8")
    script = write_script(repo, "envdump.py", ENV_DUMP)
    res = kernel.call("run.start", {"script": script, "env": {"EXTRA": "1"}})
    run_id = res["run_id"]
    notes.wait_for("run.done", run_id)
    env = json.loads((repo / res["out_dir"] / "env.json").read_text())
    assert env["ANALYST_RUN_ID"] == run_id
    assert env["ANALYST_OUT_DIR"] == str((repo / res["out_dir"]).resolve())
    assert env["ANALYST_ROOT"] == str(repo.resolve())
    assert env["MPLBACKEND"] == "module://analyst_mpl"
    assert env["MPLCONFIGDIR"] == str(tmp_path / "runs" / run_id / "mplconfig")
    assert env["PYTHONPATH"].split(os.pathsep)[0] == str(kernel.mpl_dir)
    assert (Path(env["PYTHONPATH"].split(os.pathsep)[0]) / "analyst_mpl" / "__init__.py").exists()
    assert env["PYTHONUNBUFFERED"] == "1"
    assert env["EXTRA"] == "1" and env["LANG"] == "C.UTF-8" and "HOME" in env and "PATH" in env
    assert "SECRET_TOKEN" not in env
    out = "".join(p["chunk"] for p in notes.of("run.stdout", run_id))
    assert f"cwd {repo.resolve()}" in out


def test_cancel(kernel: Kernel, repo: Path, notes: Notes, tmp_path: Path):
    script = write_script(repo, "sleepy.py", "import time\nprint('start', flush=True)\ntime.sleep(60)\n")
    run_id = kernel.call("run.start", {"script": script})["run_id"]
    notes.wait_for("run.stdout", run_id)
    assert kernel.call("run.status", {"run_id": run_id})["status"] == "running"
    assert kernel.call("run.cancel", {"run_id": run_id}) == {"ok": True}
    done = notes.wait_for("run.done", run_id, timeout=20)[0]
    assert done["status"] == "cancelled"
    assert (tmp_path / "runs" / run_id / "status").read_text().strip() == "cancelled"
    assert kernel.call("run.status", {"run_id": run_id})["status"] == "cancelled"
    assert kernel.call("run.cancel", {"run_id": run_id}) == {"ok": False}
    # the process group is gone
    pid = int((tmp_path / "runs" / run_id / "child.pid").read_text())
    wait_until(lambda: not _alive(pid), timeout=10)


def _alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def test_timeout_kills_process_group(kernel: Kernel, repo: Path, notes: Notes, tmp_path: Path):
    script = write_script(
        repo, "forever.py",
        "import subprocess, sys, time\nsubprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])\n"
        "print('go', flush=True)\ntime.sleep(60)\n",
    )
    run_id = kernel.call("run.start", {"script": script, "timeout_s": 1})["run_id"]
    done = notes.wait_for("run.done", run_id, timeout=30)[0]
    assert done["status"] == "timeout"
    assert (tmp_path / "runs" / run_id / "status").read_text().strip() == "timeout"
    meta = kernel.call("run.status", {"run_id": run_id})
    assert meta["status"] == "timeout"
    wait_until(lambda: not _alive(int((tmp_path / "runs" / run_id / "child.pid").read_text())), timeout=10)


def test_failed_exit_code_and_stderr(kernel: Kernel, repo: Path, notes: Notes):
    script = write_script(repo, "crash.py", "import sys\nsys.stderr.write('boom\\n')\nsys.exit(3)\n")
    run_id = kernel.call("run.start", {"script": script})["run_id"]
    done = notes.wait_for("run.done", run_id)[0]
    assert done["status"] == "done" and done["exit_code"] == 3
    assert "boom" in "".join(p["chunk"] for p in notes.of("run.stderr", run_id))


def test_attach_running_run_from_fresh_kernel(kernel: Kernel, repo: Path, notes: Notes, tmp_path: Path):
    script = write_script(
        repo, "slow.py",
        "import time, os\nprint('one', flush=True)\ntime.sleep(2)\n"
        "open(os.path.join(os.environ['ANALYST_OUT_DIR'], 'late.txt'), 'w').write('x')\nprint('two', flush=True)\n",
    )
    run_id = kernel.call("run.start", {"script": script})["run_id"]
    notes.wait_for("run.stdout", run_id)
    # a second kernel attaches while the run is still going; it must stream the rest and run.done
    notes2 = Notes()
    k2 = Kernel(sys.executable, repo, tmp_path / "runs", sandbox="limits", notify=notes2)
    try:
        att = k2.call("run.attach", {"run_id": run_id})
        assert att["status"] == "running" and att["exit_code"] is None
        done = notes2.wait_for("run.done", run_id, timeout=30)[0]
        assert done["status"] == "done"
        text = "".join(p["chunk"] for p in notes2.of("run.stdout", run_id))
        assert "one" in text and "two" in text
        assert any(a["artifact"]["path"].endswith("late.txt") for a in notes2.of("run.artifact", run_id))
    finally:
        k2.close()
    notes.wait_for("run.done", run_id, timeout=30)


def test_size_stable_rule_waits_for_growing_files(kernel: Kernel, repo: Path, notes: Notes):
    script = write_script(
        repo, "grow.py",
        "import os, time\np = os.path.join(os.environ['ANALYST_OUT_DIR'], 'big.txt')\n"
        "with open(p, 'w') as fh:\n    for i in range(40):\n        fh.write('x' * 100)\n        fh.flush()\n        time.sleep(0.03)\n",
    )
    run_id = kernel.call("run.start", {"script": script})["run_id"]
    notes.wait_for("run.done", run_id)
    arts = [a["artifact"] for a in notes.of("run.artifact", run_id) if a["artifact"]["path"].endswith("big.txt")]
    assert len(arts) == 1 and arts[0]["size"] == 4000 and arts[0]["kind"] == "text"


def test_artifact_scanner_rules(tmp_path: Path):
    from analyst_kernel.runs import _ArtifactScanner

    root = tmp_path / "root"
    run_dir = tmp_path / "run"
    out = root / "out" / "s" / "r"
    out.mkdir(parents=True)
    run_dir.mkdir()
    (run_dir / "artifacts.jsonl").touch()
    (out / "run.json").write_text("{}")
    big = out / "big.txt"
    big.write_text("a")
    sc = _ArtifactScanner(root, run_dir, out)
    assert sc.scan() == []  # first sighting: not yet stable
    big.write_text("ab")
    assert sc.scan() == []  # grew: still not stable
    found = sc.scan()
    assert [a["path"] for a in found] == ["out/s/r/big.txt"] and found[0]["size"] == 2
    assert sc.scan() == []  # emitted once
    # figure without sidecar waits two extra polls, then goes out without one
    (out / "fig.png").write_bytes(b"png")
    assert sc.scan() == [] and sc.scan() == [] and sc.scan() == []
    found = sc.scan()
    assert found[0]["kind"] == "figure" and "sidecar" not in found[0]
    # figure with a sidecar: sidecar attached and the json is not an artifact
    (out / "two.png").write_bytes(b"png")
    (out / "two.json").write_text(json.dumps({"title": "T"}))
    sc.scan()
    found = sc.scan()
    assert [a["path"] for a in found] == ["out/s/r/two.png"] and found[0]["sidecar"] == {"title": "T"}
    assert "out/s/r/two.json" not in sc.emitted
    # unrelated json is an artifact; run.json never is; final scan flushes new files immediately
    (out / "data.json").write_text("{}")
    (out / "late.npz").write_bytes(b"x")
    found = sc.scan(final=True)
    assert {a["path"]: a["kind"] for a in found} == {"out/s/r/data.json": "json", "out/s/r/late.npz": "array"}
    assert found[0]["rendered"] is True and found[1]["rendered"] is False
    for a in found:
        sc.record(a)
    sc2 = _ArtifactScanner(root, run_dir, out)
    sc2.load_recorded()
    assert {a["path"] for a in sc2.recorded} == {"out/s/r/data.json", "out/s/r/late.npz"}


def test_runs_list_order_and_limit(kernel: Kernel, repo: Path, notes: Notes):
    script = write_script(repo, "quick.py", "print('q')\n")
    ids = []
    for _ in range(2):
        ids.append(kernel.call("run.start", {"script": script})["run_id"])
        time.sleep(1.05)  # distinct timestamps
    for rid in ids:
        notes.wait_for("run.done", rid)
    listed = [r["run_id"] for r in kernel.call("runs.list", {"limit": 1})["runs"]]
    assert listed == [ids[-1]]
    assert [r["run_id"] for r in kernel.call("runs.list")["runs"]][:2] == ids[::-1]
