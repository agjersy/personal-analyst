# Decisions and Open Questions

*Status: phase 0 decisions closed (2026-10-16). Remaining items are deliberately left open until phase 0 is complete. Companion documents: [VISION.md](VISION.md), [ARCHITECTURE.md](ARCHITECTURE.md).*

## 1. Decision record

Every question that blocked phase 0 has been answered. Numbering is preserved from the original list so earlier discussion still lines up.

| Item | Decision | Where it landed |
|---|---|---|
| OQ-1 Desktop shell | Tauri 2 with a Rust core and a React renderer, for cross-platform support. | ARCHITECTURE.md section 3 |
| OQ-2 Platforms and remote hosts | macOS and Linux are prioritized for both the desktop app and remote targets through phase 3. Windows is deferred to phase 4. Remote hosts are reached as login-node sessions. | ARCHITECTURE.md sections 5.5, 5.6, 9 |
| OQ-3 Sandbox and network | OS-level isolation (`bubblewrap` on Linux, `sandbox-exec` on macOS), network denied by default with per-run grants and host allowlists, container backend as an opt-in, resource-limits-only fallback badged in the UI where isolation is unavailable. | ARCHITECTURE.md section 5.6 |
| OQ-4 Edit gating | `review` is the default mode, review is per hunk, protected paths and protected-region markers are in the first version. Scope declaration applies in every mode. | ARCHITECTURE.md sections 5.7, 5.8 |
| OQ-6 Script contract | Command-line options on every script and shared logic generalized into the library are hard rules with automated checks. The remaining proposed rules ship as defaults with per-repo severities. | ARCHITECTURE.md section 5.13 |
| OQ-7 Environment tooling | `uv` only. No conda-only packages are planned. | ARCHITECTURE.md section 5.2 |
| OQ-12 Model roster | None. The design and the evaluation suite are model agnostic. A capability baseline (native tool calling, streaming, at least 32k context) defines a performant model; capabilities are probed at first use. The text-protocol fallback ships as a flagged, degraded tier the evaluation suite does not certify. | ARCHITECTURE.md sections 5.7, 5.10, 10 |
| OQ-13 Where analysis runs | Remote execution over SSH is a first-version feature designed in from phase 0. | ARCHITECTURE.md section 5.5 |
| OQ-14 Data location | Data is never synced and stays where it is generated. Only rendered products (figures, logs, run metadata, equation results) are transmitted back to the UI. Data outputs are inspected in place through bounded previews. | ARCHITECTURE.md section 5.5, VISION.md principle 7 |
| OQ-17 Scheduler integration | Login-node sessions for now, behind a launcher interface so a scheduler launcher (Slurm and similar) can be added later without changing the kernel protocol or agent loop. | ARCHITECTURE.md section 5.5 |
| OQ-18 Remote repo model | The repo lives locally and is mirrored to the remote host before runs. The remote-primary alternative is rejected. | ARCHITECTURE.md section 5.5 |

## 2. Open for phase 1 and later

These are intentionally parked until phase 0 (the headless spike) is complete, because its results will inform them. None blocks phase 0.

### OQ-5. Equation lens: how much derivation, how much declaration — Blocks phase 2

**Options.** (a) Derive from the AST only; show nothing for functions that cannot be derived. (b) Declared `Math:` docstrings with numeric verification, plus derivation where possible. (c) LLM-generated LaTeX only.

**Recommendation.** (b). Derivation is ground truth but covers only pure expression functions. Declared-and-checked covers algorithms and gives a real verification signal. (c) alone reproduces the trust problem the product exists to solve.

**Decide after phase 0.** Phase 0 will measure derivation coverage on a real module. Then: is a `Math:` docstring section in SymPy syntax an acceptable convention to require? Would you rather write LaTeX directly and accept that it cannot be verified numerically?

### OQ-8. Session scope — Blocks phase 1

The spec defines a session as one repo plus one target plus one environment plus conversations.

**Decide.** Do you ever need one session to span multiple repos (for example a shared analysis library plus a project repo)? Is one environment per repo the right default? This also decides whether the library package in the contract is per repo or shared.

### OQ-9. Notebooks — Informs phase 1

The vision excludes Jupyter as a first-class format because the workflow is script-based.

**Decide.** Confirm exclusion, or ask for read-only rendering of existing `.ipynb` files so legacy work is viewable.

### OQ-10. LaTeX toolchain — Blocks phase 3

**Options.** (a) Tectonic, fetched on first use, self-contained. (b) Use the system TeX Live or MacTeX if present. (c) Both, system first.

**Recommendation.** (c).

**Decide.** Do you have a TeX installation you want used?

### OQ-11. Figures: static only or interactive — Informs phase 1

**Recommendation.** Matplotlib static capture (PNG and SVG) in the first version. Interactive Plotly or Bokeh HTML in a webview tab later.

**Decide.** How often do you use interactive plots today? If routinely, the webview tab should be in phase 1.

### OQ-15. Keybindings and editor feel — Informs phase 1

**Decide.** Zed defaults, VS Code defaults, or Vim mode from the first version? Any Zed features you use daily beyond the agent panel (multibuffers, outline) that should be in scope?

### OQ-16. Name and licensing — Informs nothing technical

**Decide.** Working name for the product, and whether it will be open source (affects library choices with copyleft licenses).

## 3. Questions phase 0 is expected to raise

Not decisions yet; things the spike should produce evidence on before the phase 1 questions are answered.

- Equation derivation coverage on an existing analysis module (feeds OQ-5).
- Sync and reconnect latency on your actual remote hosts, and whether `bubblewrap` is available there.
- Near-duplicate detection precision at the default 80 percent threshold on an existing repo: too many false positives means the threshold or the normalization needs tuning before the agent is blocked on it.
- Which capability-baseline models from the providers you configure pass the evaluation suite, and where the loop's prompts need tuning.
