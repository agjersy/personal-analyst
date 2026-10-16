# Open Questions Before the Build

*Status: first review complete (2026-10-16). Resolved items are listed first; each remaining item names the phase that is blocked until it is answered. Companion documents: [VISION.md](VISION.md), [ARCHITECTURE.md](ARCHITECTURE.md).*

## Resolved in the first review

| Item | Decision | Where it landed |
|---|---|---|
| OQ-1 Desktop shell | Tauri 2 with a Rust core and a React renderer, for cross-platform support. | ARCHITECTURE.md section 3 |
| OQ-7 Environment tooling | `uv` only. No conda-only packages are planned. | ARCHITECTURE.md section 5.2 |
| OQ-12 Model roster | None. The design and the evaluation suite are model agnostic: no model-specific code paths, capabilities probed at first use, a capability baseline (native tool calling, streaming, at least 32k context) defines a "performant model". | ARCHITECTURE.md sections 5.7, 5.10, 10 |
| OQ-13 Where analysis runs | Remote execution over SSH is a first-version feature, designed in from phase 0. The local repo is canonical; remote targets hold a synced mirror; runs survive disconnects. | ARCHITECTURE.md section 5.5 |
| OQ-6 (partial) Script contract | Two rules confirmed as the ones enforced manually today and made hard failures with automated checks: command-line options on every script, and shared logic generalized into the library rather than re-implemented across scripts. | ARCHITECTURE.md section 5.13 |

Items marked **Blocks phase 0** need an answer before the headless spike starts. The rest can be decided while phase 0 is underway.

## OQ-2. Primary operating system and remote hosts — Blocks phase 0

Tauri covers all three desktop platforms, but the local sandbox, the terminal, and the SSH sync path still differ per OS, and the remote side matters as much now that remote execution is in scope.

**Decide.** Which OS do you work on daily? What are the remote hosts (Linux distribution, whether `bubblewrap` or unprivileged user namespaces are available, whether `rsync` is installed)? The spec can bootstrap `uv` and the kernel on a host, but cannot install `bubblewrap` without root.

## OQ-3. Sandbox strength and network policy — Blocks phase 0

**Options.** (a) OS-level isolation (`bubblewrap`, `sandbox-exec`) with path scoping and network denied by default. (b) Container backend (Podman or Docker) for every run. (c) Path scoping and resource limits only, no isolation.

**Recommendation.** (a) as the default with (b) as an opt-in backend. (c) is the automatic fallback on hosts where (a) is unavailable, badged in the UI, and should not be the design target.

**Decide.** Do your analysis scripts need network access routinely (downloading datasets, calling services)? If so the default should be "ask" rather than "deny", and the allowlist model matters more. Is an unsandboxed fallback on shared remote hosts acceptable, given the SSH account boundary?

## OQ-4. Edit gating default — Blocks phase 0

**Options.** (a) `review`: pause on every proposal. (b) `auto-new-files`. (c) `auto` with scope declaration and checkpoints as the safety net.

**Recommendation.** (a) by default, because unwanted edits are a stated pain point, with a one-click session toggle to (b) or (c) when you trust a task. Scope declaration and protected paths apply in every mode.

**Decide.** Is per-hunk review the right granularity, or do you want per-file accept and reject only? Should protected-region comment markers exist in the first version?

## OQ-5. Equation lens: how much derivation, how much declaration — Blocks phase 2, informs phase 0

**Options.** (a) Derive from the AST only; show nothing for functions that cannot be derived. (b) Declared `Math:` docstrings with numeric verification, plus derivation where possible. (c) LLM-generated LaTeX only.

**Recommendation.** (b). Derivation is ground truth but covers only pure expression functions. Declared-and-checked covers algorithms and gives a real verification signal. (c) alone reproduces the trust problem you are trying to solve.

**Decide.** Is a `Math:` docstring section in SymPy syntax an acceptable convention to require in your repos? Would you rather write LaTeX directly and accept that it cannot be verified numerically? Which of your existing modules would be the first test case for derivation coverage?

## OQ-6. Script contract details — Blocks phase 0 (partly resolved)

The two rules you enforce manually today are now hard rules with automated checks (ARCHITECTURE.md section 5.13). The remaining proposed rules (output layout, seeds as options, `Math:` sections, units in docstrings) are guesses.

**Decide.**
- Mark up the proposed rules: which are right, which are missing (naming, logging, data loading conventions, testing), and which should be warnings versus hard failures.
- For the generalization rule: may a script define any helper functions at all, or must every function live in the library? The spec allows small private helpers under a size limit.
- For duplicate detection: is a near-duplicate warning at 80 percent token similarity the right starting threshold, and should the agent be blocked from proposing an edit that introduces one, or only warned?
- Is `analysis/` the right default name for the library package, and do you keep one library per repo or a shared library across repos (see OQ-8)?

## OQ-7. Environment tooling — Resolved

`uv` only; no conda-only packages planned. Kept here so the numbering stays stable.

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

## OQ-12. Model roster — Resolved, one follow-up

The design is model agnostic and the evaluation suite targets a capability baseline rather than a list. One follow-up remains.

**Decide.** Should the text-protocol fallback for models that fail the tool-calling probe exist at all in the first version? It is cheap to build but it is a degraded path that the evaluation suite will not certify. The alternative is to refuse such models with a clear message.

## OQ-13. Where analysis runs — Resolved, see OQ-17 and OQ-18

Remote execution over SSH is designed in (ARCHITECTURE.md section 5.5). Two follow-ups are split out below.

## OQ-14. Data location and size — Blocks phase 0

With remote execution in scope, data location decides the sync model. The spec keeps data on the target and never syncs it.

**Decide.** Where does your data live on each host (inside the repo, a shared data directory, a NAS, object storage)? Typical sizes? Does any data need to move between local and remote targets, or is it always consumed where it lives? Does a derived-data directory need to sync back alongside figures?

## OQ-17. Scheduler integration on remote hosts — Informs phase 0

The spec runs scripts directly on the SSH host as detached processes. On a cluster, runs may need to be submitted through a scheduler such as Slurm, which changes how runs start, how output is collected, and how long they take to begin.

**Decide.** Are your remote hosts standalone machines (workstation, GPU box) or scheduler-managed clusters? If a scheduler is involved, should the first version support it, or is a login-node run acceptable to start?

## OQ-18. Remote repo model — Informs phase 0

The spec makes the local repo canonical and mirrors it to the host before each run. This is simple and keeps editing, checkpoints, and diffs fast and offline-capable. It assumes the code may live on your local machine.

**Decide.** Is there any repo or data-adjacent code that must not leave the remote host? If so, a "remote-primary" mode (repo lives only on the host, edited over SFTP) is needed, and it is a materially different design that should be chosen before phase 0 rather than retrofitted.

## OQ-15. Keybindings and editor feel — Informs phase 1

**Decide.** Zed defaults, VS Code defaults, or Vim mode from the first version? Any Zed features you use daily beyond the agent panel (multibuffers, outline, collaboration) that should be in scope?

## OQ-16. Name and licensing — Informs nothing technical

**Decide.** Working name for the product, and whether it will be open source (affects library choices with copyleft licenses, such as some PDF and terminal components).

---

## Suggested order of decisions

1. OQ-2, OQ-3, OQ-14, OQ-17, OQ-18 together: the remote hosts, where data lives, and how runs are contained and submitted.
2. OQ-6, OQ-4, OQ-12: what phase 0 must prove.
3. OQ-8, OQ-11, OQ-15: shape of phase 1.
4. OQ-5, OQ-10, OQ-9, OQ-16: can trail the spike.
