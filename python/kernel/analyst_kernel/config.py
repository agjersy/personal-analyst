"""Project configuration: ``<root>/.analyst/project.toml`` with the defaults of PROTOCOL.md 2.2."""

from __future__ import annotations

import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

DEFAULT_SCRIPTS = ["scripts/**/*.py"]
DEFAULT_LIBRARY = "analysis"
DEFAULT_OUT_DIR = "out"
DEFAULT_DATA_DIRS = ["data"]
DEFAULT_TIMEOUT_S = 600

DEFAULT_MAX_HELPER_LINES = 15
DEFAULT_NEAR_DUPLICATE_THRESHOLD = 0.80
DEFAULT_MIN_TOKENS_FOR_SIMILARITY = 30
DEFAULT_MATH_REQUIRED: list[str] = []
DEFAULT_NUMERICAL_MODULES = [
    "numpy",
    "scipy",
    "pandas",
    "sklearn",
    "jax",
    "torch",
    "xarray",
    "statsmodels",
]
DEFAULT_SCRIPT_ALLOWED_FUNCTIONS = ["main", "build_parser", "parse_args"]

DEFAULT_POLICY: dict[str, Any] = {
    "edits": "review",
    "runs": "auto",
    "packages": "ask",
    "shell": "ask",
    "network": "deny",
    "git": "ask",
    "protected": [],
    "max_steps": 40,
}


@dataclass
class ProjectConfig:
    scripts: list[str] = field(default_factory=lambda: list(DEFAULT_SCRIPTS))
    library: str = DEFAULT_LIBRARY
    out_dir: str = DEFAULT_OUT_DIR
    data_dirs: list[str] = field(default_factory=lambda: list(DEFAULT_DATA_DIRS))
    timeout_s: float = DEFAULT_TIMEOUT_S

    max_helper_lines: int = DEFAULT_MAX_HELPER_LINES
    near_duplicate_threshold: float = DEFAULT_NEAR_DUPLICATE_THRESHOLD
    min_tokens_for_similarity: int = DEFAULT_MIN_TOKENS_FOR_SIMILARITY
    math_required: list[str] = field(default_factory=lambda: list(DEFAULT_MATH_REQUIRED))
    numerical_modules: list[str] = field(default_factory=lambda: list(DEFAULT_NUMERICAL_MODULES))
    script_allowed_functions: list[str] = field(
        default_factory=lambda: list(DEFAULT_SCRIPT_ALLOWED_FUNCTIONS)
    )
    severity: dict[str, str] = field(default_factory=dict)

    policy: dict[str, Any] = field(default_factory=lambda: dict(DEFAULT_POLICY))
    targets: dict[str, dict[str, Any]] = field(default_factory=dict)

    raw: dict[str, Any] = field(default_factory=dict)

    def to_json(self) -> dict[str, Any]:
        return {
            "project": {
                "scripts": self.scripts,
                "library": self.library,
                "out_dir": self.out_dir,
                "data_dirs": self.data_dirs,
                "timeout_s": self.timeout_s,
            },
            "contract": {
                "max_helper_lines": self.max_helper_lines,
                "near_duplicate_threshold": self.near_duplicate_threshold,
                "min_tokens_for_similarity": self.min_tokens_for_similarity,
                "math_required": self.math_required,
                "numerical_modules": self.numerical_modules,
                "script_allowed_functions": self.script_allowed_functions,
                "severity": self.severity,
            },
            "policy": self.policy,
            "targets": self.targets,
        }


def _list_of_str(value: Any, default: list[str]) -> list[str]:
    if isinstance(value, list) and all(isinstance(v, str) for v in value):
        return list(value)
    return list(default)


def config_from_dict(data: dict[str, Any]) -> ProjectConfig:
    project = data.get("project", {}) if isinstance(data.get("project"), dict) else {}
    contract = data.get("contract", {}) if isinstance(data.get("contract"), dict) else {}
    severity_raw = contract.get("severity", {})
    severity = {
        str(k): str(v)
        for k, v in (severity_raw.items() if isinstance(severity_raw, dict) else [])
        if str(v) in ("error", "warn")
    }
    policy = dict(DEFAULT_POLICY)
    if isinstance(data.get("policy"), dict):
        policy.update(data["policy"])
    targets = data.get("targets", {}) if isinstance(data.get("targets"), dict) else {}

    def num(value: Any, default: float) -> float:
        return float(value) if isinstance(value, (int, float)) and not isinstance(value, bool) else default

    return ProjectConfig(
        scripts=_list_of_str(project.get("scripts"), DEFAULT_SCRIPTS),
        library=str(project.get("library", DEFAULT_LIBRARY)),
        out_dir=str(project.get("out_dir", DEFAULT_OUT_DIR)).strip("/") or DEFAULT_OUT_DIR,
        data_dirs=_list_of_str(project.get("data_dirs"), DEFAULT_DATA_DIRS),
        timeout_s=num(project.get("timeout_s"), DEFAULT_TIMEOUT_S),
        max_helper_lines=int(num(contract.get("max_helper_lines"), DEFAULT_MAX_HELPER_LINES)),
        near_duplicate_threshold=num(
            contract.get("near_duplicate_threshold"), DEFAULT_NEAR_DUPLICATE_THRESHOLD
        ),
        min_tokens_for_similarity=int(
            num(contract.get("min_tokens_for_similarity"), DEFAULT_MIN_TOKENS_FOR_SIMILARITY)
        ),
        math_required=_list_of_str(contract.get("math_required"), DEFAULT_MATH_REQUIRED),
        numerical_modules=_list_of_str(contract.get("numerical_modules"), DEFAULT_NUMERICAL_MODULES),
        script_allowed_functions=_list_of_str(
            contract.get("script_allowed_functions"), DEFAULT_SCRIPT_ALLOWED_FUNCTIONS
        ),
        severity=severity,
        policy=policy,
        targets={str(k): dict(v) for k, v in targets.items() if isinstance(v, dict)},
        raw=data,
    )


def load_config(root: Path) -> ProjectConfig:
    """Load ``<root>/.analyst/project.toml``; a missing file means all defaults."""
    path = Path(root) / ".analyst" / "project.toml"
    if not path.is_file():
        return ProjectConfig()
    with path.open("rb") as fh:
        try:
            data = tomllib.load(fh)
        except tomllib.TOMLDecodeError:
            return ProjectConfig()
    return config_from_dict(data)
