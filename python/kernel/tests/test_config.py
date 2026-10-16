from pathlib import Path

from analyst_kernel.config import ProjectConfig, config_from_dict, load_config


def test_defaults_when_missing(tmp_path: Path):
    cfg = load_config(tmp_path)
    assert cfg == ProjectConfig()
    assert cfg.scripts == ["scripts/**/*.py"]
    assert cfg.library == "analysis"
    assert cfg.out_dir == "out"
    assert cfg.data_dirs == ["data"]
    assert cfg.timeout_s == 600
    assert cfg.max_helper_lines == 15
    assert cfg.near_duplicate_threshold == 0.80
    assert cfg.min_tokens_for_similarity == 30
    assert cfg.math_required == []
    assert "numpy" in cfg.numerical_modules and "statsmodels" in cfg.numerical_modules
    assert cfg.script_allowed_functions == ["main", "build_parser", "parse_args"]
    assert cfg.severity == {}
    assert cfg.policy["edits"] == "review" and cfg.policy["max_steps"] == 40


def test_fixture_config(repo: Path):
    cfg = load_config(repo)
    assert cfg.timeout_s == 120
    assert cfg.math_required == ["analysis/models.py"]
    assert cfg.severity == {"lib.near-duplicate": "warn", "doc.docstring": "error"}


def test_partial_and_invalid_values(tmp_path: Path):
    d = tmp_path / ".analyst"
    d.mkdir()
    (d / "project.toml").write_text(
        '[project]\nout_dir = "results/"\ntimeout_s = "soon"\n[contract]\nmax_helper_lines = 3\n'
        '[contract.severity]\n"cli.help-text" = "warn"\n"bogus" = "fatal"\n[targets.lab]\ndata_dirs = ["/data"]\n'
    )
    cfg = load_config(tmp_path)
    assert cfg.out_dir == "results"
    assert cfg.timeout_s == 600  # invalid value falls back to the default
    assert cfg.max_helper_lines == 3
    assert cfg.severity == {"cli.help-text": "warn"}
    assert cfg.targets == {"lab": {"data_dirs": ["/data"]}}


def test_broken_toml_means_defaults(tmp_path: Path):
    d = tmp_path / ".analyst"
    d.mkdir()
    (d / "project.toml").write_text("[project\n")
    assert load_config(tmp_path) == ProjectConfig()
    assert config_from_dict({}) == ProjectConfig()
