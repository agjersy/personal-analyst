# Personal Analyst — Application Design and Architecture

*Status: revised after first review (2026-10-16). Companion documents: [VISION.md](VISION.md), [OPEN-QUESTIONS.md](OPEN-QUESTIONS.md).*

This document specifies the architecture of the desktop application described in the vision. It is written to be built from. Decisions confirmed in review are marked as such; where a decision is still open, it states the recommendation and points to the open-questions document.

## 1. Decision summary

| # | Decision | Resolution | Status |
|---|---|---|---|
| D1 | Desktop shell | Tauri 2: Rust core, React and TypeScript renderer in the system webview | Decided |
| D2 | Analysis runtime | Python, one `uv`-managed virtual environment per session. `uv` only; no conda | Decided |
| D3 | App tooling runtime | A hidden, app-owned Python environment per execution target for the kernel, SymPy, Ruff, and language servers | Recommended |
| D4 | Model access | One internal interface, two wire adapters (OpenAI-compatible chat completions, Anthropic Messages). No model-specific code paths; capabilities probed, not hand-listed | Decided |
| D5 | Execution targets | Every session binds to a target: local, or a remote host over SSH. Kernel protocol is transport-agnostic. Local repo is canonical; remote holds a synced mirror | Decided |
| D6 | Sandbox | `bubblewrap` on Linux (local and remote), `sandbox-exec` on macOS, container backend optional, unsandboxed with a visible badge where neither exists | Open, see OQ-2 and OQ-3 |
| D7 | Edit gating default | Review mode: agent pauses on each edit proposal until accepted | Open, see OQ-4 |
| D8 | Equation source | AST-derived for pure expressions, declared-and-checked for everything else, LLM-explained marked unverified | Open, see OQ-5 |
| D9 | Script contract | CLI options on every script and shared logic in the library package are hard rules with automated checks; remaining rules per OQ-6 | Partly decided |
| D10 | Editor | CodeMirror 6 (Monaco as the fallback if a needed feature is missing) | Recommended |
| D11 | Docking | `dockview` for in-window panels; Tauri `WebviewWindow` for undocked panels | Recommended |
| D12 | LaTeX toolchain | Tectonic, downloaded on first use; system TeX used if present | Open, see OQ-10 |
| D13 | Persistence | SQLite for indexes, JSONL for transcripts, TOML for config, git refs for checkpoints | Recommended |
| D14 | External tools | MCP client (stdio and streamable HTTP) via the official Rust SDK; built-in tools share the same schema | Recommended |
| D15 | Evaluation | Model-agnostic task suite with automatic graders; produces a scorecard per configured model; no model is privileged | Decided |

## 2. System overview

```mermaid
flowchart LR
  subgraph UI["Renderer (React, system webview)"]
    Chat[Agent chat]
    Editor[Editor + diff review]
    Figs[Figures panel]
    Eqs[Equations panel]
    Pkgs[Packages panel]
    Docs[Markdown / PDF preview]
    Term[Terminal]
  end

  subgraph Core["Core (Tauri backend, Rust)"]
    SM[Session manager]
    AL[Agent loop]
    PE[Policy engine + edit gating]
    MP[Model providers]
    TR[Tool registry + MCP client]
    TG[Target manager\nlocal | ssh]
    EM[Environment manager]
    RK[Kernel client]
    SY[Repo sync]
    FS[File + git service]
    CK[Checkpoints]
    ST[(SQLite / JSONL)]
  end

  subgraph Target["Execution target (local machine or SSH host)"]
    K[analyst-kernel\n(tooling env)]
    SB[Sandbox]
    V[(Session venv\nvia uv)]
    Script[User script\n+ figure capture backend]
    Mirror[(Repo mirror\nremote only)]
  end

  LLM[(Ollama / OpenAI-compatible\nAnthropic)]
  MCP[(MCP servers)]
  Git[(Local analysis repo)]

  UI <-- typed IPC --> Core
  AL --> MP --> LLM
  AL --> TR --> MCP
  AL --> PE
  AL --> RK
  TG --> RK
  TG --> SY
  RK -- stdio or ssh --> K
  SY -- rsync / tar over ssh --> Mirror
  K --> SB --> Script
  Script --> V
  EM --> V
  FS --> Git
  CK --> Git
```

Three tiers:

- **Renderer.** All visible surfaces. Holds no authority: it cannot write files, run code, or call models except through typed IPC requests to the core.
- **Core.** Owns sessions, the agent loop, policy, model access, tools, targets, and persistence. One process per app, one session context per tab. The analysis repo on the local disk is always the source of truth for code.
- **Execution target.** Where the kernel, the sandbox, the session venv, and the user's scripts run. It is either the local machine or a remote host reached over SSH. The core talks to the kernel with the same protocol either way.

## 3. Desktop stack (D1)

**Decided: Tauri 2 with a Rust core and a React/TypeScript renderer.**

