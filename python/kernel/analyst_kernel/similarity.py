"""Normalized-AST fingerprints and token-stream similarity (PROTOCOL.md section 3)."""

from __future__ import annotations

import ast
import copy
import difflib
import hashlib
import io
import keyword
import tokenize
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .astutil import FuncDef, iter_functions, library_files, parse_file, read_source, script_files
from .config import ProjectConfig


@dataclass
class FunctionRecord:
    path: str
    qualname: str
    name: str
    line: int
    kind: str  # "script" | "library"
    fingerprint: str
    n_statements: int
    tokens: list[str] = field(default_factory=list)
    signature: str = ""

    def loc(self) -> dict[str, Any]:
        return {"path": self.path, "qualname": self.qualname, "line": self.line}


# ---- fingerprint ----------------------------------------------------------------


class _Normalizer(ast.NodeTransformer):
    def __init__(self, arg_positions: dict[str, int]) -> None:
        self.arg_positions = arg_positions

    def visit_Name(self, node: ast.Name) -> ast.AST:
        pos = self.arg_positions.get(node.id)
        node.id = f"A{pos}" if pos is not None else "N"
        return node

    def visit_Constant(self, node: ast.Constant) -> ast.AST:
        node.value = f"C:{type(node.value).__name__}"
        return node

    def visit_arg(self, node: ast.arg) -> ast.AST:
        pos = self.arg_positions.get(node.arg)
        node.arg = f"A{pos}" if pos is not None else "N"
        node.annotation = None
        return node


def _strip_docstring(body: list[ast.stmt]) -> list[ast.stmt]:
    if body and isinstance(body[0], ast.Expr) and isinstance(body[0].value, ast.Constant) and isinstance(body[0].value.value, str):
        return body[1:]
    return body


def fingerprint(node: ast.FunctionDef | ast.AsyncFunctionDef) -> str:
    args = node.args
    names = [a.arg for a in args.posonlyargs + args.args + args.kwonlyargs]
    if args.vararg:
        names.append(args.vararg.arg)
    if args.kwarg:
        names.append(args.kwarg.arg)
    positions = {name: i for i, name in enumerate(names)}
    body = [copy.deepcopy(s) for s in _strip_docstring(node.body)]
    module = ast.Module(body=body, type_ignores=[])
    normalized = _Normalizer(positions).visit(module)
    text = ast.dump(normalized, include_attributes=False)
    return hashlib.sha1(text.encode("utf-8")).hexdigest()


def statement_count(node: ast.FunctionDef | ast.AsyncFunctionDef) -> int:
    body = _strip_docstring(node.body)
    return sum(1 for s in body for _ in _walk_stmts(s))


def _walk_stmts(stmt: ast.stmt):
    yield stmt
    for child in ast.iter_child_nodes(stmt):
        if isinstance(child, ast.stmt):
            yield from _walk_stmts(child)
        elif isinstance(child, list):  # pragma: no cover - iter_child_nodes flattens lists
            pass
    for name in ("body", "orelse", "finalbody", "handlers"):
        for child in getattr(stmt, name, []) or []:
            if isinstance(child, ast.ExceptHandler):
                for sub in child.body:
                    yield from _walk_stmts(sub)


# ---- token stream ----------------------------------------------------------------

_DROP = {
    tokenize.COMMENT,
    tokenize.NL,
    tokenize.INDENT,
    tokenize.DEDENT,
    tokenize.ENCODING,
    tokenize.ENDMARKER,
}
_STRING_KINDS = {tokenize.STRING} | {
    getattr(tokenize, n) for n in ("FSTRING_START", "FSTRING_MIDDLE", "FSTRING_END") if hasattr(tokenize, n)
}


def token_stream(source: str) -> list[str]:
    out: list[str] = []
    prev_op_dot = False
    try:
        toks = list(tokenize.generate_tokens(io.StringIO(source).readline))
    except (tokenize.TokenError, SyntaxError, IndentationError):
        return []
    for tok in toks:
        kind = tok.type
        if kind in _DROP:
            continue
        if kind == tokenize.NAME:
            if keyword.iskeyword(tok.string) or prev_op_dot:
                out.append(tok.string)
            else:
                out.append("N")
        elif kind == tokenize.NUMBER:
            out.append("0")
        elif kind in _STRING_KINDS:
            out.append("S")
        elif kind == tokenize.NEWLINE:
            out.append("\n")
        else:
            out.append(tok.string)
        prev_op_dot = kind == tokenize.OP and tok.string == "."
    return out


def similarity(a: list[str], b: list[str]) -> float:
    if not a or not b:
        return 0.0
    sm = difflib.SequenceMatcher(None, a, b, autojunk=False)
    return sm.ratio()


