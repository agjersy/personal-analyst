from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

from analyst_kernel.errors import RpcError


def make_outputs(repo: Path) -> Path:
    out = repo / "out" / "demo" / "r1"
    out.mkdir(parents=True)
    (out / "table.csv").write_text("a,b\n1,2\n3,4\n")
    (out / "notes.txt").write_text("hello\n" * 10)
    (out / "meta.json").write_text(json.dumps({"k": 1}))
    (out / "blob.bin").write_bytes(bytes(range(256)))
    (out / "fig.png").write_bytes(b"\x89PNG\r\n")
    (out / "fig.json").write_text("{}")
    subprocess.run(
        [sys.executable, "-c",
         f"import numpy as np; np.save({str(out / 'arr.npy')!r}, np.arange(12.0).reshape(3,4)); "
         f"np.savez({str(out / 'multi.npz')!r}, x=np.ones(3), y=np.zeros((2,2), dtype=int))"],
        check=True,
    )
    return out


def test_out_list_default_and_by_path_and_run(kernel, repo):
    out = make_outputs(repo)
    entries = kernel.call("out.list")["entries"]
    paths = {e["path"]: e for e in entries}
    assert "out/demo/r1/table.csv" in paths
    assert paths["out/demo/r1/table.csv"]["kind"] == "table"
    assert paths["out/demo/r1/fig.png"]["kind"] == "figure"
    assert paths["out/demo/r1/arr.npy"]["kind"] == "array"
    assert paths["out/demo/r1/blob.bin"]["kind"] == "file"
    assert paths["out/demo/r1/meta.json"]["kind"] == "json"
    assert paths["out/demo/r1/notes.txt"]["size"] == 60
    assert paths["out/demo/r1/notes.txt"]["mtime"].endswith("Z")
    sub = kernel.call("out.list", {"path": "out/demo"})["entries"]
    assert len(sub) == len(entries)
    assert kernel.call("out.list", {"path": "out/nothing"})["entries"] == []
    assert out.is_dir()


def test_out_read_text_kinds(kernel, repo):
    make_outputs(repo)
    r = kernel.call("out.read", {"path": "out/demo/r1/table.csv"})
    assert r == {"kind": "table", "preview": "a,b\n1,2\n3,4\n", "truncated": False,
                 "summary": {"rows_sampled": 2, "columns": ["a", "b"]}}
    r = kernel.call("out.read", {"path": "out/demo/r1/notes.txt", "max_bytes": 12})
    assert r["kind"] == "text" and r["preview"] == "hello\nhello\n" and r["truncated"] is True
    r = kernel.call("out.read", {"path": "out/demo/r1/meta.json"})
    assert r["kind"] == "json" and json.loads(r["preview"]) == {"k": 1} and r["summary"] is None
    r = kernel.call("out.read", {"path": "out/demo/r1/blob.bin"})
    assert r == {"kind": "file", "preview": "", "truncated": True, "summary": None}
    r = kernel.call("out.read", {"path": "out/demo/r1/fig.png"})
    assert r["kind"] == "figure" and r["preview"] == ""


def test_out_read_arrays_via_session_python(kernel, repo):
    make_outputs(repo)
    r = kernel.call("out.read", {"path": "out/demo/r1/arr.npy"})
    assert r["kind"] == "array"
    assert r["summary"] == {"shape": [3, 4], "dtype": "float64", "min": 0.0, "max": 11.0, "mean": 5.5}
    r = kernel.call("out.read", {"path": "out/demo/r1/multi.npz"})
    assert r["kind"] == "array"
    assert r["summary"]["arrays"]["x"]["shape"] == [3] and r["summary"]["arrays"]["y"]["dtype"].startswith("int")


def test_out_read_array_without_numpy_is_file(kernel, repo, tmp_path):
    make_outputs(repo)
    # a "python" that cannot import numpy: the kernel must degrade to kind "file"
    fake = tmp_path / "nonumpy.sh"
    fake.write_text(f"#!/bin/sh\nexec {sys.executable} -I -S \"$@\"\n")
    fake.chmod(0o755)
    kernel.python = str(fake)
    r = kernel.call("out.read", {"path": "out/demo/r1/arr.npy"})
    assert r["kind"] == "file" and r["summary"] is None and r["preview"] == ""


def test_out_read_errors(kernel):
    with pytest.raises(RpcError):
        kernel.call("out.read", {"path": "out/none.txt"})
    with pytest.raises(RpcError):
        kernel.call("out.read", {"path": ""})
    with pytest.raises(RpcError):
        kernel.call("out.read", {"path": "data/sample.csv", "max_bytes": 0})


def test_out_list_by_run_id(kernel, notes):
    res = kernel.call("run.start", {"script": "scripts/fit_decay.py"})
    notes.wait_for("run.done", res["run_id"])
    entries = kernel.call("out.list", {"run_id": res["run_id"]})["entries"]
    kinds = {e["path"].rsplit("/", 1)[-1]: e["kind"] for e in entries}
    assert kinds["decay-fit.png"] == "figure" and kinds["fit.json"] == "json" and kinds["run.json"] == "json"