| Layer | Choice | Notes |
|---|---|---|
| Shell | Tauri 2 | Native window and menus, multi-window for undocked panels, sidecar binaries, updater. Renders in WebKit (macOS, Linux) or WebView2 (Windows). |
| Core | Rust on `tokio` | All services in section 5. Compiled into the Tauri backend; also linked into a headless CLI for phase 0 and evals. |
| IPC | Tauri commands and events, types generated with `specta` and `tauri-specta` | Request/response as commands; streams (model tokens, run output, artifacts) as events on a per-session channel. One definition in the `protocol` crate produces the TypeScript bindings. |
| Model clients | `reqwest` with SSE streaming, hand-written adapters | Two adapters are a few hundred lines each and keep full control over tool-call parsing. No provider SDKs. |
| MCP | `rmcp` (official Rust SDK), client role | stdio and streamable HTTP transports. |
| Storage | `rusqlite`, `toml`, JSONL | See section 7. |
| File watching | `notify` | Working-tree diffing between turns, `out/` watching for terminal runs. |
| Terminals | `portable-pty` in the core, `xterm.js` in the renderer | A remote terminal is a local pty running `ssh -t`. |
| Git | System `git` through the CLI | Plumbing commands for checkpoints; porcelain for status and diff. |
| SSH | System OpenSSH client (`ssh`, `scp`), `rsync` where present | See section 5.5. No SSH reimplementation in the first version. |
| Editor | CodeMirror 6 | Lighter and more consistent across the three webviews than Monaco. Python syntax, LSP client, inline diff widgets, code lenses, read-only ranges. |
| Language tooling | `ruff server` (lint, format) and `ty` or `jedi-language-server` (hover, definitions), both run from the tooling environment on the target | No Node dependency. The core proxies LSP over IPC, so on a remote target the language server runs where the venv is and sees the real packages. |
| Docking | `dockview` | Floating groups in-window; undock creates a second `WebviewWindow` bound to the same session. |
| Rendering | KaTeX, `react-markdown` with `remark-math`, `pdf.js` | |
| Sidecars | `uv`, `rg`, optional `tectonic` | Bundled per platform through Tauri's external-binary mechanism. |

Known cost: WebKitGTK on Linux is slower than Chromium for heavy canvases and large DOMs. The figures panel and editor must be tested on Linux early, and the figure viewer uses `<img>` elements with cached raster files rather than live canvases.

## 4. Domain model

| Entity | Definition | Owned by |
|---|---|---|
| Workspace | The app-level state: provider config, targets, global skills, global MCP servers, list of sessions. | `~/.analyst/` |
| Target | Where runs execute: `local`, or an SSH host with a remote root directory. Holds its own tooling env, venvs, and run records. | `~/.analyst/config.toml` |
| Session | A repo path + a target + an environment + one or more conversations + layout + run history. One tab per session. | `~/.analyst/sessions/<id>/` |
| Repo | A git repository on the local disk containing analysis code and the committed project config in `.analyst/`. Canonical for code. | user's filesystem |
| Mirror | The synced copy of the repo on a remote target. Never edited directly; rebuilt from the local repo before runs. | `<remote_root>/<repo>` on the target |
| Environment | A Python virtual environment managed by `uv`, living on the target. | `<repo>/.venv` or `<mirror>/.venv` |
| Conversation | An ordered list of turns with one model configuration. A session can have several. | session store (JSONL) |
| Turn | One user message and everything the agent did in response: model calls, tool calls, proposals, runs. | transcript |
| Proposal | A pending change set (file edits, package installs, commands) awaiting policy decision. | core, in memory + transcript |
| Run | One execution of a script with arguments on a target, producing stdout, stderr, exit code, artifacts, and timing. Survives disconnects. | SQLite locally; `~/.analyst/runs/<id>/` on the target |
| Artifact | A figure, table, or file produced by a run, with a sidecar metadata record. Pulled back to the local `out/` from remote targets. | `<repo>/out/` |
| Equation | A rendered mathematical statement bound to a code location, with a provenance tier and verification status. | computed, cached in SQLite |
| Checkpoint | A snapshot of the local working tree taken before an agent turn that may change files. | git ref `refs/analyst/checkpoints/<id>` |
| Skill | A markdown instruction set with trigger metadata, loaded into the agent's context on demand. | `.analyst/skills/`, `~/.analyst/skills/` |
| Provider / Model | A configured endpoint plus a model identifier and its probed capability record. | `~/.analyst/config.toml` + keychain |
| Policy | The per-session permission rules the agent operates under. | `.analyst/project.toml` with session overrides |

## 5. Core services

### 5.1 Session manager

Responsibilities: create, open, close, and switch sessions; restore layout; keep each session's target connection, kernel, terminals, file watcher, and conversation alive while open; suspend them when closed.

Creating a session:

