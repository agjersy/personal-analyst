# Open Questions Before the Build

*Status: awaiting decisions. Each item names the phase that is blocked until it is answered. Companion documents: [VISION.md](VISION.md), [ARCHITECTURE.md](ARCHITECTURE.md).*

Items marked **Blocks phase 0** need an answer before the headless spike starts. The rest can be decided while phase 0 is underway.

## OQ-1. Desktop shell: Electron or Tauri — Blocks phase 1

**Options.** (a) Electron with a TypeScript core. (b) Tauri 2 with a Rust core and a web renderer. (c) Native with Zed's GPUI.

**Recommendation.** (a). The strongest libraries for the parts the shell owns (editor, terminal, docking, MCP, model clients, PDF, math rendering) are JavaScript, and a single language keeps the project tractable for one builder. (b) is viable if idle memory and binary size matter to you more than ecosystem; it adds a second language and leaves Pyright and similar Node tools needing a sidecar anyway. (c) is not supported for third-party apps.

**Decide.** Is the Electron footprint acceptable, or do you want to pay for Rust up front?

## OQ-2. Primary operating system — Blocks phase 0

The sandbox implementation, the terminal, and the installer differ per platform. The spec assumes macOS and Linux first and Windows later without a sandbox.

**Decide.** Which OS do you work on daily, and is Windows support needed at all?

## OQ-3. Sandbox strength and network policy — Blocks phase 0

**Options.** (a) OS-level isolation (`bubblewrap`, `sandbox-exec`) with path scoping and network denied by default. (b) Container backend (Podman or Docker) for every run. (c) Path scoping only, no isolation.

**Recommendation.** (a) as the default with (b) as an opt-in backend. (c) is not enough once the agent can run arbitrary scripts.

**Decide.** Do your analysis scripts need network access routinely (downloading datasets, calling services)? If so the default should be "ask" rather than "deny", and the allowlist model matters more.

## OQ-4. Edit gating default — Blocks phase 0

**Options.** (a) `review`: pause on every proposal. (b) `auto-new-files`. (c) `auto` with scope declaration and checkpoints as the safety net.

**Recommendation.** (a) by default, because unwanted edits are a stated pain point, with a one-click session toggle to (b) or (c) when you trust a task. Scope declaration and protected paths apply in every mode.

**Decide.** Is per-hunk review the right granularity, or do you want per-file accept and reject only? Should protected-region comment markers exist in the first version?

## OQ-5. Equation lens: how much derivation, how much declaration — Blocks phase 2, informs phase 0

**Options.** (a) Derive from the AST only; show nothing for functions that cannot be derived. (b) Declared `Math:` docstrings with numeric verification, plus derivation where possible. (c) LLM-generated LaTeX only.

**Recommendation.** (b). Derivation is ground truth but covers only pure expression functions. Declared-and-checked covers algorithms and gives a real verification signal. (c) alone reproduces the trust problem you are trying to solve.

**Decide.** Is a `Math:` docstring section in SymPy syntax an acceptable convention to require in your repos? Would you rather write LaTeX directly and accept that it cannot be verified numerically? Which of your existing modules would be the first test case for derivation coverage?

## OQ-6. Script contract details — Blocks phase 0

The spec proposes a default contract (ARCHITECTURE.md section 5.12). It is a guess at the rules you enforce by hand.

**Decide.** Please mark up that list: which rules are right, which are missing (naming, logging, units, data loading conventions, testing), and which should be warnings versus hard failures.

## OQ-7. Environment tooling — Blocks phase 0

**Recommendation.** `uv` only. It is fast, manages Python versions, and produces lockfiles.

**Decide.** Do any of your analyses depend on conda-only packages (GPU stacks, geospatial, some scientific binaries)? If yes, a conda or pixi backend moves up the roadmap.

## OQ-8. Session scope — Blocks phase 1

The spec defines a session as one repo plus one environment plus conversations.

**Decide.** Do you ever need one session to span multiple repos (for example a shared analysis library plus a project repo)? Is one environment per repo the right default, or do you reuse environments across repos?

## OQ-9. Notebooks — Informs phase 1

The vision excludes Jupyter as a first-class format because your workflow is script-based.

**Decide.** Confirm exclusion, or ask for read-only rendering of existing `.ipynb` files so legacy work is viewable.

## OQ-10. LaTeX toolchain — Blocks phase 3

**Options.** (a) Tectonic, fetched on first use, self-contained. (b) Use the system TeX Live or MacTeX if present. (c) Both, system first.

**Recommendation.** (c). Tectonic guarantees a working compile on a fresh machine; your existing TeX installation may have packages and fonts you rely on.

**Decide.** Do you have a TeX installation you want used?

## OQ-11. Figures: static only or interactive — Informs phase 1

**Recommendation.** Matplotlib static capture (PNG and SVG) in the first version. Interactive Plotly or Bokeh HTML in a webview tab later.

**Decide.** How often do you use interactive plots today? If routinely, the webview tab should be in phase 1.

## OQ-12. Model roster for first-class support — Blocks phase 0

Tool-calling reliability varies widely across open-weight models and quantizations. Phase 0's evaluation suite needs a concrete list.

**Decide.** Which models do you run on Ollama today (names, sizes, quantizations)? Which frontier providers do you hold keys for? Is a text-based tool protocol worth supporting for models with weak native tool calling, or should unsupported models simply be flagged?

## OQ-13. Where analysis runs — Blocks phase 0 design of the kernel

Your models are remote. The spec assumes analysis runs locally.

**Decide.** Do you also need runs to execute on a remote machine (a GPU box, a cluster login node) over SSH, with figures streamed back? If yes, the kernel protocol should be designed transport-agnostic from the start, which is cheap now and expensive later.

## OQ-14. Data location and size — Informs phase 0

**Decide.** Where does your data live (inside the repo, a data directory, a NAS, object storage)? Typical sizes? This sets the sandbox's data directory model and whether a data browser belongs in the UI.

## OQ-15. Keybindings and editor feel — Informs phase 1

**Decide.** Zed defaults, VS Code defaults, or Vim mode from the first version? Any Zed features you use daily beyond the agent panel (multibuffers, outline, collaboration) that should be in scope?

## OQ-16. Name and licensing — Informs nothing technical

**Decide.** Working name for the product, and whether it will be open source (affects library choices with copyleft licenses, such as some PDF and terminal components).

---

## Suggested order of decisions

1. OQ-2, OQ-3, OQ-13 together: where code runs and how it is contained.
2. OQ-12, OQ-7, OQ-6, OQ-4: what phase 0 must prove.
3. OQ-1, OQ-8, OQ-11, OQ-15: shape of phase 1.
4. OQ-5, OQ-10, OQ-9, OQ-14, OQ-16: can trail the spike.
