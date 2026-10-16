# Personal Analyst — Application Design and Architecture

*Status: draft for review. Companion documents: [VISION.md](VISION.md), [OPEN-QUESTIONS.md](OPEN-QUESTIONS.md).*

This document specifies the architecture of the desktop application described in the vision. It is written to be built from. Where a decision is still open, it states the recommendation and points to the open-questions document.

## 1. Decision summary

| # | Decision | Recommendation | Status |
|---|---|---|---|
| D1 | Desktop shell | Electron with a TypeScript core in the main process and a React renderer | Open, see OQ-1 |
| D2 | Analysis runtime | Python, one `uv`-managed virtual environment per session | Recommended |
| D3 | App tooling runtime | A hidden, app-owned Python environment for the kernel, SymPy, and Ruff, separate from session environments | Recommended |
| D4 | Model access | One internal interface, two wire adapters: OpenAI-compatible chat completions and Anthropic Messages | Recommended |
| D5 | Sandbox | `bubblewrap` on Linux, `sandbox-exec` on macOS, container backend optional, Windows via WSL2 later | Open, see OQ-2 and OQ-3 |
| D6 | Edit gating default | Review mode: agent pauses on each edit proposal until accepted | Open, see OQ-4 |
| D7 | Equation source | AST-derived for pure expressions, declared-and-checked for everything else, LLM-explained marked unverified | Open, see OQ-5 |
| D8 | Editor | Monaco (CodeMirror 6 as the alternative) | Recommended |
| D9 | Docking | `dockview` for in-window panels, Electron `BrowserWindow` for undocked panels | Recommended |
| D10 | LaTeX toolchain | Tectonic, downloaded on first use | Open, see OQ-10 |
| D11 | Persistence | SQLite for indexes, JSONL for transcripts, TOML for config, git refs for checkpoints | Recommended |
| D12 | External tools | MCP client (stdio and streamable HTTP); built-in tools share the same schema | Recommended |

## 2. System overview

```mermaid
flowchart LR
  subgraph UI["Renderer (React)"]
    Chat[Agent chat]
    Editor[Editor + diff review]
    Figs[Figures panel]
    Eqs[Equations panel]
    Pkgs[Packages panel]
    Docs[Markdown / PDF preview]
    Term[Terminal]
  end

  subgraph Core["Core (Electron main, TypeScript)"]
    SM[Session manager]
    AL[Agent loop]
    PE[Policy engine + edit gating]
    MP[Model providers]
    TR[Tool registry + MCP client]
    EM[Environment manager]
    RK[Kernel client]
    FS[File + git service]
    CK[Checkpoints]
    ST[(SQLite / JSONL)]
  end

  subgraph Py["Per-session Python"]
    K[analyst-kernel\n(tooling env)]
    SB[Sandbox\nbwrap / sandbox-exec]
    V[(Session venv\nvia uv)]
    Script[User script\n+ figure capture backend]
  end

  LLM[(Ollama / OpenAI-compatible\nAnthropic)]
  MCP[(MCP servers:\nWolfram, MATLAB, Julia, DB...)]
  Git[(Analysis repo)]

  UI <-- IPC --> Core
  AL --> MP --> LLM
  AL --> TR --> MCP
  AL --> PE
  AL --> RK --> K
  K --> SB --> Script
  Script --> V
  EM --> V
  FS --> Git
  CK --> Git
```

Three tiers:

- **Renderer.** All visible surfaces. Holds no authority: it cannot write files, run code, or call models except through typed IPC requests to the core.
- **Core.** Owns sessions, the agent loop, policy, model access, tools, and persistence. One process per app, one session context per tab.
- **Per-session Python.** A kernel process (from the app-owned tooling environment) that runs user scripts inside the sandbox using the session's virtual environment, captures figures, indexes scripts, and performs equation analysis.

## 3. Desktop stack (D1)

**Recommendation: Electron, TypeScript core, React renderer.**

Why, given that you like Zed and Zed is native Rust:

- The analysis-specific heavy lifting (AST analysis, SymPy, figure capture, environment management) is Python regardless of shell. The shell's job is orchestration, model access, MCP, editor, and layout. The TypeScript ecosystem is strongest exactly there: Monaco (the VS Code editor), `xterm.js` and `node-pty`, `dockview`, KaTeX, `pdf.js`, the official MCP TypeScript SDK, and mature streaming clients for every model provider.
- One language for core and renderer means one build, one test runner, and one type system for IPC messages.
- Memory and binary size are the cost. For a single-user workbench that already runs Python processes and a model client, this is acceptable.