1. Choose a repo (existing directory, `git init` a new one, or clone). Optionally apply a template that creates `scripts/`, `analysis/`, `data/`, `out/`, `notes.md`, and `.analyst/project.toml`.
2. Choose a target: local or one of the configured SSH hosts. For a remote target the core bootstraps the host if needed (5.5) and performs the first sync.
3. Bind an environment on the target: adopt an existing `.venv`, or run `uv venv` plus `uv sync` from the repo's `pyproject.toml` or `requirements.txt`.
4. Start the kernel on the target, index scripts and the library, and compute the project map.
5. Choose a model (defaults from workspace config).

A session can change target later; doing so re-binds the environment and re-syncs. Session tabs persist across app restarts. Closing a tab suspends the session; it does not delete anything.

### 5.2 Environment manager (D2)

`uv` is the only environment tool. It is fast, manages Python versions itself, and produces a lockfile. The app bundles the `uv` binary for the local target and installs it on remote targets. Conda is out of scope; packages must be installable from PyPI or a configured index.

Operations exposed to both the user (Packages panel) and the agent (tools, gated by policy), all executed on the session's target:

- list installed packages with versions and whether they are in the lockfile
- add or remove a package (`uv add` / `uv remove` when `pyproject.toml` exists, else `uv pip install`)
- sync to lockfile
- choose Python version
- show outdated packages

Because the lockfile lives in the local repo and is synced, the environment is reproducible on any target: switching a session from local to a remote host is `uv sync` on the other side.

Every operation is logged to the session and is a visible card in the conversation if the agent initiated it. Package changes by the agent default to "ask".

### 5.3 App tooling environment (D3)

Each target carries an app-owned Python environment at `~/.analyst/tooling/` (created with `uv python install` at a pinned version) containing `analyst-kernel`, SymPy, Ruff, and the language server. The kernel wheel ships inside the app bundle and is copied to the target, so bootstrapping a remote host needs PyPI only for SymPy and friends. Session environments are never polluted with app dependencies. The only thing injected into a user script's process is a single stdlib-only module, `analyst_mpl`, placed on `PYTHONPATH` for figure capture.

### 5.4 Kernel protocol

The kernel is a long-lived process per session, launched from the tooling environment on the target with the session's interpreter path and repo (or mirror) root as arguments. Transport is JSON-RPC 2.0 over stdio. The core sees it through a `KernelTransport` trait with two implementations, `LocalStdio` and `SshStdio`; nothing above the transport knows which one is in use.

| Method | Purpose |
|---|---|
| `env.info` | Interpreter path, version, site-packages, sandbox availability, host name. |
| `env.packages` | Installed packages (delegates to `uv`). |
| `scripts.index` | Walk `scripts/` (configurable globs) and parse each file's `argparse` definitions with the AST, without importing. Returns name, description, options, defaults, types. |
| `lib.index` | Walk the library package (`analysis/` by default) and return every public function with signature, docstring summary, `Math:` presence, and the scripts that call it. Used by the agent before writing new functions and by the generalization checks. |
| `run.start` | Execute a script with arguments inside the sandbox using the session interpreter. The run is detached (`setsid`) with output and status written to `~/.analyst/runs/<run-id>/` on the target. Returns a run id and streams `run.stdout`, `run.stderr`, `run.artifact`, and `run.done` notifications. |
| `run.attach` | Resume streaming a run from recorded offsets after a kernel restart or a dropped connection. |
| `run.cancel` | Terminate a run (process group). |
| `exec.snippet` | Run a short snippet in a persistent REPL subprocess in the session venv, also sandboxed. For quick checks by the agent or the user. |
| `eq.extract` | Equation extraction for a file or function (section 5.11). |
| `eq.check` | Numeric consistency check of a declared equation against a function. |
| `contract.check` | Script contract checker (section 5.13). |
| `contract.similar` | Near-duplicate function detection across scripts and the library (section 5.13). |
| `lint.run` | Ruff diagnostics for a file. |

The REPL subprocess for `exec.snippet` is the only stateful execution surface; script runs are always fresh processes so results are reproducible from the terminal. The kernel holds no state that is not on disk in the run directory, so a kernel restart is always safe.

### 5.5 Execution targets and remote execution over SSH (D5)

**Decided: remote execution is a first-version feature, designed in from phase 0.**

A target is where code runs. The session's editor, agent, policy engine, checkpoints, and transcripts are always local; the kernel, sandbox, venv, data, and runs are on the target.

**Configuration**

```toml
[[targets]]
name = "lab-gpu"
kind = "ssh"
host = "lab-gpu"                # an alias from ~/.ssh/config; keys, jump hosts, and agent forwarding come from there
remote_root = "~/analyst"       # mirrors live at <remote_root>/<repo-name>
python = "3.12"                 # uv-managed on the host
sandbox = "auto"                # auto | bwrap | none
data_dirs = ["/data/shared/run3"]   # read-only mounts for the sandbox, target-local paths
```

