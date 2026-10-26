# Decisions and Open Questions

*Status: phase 0 built and reviewed; phase 1 decisions closed (2026-10-26). Remaining items are for phase 2 and later. Companion documents: [VISION.md](VISION.md), [ARCHITECTURE.md](ARCHITECTURE.md).*

## 1. Decision record

Every question that blocked phase 0 has been answered. Numbering is preserved from the original list so earlier discussion still lines up.

| Item | Decision | Where it landed |
|---|---|---|
| OQ-1 Desktop shell | Tauri 2 as a host-native desktop application for macOS and Linux (native windows, menus, installers; no browser, no local server; interface drawn by the OS webview component). | ARCHITECTURE.md section 3 |
| OQ-2 Platforms and remote hosts | macOS and Linux are prioritized for both the desktop app and remote targets through phase 3. Windows is deferred to phase 4. Remote hosts are reached as login-node sessions. | ARCHITECTURE.md sections 5.5, 5.6, 9 |
| OQ-3 Sandbox and network | OS-level isolation (`bubblewrap` on Linux, `sandbox-exec` on macOS), network denied by default with per-run grants and host allowlists, container backend as an opt-in, resource-limits-only fallback badged in the UI where isolation is unavailable. | ARCHITECTURE.md section 5.6 |
| OQ-4 Edit gating | `review` is the default mode, review is per hunk, protected paths and protected-region markers are in the first version. Scope declaration applies in every mode. | ARCHITECTURE.md sections 5.7, 5.8 |
| OQ-6 Script contract | Command-line options on every script and shared logic generalized into the library are hard rules with automated checks. The remaining proposed rules ship as defaults with per-repo severities. | ARCHITECTURE.md section 5.13 |
| OQ-7 Environment tooling | `uv` only. No conda-only packages are planned. | ARCHITECTURE.md section 5.2 |
| OQ-12 Model roster | None. The design and the evaluation suite are model agnostic. A capability baseline (native tool calling, streaming, at least 32k context) defines a performant model; capabilities are probed at first use. The text-protocol fallback ships as a flagged, degraded tier the evaluation suite does not certify. | ARCHITECTURE.md sections 5.7, 5.10, 10 |
| OQ-13 Where analysis runs | Remote execution over SSH is a first-version feature designed in from phase 0. | ARCHITECTURE.md section 5.5 |
| OQ-14 Data location | Data is never synced and stays where it is generated. Only rendered products (figures, logs, run metadata, equation results) are transmitted back to the UI. Data outputs are inspected in place through bounded previews. | ARCHITECTURE.md section 5.5, VISION.md principle 7 |
| OQ-17 Remote runner and scheduler | Default is fully local. The default remote runner is a native environment on a remote Linux machine (Ubuntu or Fedora) over SSH; a cluster login node is the same target kind. Slurm scheduler integration is a planned later extension behind the launcher interface. | ARCHITECTURE.md section 5.5 |
| OQ-18 Remote repo model | The repo lives locally and is mirrored to the remote host before runs. The remote-primary alternative is rejected. | ARCHITECTURE.md section 5.5 |
| OQ-8 Session scope | One repo per session: one repo, one target, one venv, several conversations. A library shared by several repos is installed into each repo's venv as a package dependency, so the contract's library rule still applies per repo. | ARCHITECTURE.md sections 4, 5.1 |
| OQ-9 Notebooks | Excluded entirely from the first desktop version. | VISION.md section 9 |
| OQ-11 Figures | Static Matplotlib capture (PNG and SVG) in phase 1. Interactive Plotly or Bokeh output is a later phase. | ARCHITECTURE.md section 5.12 |
| OQ-15 Editor feel | Zed's keymap and command palette conventions. Vim mode deferred to phase 4. | ARCHITECTURE.md section 6 |
| OQ-16 Name and licensing | The product keeps the name "Personal Analyst": binary `analyst`, config in `~/.analyst`, bundle identifier derived from it. The repository stays private for now with the license decided later; only permissively licensed dependencies (no GPL or AGPL) so both options stay open. | this record |

## 2. Open for phase 2 and later

Nothing below blocks phase 1.

### OQ-5. Equation lens: how much derivation, how much declaration — Blocks phase 2

**Options.** (a) Derive from the AST only; show nothing for functions that cannot be derived. (b) Declared `Math:` docstrings with numeric verification, plus derivation where possible. (c) LLM-generated LaTeX only.

**Recommendation.** (b). Derivation is ground truth but covers only pure expression functions. Declared-and-checked covers algorithms and gives a real verification signal. (c) alone reproduces the trust problem the product exists to solve.

**Decide after phase 0.** Phase 0 will measure derivation coverage on a real module. Then: is a `Math:` docstring section in SymPy syntax an acceptable convention to require? Would you rather write LaTeX directly and accept that it cannot be verified numerically?

### OQ-10. LaTeX toolchain — Blocks phase 3

**Options.** (a) Tectonic, fetched on first use, self-contained. (b) Use the system TeX Live or MacTeX if present. (c) Both, system first.

**Recommendation.** (c).

**Decide.** Do you have a TeX installation you want used?

## 3. Evidence to gather before phase 2

Not decisions; things the phase 1 workbench should produce evidence on before OQ-5 and OQ-10 are answered.

- Equation derivation coverage on an existing analysis module (feeds OQ-5).
- Sync and reconnect latency on your actual remote hosts, and whether `bubblewrap` is available there.
- Near-duplicate detection precision at the default 80 percent threshold on an existing repo: too many false positives means the threshold or the normalization needs tuning before the agent is blocked on it.
- Which capability-baseline models from the providers you configure pass the evaluation suite, and where the loop's prompts need tuning.