Alternative: Tauri 2 with a Rust core. Smaller binary, lower idle memory, Rust for process supervision. Costs: two languages, a less mature MCP and model-client ecosystem in Rust, and a sidecar would be needed anyway for Node-based tools such as Pyright. A third option, building on Zed's GPUI, is not recommended: it is not a supported framework for third-party apps.

The renderer and core communicate over a typed IPC layer (request/response plus event streams) defined once in a shared `protocol` package so the Tauri alternative remains possible if the decision is revisited.

## 4. Domain model

| Entity | Definition | Owned by |
|---|---|---|
| Workspace | The app-level state: provider config, global skills, global MCP servers, list of sessions. | `~/.analyst/` |
| Session | A repo path + an environment + one or more conversations + layout + run history. One tab per session. | `~/.analyst/sessions/<id>/` |
| Repo | A git repository containing analysis code and the committed project config in `.analyst/`. | user's filesystem |
| Environment | A Python virtual environment managed by `uv`, bound to a session (and usually to a repo). | `<repo>/.venv` by default |
| Conversation | An ordered list of turns with one model configuration. A session can have several. | session store (JSONL) |
| Turn | One user message and everything the agent did in response: model calls, tool calls, proposals, runs. | transcript |
| Proposal | A pending change set (file edits, package installs, commands) awaiting policy decision. | core, in memory + transcript |
| Run | One execution of a script with arguments, producing stdout, stderr, exit code, artifacts, and timing. | SQLite + `out/` |
| Artifact | A figure, table, or file produced by a run, with a sidecar metadata record. | `<repo>/out/` |
| Equation | A rendered mathematical statement bound to a code location, with a provenance tier and verification status. | computed, cached in SQLite |
| Checkpoint | A snapshot of the working tree taken before an agent turn that may change files. | git ref `refs/analyst/checkpoints/<id>` |
| Skill | A markdown instruction set with trigger metadata, loaded into the agent's context on demand. | `.analyst/skills/`, `~/.analyst/skills/` |
| Provider / Model | A configured endpoint plus a model identifier and its capability flags. | `~/.analyst/config.toml` + keychain |
| Policy | The per-session permission rules the agent operates under. | `.analyst/project.toml` with session overrides |

## 5. Core services

### 5.1 Session manager

Responsibilities: create, open, close, and switch sessions; restore layout; keep each session's kernel, terminal, file watcher, and conversation alive while open; suspend them when closed.

Creating a session:

1. Choose a repo (existing directory, `git init` a new one, or clone). Optionally apply a template that creates `scripts/`, `analysis/`, `data/`, `out/`, `notes.md`, and `.analyst/project.toml`.
2. Bind an environment: adopt an existing `.venv`, or run `uv venv` plus `uv sync` if a `pyproject.toml` or `requirements.txt` exists.
3. Start the kernel for the session, index scripts, and compute the project map.
4. Choose a model (defaults from workspace config).

Session tabs persist across app restarts. Closing a tab suspends the session; it does not delete anything.

### 5.2 Environment manager (D2)

`uv` is the only environment tool in the first version: it is fast, manages Python versions itself, and produces a lockfile. The app bundles the `uv` binary.

Operations exposed to both the user (Packages panel) and the agent (tools, gated by policy):

- list installed packages with versions and whether they are in the lockfile
- add or remove a package (`uv add` / `uv remove` when `pyproject.toml` exists, else `uv pip install`)
- sync to lockfile
- choose Python version
- show outdated packages

Every operation is logged to the session and is a visible card in the conversation if the agent initiated it. Package changes by the agent default to "ask".

### 5.3 App tooling environment (D3)

The app ships its own Python (through `uv python install`, pinned version) with: `analyst-kernel`, SymPy, Ruff, and `libcst`. Session environments are never polluted with app dependencies. The only thing injected into a user script's process is a single stdlib-only module, `analyst_mpl`, placed on `PYTHONPATH` for figure capture.

### 5.4 Kernel protocol

The kernel is a long-lived process per session, launched from the tooling environment with the session's interpreter path as an argument. Transport is JSON-RPC 2.0 over stdio.