The local target needs no configuration.

**Connection.** The core uses the system OpenSSH client with connection multiplexing (`ControlMaster=auto`, `ControlPersist`, a control socket under `~/.analyst/ssh/`). One master connection per target per session; the kernel's stdio, each terminal, and each sync are channels on it. This respects the user's existing SSH configuration, keys, agent, and bastions without the app reimplementing any of it. A Rust SSH implementation can replace this later behind the same `Transport` trait if the CLI dependency becomes a problem (Windows lacks `ControlMaster`; there the core opens separate connections).

**Bootstrap.** On first use of a target the core checks for `~/.analyst/bin/uv` and `~/.analyst/tooling/` on the host and creates them if missing: `uv` from its installer script or by copying the bundled binary for the host's architecture, then the tooling environment from the kernel wheel in the app bundle. Bootstrap is idempotent and version-checked, so an app upgrade refreshes the remote kernel on next connect.

**Repo sync model: local canonical, remote mirror.** The local repo is the source of truth for code. Before each run, and on demand, the core syncs the mirror:

1. Build the file list with `git ls-files --cached --others --exclude-standard` so tracked and untracked-but-not-ignored files are included and `out/`, `.venv/`, and other ignored paths are not.
2. Push with `rsync -a --delete --files-from=-` over the multiplexed connection. Where `rsync` is absent on either end, fall back to a `tar` pipe over `ssh` (full copy of the file list; acceptable for repo-sized trees).
3. Record the synced tree hash so a run is attributed to an exact code state, and so a run with no code changes skips the sync.

Syncs are typically well under a second on a warm connection. The mirror is never edited in place; any file the agent or user changes is changed locally and re-synced.

**Artifacts flow back.** A run writes to `out/` on the target. `run.artifact` events carry the target path and metadata; the core pulls each artifact into the local `out/` as it appears (eagerly for figures and small files, lazily on first view for large ones). The local `out/` is therefore a superset view across targets, and the figures panel does not care where a figure was produced.

**Data stays on the target.** Data directories are configured per target and are not synced. Scripts refer to data through CLI options whose defaults are repo-relative or come from `.analyst/project.toml` per-target overrides, so the same script runs locally against a sample and remotely against the full dataset without edits.

**Runs survive disconnects.** The kernel detaches each run and records stdout, stderr, exit status, and artifact list under `~/.analyst/runs/<run-id>/` on the target. If the SSH connection drops, the run continues. On reconnect, the core restarts the kernel if needed and calls `run.attach` to resume streaming from the last offsets. The run card shows "running, disconnected" rather than failing.

**Terminal.** A remote terminal is a local pty running `ssh -t <host>` that changes to the mirror directory, activates the venv, and exports the figure-capture environment. Figures produced from that terminal appear in the figures panel through the same `out/` watcher, running on the target through the kernel.

**Sandbox on remote hosts.** Linux hosts with `bubblewrap` and unprivileged user namespaces get the same sandbox as a local Linux machine. Many shared hosts disable user namespaces; there the kernel falls back to resource limits (`ulimit`, timeouts) and the run is badged "unsandboxed" in the UI. The SSH account boundary and the policy engine still apply. Scheduler integration (submitting runs through Slurm or similar) is not designed here; see OQ-17.

**What the user sees.** The session tab shows the target name and connection state. When a remote target is unreachable, editing, chat planning, equation viewing, and reading past runs continue to work; run and package tools report the target as offline, and the agent is told so in its context.

**Deferred alternative.** A "remote-primary" mode where the repo lives only on the host and the local side is a cache (editing over SFTP) is not in the first version. It would be the right model for repos that must not leave the host; see OQ-18.

### 5.6 Sandbox (D6)

Scripts run with least privilege:

| Resource | Default |
|---|---|
| Filesystem read | Repo or mirror, session venv, Python installation, configured data directories (read-only), system libraries. Home directory otherwise hidden. |
| Filesystem write | `out/`, `data/derived/` (configurable), a per-run scratch directory, the matplotlib cache. Source directories are read-only during runs so a script cannot modify code. |
| Network | Denied. Per-session or per-run grant with an allowlist of hosts. |
| Processes | Can spawn child processes within the same sandbox. Cannot escalate. |
| Resources | Wall-clock timeout (default 10 minutes), memory cap, CPU count, all configurable. |
| Environment variables | Minimal allowlist. API keys are never passed into the sandbox. |

Implementation per platform:

- **Linux (local or remote):** `bubblewrap` (`bwrap`) with bind mounts matching the table above and `--unshare-net` unless granted. Fallback to Landlock plus `seccomp` if `bwrap` is unavailable; fallback to resource limits only, badged, if user namespaces are disabled.
- **macOS:** `sandbox-exec` with a generated profile. It is deprecated in documentation but functional and used by other agent tools; the alternative is a container backend.
- **Container backend (optional):** Podman or Docker with the venv and repo mounted. Slower start, strongest isolation.
- **Windows (local):** no OS sandbox in the first version. Runs are marked "unsandboxed" in the UI. A Windows user with a Linux SSH target gets a sandbox by running remotely, which is expected to be the common case.

