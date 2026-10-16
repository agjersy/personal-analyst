# Personal Analyst

A desktop workbench for scientific, technical, and numerical analysis: an AI agent writes reusable Python analysis tools under your supervision, and the application makes verifying that work (code, math, figures) fast.

Model agnostic (open-weight models via Ollama or any OpenAI-compatible server, frontier models via API keys), local first, sandboxed execution, with runs on this machine or on a remote Linux host over SSH.

## Status

**Phase 0 (headless core) is built.** There is no desktop UI yet. What exists:

- `analyst` CLI: `init`, `index`, `check`, `run`, `eq`, `env`, `target`, `chat`, `eval`
- Rust core: policy engine with review-gated edits and scope declaration, OpenAI-compatible and Anthropic streaming adapters with capability probing, kernel client over local stdio or SSH, repo sync and remote bootstrap, git checkpoints, agent loop with plan/build/run modes
- Python kernel: sandboxed detached runs with figure capture, script and library indexes, equation extraction (derived from code, or declared and numerically checked), the script contract checker with duplicate-logic detection, Ruff integration
- Evaluation suite: four model-agnostic tasks with declarative graders and a scripted model server that drives the whole stack in CI

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) to build and run it.

## Documents

- [docs/VISION.md](docs/VISION.md): product vision, principles, core experiences, roadmap
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): application design and architecture
- [docs/PROTOCOL.md](docs/PROTOCOL.md): the contract between the kernel, the core, and the evaluation suite
- [docs/OPEN-QUESTIONS.md](docs/OPEN-QUESTIONS.md): decision record and the questions left open for later phases
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md): layout, setup, running the CLI and the evaluation suite