| Method | Purpose |
|---|---|
| `env.info` | Interpreter path, version, site-packages, sandbox availability. |
| `env.packages` | Installed packages (delegates to `uv`). |
| `scripts.index` | Walk `scripts/` (configurable globs) and parse each file's `argparse` definitions with the AST, without importing. Returns name, description, options, defaults, types. |
| `run.start` | Execute a script with arguments inside the sandbox using the session interpreter. Returns a run id. Streams `run.stdout`, `run.stderr`, `run.artifact`, and `run.done` notifications. |
| `run.cancel` | Terminate a run (process group). |
| `exec.snippet` | Run a short snippet in a persistent REPL subprocess in the session venv, also sandboxed. For quick checks by the agent or the user. |
| `eq.extract` | Equation extraction for a file or function (section 5.10). |
| `eq.check` | Numeric consistency check of a declared equation against a function. |
| `contract.check` | Script contract checker (section 5.12). |
| `lint.run` | Ruff diagnostics for a file. |

The REPL subprocess for `exec.snippet` is the only stateful execution surface; script runs are always fresh processes so results are reproducible from the terminal.

### 5.5 Sandbox (D5)

Scripts run with least privilege:

| Resource | Default |
|---|---|
| Filesystem read | Repo, session venv, Python installation, configured data directories (read-only), system libraries. Home directory otherwise hidden. |
| Filesystem write | `<repo>/out/`, `<repo>/data/derived/` (configurable), a per-run scratch directory, the matplotlib cache. Source directories are read-only during runs so a script cannot modify code. |
| Network | Denied. Per-session or per-run grant with an allowlist of hosts. |
| Processes | Can spawn child processes within the same sandbox. Cannot escalate. |
| Resources | Wall-clock timeout (default 10 minutes), memory cap, CPU count, all configurable. |
| Environment variables | Minimal allowlist. API keys are never passed into the sandbox. |

Implementation per platform:

- **Linux:** `bubblewrap` (`bwrap`) with bind mounts matching the table above and `--unshare-net` unless granted. Fallback to Landlock plus `seccomp` if `bwrap` is unavailable.
- **macOS:** `sandbox-exec` with a generated profile. It is deprecated in documentation but functional and used by other agent tools; the alternative is a container backend.
- **Container backend (optional, all platforms):** Podman or Docker with the venv and repo mounted. Slower start, strongest isolation, and the path to remote execution.
- **Windows:** no OS sandbox in the first version. Runs are marked "unsandboxed" in the UI. WSL2 backend later.

The sandbox protects the machine from the agent's code. It does not protect the repo from the agent's edits; that is the policy engine's job (5.6).

### 5.6 Agent loop

The loop is the product's core and is designed around the analysis cycle: plan, propose, run, verify. It is deliberately a single-threaded, inspectable loop rather than a multi-agent system.

**Context assembly for each model call**

1. System prompt: role, the script contract (5.12), output conventions, and the active policy summary so the model knows what will require approval.
2. Project map: directory tree (depth-limited), the script index from `scripts.index` (name, description, options), environment summary (Python version, key packages), git status, and the list of files changed by the user since the last turn.
3. Skills whose triggers match (5.15), in full.
4. Conversation history, compacted when it exceeds a configurable fraction of the model's context window. Compaction summarizes older turns and keeps a pinned-facts block (decisions, file names, parameter values) that the agent maintains explicitly.
5. The current user message plus any attachments: file references, figure references, selected text from the editor.

Large outputs (run logs, file contents) are never inlined past a size limit; they are stored as artifacts and the model receives a truncated view plus an artifact id it can read with a tool.

**Modes** (per conversation, switchable mid-conversation)

| Mode | Tools available | Use |
|---|---|---|
| Plan | Read-only: files, search, script index, equations, run history. | Discuss and plan before touching anything. |
| Build | Everything, subject to policy. | Write and change tools. |
| Run | Read-only plus `run.script`, `exec.snippet`, and artifact tools. No edits, no installs. | Exploration with existing tools, parameter sweeps, interpretation. |

**Tool set (built-in)**

