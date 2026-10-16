"""Docstring section parsing for ``Math:`` and ``LaTeX:`` (PROTOCOL.md section 3). No SymPy here."""

from __future__ import annotations

import ast
import re
from dataclasses import dataclass, field

_SECTION_RE = re.compile(r"^(Math|LaTeX):(?:\s+(.*))?\s*$", re.IGNORECASE)


@dataclass
class MathSection:
    kind: str  # "math" | "latex"
    header_text: str | None
    lines: list[str] = field(default_factory=list)  # stripped indented lines

    @property
    def equations(self) -> list[tuple[str, str]]:
        """Split ``lhs = expr`` lines; lines without ``=`` become ``("", line)``."""
        out: list[tuple[str, str]] = []
        for line in self.lines:
            if "=" in line and "==" not in line:
                lhs, rhs = line.split("=", 1)
                out.append((lhs.strip(), rhs.strip()))
            else:
                out.append(("", line.strip()))
        return out


def _indent(line: str) -> int:
    return len(line) - len(line.lstrip(" \t"))


def parse_sections(doc: str | None) -> list[MathSection]:
    """Return every Math:/LaTeX: section in a docstring, in order."""
    if not doc:
        return []
    lines = doc.expandtabs(4).splitlines()
    sections: list[MathSection] = []
    i = 0
    while i < len(lines):
        m = _SECTION_RE.match(lines[i].strip())
        if m and _indent(lines[i]) <= _indent_of_body(lines):
            kind = m.group(1).lower()
            header = m.group(2).strip() if m.group(2) else None
            base = _indent(lines[i])
            body: list[str] = []
            j = i + 1
            while j < len(lines):
                raw = lines[j]
                if not raw.strip():
                    # blank line inside a section is allowed only if more indented lines follow
                    k = j + 1
                    while k < len(lines) and not lines[k].strip():
                        k += 1
                    if k < len(lines) and _indent(lines[k]) > base:
                        j = k
                        continue
                    break
                if _indent(raw) <= base:
                    break
                body.append(raw.strip())
                j += 1
            sections.append(MathSection(kind=kind, header_text=header, lines=body))
            i = j
            continue
        i += 1
    return sections


def _indent_of_body(lines: list[str]) -> int:
    """Indentation of the docstring body (minimum indent of non-blank lines after the first)."""
    indents = [_indent(ln) for ln in lines[1:] if ln.strip()]
    return min(indents) if indents else 0


def math_section(doc: str | None) -> MathSection | None:
    for s in parse_sections(doc):
        if s.kind == "math" and s.lines:
            return s
    return None


def latex_section(doc: str | None) -> MathSection | None:
    for s in parse_sections(doc):
        if s.kind == "latex" and s.lines:
            return s
    return None


def has_math(doc: str | None) -> bool:
    return math_section(doc) is not None


def doc_summary(doc: str | None) -> str | None:
    """First paragraph of a docstring, joined on one line."""
    if not doc:
        return None
    para: list[str] = []
    for line in doc.strip().splitlines():
        if not line.strip():
            if para:
                break
            continue
        if _SECTION_RE.match(line.strip()):
            break
        para.append(line.strip())
    return " ".join(para) if para else None


def function_docstring(node: ast.AST) -> str | None:
    try:
        return ast.get_docstring(node, clean=True)
    except TypeError:
        return None
