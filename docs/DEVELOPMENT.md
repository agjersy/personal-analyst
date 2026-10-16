# Development

Phase 0 is a headless build: the Rust core and CLI, the Python kernel, and the evaluation suite, with no desktop UI yet. See [ARCHITECTURE.md](ARCHITECTURE.md) for the design and [PROTOCOL.md](PROTOCOL.md) for the contract between components.

## Layout

```
Cargo.toml             Rust workspace
crates/protocol/       serde types for the kernel protocol, config, transcripts, model events, evals
crates/core/           sessions, agent loop, policy, providers, kernel client, targets, sync, checkpoints
crates/cli/            the `analyst` binary
python/kernel/         analyst_kernel (per-target kernel) and analyst_mpl (figure capture backend)
evals/tasks/           model-agnostic evaluation tasks with fixture repos
docs/                  vision, architecture, protocol, decisions
```

## Prerequisites

- Rust stable (1.80 or newer), `cargo`
- Python 3.11 or newer, `uv`, `ruff`
- `git`, `rg` (ripgrep)
- For sandboxed runs: `bwrap` (Linux) or macOS. Without either the kernel falls back to resource limits and reports `sandbox: "limits"`.
- For remote targets: OpenSSH client, and `rsync` on both ends (optional; a `tar` fallback is used otherwise)

## Setup

```sh
# Python kernel and its tests
cd python/kernel
uv sync            # creates python/kernel/.venv with sympy, ruff, pytest, numpy, matplotlib
uv run pytest -q
uv run ruff check .
cd ../..

# Rust workspace
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Running the CLI in development

The CLI resolves the tooling Python (the interpreter that runs the kernel) in this order: `ANALYST_TOOLING_PYTHON`, `tooling_python` in `~/.analyst/config.toml`, `python/kernel/.venv/bin/python` relative to the workspace (dev mode), then `python3` on `PATH`. After `uv sync` in `python/kernel`, dev mode works with no configuration.

```sh
cargo run -q -p analyst-cli -- init /tmp/demo        # new analysis repo with venv
cd /tmp/demo
analyst index                                        # scripts and library index
analyst check                                        # contract + lint
analyst run scripts/example.py -- --help
analyst eq analysis/models.py --check
analyst chat -m <provider>/<model> "..."             # needs a provider in ~/.analyst/config.toml
analyst eval run --model <provider>/<model> --tasks <workspace>/evals/tasks
```

Provider configuration lives in `~/.analyst/config.toml` (schema in PROTOCOL.md section 2.1). API keys are read from the environment variable named by `api_key_env`. Override the home directory with `--home` or `ANALYST_HOME`.

## Evaluation suite

Each task under `evals/tasks/<name>/` has a `task.toml` (prompt, mode, checks) and a `fixture/` directory that becomes a fresh git repo for the run. Checks inspect only the resulting repo, the kernel's reports, and the transcript; none reference a model. Results go to `evals/results/`.

## Conventions

- Rust: `cargo fmt`, clippy clean, no `unwrap()` on I/O paths in library code.
- Python: Ruff clean, stdlib plus SymPy and Ruff only in the kernel; `analyst_mpl` is stdlib plus Matplotlib only.
- The kernel never imports user code in its own process; evaluation of user functions happens in subprocesses using the session interpreter.
- Protocol changes bump `protocol_version` and are made on both sides in the same commit.
