"""``eq.extract`` and ``eq.check`` (PROTOCOL.md 1.3 and section 3). SymPy is imported lazily."""

from __future__ import annotations

import ast
import json
import math
import random
import re
import subprocess
import threading
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Any

from .astutil import FuncDef, iter_functions, module_name, parse_file
from .docstrings import function_docstring, latex_section, math_section
from .errors import SCRIPT_NOT_FOUND, RpcError, invalid_params
from .paths import rel_to_root, resolve_in_root
from .sandbox import SandboxSpec, rlimit_prologue, wrap

if TYPE_CHECKING:  # pragma: no cover
    pass

SIMPLIFY_BUDGET_S = 2.0
DEFAULT_TRIALS = 32
DEFAULT_TOL = 1e-6
ABS_FLOOR = 1e-9
DEFAULT_RANGE = (0.1, 10.0)
CHECK_TIMEOUT_S = 60.0

SUPPORTED_FUNCS = {
    "exp": "exp",
    "log": "log",
    "log10": "log10",
    "log2": "log2",
    "sqrt": "sqrt",
    "sin": "sin",
    "cos": "cos",
    "tan": "tan",
    "arcsin": "asin",
    "arccos": "acos",
    "arctan": "atan",
    "asin": "asin",
    "acos": "acos",
    "atan": "atan",
    "sinh": "sinh",
    "cosh": "cosh",
    "tanh": "tanh",
    "abs": "Abs",
}
MODULE_PREFIXES = ("np", "numpy", "math")
CONSTANTS = {"pi": "pi", "e": "E"}

_MATH_TEXT_RE = re.compile(r"^[\w\s+\-*/^().,]*$")


class Unsupported(Exception):
    """Derivation stopped on a construct outside the tier A subset."""


@dataclass
class Extracted:
    function: str
    qualname: str
    line: int
    tier: str
    latex: str | None
    source_expr: str | None
    status: str
    reason: str | None
    expr: Any = None  # sympy expression for derived/declared
    params: list[str] | None = None
    declared_expr: Any = None  # the Math: expression even when the tier is "derived"
    declared_error: str | None = None

    def to_json(self) -> dict[str, Any]:
        return {
            "function": self.function,
            "qualname": self.qualname,
            "line": self.line,
            "tier": self.tier,
            "latex": self.latex,
            "source_expr": self.source_expr,
            "status": self.status,
            "reason": self.reason,
        }


# ---- tier A --------------------------------------------------------------------


def _sym():
    import sympy

    return sympy


def param_names(node: ast.FunctionDef | ast.AsyncFunctionDef, is_method: bool = False) -> list[str]:
    args = node.args
    names = [a.arg for a in args.posonlyargs + args.args + args.kwonlyargs]
    if is_method and names and names[0] in ("self", "cls"):
        names = names[1:]
    return names


def _node_desc(node: ast.AST) -> str:
    kind = type(node).__name__
    try:
        text = ast.unparse(node)
    except Exception:
        text = ""
    if len(text) > 40:
        text = text[:37] + "..."
    return f"{kind} `{text}`" if text else kind