The sandbox protects the host from the agent's code. It does not protect the repo from the agent's edits; that is the policy engine's job (5.8).

### 5.7 Agent loop

The loop is the product's core and is designed around the analysis cycle: plan, propose, run, verify. It is deliberately a single-threaded, inspectable loop rather than a multi-agent system, and it contains no model-specific branches.

**Context assembly for each model call**

1. System prompt: role, the script contract (5.13), output conventions, and the active policy summary so the model knows what will require approval.
2. Project map: directory tree (depth-limited), the script index (name, description, options), the library index (public functions with one-line docstrings), environment summary (Python version, key packages), target name and state, git status, and the list of files changed by the user since the last turn.
3. Skills whose triggers match (5.16), in full.
4. Conversation history, compacted when it exceeds a configurable fraction of the model's context window. Compaction summarizes older turns and keeps a pinned-facts block (decisions, file names, parameter values) that the agent maintains explicitly.
5. The current user message plus any attachments: file references, figure references, selected text from the editor.

Large outputs (run logs, file contents) are never inlined past a size limit; they are stored as artifacts and the model receives a truncated view plus an artifact id it can read with a tool.

**Modes** (per conversation, switchable mid-conversation)

| Mode | Tools available | Use |
|---|---|---|
| Plan | Read-only: files, search, script and library index, equations, run history. | Discuss and plan before touching anything. |
| Build | Everything, subject to policy. | Write and change tools. |
| Run | Read-only plus `run.script`, `exec.snippet`, and artifact tools. No edits, no installs. | Exploration with existing tools, parameter sweeps, interpretation. |

**Tool set (built-in)**

| Tool | Policy class |
|---|---|
| `fs.read`, `fs.list`, `fs.search` (ripgrep, local) | read |
| `scripts.index`, `lib.index`, `runs.list`, `runs.get` | read |
| `eq.extract`, `eq.check`, `contract.check`, `contract.similar`, `lint.run` | read |
| `fig.get` (returns image for vision-capable models), `fig.list` | read |
| `target.info` (name, state, sandbox level, data dirs) | read |
| `fs.propose_edit` (unified diff or full file) | edit |
| `fs.propose_create`, `fs.propose_delete` | edit |
| `run.script`, `exec.snippet` | run |
| `pkg.add`, `pkg.remove`, `pkg.sync` | package |
| `shell.run` (inside sandbox on the target, arbitrary command) | shell |
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
      if tool is run:       sync mirror if target is remote and tree changed
      execute tool; append result to context
    after any accepted edit: run lint + contract check (including similarity); feed diagnostics back
      (at most N auto-fix iterations, then surface to user)
    if model stops without tool calls: break
  summarize turn: files changed, runs made, figures produced, open issues
  update pinned facts
```

Pausing on "ask" is a real pause: the model's partial turn is held, the user acts in the UI, and the loop resumes with the decision as a tool result. This mirrors the Zed experience you already use, with finer-grained control.

**Verification steps built into the loop**

- After edits: Ruff, `py_compile`, and the contract checker run automatically, including the generalization checks. Failures go back to the model before the user sees a "done".
- After runs: non-zero exit codes and tracebacks go back to the model with the run's stderr. The user sees the run card either way.
- On request or by policy: `eq.check` runs on functions with declared equations; mismatches are surfaced as a red badge in the Equations panel and as a tool result the model must address.

**Scope declaration.** In Build mode the agent's first action on a new task is a short plan listing the files it intends to touch. Edits outside the declared set require approval even when the session is in auto-accept mode. This directly addresses "the agent changed things I didn't ask for".

**Reuse before writing.** The system prompt requires the agent to consult `lib.index` and `fs.search` before defining a new function, and to extend or call an existing library function when one covers the need. The checker enforces the result (5.13).

**Capability baseline and fallback.** The loop targets models with native tool calling, streaming, and a context window of at least 32k tokens; this is the definition of a "performant model" for this product. Models that fail the tool-calling probe (5.10) can run through a text protocol (fenced JSON blocks) that the core parses. It is a degraded tier, flagged in the UI, not a supported mode for the eval suite.

### 5.8 Policy engine and edit gating (D7)

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

### 5.9 Checkpoints

Before every turn that could change files, the core writes the local working tree (including untracked, excluding ignored) to a tree object and stores it under `refs/analyst/checkpoints/<turn-id>` using git plumbing (`git add -A` into a temporary index, `git write-tree`, `git commit-tree`). This never touches the user's index, branches, or history, and these refs are never pushed. "Revert this turn" restores the tree and marks the remote mirror stale. Checkpoints are pruned by age and count.

### 5.10 Model provider layer (D4)

**Decided: model agnostic.** No prompt text, tool schema, or code path is conditioned on a model name. The loop assumes the capability baseline in 5.7 and adapts only through a probed capability record.

One internal interface:

```rust
trait ModelClient {
    fn stream(&self, req: ChatRequest) -> impl Stream<Item = ModelEvent>;  // TextDelta | ToolCall | Usage | Stop
}

