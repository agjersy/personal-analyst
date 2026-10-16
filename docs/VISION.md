# Personal Analyst — Product Vision

*Working name. Status: draft for review. Companion documents: [ARCHITECTURE.md](ARCHITECTURE.md), [OPEN-QUESTIONS.md](OPEN-QUESTIONS.md).*

## 1. One line

A desktop workbench for scientific, technical, and numerical analysis in which an AI agent writes reusable Python analysis tools under your supervision, and the application makes verifying that work (code, math, figures) fast.

## 2. Where this comes from

The current workflow is an analysis git repo, an agent session in Zed, and open-weight models served remotely by Ollama. You steer the agent to write targeted Python scripts, verify them by reading the code and inspecting Matplotlib output, sometimes edit the code yourself, and hold the scripts to a house standard: reusable functions and a command-line interface so every tool can be run by hand from the terminal to play with inputs.

The workflow is sound. It is the tooling around it that breaks or slows down:

| Pain point | What actually happens |
|---|---|
| Verifying math buried in code | Reconstructing the equations from NumPy expressions is slow and error-prone. The thing you need to check is the math, but what you are shown is the implementation. |
| Hand edits versus agent edits | Editing directly is often faster than explaining a change, but the editor and the agent are separate contexts and the agent can overwrite or misread what you changed. |
| Unwanted agent actions | The agent modifies files or changes behavior you did not ask about. Catching this costs attention on every turn. |
| Fragmented surfaces | Editor, terminal, image viewer, notes, package management, and model configuration are separate tools. Switching between analysis repos and sessions is manual. |
| Conventions enforced by hand | Reusability and CLI options have to be re-requested and re-checked in every session. |

The product keeps the workflow and removes the friction: one application, one agent loop tuned for analysis work, and verification surfaces built in.

## 3. Vision

**Personal Analyst is a verification-first analysis workbench.** Its job is not to make the agent do more. Its job is to make everything the agent does cheap to check and easy to control, so that you can move fast without giving up understanding of your own analysis.

### Principles

1. **Show, don't ask for trust.** Every agent output is presented in the form that is easiest to verify: diffs for code, rendered equations for math, figures for results, logs and exit codes for runs. Where a check can be automated (does the declared equation match the code numerically? does the script satisfy the contract?), the app runs it and shows the result.
2. **You own the code.** Agent edits are proposals until accepted, with the strictness configurable per session. Files and directories can be protected. Every agent turn is checkpointed so any change is one click from undone. The agent never touches anything outside the scope it declared.
3. **Tools, not transcripts.** The product of a session is a repository of reusable, CLI-driven analysis tools, not a chat log or a notebook. The agent is held to a script contract that the app checks automatically.
4. **Three doors into the same loop.** Agent-driven, editor-driven, and terminal-driven work all see the same runs, figures, and equations. You can enter at whichever is fastest for the task at hand.
5. **Model agnostic, local first.** Open-weight models through Ollama (or any OpenAI-compatible server) and frontier models through provider API keys, with the same agent loop. The application itself has no cloud dependency and no telemetry. The only data that leaves the machine is what you send to the model provider you chose, and the app tells you which that is.
6. **Contained by default.** Code the agent runs is sandboxed: scoped filesystem, no network unless you grant it, bounded time and resources.

## 4. Who it is for

- **Primary:** you, a single user who does quantitative analysis, reads code, and reads math. Scientist or engineer. Comfortable with git, Python, and the terminal.
- **Secondary (later):** others with the same profile who want a self-hosted, model-agnostic alternative to notebook-centric assistants.
- **Not for:** non-programmers, BI dashboards, or notebook-first data science in the first version.

## 5. What it is and is not

**It is:** a Zed-like editor, an analysis-specialized agent, a Python runtime and package manager, a figure viewer, an equation lens, a document preview and compiler, and a plugin surface for external engines, in one window with sessions as tabs.

**It is not:** a Jupyter replacement, a general-purpose IDE, a BI tool, or a hosted service.

## 6. Core experiences

These walkthroughs are the acceptance bar for the first usable version.

**Start a session.** Open or create an analysis repo. The app creates (or adopts) a Python virtual environment with `uv`, indexes the repo's scripts and their CLI options, and asks which model to use. The session appears as a tab with its own conversation, environment, layout, and run history.

**Ask for an analysis.** "Fit the decay curves in `data/run3` with a double exponential and plot the residuals." The agent shows a short plan naming the files it intends to create or change. It proposes `analysis/fitting.py` (the model function and fitter) and `scripts/fit_decay.py` (a thin CLI). Each proposal appears as a diff card. You accept. The agent runs the script in the sandbox. The figures appear as tabs in the Figures panel. The Equations panel shows the double-exponential model in rendered LaTeX, derived from the function body, with a green badge: the declared equation was evaluated against the code on random inputs and agrees.