class _Deriver:
    def __init__(self, params: list[str]) -> None:
        self.sympy = _sym()
        self.symbols = {p: self.sympy.Symbol(p) for p in params}
        self.locals: dict[str, Any] = {}

    def derive(self, node: ast.FunctionDef | ast.AsyncFunctionDef):
        body = list(node.body)
        if body and isinstance(body[0], ast.Expr) and isinstance(body[0].value, ast.Constant) and isinstance(body[0].value.value, str):
            body = body[1:]
        if not body:
            raise Unsupported(f"empty body at line {node.lineno}")
        for stmt in body[:-1]:
            self._assign(stmt)
        last = body[-1]
        if not isinstance(last, ast.Return) or last.value is None:
            raise Unsupported(f"{_node_desc(last)} at line {last.lineno} (expected a final return)")
        return self.expr(last.value)

    def _assign(self, stmt: ast.stmt) -> None:
        if isinstance(stmt, ast.Assign) and len(stmt.targets) == 1 and isinstance(stmt.targets[0], ast.Name):
            name, value = stmt.targets[0].id, stmt.value
        elif isinstance(stmt, ast.AnnAssign) and isinstance(stmt.target, ast.Name) and stmt.value is not None:
            name, value = stmt.target.id, stmt.value
        else:
            raise Unsupported(f"{_node_desc(stmt)} at line {stmt.lineno}")
        self.locals[name] = self.expr(value)

    def expr(self, node: ast.expr):
        sp = self.sympy
        if isinstance(node, ast.Name):
            if node.id in self.locals:
                return self.locals[node.id]
            if node.id in self.symbols:
                return self.symbols[node.id]
            raise Unsupported(f"unknown name `{node.id}` at line {node.lineno}")
        if isinstance(node, ast.Constant):
            if isinstance(node.value, bool) or not isinstance(node.value, (int, float)):
                raise Unsupported(f"{_node_desc(node)} at line {node.lineno}")
            if isinstance(node.value, int):
                return sp.Integer(node.value)
            if math.isfinite(node.value) and float(node.value).is_integer():
                return sp.Integer(int(node.value))
            return sp.Float(node.value)
        if isinstance(node, ast.UnaryOp):
            if isinstance(node.op, ast.USub):
                return -self.expr(node.operand)
            if isinstance(node.op, ast.UAdd):
                return self.expr(node.operand)
            raise Unsupported(f"{_node_desc(node)} at line {node.lineno}")
        if isinstance(node, ast.BinOp):
            left, right = self.expr(node.left), self.expr(node.right)
            op = node.op
            if isinstance(op, ast.Add):
                return left + right
            if isinstance(op, ast.Sub):
                return left - right
            if isinstance(op, ast.Mult):
                return left * right
            if isinstance(op, ast.Div):
                return left / right
            if isinstance(op, ast.Pow):
                return left**right
            raise Unsupported(f"operator {type(op).__name__} at line {node.lineno}")
        if isinstance(node, ast.Attribute):
            if isinstance(node.value, ast.Name) and node.value.id in MODULE_PREFIXES and node.attr in CONSTANTS:
                return getattr(sp, CONSTANTS[node.attr])
            raise Unsupported(f"{_node_desc(node)} at line {node.lineno}")
        if isinstance(node, ast.Call):
            return self._call(node)
        raise Unsupported(f"{_node_desc(node)} at line {node.lineno}")

    def _call(self, node: ast.Call):
        sp = self.sympy
        func = node.func
        if isinstance(func, ast.Name):
            name = func.id
        elif isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name) and func.value.id in MODULE_PREFIXES:
            name = func.attr
        else:
            raise Unsupported(f"call to {_node_desc(func)} at line {node.lineno}")
        if name not in SUPPORTED_FUNCS or node.keywords or len(node.args) != 1 or isinstance(node.args[0], ast.Starred):
            raise Unsupported(f"call to `{name}` at line {node.lineno}")
        arg = self.expr(node.args[0])
        target = SUPPORTED_FUNCS[name]
        if target == "log10":
            return sp.log(arg, 10)
        if target == "log2":
            return sp.log(arg, 2)
        return getattr(sp, target)(arg)


def simplify_with_budget(expr, seconds: float = SIMPLIFY_BUDGET_S):
    """``sympy.simplify`` on a worker thread; the unsimplified form when the budget runs out."""
    sp = _sym()
    box: list[Any] = []

    def work() -> None:
        try:
            box.append(sp.simplify(expr))
        except Exception:
            pass

    t = threading.Thread(target=work, daemon=True, name="sympy-simplify")
    t.start()
    t.join(seconds)
    if box:
        return box[0]
    return expr


