"""``lib.index``: public library functions by AST, with callers by name match."""

from __future__ import annotations

import ast
from collections import defaultdict
from pathlib import Path
from typing import Any

from .astutil import (
    FuncDef,
    callee_name,
    iter_functions,
    library_files,
    module_name,
    parse_file,
    script_files,
)
from .docstrings import doc_summary, function_docstring, has_math


def signature_string(node: ast.FunctionDef | ast.AsyncFunctionDef) -> str:
    sig = f"({ast.unparse(node.args)})"
    if node.returns is not None:
        sig += f" -> {ast.unparse(node.returns)}"
    return sig


def collect_call_sites(root: Path, files: list[Path]) -> dict[str, list[dict[str, Any]]]:
    """Map bare callee name -> call sites across the given files."""
    sites: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for path in files:
        tree = parse_file(path)
        if tree is None:
            continue
        rel = path.resolve().relative_to(root.resolve()).as_posix()
        for node in ast.walk(tree):
            if isinstance(node, ast.Call):
                name = callee_name(node)
                if name:
                    sites[name].append({"path": rel, "line": node.lineno})
    return sites


def library_functions(root: Path, package: str) -> list[FuncDef]:
    out: list[FuncDef] = []
    for path in library_files(root, package):
        tree = parse_file(path)
        if tree is None:
            continue
        out.extend(iter_functions(tree, path, root))
    return out


def index_lib(root: Path, package: str, script_globs: list[str]) -> list[dict[str, Any]]:
    funcs = [f for f in library_functions(root, package) if f.is_public and not _private_module(f)]
    files = library_files(root, package) + script_files(root, script_globs)
    sites = collect_call_sites(root, files)
    result: list[dict[str, Any]] = []
    for f in funcs:
        doc = function_docstring(f.node)
        callers = [
            s for s in sites.get(f.name, []) if not (s["path"] == f.rel and s["line"] == f.line)
        ]
        result.append(
            {
                "module": module_name(root, f.path),
                "qualname": f.qualname,
                "path": f.rel,
                "line": f.line,
                "signature": signature_string(f.node),
                "doc_summary": doc_summary(doc),
                "has_math": has_math(doc),
                "callers": callers,
            }
        )
    return result


def _private_module(f: FuncDef) -> bool:
    return any(part.startswith("_") and part != "__init__.py" for part in Path(f.rel).parts)
