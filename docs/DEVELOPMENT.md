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

Each task under `evals/tasks/<name>/` has a `task.toml` (prompt, mode, checks) and a `fixture/` directory that becomes a fresh git repo for the run (with `uv sync` for its `pyproject.toml`). Checks inspect only the resulting repo, the kernel's reports, and the transcript; none reference a model. Results go to `evals/results/`.

```sh
analyst eval run --model <provider>/<model> --tasks evals/tasks --auto            # all tasks
analyst eval run --model <provider>/<model> --tasks evals/tasks --task parameter-sweep --auto
```

### Exercising the loop without a model

`evals/fake_model_server.py` is a scripted OpenAI-compatible server (stdlib only). It answers the capability probe, plays the `parameter-sweep` task (three runs, no edits) and the `reuse-library-function` task (scope declaration, file creation, run), and declines everything else. It drives the real provider adapter, agent loop, policy engine, tools, kernel, graders, and scorecard, so it is the integration test for the whole headless stack and runs in CI.

```sh
python3 evals/fake_model_server.py --port 8765 &
mkdir -p /tmp/analyst-home && cat > /tmp/analyst-home/config.toml <<'EOF'
[defaults]
model = "fake/scripted"
[[providers]]
name = "fake"
kind = "openai-compatible"
base_url = "http://127.0.0.1:8765/v1"
EOF
ANALYST_HOME=/tmp/analyst-home analyst eval run --model fake/scripted --tasks evals/tasks --task parameter-sweep --auto
ANALYST_HOME=/tmp/analyst-home analyst eval run --model fake/scripted --tasks evals/tasks --task reuse-library-function --auto
```

Both must report PASS. The other two tasks fail by design against the scripted server; they need a real model.

## Troubleshooting

**`sandbox-exec: execvp() of '.../.venv/bin/python' failed: Operation not permitted` (macOS)**, or a run that dies immediately under `bwrap` on Linux. The sandbox hides the home directory, and the venv's `python` is a symlink to a base interpreter that `uv` keeps under `~/.local/share/uv/python`. The kernel resolves that symlink (and `home =` in the venv's `pyvenv.cfg`) and allows the base installation read-only, and it lists every path in both its given and canonical form (`/tmp` is `/private/tmp` on macOS). If a run is still denied:

The macOS sandbox also checks `file-read-metadata` on every directory along a path during lookup, so the profile grants metadata (stat only) on the chain of ancestors between your home directory and each allowed subtree; without that, an allow rule for a subtree under home has no effect. If a run is still denied:

1. Run it with `--sandbox limits` to confirm the script itself works: `analyst --sandbox limits run scripts/x.py`. This applies resource limits only and is badged as such.
2. Print and test the exact wrapper for your repo and interpreter, outside the CLI:
   ```sh
   python/kernel/.venv/bin/python -m analyst_kernel.sandbox --python /path/to/repo/.venv/bin/python --root /path/to/repo --run
   ```
   It prints the profile (or `bwrap` argv) and then executes a one-line Python command under it. On macOS, denials are also logged by the system: `log stream --style compact --predicate 'sender == "Sandbox"'` in another terminal shows the operation and path that was refused.
3. Add that path to `data_dirs` in `.analyst/project.toml` if it is data, or report it if it is part of the interpreter or a system location.

**Matplotlib cannot find a font cache or config.** Runs set `MPLCONFIGDIR` inside the run directory, which is writable; the user's `~/.matplotlib` is intentionally hidden.

## Conventions

- Rust: `cargo fmt`, clippy clean, no `unwrap()` on I/O paths in library code.
- Python: Ruff clean, stdlib plus SymPy and Ruff only in the kernel; `analyst_mpl` is stdlib plus Matplotlib only.
- The kernel never imports user code in its own process; evaluation of user functions happens in subprocesses using the session interpreter.
- Protocol changes bump `protocol_version` and are made on both sides in the same commit.