def derive_function(node: ast.FunctionDef | ast.AsyncFunctionDef, params: list[str]):
    """Tier A: returns a simplified SymPy expression or raises Unsupported."""
    raw = _Deriver(params).derive(node)
    return simplify_with_budget(raw)


# ---- tier B --------------------------------------------------------------------


def parse_math_text(text: str, local: dict[str, Any]):
    from sympy.parsing.sympy_parser import (
        auto_number,
        auto_symbol,
        convert_xor,
        factorial_notation,
        lambda_notation,
        parse_expr,
        repeated_decimals,
    )

    if not _MATH_TEXT_RE.match(text) or "__" in text:
        raise ValueError(f"unsupported characters in {text!r}")
    transformations = (lambda_notation, auto_symbol, repeated_decimals, auto_number, factorial_notation, convert_xor)
    return parse_expr(text, local_dict=dict(local), transformations=transformations, evaluate=True)


def parse_math_section(lines: list[tuple[str, str]], params: list[str]):
    """Parse ``lhs = expr`` lines; returns (lhs, expr) of the last line.

    Earlier lines define intermediate symbols that are substituted into later ones.
    """
    sp = _sym()
    local: dict[str, Any] = {p: sp.Symbol(p) for p in params}
    result = None
    lhs_final = ""
    for lhs, rhs in lines:
        expr = parse_math_text(rhs, local)
        if lhs:
            if not re.match(r"^[A-Za-z_]\w*$", lhs):
                raise ValueError(f"invalid left-hand side {lhs!r}")
            local[lhs] = expr
        result = expr
        lhs_final = lhs
    if result is None:
        raise ValueError("empty Math: section")
    allowed = set(params)
    unknown = sorted(str(s) for s in result.free_symbols if str(s) not in allowed)
    if unknown:
        raise ValueError(f"unknown symbol(s) in Math: section: {', '.join(unknown)}")
    return lhs_final, result


# ---- extraction ----------------------------------------------------------------


def _extract_one(f: FuncDef) -> Extracted:
    sp = _sym()
    params = param_names(f.node, f.is_method)
    doc = function_docstring(f.node)
    out = Extracted(f.name, f.qualname, f.line, "none", None, None, "unsupported", None, params=params)

    declared_expr = None
    declared_lhs = ""
    declared_error: str | None = None
    ms = math_section(doc)
    if ms is not None:
        try:
            declared_lhs, declared_expr = parse_math_section(ms.equations, params)
        except Exception as exc:
            declared_error = f"Math: section: {exc}"
    out.declared_expr = declared_expr
    out.declared_error = declared_error

    reason_a: str
    try:
        expr = derive_function(f.node, params)
    except Unsupported as exc:
        reason_a = str(exc)
    except Exception as exc:
        reason_a = f"{type(exc).__name__}: {exc}"
    else:
        out.tier = "derived"
        out.status = "ok"
        out.expr = expr
        out.source_expr = str(expr)
        out.latex = sp.latex(expr)
        return out

    if ms is not None:
        out.tier = "declared"
        if declared_expr is not None:
            out.status = "ok"
            out.expr = declared_expr
            out.source_expr = str(declared_expr)
            lhs_sym = sp.Symbol(declared_lhs) if declared_lhs else None
            out.latex = f"{sp.latex(lhs_sym)} = {sp.latex(declared_expr)}" if lhs_sym is not None else sp.latex(declared_expr)
        else:
            out.status = "error"
            out.reason = declared_error
        return out

    ls = latex_section(doc)
    if ls is not None:
        out.tier = "latex"
        out.status = "ok"
        out.latex = " \\\\\n".join(ls.lines)
        out.reason = "display only; not verifiable"
        return out

    out.tier = "none"
    out.status = "unsupported"
    out.reason = reason_a
    return out