| Tool | Policy class |
|---|---|
| `fs.read`, `fs.list`, `fs.search` (ripgrep) | read |
| `scripts.index`, `runs.list`, `runs.get` | read |
| `eq.extract`, `eq.check`, `contract.check`, `lint.run` | read |
| `fig.get` (returns image for vision-capable models), `fig.list` | read |
| `fs.propose_edit` (unified diff or full file) | edit |
| `fs.propose_create`, `fs.propose_delete` | edit |
| `run.script`, `exec.snippet` | run |
| `pkg.add`, `pkg.remove`, `pkg.sync` | package |
| `shell.run` (inside sandbox, arbitrary command) | shell |
| `git.status`, `git.diff` | read |
| `git.commit` | git |
| `doc.render`, `doc.insert_figure` | edit |
| `mcp.<server>.<tool>` | per server |

**Turn algorithm**

```
on user message:
  if mode allows edits: create checkpoint
  diff working tree against last-known state; add user-changed files to context
  loop (bounded by max_steps and token budget):
    stream model response
    for each tool call, in order:
      decision = policy.evaluate(tool, args)
      if decision == deny:  return error result to model
      if decision == ask:   pause, show card in chat, await user; on reject return error result
      if tool is edit:      stage into proposal set; if policy == review, pause here (same as ask)
      execute tool; append result to context
    after any accepted edit: run lint + contract check; feed diagnostics back automatically
      (at most N auto-fix iterations, then surface to user)
    if model stops without tool calls: break
  summarize turn: files changed, runs made, figures produced, open issues
  update pinned facts
```

Pausing on "ask" is a real pause: the model's partial turn is held, the user acts in the UI, and the loop resumes with the decision as a tool result. This mirrors the Zed experience you already use, with finer-grained control.

**Verification steps built into the loop**

- After edits: Ruff, `py_compile`, and the contract checker run automatically. Failures go back to the model before the user sees a "done".
- After runs: non-zero exit codes and tracebacks go back to the model with the run's stderr. The user sees the run card either way.
- On request or by policy: `eq.check` runs on functions with declared equations; mismatches are surfaced as a red badge in the Equations panel and as a tool result the model must address.

**Scope declaration.** In Build mode the agent's first action on a new task is a short plan listing the files it intends to touch. Edits outside the declared set require approval even when the session is in auto-accept mode. This directly addresses "the agent changed things I didn't ask for".

**Tool-calling fallback.** Models without reliable native tool calling can use a text protocol (fenced JSON blocks) that the core parses. This is a per-model flag. Which models get first-class support is OQ-12.

### 5.7 Policy engine and edit gating (D6)

Policy is declarative, lives in `.analyst/project.toml`, and can be overridden per session from the UI without editing the file.

```toml
[policy]
edits    = "review"        # review | auto-new-files | auto
runs     = "auto"          # auto | ask
packages = "ask"           # ask | auto | deny
shell    = "ask"           # ask | auto | deny
network  = "deny"          # deny | ask | allow
git      = "ask"           # commits by the agent
protected = ["analysis/core/**", "data/**"]   # never editable by the agent
max_steps = 40
max_run_seconds = 600

[policy.mcp]
wolfram = "ask"
```

Edit gating states:

| Mode | Behavior |
|---|---|
| `review` | The turn pauses on each proposal. The diff appears inline in the editor (accept or reject per hunk) and as a card in chat. The turn resumes with the accepted content. |
| `auto-new-files` | New files are written immediately; modifications to existing files pause for review. |
| `auto` | All in-scope edits apply immediately. Out-of-scope edits still pause. A checkpoint exists for every turn. |

Conflict detection: a proposal carries the content hash of the file it was based on. If the user changed the file meanwhile, the proposal is rejected with a conflict and the agent must re-read.

Protected regions inside a file are marked with comments (`# analyst: lock` / `# analyst: unlock`); proposals touching them are denied.

### 5.8 Checkpoints

Before every turn that could change files, the core writes the working tree (including untracked, excluding ignored) to a tree object and stores it under `refs/analyst/checkpoints/<turn-id>` using git plumbing (`git add -A` into a temporary index, `git write-tree`, `git commit-tree`). This never touches the user's index, branches, or history, and these refs are never pushed. "Revert this turn" restores the tree. Checkpoints are pruned by age and count.

### 5.9 Model provider layer (D4)

One internal interface:

```ts
interface ModelClient {
  stream(req: ChatRequest): AsyncIterable<ModelEvent>;   // text delta | tool call | usage | stop
}
interface ModelInfo {
  id: string; provider: string; contextWindow: number;
  tools: "native" | "text-protocol" | "none";
  vision: boolean; reasoning: boolean;
}
```