struct ModelCapabilities {
    context_window: u32,
    tools: ToolSupport,      // Native | TextProtocol | None
    vision: bool,
    probed_at: Timestamp,
}
```

Two wire adapters cover every provider you named and most others:

| Adapter | Covers |
|---|---|
| OpenAI-compatible chat completions (streaming, tools, images) | Ollama (`/v1`), vLLM, llama.cpp server, LM Studio, OpenAI, OpenRouter, Mistral, DeepSeek, Groq, Gemini's OpenAI-compatible endpoint |
| Anthropic Messages (streaming, tools, images) | Anthropic |

An optional Ollama-native adapter adds model listing and pulling from the UI; it is not needed for chat.

**Capability probing.** On first use of a model (and on demand) the core runs a short probe: one exchange requiring a tool call with a known argument, one with an image if the user allows it, and a query of the provider's model-info endpoint (`/v1/models`, Ollama `/api/show`) for the context window. Results are cached in the capability record with a timestamp. Manual overrides are allowed but not required. This replaces any hand-maintained per-model list.

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
id = "<any model served there>"
# capabilities are probed; override only if the probe is wrong, e.g. context_window = 65536
```

API keys are stored in the OS keychain (Tauri `stronghold` or the platform keyring crate), with environment variables as a fallback. The chat header always shows which provider is active and whether it is local or remote, so you know what leaves the machine.

Model choice is per conversation. A later feature allows different models for roles (planner, coder, summarizer); the interface already supports it.

### 5.11 Equation lens (D8)

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

The `Math:` docstring convention is deliberately dependency-free: the user's repo needs no package from the app. Extraction runs in the tooling environment on the target; numeric checks run in the session venv there.

### 5.12 Figure pipeline

Capture happens in the script's process through a Matplotlib backend injected by the kernel (`MPLBACKEND=module://analyst_mpl`, module on `PYTHONPATH`). It overrides `show()` and wraps `savefig()`:

- each figure is written to `out/figures/<script>/<run-id>/<name>.png` and `.svg` (optionally a pickle of the Figure for re-styling later)
- a sidecar JSON records script, arguments, run id, target, figure title, axes labels, and timestamp
- the kernel watches the run's figure directory and emits `run.artifact` events; on a remote target the core pulls the files into the local `out/`

Terminal runs get the same treatment two ways: the app's own terminals export the capture environment automatically (`source .analyst/env.sh` does the same in any other shell), and the kernel's watcher on `out/figures/**` picks up anything else.

Figures panel:

- tabs per figure, grouped by run; latest run of a script replaces the tab content by default, with a history slider to go back
- split view to compare two runs of the same figure, including runs from different targets
- pin a figure so it survives new runs
- undock into its own OS window (section 6)
- "send to agent" attaches the image to the next message for vision-capable models, or the sidecar metadata for others

Interactive figures (Plotly, Bokeh HTML) are rendered in a webview tab in a later phase (OQ-11).

### 5.13 Script contract and checker (D9)

The contract encodes the conventions you enforce by hand today. It is both a prompt (the agent is told the rules) and a checker (the kernel verifies them with AST analysis). Two rules are confirmed as the ones you currently enforce manually and are hard failures by default; the rest are proposals pending OQ-6.

**Confirmed rule 1: every script is a command-line tool.**

- A script builds an `argparse` parser with a description and help text on every option, defines `main(args)`, and has an `if __name__ == "__main__":` guard.
- Every input (data path, parameter, output location, seed, plot options) is an option with a default. Defaults for paths are repo-relative.
- No input is hard-coded: a literal path or numeric constant that flows into a library call from `main` without passing through the parser is a violation.

Checker rules: `cli.parser-required`, `cli.help-text`, `cli.main-guard`, `cli.option-for-every-input`, `cli.no-absolute-paths`.

**Confirmed rule 2: shared logic lives in the library, not in scripts.**

- Computational code lives in an importable package (`analysis/` by default) as documented functions. Scripts are thin: parse arguments, call library functions, save outputs.
- A script may define only `main`, `build_parser`, and small private helpers (configurable: default under 15 lines, no numerical work). Anything else must be in the library.
- No two functions in the repo may implement the same logic. Before writing a function the agent must check the library index.

Checker rules:

| Rule | Detection |
|---|---|
| `lib.script-defines-logic` | A script defines a function outside the allowed set, or a helper exceeds the size limit or calls numerical libraries. |
| `lib.duplicate` | Two functions anywhere in `scripts/` or `analysis/` have the same normalized AST fingerprint (names and literals stripped, structure hashed). Hard failure. |
| `lib.near-duplicate` | Token-sequence similarity above a threshold (default 0.8) between two functions, or identical signature and name in a script and the library. Warning with both locations and a "promote to library" or "reuse" hint. |
| `lib.unused-twin` | A script calls a locally defined function while a library function with a matching name or signature exists. |

`contract.similar` exposes the same analysis on demand so the agent can run it before proposing an edit, and the UI offers "promote to library" as a refactor action that moves a function into `analysis/` and rewrites the call sites as a reviewable proposal.

**Proposed rules (pending OQ-6)**

3. Outputs go to `out/` under a subdirectory named after the script. Figures are saved with descriptive names. A run metadata JSON is written with the arguments used.
4. Random seeds are options with defaults.
5. Functions in modules listed under `contract.math_required` carry a `Math:` docstring section.
6. Library functions have docstrings with parameter units where applicable.

Each rule has a severity (`error` or `warn`) in `.analyst/project.toml`. Checker output is a list of diagnostics with file, line, rule id, and a fix hint, shown in the editor and fed back to the agent after each edit.

### 5.14 Documents

- **Markdown:** edited in the main editor, previewed live in a panel using `react-markdown` with `remark-math` and KaTeX. Figure embeds support both plain image paths and run references such as `{{fig: fit_decay/residuals}}` that resolve to the latest run, so notes stay current as analyses are rerun.
- **LaTeX:** `.tex` files compile with Tectonic (self-contained, fetches packages on demand) to PDF, shown in a `pdf.js` viewer panel. Compile on save with a debounce. Errors are parsed into editor diagnostics. SyncTeX click-to-source later. Compilation is always local.
- **Agent support:** `doc.insert_figure` and `doc.render` tools; a "report" skill that knows the document structure.

Each session gets a `notes.md` by default.

### 5.15 Terminal

`xterm.js` in the renderer over a `portable-pty` in the core, opened in the repo root with the session venv activated and the figure-capture environment set. On a remote target the pty runs `ssh -t` into the mirror directory with the same setup. Multiple terminals per session. This is the manual door into the same loop: anything run here produces figures and run records like an agent run.

### 5.16 Skills and configuration

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

Configuration layering, lowest to highest precedence: built-in defaults, `~/.analyst/config.toml`, `<repo>/.analyst/project.toml`, per-target overrides inside it, session overrides from the UI.

## 6. User interface

The layout follows Zed's shape so the muscle memory transfers, with analysis panels added.

```
┌──────────────────────────────────────────────────────────────────────────┐
│ [decay-analysis · lab-gpu ●] [beam-sim · local] [+]          model ▾  ⚙ │
├──────────┬──────────────────────────────────────┬────────────────────────┤
│ Project  │ Editor tabs: fitting.py | fit_decay.py│ Agent chat            │
│ Scripts  │ ─────────────────────────────────────│ mode: Build ▾         │
│ Library  │   def double_exp(t, A, tau1, ...):   │                       │
│ Runs     │   ∑ y = A e^{-t/τ₁} + B e^{-t/τ₂} ✓  │ [plan card]           │
│ Skills   │       return A * np.exp(-t/tau1) ... │ [diff card ✓ ✗]       │
│          │                                      │ [run card · lab-gpu]  │
│          ├──────────────────────────────────────┤                       │
│          │ Terminal | Run output | Diagnostics  │                       │
├──────────┴──────────────────────────────────────┴────────────────────────┤
│ Figures ◧ | Equations | Packages | Preview | (dockable, undockable)      │
└──────────────────────────────────────────────────────────────────────────┘
```

Panels and their primary interactions:

| Panel | Interactions |
|---|---|
| Session tabs | Switch, create, close, duplicate with a new conversation. Each shows its target and connection state and restores its full layout. |
| Project sidebar | File tree. Scripts view lists each script with its options parsed from the index and a form to run it with chosen arguments. Library view lists public functions with callers and "promote to library" candidates from the checker. Runs view with history, target, and filters. |
| Editor | CodeMirror 6 with Python syntax, Ruff diagnostics, hover and definitions from the language server on the target. Inline diff review of agent proposals. Code-lens for equations. Protected-region markers. |
| Agent chat | Streaming messages. Cards for plans, proposals, runs, package changes, MCP calls, and approvals. Mode switch. Model switch. Context indicator (tokens used, compaction state). Attach files, figures, or editor selections. |
| Figures | Section 5.12. |
| Equations | Section 5.11. |
| Packages | Installed list on the session's target, search and add, remove, sync, Python version. Pending agent requests show here too. |
| Preview | Markdown or PDF. Follows the active document. |
| Terminal / Run output / Diagnostics | Bottom dock. Run output shows structured runs with stdout, stderr, target, and artifact links. |