def extract_file(root: Path, path: str, function: str | None = None) -> list[Extracted]:
    file = resolve_in_root(root, path)
    if not file.is_file() or file.suffix != ".py":
        raise RpcError(SCRIPT_NOT_FOUND, f"{path} is not an existing Python file")
    tree = parse_file(file)
    if tree is None:
        raise RpcError(SCRIPT_NOT_FOUND, f"{path} could not be parsed")
    funcs = iter_functions(tree, file, root)
    if function:
        funcs = [f for f in funcs if f.qualname == function or f.name == function]
    return [_extract_one(f) for f in funcs]


def eq_extract(root: Path, path: str, function: str | None = None) -> list[dict[str, Any]]:
    if not isinstance(path, str) or not path:
        raise invalid_params("path must be a non-empty string")
    return [e.to_json() for e in extract_file(root, path, function)]


# ---- eq.check --------------------------------------------------------------------

CHECK_DRIVER = r'''
import importlib, importlib.util, json, math, sys
req = json.loads(sys.stdin.read())
sys.path.insert(0, req["root"])
sys.path.insert(0, req["dir"])
def load():
    name = req.get("module")
    if name:
        try:
            return importlib.import_module(name)
        except Exception:
            pass
    spec = importlib.util.spec_from_file_location(req["stem"], req["path"])
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod
try:
    mod = load()
    fn = mod
    for part in req["function"].split("."):
        fn = getattr(fn, part)
except Exception as exc:
    print(json.dumps({"error": "%s: %s" % (type(exc).__name__, exc)}))
    sys.exit(0)
def scalar(v):
    try:
        import numpy as np
        a = np.asarray(v)
        if a.size != 1:
            raise ValueError("non-scalar result of shape %s" % (a.shape,))
        return float(a.reshape(-1)[0])
    except ImportError:
        return float(v)
results = []
for inputs in req["inputs"]:
    try:
        results.append({"value": scalar(fn(**inputs)), "error": None})
    except Exception as exc:
        results.append({"value": None, "error": "%s: %s" % (type(exc).__name__, exc)})
print(json.dumps({"results": results}))
'''


def _sample_inputs(params: list[str], ranges: dict[str, Any] | None, trials: int) -> list[dict[str, float]]:
    rng = random.Random(0)
    bounds: dict[str, tuple[float, float]] = {}
    for p in params:
        lo, hi = DEFAULT_RANGE
        if ranges and p in ranges:
            r = ranges[p]
            if not (isinstance(r, (list, tuple)) and len(r) == 2):
                raise invalid_params(f"ranges[{p!r}] must be [lo, hi]")
            lo, hi = float(r[0]), float(r[1])
        bounds[p] = (lo, hi)
    return [{p: rng.uniform(*bounds[p]) for p in params} for _ in range(trials)]