Two wire adapters cover every provider you named and most others:

| Adapter | Covers |
|---|---|
| OpenAI-compatible chat completions (streaming, tools, images) | Ollama (`/v1`), vLLM, llama.cpp server, LM Studio, OpenAI, OpenRouter, Mistral, DeepSeek, Groq, Gemini's OpenAI-compatible endpoint |
| Anthropic Messages (streaming, tools, images) | Anthropic |

An optional Ollama-native adapter adds model listing and pulling from the UI; it is not needed for chat.

Provider configuration in `~/.analyst/config.toml`:

```toml
[[providers]]
name = "ollama-lab"
kind = "openai-compatible"
base_url = "http://lab-gpu:11434/v1"
# api_key resolved from keychain entry "ollama-lab" or env var OLLAMA_LAB_API_KEY

[[providers]]
name = "anthropic"
kind = "anthropic"

[[models]]
provider = "ollama-lab"
id = "qwen3-coder:30b"
context_window = 131072
tools = "native"
vision = false
```

API keys are stored in the OS keychain through Electron's `safeStorage`, with environment variables as a fallback. The chat header always shows which provider is active and whether it is local or remote, so you know what leaves the machine.

Model choice is per conversation. A later feature allows different models for roles (planner, coder, summarizer); the interface already supports it.

### 5.10 Equation lens (D7)

The goal is to show the math a function implements, derived from the code wherever possible, and to make the gap between "declared" and "implemented" visible when derivation is not possible.

Three provenance tiers, each shown with a distinct badge:

**Tier A, derived.** For functions whose body is a sequence of assignments ending in a return, composed of arithmetic, powers, NumPy and `math` element-wise functions (`exp`, `log`, `sqrt`, `sin`, ...), constants, and references to parameters, the kernel transpiles the AST into a SymPy expression without importing or executing the user's module. Assignments are substituted, the result is simplified lightly and rendered to LaTeX. Array indexing, loops, conditionals, and calls to unknown functions end the derivation for that function (it falls to tier B). This tier is ground truth by construction.

**Tier B, declared and checked.** The function's docstring carries a `Math:` section in SymPy-parsable syntax:

```python
def double_exp(t, A, tau1, B, tau2):
    """Two-component exponential decay.

    Math:
        y = A*exp(-t/tau1) + B*exp(-t/tau2)
    """
    return A * np.exp(-t / tau1) + B * np.exp(-t / tau2)
```

The kernel renders the declaration to LaTeX with SymPy and verifies it numerically: it lambdifies the declared expression, calls the real function in the session venv on random inputs within declared or default ranges, and compares within tolerance. Badge: green (agrees), red (mismatch, with the inputs that disagree), grey (could not evaluate). A `LaTeX:` section is also accepted for display-only material (derivations, algorithm statements) and is always marked unverified.

**Tier C, explained.** On request, the agent writes an explanation with equations for a function or algorithm that neither tier A nor B covers. Always marked unverified and stored outside the source file.

Surfaces:

- **Equations panel:** every equation in the current file or module, rendered with KaTeX, with badge, function name, and line. Click to jump. Filter by tier or status.
- **Editor decorations:** a code-lens line above each covered function ("view equation", badge).
- **Agent tools:** `eq.extract` and `eq.check` so the agent verifies its own work and the script contract can require `Math:` sections on functions in designated modules.

The `Math:` docstring convention is deliberately dependency-free: the user's repo needs no package from the app.

### 5.11 Figure pipeline

Capture happens in the script's process through a Matplotlib backend injected by the kernel (`MPLBACKEND=module://analyst_mpl`, module on `PYTHONPATH`). It overrides `show()` and wraps `savefig()`:

- each figure is written to `out/figures/<script>/<run-id>/<name>.png` and `.svg` (optionally a pickle of the Figure for re-styling later)
- a sidecar JSON records script, arguments, run id, figure title, axes labels, and timestamp
- the kernel watches the run's figure directory and emits `run.artifact` events

