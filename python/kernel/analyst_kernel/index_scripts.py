"""``scripts.index``: argparse definitions by AST only (PROTOCOL.md 1.3)."""

from __future__ import annotations

import ast
from pathlib import Path
from typing import Any

from .astutil import dotted, is_main_guard, literal_or_none, parse_file, script_files

_TYPE_NAMES = {
    "int": "int",
    "float": "float",
    "str": "str",
    "Path": "Path",
    "pathlib.Path": "Path",
    "builtins.int": "int",
    "builtins.float": "float",
    "builtins.str": "str",
}

_STORE_BOOL_DEFAULTS = {"store_true": False, "store_false": True}


def is_argparser_call(node: ast.AST) -> bool:
    if not isinstance(node, ast.Call):
        return False
    name = dotted(node.func)
    return name is not None and (name == "ArgumentParser" or name.endswith(".ArgumentParser"))


def is_add_argument_call(node: ast.AST) -> bool:
    return (
        isinstance(node, ast.Call)
        and isinstance(node.func, ast.Attribute)
        and node.func.attr == "add_argument"
    )


def kw(call: ast.Call, name: str) -> ast.expr | None:
    for k in call.keywords:
        if k.arg == name:
            return k.value
    return None


def _string(node: ast.expr | None) -> str | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    if isinstance(node, ast.JoinedStr):
        return "".join(
            v.value for v in node.values if isinstance(v, ast.Constant) and isinstance(v.value, str)
        )
    return None


def _default_value(node: ast.expr | None, action: str | None) -> Any:
    if node is None:
        return _STORE_BOOL_DEFAULTS.get(action or "")
    lit = literal_or_none(node)
    if lit is not None or (isinstance(node, ast.Constant) and node.value is None):
        return lit
    # Path("data/x.csv") -> "data/x.csv"
    if isinstance(node, ast.Call) and len(node.args) == 1:
        callee = dotted(node.func)
        if callee in ("Path", "pathlib.Path"):
            return literal_or_none(node.args[0])
    return None


def _type_name(node: ast.expr | None) -> str | None:
    if node is None:
        return None
    name = dotted(node)
    if name is None:
        return None
    return _TYPE_NAMES.get(name)


def infer_dest(flags: list[str], dest_kw: str | None) -> str:
    if dest_kw:
        return dest_kw
    longs = [f for f in flags if f.startswith("--")]
    if longs:
        return longs[0].lstrip("-").replace("-", "_")
    if flags and not flags[0].startswith("-"):
        return flags[0].replace("-", "_")
    if flags:
        return flags[0].lstrip("-").replace("-", "_")
    return ""


def option_from_call(call: ast.Call) -> dict[str, Any]:
    flags = [s for s in (_string(a) for a in call.args) if s is not None]
    action_node = kw(call, "action")
    action = _string(action_node)
    if action is None and action_node is not None:
        action = ast.unparse(action_node)
    choices_node = kw(call, "choices")
    choices = literal_or_none(choices_node)
    if isinstance(choices, (tuple, set, frozenset)):
        choices = list(choices)
    elif not isinstance(choices, list):
        choices = None
    required_node = kw(call, "required")
    required = bool(literal_or_none(required_node)) if required_node is not None else False
    if flags and not flags[0].startswith("-"):
        required = True  # positional arguments are always required
    return {
        "flags": flags,
        "dest": infer_dest(flags, _string(kw(call, "dest"))),
        "type": _type_name(kw(call, "type")),
        "default": _default_value(kw(call, "default"), action),
        "help": _string(kw(call, "help")),
        "required": required,
        "choices": choices,
        "action": action,
        "line": call.lineno,
        "has_help": kw(call, "help") is not None,
    }


def index_script(path: Path, root: Path) -> dict[str, Any]:
    rel = path.resolve().relative_to(root.resolve()).as_posix()
    info: dict[str, Any] = {
        "path": rel,
        "name": path.stem,
        "description": None,
        "has_main": False,
        "has_main_guard": False,
        "options": [],
        "parser_calls": 0,
        "parse_error": None,
    }
    tree = parse_file(path)
    if tree is None:
        info["parse_error"] = "could not parse"
        return info
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == "main":
            info["has_main"] = True
        if is_main_guard(node):
            info["has_main_guard"] = True
    for node in ast.walk(tree):
        if is_argparser_call(node):
            info["parser_calls"] += 1
            desc = _string(kw(node, "description"))
            if desc is not None and info["description"] is None:
                info["description"] = desc
        elif is_add_argument_call(node):
            info["options"].append(option_from_call(node))
    return info


def public_option(opt: dict[str, Any]) -> dict[str, Any]:
    keys = ("flags", "dest", "type", "default", "help", "required", "choices", "action")
    return {k: opt[k] for k in keys}


def public_script(info: dict[str, Any]) -> dict[str, Any]:
    return {
        "path": info["path"],
        "name": info["name"],
        "description": info["description"],
        "has_main": info["has_main"],
        "has_main_guard": info["has_main_guard"],
        "options": [public_option(o) for o in info["options"]],
    }


def index_scripts(root: Path, globs: list[str]) -> list[dict[str, Any]]:
    return [index_script(p, root) for p in script_files(root, globs)]