# ---- collection -----------------------------------------------------------------


def function_source(source: str, node: ast.AST) -> str:
    seg = ast.get_source_segment(source, node)
    if seg is None:
        return ""
    # dedent so nested definitions tokenize cleanly
    lines = seg.splitlines()
    if not lines:
        return ""
    import textwrap

    return textwrap.dedent(seg)


def record_for(f: FuncDef, source: str, kind: str) -> FunctionRecord:
    return FunctionRecord(
        path=f.rel,
        qualname=f.qualname,
        name=f.name,
        line=f.line,
        kind=kind,
        fingerprint=fingerprint(f.node),
        n_statements=statement_count(f.node),
        tokens=token_stream(function_source(source, f.node)),
        signature=ast.unparse(f.node.args),
    )


def collect_functions(root: Path, config: ProjectConfig, files: list[tuple[Path, str]] | None = None) -> list[FunctionRecord]:
    """All functions in scripts and the library (or in ``files``: list of (path, kind))."""
    if files is None:
        files = [(p, "script") for p in script_files(root, config.scripts)]
        files += [(p, "library") for p in library_files(root, config.library)]
    records: list[FunctionRecord] = []
    for path, kind in files:
        tree = parse_file(path)
        if tree is None:
            continue
        source = read_source(path)
        for f in iter_functions(tree, path, root):
            records.append(record_for(f, source, kind))
    return records


def find_pairs(
    records: list[FunctionRecord], threshold: float, min_tokens: int
) -> tuple[list[tuple[FunctionRecord, FunctionRecord]], list[tuple[FunctionRecord, FunctionRecord, float]]]:
    """Exact duplicates (>= 3 statements) and near-duplicates above ``threshold``."""
    duplicates: list[tuple[FunctionRecord, FunctionRecord]] = []
    dup_keys: set[tuple[int, int]] = set()
    by_fp: dict[str, list[int]] = {}
    for i, r in enumerate(records):
        by_fp.setdefault(r.fingerprint, []).append(i)
    for idxs in by_fp.values():
        if len(idxs) < 2:
            continue
        for a in range(len(idxs)):
            for b in range(a + 1, len(idxs)):
                ra, rb = records[idxs[a]], records[idxs[b]]
                if ra.n_statements >= 3 and rb.n_statements >= 3:
                    duplicates.append((ra, rb))
                    dup_keys.add((idxs[a], idxs[b]))
    near: list[tuple[FunctionRecord, FunctionRecord, float]] = []
    eligible = [i for i, r in enumerate(records) if len(r.tokens) >= min_tokens]
    for x in range(len(eligible)):
        for y in range(x + 1, len(eligible)):
            i, j = eligible[x], eligible[y]
            if (i, j) in dup_keys or records[i].fingerprint == records[j].fingerprint:
                continue
            ra, rb = records[i], records[j]
            sm = difflib.SequenceMatcher(None, ra.tokens, rb.tokens, autojunk=False)
            if sm.real_quick_ratio() < threshold or sm.quick_ratio() < threshold:
                continue
            score = sm.ratio()
            if score >= threshold:
                near.append((ra, rb, score))
    return duplicates, near


def find_twins(records: list[FunctionRecord]) -> list[tuple[FunctionRecord, FunctionRecord]]:
    """Script functions whose name equals a library function's name."""
    lib_by_name: dict[str, list[FunctionRecord]] = {}
    for r in records:
        if r.kind == "library":
            lib_by_name.setdefault(r.name, []).append(r)
    twins: list[tuple[FunctionRecord, FunctionRecord]] = []
    for r in records:
        if r.kind == "script":
            for lib in lib_by_name.get(r.name, []):
                twins.append((r, lib))
    return twins


def similar_pairs(root: Path, config: ProjectConfig, threshold: float | None = None) -> list[dict[str, Any]]:
    thr = float(threshold) if threshold is not None else config.near_duplicate_threshold
    records = collect_functions(root, config)
    duplicates, near = find_pairs(records, thr, config.min_tokens_for_similarity)
    pairs: list[dict[str, Any]] = []
    for a, b in duplicates:
        pairs.append({"kind": "duplicate", "score": 1.0, "a": a.loc(), "b": b.loc()})
    for a, b, score in near:
        pairs.append({"kind": "near-duplicate", "score": round(score, 4), "a": a.loc(), "b": b.loc()})
    for a, b in find_twins(records):
        pairs.append(
            {"kind": "unused-twin", "score": round(similarity(a.tokens, b.tokens), 4), "a": a.loc(), "b": b.loc()}
        )
    return pairs