Terminal runs get the same treatment two ways: `source .analyst/env.sh` in the embedded terminal sets the same environment variables (done automatically for the app's own terminal), and a file watcher on `out/figures/**` picks up anything else.

Figures panel:

- tabs per figure, grouped by run; latest run of a script replaces the tab content by default, with a history slider to go back
- split view to compare two runs of the same figure
- pin a figure so it survives new runs
- undock into its own OS window (section 6)
- "send to agent" attaches the image to the next message for vision-capable models, or the sidecar metadata for others

Interactive figures (Plotly, Bokeh HTML) are rendered in a webview tab in a later phase (OQ-11).

### 5.12 Script contract and checker

The contract encodes the conventions you enforce by hand today. It is both a prompt (the agent is told the rules) and a checker (the kernel verifies them with AST analysis).

Default contract, configurable in `.analyst/project.toml`:

1. Scripts live in `scripts/` and are thin: argument parsing, calling library functions, saving outputs.
2. Every script defines `main(args)` and an `if __name__ == "__main__":` guard, and builds an `argparse` parser with a description and help text on every option.
3. All inputs are options with defaults. Paths default to repo-relative locations. No hard-coded absolute paths.
4. Computational code lives in an importable package (`analysis/`) as functions with docstrings. Pure functions where possible.
5. Outputs go to `out/` under a subdirectory named after the script. Figures are saved with descriptive names. A run metadata JSON is written with the arguments used.
6. Random seeds are options with defaults.
7. Functions in modules listed under `contract.math_required` carry a `Math:` docstring section.

Checker output is a list of diagnostics with file, line, rule id, and a fix hint, shown in the editor and fed back to the agent after each edit.

### 5.13 Documents

- **Markdown:** edited in the main editor, previewed live in a panel using `react-markdown` with `remark-math` and KaTeX. Figure embeds support both plain image paths and run references such as `{{fig: fit_decay/residuals}}` that resolve to the latest run, so notes stay current as analyses are rerun.
- **LaTeX:** `.tex` files compile with Tectonic (self-contained, fetches packages on demand) to PDF, shown in a `pdf.js` viewer panel. Compile on save with a debounce. Errors are parsed into editor diagnostics. SyncTeX click-to-source later.
- **Agent support:** `doc.insert_figure` and `doc.render` tools; a "report" skill that knows the document structure.

Each session gets a `notes.md` by default.

### 5.14 Terminal

`xterm.js` with `node-pty`, opened in the repo root with the session venv activated and the figure-capture environment set. Multiple terminals per session. This is the manual door into the same loop: anything run here produces figures and run records like an agent run.

### 5.15 Skills and configuration

Skills are markdown files with front matter:

```markdown
---
name: curve-fitting
triggers: ["fit", "regression", "least squares"]
paths: ["analysis/fitting*.py"]
always: false
---
When fitting models: report parameter uncertainties from the covariance, plot residuals, ...
```

Skills load from `~/.analyst/skills/` (global) and `<repo>/.analyst/skills/` (project). Matching is by trigger words in the user message and by path globs of files in the context; `always: true` skills are always loaded. The user can see and toggle which skills are active in a given turn.

Configuration layering, lowest to highest precedence: built-in defaults, `~/.analyst/config.toml`, `<repo>/.analyst/project.toml`, session overrides from the UI.

## 6. User interface

The layout follows Zed's shape so the muscle memory transfers, with analysis panels added.

```
┌─────────────────────────────────────────────────────────────────────┐
│ [Session: decay-analysis ▾] [Session: beam-sim ▾] [+]   model ▾  ⚙ │
├──────────┬──────────────────────────────────────┬───────────────────┤
│ Project  │ Editor tabs: fitting.py | fit_decay.py│ Agent chat        │
│ Scripts  │ ─────────────────────────────────────│ mode: Build ▾     │
│ Runs     │   def double_exp(t, A, tau1, ...):   │                   │
│ Skills   │   ∑ y = A e^{-t/τ₁} + B e^{-t/τ₂} ✓  │ [plan card]       │
│          │       return A * np.exp(-t/tau1) ... │ [diff card ✓ ✗]   │
│          │                                      │ [run card + figs] │
│          ├──────────────────────────────────────┤                   │
│          │ Terminal | Run output | Diagnostics  │                   │
├──────────┴──────────────────────────────────────┴───────────────────┤
│ Figures ◧ | Equations | Packages | Preview | (dockable, undockable) │
└─────────────────────────────────────────────────────────────────────┘
```

Panels and their primary interactions:

| Panel | Interactions |
|---|---|
| Session tabs | Switch, create, close, duplicate with a new conversation. Each restores its full layout. |
| Project sidebar | File tree. Scripts view lists each script with its options parsed from the index; a form to run it with chosen arguments. Runs view with history and filters. |
| Editor | Monaco with Python syntax, Ruff diagnostics, Pyright hover and definitions in a later phase. Inline diff review of agent proposals. Code-lens for equations. Protected-region markers. |
| Agent chat | Streaming messages. Cards for plans, proposals, runs, package changes, MCP calls, and approvals. Mode switch. Model switch. Context indicator (tokens used, compaction state). Attach files, figures, or editor selections. |
| Figures | Section 5.11. |
| Equations | Section 5.10. |
| Packages | Installed list, search and add, remove, sync, Python version. Pending agent requests show here too. |
| Preview | Markdown or PDF. Follows the active document. |
| Terminal / Run output / Diagnostics | Bottom dock. Run output shows structured runs with stdout, stderr, and artifact links. |

Docking uses `dockview`: any panel can be dragged into any dock group or into a floating group. Undocking into a separate OS window creates a second `BrowserWindow` hosting that panel, subscribed to the same session state; this is what makes a figures-only window on a second monitor work.

Command palette and keybindings follow Zed's defaults where they apply. A Vim mode via Monaco's `monaco-vim` is a later phase.

## 7. Storage layout

```
~/.analyst/
  config.toml              providers, models, global policy defaults, UI prefs
  skills/                  global skills
  mcp.json                 global MCP servers
  tooling/                 app-owned Python environment (uv-managed)
  sessions/<id>/
    session.toml           repo path, env path, layout, active model
    conversations/<id>.jsonl
    index.sqlite           runs, artifacts, equations cache, checkpoints
<repo>/
  .analyst/
    project.toml           contract, policy, data dirs, model prefs   (committed)
    skills/                project skills                            (committed)
    mcp.json               project MCP servers                        (committed)
    env.sh                 generated; figure capture env for terminals (ignored)
  scripts/  analysis/  data/  out/  notes.md
```

`.analyst/project.toml` is meant to be committed so a repo carries its own conventions. Everything under `out/` and `.analyst/env.sh` is git-ignored by the template.

## 8. Security and privacy

- No telemetry. No network calls except to configured model providers, configured MCP servers, package indexes during installs, and Tectonic's package fetches.
- API keys in the OS keychain. Never logged, never passed into the sandbox or into MCP server environments unless explicitly configured for that server.
- The chat UI shows provider locality (local or remote) at all times. A per-session "data directories" list controls what the agent can read; nothing outside the repo and that list is readable by tools or by the sandbox.
- MCP servers run as separate processes with their own configured permissions; their tools go through the same policy engine as built-ins.
- Transcripts stay local. Export is explicit.

## 9. Cross-platform

macOS and Linux first (sandbox available). Windows runs without a sandbox initially, clearly indicated, with a WSL2 backend planned. `uv`, `ripgrep`, and Tectonic are bundled or fetched per platform.

## 10. Testing and evaluation

- **Unit and integration:** core services in TypeScript (Vitest), kernel in Python (pytest), IPC contract tests from the shared protocol package, sandbox tests that assert denied writes and denied network.
- **Agent evaluations:** a suite of representative analysis tasks (fit a model, process a dataset, produce a figure, refactor a script to the contract) with automatic checks: contract passes, run succeeds, expected artifacts exist, declared equations verify. The suite runs against every configured model and produces a compatibility report. This is the primary tool for tuning prompts and for deciding which open-weight models to recommend.
- **UI:** Playwright for the renderer, with a fake core for determinism.

## 11. Repository structure and build order

```
apps/desktop/          Electron main + React renderer
packages/protocol/     IPC and kernel message types (TypeScript + JSON schema)
packages/core/         sessions, agent loop, policy, providers, tools, MCP, storage
packages/kernel/       Python: analyst_kernel (runs, index, equations, contract) and analyst_mpl
packages/evals/        agent evaluation tasks and runner
packages/cli/          headless harness used in phase 0 and for evals
docs/
```

Build order follows the roadmap in the vision document. Phase 0 produces `packages/kernel`, the provider layer, the policy engine, the agent loop, and the CLI harness, with no UI. That phase answers the riskiest questions (tool-calling reliability of open-weight models, sandbox behavior, equation derivation coverage) before any interface work.