**Verify and tweak.** You open `fitting.py` in the editor, change the initial guesses, and save. In the embedded terminal you run `python scripts/fit_decay.py --tau0 1.2`. The new figure appears beside the previous run for comparison. The agent's next turn sees your edit because the app diffs the working tree before each turn.

**Explore.** You switch the session to run-only mode and ask the agent to sweep `--tau0` over a range and summarize the fit quality. It can run existing tools with new arguments but cannot edit code. Results and figures are grouped under one run set.

**Write up.** Session notes live in `notes.md` with rendered math and embedded figures that reference the latest run. For a formal write-up, `report.tex` compiles to PDF in a preview panel, and the agent can insert figures and equations into it on request.

**Switch.** A second session tab holds a different repo, its own virtual environment, its own conversation, and its own layout. Switching restores everything, including undocked figure windows on a second monitor.

## 7. Feature pillars and priority

| Pillar | First version | Later |
|---|---|---|
| Agent loop for analysis | Plan, propose, run, verify cycle. Review-gated edits. Script contract enforcement. Run-only mode. Configurable policies and skills. | Multi-model roles (planner, coder, reviewer). Parallel sub-tasks. |
| Sandboxed Python runtime | Per-session venv via `uv`. OS-level sandbox on macOS and Linux. Structured run output with figure capture. | Container backend. Remote execution over SSH. Windows sandbox. |
| Editor | Monaco-based editor with Python diagnostics, inline diff review, protected regions. | Vim mode. LSP hover and go-to-definition. |
| Figure viewer | Dockable and undockable tabs. Run history. Side-by-side comparison. Send a figure to a vision-capable model. | Interactive Plotly and Bokeh output. Figure annotation. |
| Equation lens | LaTeX derived from pure-expression functions. Declared equations checked numerically against code. Click to jump to source. | Algorithm-level pseudocode views. Unit and dimension checks. |
| Sessions | Tabs for repo plus venv plus conversation plus layout. Checkpoints and one-click revert. | Session templates. Archival and search across sessions. |
| Packages | Visible, editable package list. Agent installs require approval. Lockfile aware. | Conda-forge backend for packages that need it. |
| Documents | Markdown with KaTeX preview. LaTeX compile to PDF with embedded viewer. | SyncTeX click-to-source. Report templates. |
| External tools | MCP client for stdio and HTTP servers (Wolfram, MATLAB, Julia, databases, literature search). | Marketplace-style discovery. |
| Models | Any OpenAI-compatible endpoint plus Anthropic. API keys in the OS keychain. Per-session model choice. | Automatic capability probing. Cost and token budgets. |

## 8. Success criteria

- **Time to verify.** For any function the agent writes, its equation is visible in rendered form without reading the implementation, and a mismatch between declared math and code is flagged automatically.
- **No unwanted edits.** No file changes on disk without consent at the level you configured, and every agent change is revertible in one action.
- **Reuse by construction.** Every analysis the agent produces is runnable from a terminal with `--help` and satisfies the script contract.
- **Switching is free.** Changing session restores editor state, figures, terminal, and conversation in under two seconds.
- **Model freedom.** The full loop works with at least one tool-calling open-weight model on Ollama and at least two frontier providers, with no code changes to switch.

## 9. Non-goals for the first version

Jupyter notebooks as a first-class format. Analysis languages other than Python (external engines are reachable through MCP). Multi-user collaboration. Cloud sync. Mobile. Automated model selection or AutoML.

## 10. Roadmap

| Phase | Goal | Exit criterion |
|---|---|---|
| 0. Headless spike | Agent loop, kernel, sandbox, and script contract as a CLI harness with no UI. Validate tool-calling reliability of candidate models. | Three representative analysis tasks complete end to end with one Ollama model and one frontier model. |
| 1. Workbench | Sessions, editor, chat, runner, figure viewer, terminal, packages panel. | You can do a full day's analysis in the app without opening Zed. |
| 2. Verification | Equation lens with consistency checks, contract checker, checkpoints and revert, run comparison. | Equation badges and diff review cover the walkthroughs in section 6. |
| 3. Extension and documents | MCP servers, skills, markdown and LaTeX preview and compile, report assembly. | An external engine is used in an analysis and a PDF report is produced from the app. |
| 4. Polish | Undockable windows, keybindings and Vim mode, performance, Windows support. | Daily-driver quality. |

See [OPEN-QUESTIONS.md](OPEN-QUESTIONS.md) for the decisions needed before phase 0 begins.