Docking uses `dockview`: any panel can be dragged into any dock group or into a floating group. Undocking into a separate OS window creates a second Tauri `WebviewWindow` hosting that panel, subscribed to the same session's event channel; this is what makes a figures-only window on a second monitor work.

Command palette and keybindings follow Zed's defaults where they apply. A Vim mode via CodeMirror's `@replit/codemirror-vim` is a later phase.

## 7. Storage layout

```
~/.analyst/                              (local machine)
  config.toml              providers, models, targets, global policy defaults, UI prefs
  skills/                  global skills
  mcp.json                 global MCP servers
  tooling/                 app-owned Python environment for the local target
  ssh/                     control sockets for multiplexed connections
  sessions/<id>/
    session.toml           repo path, target, env path, layout, active model
    conversations/<id>.jsonl
    index.sqlite           runs, artifacts, equations cache, checkpoints, capability records

~/.analyst/                              (each remote target)
  bin/uv
  tooling/                 kernel, SymPy, Ruff, language server
  runs/<run-id>/           stdout, stderr, status, artifact list (survive disconnects)

<repo>/                                  (local; mirrored to <remote_root>/<repo> on remote targets)
  .analyst/
    project.toml           contract, policy, data dirs, per-target overrides   (committed)
    skills/                project skills                                      (committed)
    mcp.json               project MCP servers                                  (committed)
    env.sh                 generated; figure capture env for terminals          (ignored)
  scripts/  analysis/  data/  out/  notes.md  pyproject.toml  uv.lock
```

`.analyst/project.toml`, `pyproject.toml`, and `uv.lock` are meant to be committed so a repo carries its own conventions and environment. Everything under `out/`, `.venv/`, and `.analyst/env.sh` is git-ignored by the template and excluded from sync.

## 8. Security and privacy

- No telemetry. No network calls except to configured model providers, configured MCP servers, SSH targets, package indexes during installs, and Tectonic's package fetches.
- API keys in the OS keychain. Never logged, never passed into the sandbox, never sent to a remote target, and never placed in MCP server environments unless explicitly configured for that server.
- SSH credentials are never handled by the app; it relies on the user's OpenSSH configuration and agent.
- The chat UI shows provider locality (local or remote) at all times. A per-target "data directories" list controls what the agent can read; nothing outside the repo and that list is readable by tools or by the sandbox.
- MCP servers run as separate processes with their own configured permissions; their tools go through the same policy engine as built-ins.
- Transcripts stay local. Export is explicit.

## 9. Cross-platform

Tauri builds for macOS, Linux, and Windows from one codebase. Sandboxing is available on macOS and Linux locally and on Linux remote hosts; Windows users are expected to run against a Linux target for sandboxed execution and get a clear badge otherwise. `uv`, `rg`, and Tectonic are bundled or fetched per platform. Windows uses the built-in OpenSSH client without connection multiplexing and the `tar` sync fallback when `rsync` is absent.

## 10. Testing and evaluation (D15)

- **Unit and integration:** core crates with `cargo test`, kernel with `pytest`, IPC contract tests generated from the `protocol` crate, sandbox tests that assert denied writes and denied network, SSH tests against a containerized `sshd` covering bootstrap, sync, detached runs, and reconnect.
- **Agent evaluations, model-agnostic by design.** A suite of representative analysis tasks (fit a model, process a dataset, produce a figure, refactor a script to the contract, extend an existing library function instead of duplicating it) with automatic graders that inspect only the resulting repo and runs: contract passes, run succeeds, expected artifacts exist, declared equations verify, no edits outside the declared scope, no duplicate logic introduced. The harness takes any configured provider and model and emits a scorecard: task success rate, contract compliance, equation verification rate, unwanted-edit rate, tool-call error rate, steps and tokens to completion. No task or grader references a model; the suite is a regression test for prompts and tools and a compatibility report for whichever models you point it at.
- **UI:** Playwright against the renderer with a fake core for determinism; Tauri's WebDriver harness for smoke tests on each platform.

## 11. Repository structure and build order

```
apps/desktop/
  src-tauri/           Tauri shell: commands, events, window management (thin glue over crates/core)
  src/                 React renderer
crates/protocol/       IPC and kernel message types; TypeScript bindings generated with specta
crates/core/           sessions, agent loop, policy, providers, tools, MCP, targets, sync, storage
crates/cli/            headless harness for phase 0 and evals
python/kernel/         analyst_kernel (runs, index, equations, contract) and analyst_mpl
evals/                 task suite, graders, scorecard runner
docs/
```

Build order follows the roadmap in the vision document. Phase 0 produces `crates/core` (without UI glue), `python/kernel`, the local and SSH transports, and the CLI harness. That phase answers the riskiest questions (remote sync and reconnect behavior, sandbox behavior on real hosts, equation derivation coverage, and how the loop performs across several models from different providers) before any interface work.