def eq_check(
    root: Path,
    python: str,
    sandbox_kind: str,
    spec_factory: Callable[[], SandboxSpec],
    env: dict[str, str],
    path: str,
    function: str,
    trials: int | None = None,
    tol: float | None = None,
    ranges: dict[str, Any] | None = None,
) -> dict[str, Any]:
    if not isinstance(function, str) or not function:
        raise invalid_params("function must be a non-empty string")
    n = int(trials) if trials is not None else DEFAULT_TRIALS
    tolerance = float(tol) if tol is not None else DEFAULT_TOL
    if n <= 0:
        raise invalid_params("trials must be positive")

    def unavailable(reason: str) -> dict[str, Any]:
        return {
            "status": "unavailable",
            "trials": 0,
            "max_abs_err": None,
            "max_rel_err": None,
            "counterexample": None,
            "reason": reason,
        }

    found = extract_file(root, path, function)
    if not found:
        return unavailable(f"function {function!r} not found in {path}")
    eq = found[0]
    expr = eq.declared_expr if eq.declared_expr is not None else eq.expr
    if expr is None:
        if eq.declared_error:
            return unavailable(eq.declared_error)
        return unavailable(eq.reason or "no declared or derived equation")
    params = eq.params or []
    if "." in eq.qualname:
        return unavailable("methods are not supported by eq.check")
    file = resolve_in_root(root, path)
    tree = parse_file(file)
    node = next((f.node for f in iter_functions(tree, file, root) if f.qualname == eq.qualname), None)  # type: ignore[arg-type]
    if node is not None and (node.args.vararg or node.args.kwarg):
        return unavailable("functions with *args/**kwargs are not supported by eq.check")

    sp = _sym()
    symbols = [sp.Symbol(p) for p in params]
    try:
        fn = sp.lambdify(symbols, expr, modules=["math"])
    except Exception as exc:
        return unavailable(f"could not lambdify expression: {exc}")

    inputs = _sample_inputs(params, ranges, n)
    expected: list[float | None] = []
    for row in inputs:
        try:
            val = fn(**row)
            val = float(val)
            expected.append(val if math.isfinite(val) else None)
        except Exception:
            expected.append(None)
    if all(v is None for v in expected):
        return unavailable("the declared expression could not be evaluated on the sampled inputs")

    req = {
        "root": str(root),
        "dir": str(file.parent),
        "path": str(file),
        "stem": file.stem,
        "module": module_name(root, file) if _in_package(root, file) else None,
        "function": eq.qualname,
        "inputs": inputs,
    }
    spec = spec_factory()
    probe = wrap(sandbox_kind, spec, [])
    driver = rlimit_prologue(probe.rlimit_as, probe.rlimit_nofile) + CHECK_DRIVER
    cmd = wrap(sandbox_kind, spec, [python, "-I", "-c", driver])
    try:
        proc = subprocess.run(
            cmd.argv,
            input=json.dumps(req),
            capture_output=True,
            text=True,
            timeout=CHECK_TIMEOUT_S,
            cwd=str(root),
            env=env,
        )
    except subprocess.TimeoutExpired:
        return unavailable(f"evaluation exceeded {CHECK_TIMEOUT_S:g}s")
    except OSError as exc:
        return unavailable(f"could not start session python: {exc}")
    try:
        data = json.loads(proc.stdout.strip().splitlines()[-1])
    except (ValueError, IndexError):
        err = (proc.stderr or "").strip().splitlines()
        return unavailable("driver produced no result: " + (err[-1] if err else f"exit {proc.returncode}"))
    if "error" in data:
        return unavailable(f"could not evaluate {function}: {data['error']}")
    results = data.get("results", [])

    max_abs = 0.0
    max_rel = 0.0
    evaluated = 0
    counterexample = None
    first_error: str | None = None
    for row, exp_v, res in zip(inputs, expected, results):
        if res.get("error"):
            first_error = first_error or res["error"]
            continue
        act = res.get("value")
        if exp_v is None or act is None:
            continue
        evaluated += 1
        if not math.isfinite(act):
            abs_err = math.inf
        else:
            abs_err = abs(exp_v - act)
        max_abs = max(max_abs, abs_err)
        if exp_v != 0:
            max_rel = max(max_rel, abs_err / abs(exp_v))
        if abs_err > max(ABS_FLOOR, tolerance * abs(exp_v)) and counterexample is None:
            counterexample = {"inputs": row, "expected": exp_v, "actual": act}
    if evaluated == 0:
        return unavailable(f"could not evaluate {function}: {first_error or 'no finite results'}")
    return {
        "status": "mismatch" if counterexample else "agree",
        "trials": evaluated,
        "max_abs_err": _finite(max_abs),
        "max_rel_err": _finite(max_rel),
        "counterexample": counterexample,
        "reason": first_error if first_error and counterexample is None else None,
    }


def _finite(v: float) -> float | None:
    return v if math.isfinite(v) else None


def _in_package(root: Path, file: Path) -> bool:
    rel = file.resolve().relative_to(root.resolve())
    parts = rel.parts[:-1]
    if not parts:
        return False
    cur = root
    for part in parts:
        cur = cur / part
        if not (cur / "__init__.py").is_file():
            return False
    return True


__all__ = ["Unsupported", "derive_function", "eq_check", "eq_extract", "extract_file", "rel_to_root"]
