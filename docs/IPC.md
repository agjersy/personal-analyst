# Desktop IPC Contract

*Status: phase 1 baseline. Normative for `apps/desktop/src-tauri` (Rust) and `apps/desktop/src` (React). Shared types live in `crates/protocol/src/ui.rs` with `ts-rs` export to `apps/desktop/src/ipc/generated/`; the TypeScript shapes below are what that export must produce. Kernel-level types (ScriptInfo, LibFunction, RunMeta, Artifact, FigureSidecar, Equation, Diagnostic, SimilarPair) are the ones already defined in [PROTOCOL.md](PROTOCOL.md) and are re-exported unchanged.*

## 1. Shape of the boundary

- The renderer calls **commands** (Tauri `invoke`) for request/response. Every command takes a JSON object of named arguments and returns a JSON value or a structured error `{ code: string, message: string, detail?: unknown }`. Error codes: `not_found`, `conflict`, `policy_denied`, `kernel_offline`, `target_offline`, `invalid`, `io`, `internal`.
- The backend pushes **events** on a per-session **channel** (`tauri::ipc::Channel<SessionEvent>`) that the renderer opens with `session_subscribe`. One channel per open session per window. Events are ordered per session.
- Long operations (bootstrap, sync, package changes) return a `task_id` immediately and report through `task_log` and `task_done` events.
- The renderer never touches the filesystem, processes, or network directly. In a plain browser (no Tauri) the renderer runs against `MockBackend`, which implements the same interface in memory; this is how the frontend is developed and tested headlessly.
- All paths crossing the boundary are repo-relative with forward slashes unless the field is named `*_abs`.

The renderer-side interface (`src/ipc/backend.ts`):

```ts
export interface Backend {
  invoke<K extends keyof Commands>(cmd: K, args: Commands[K]["args"]): Promise<Commands[K]["result"]>;
  subscribe(session: SessionId, onEvent: (e: SessionEvent) => void): Promise<() => void>;   // returns unsubscribe
}
```

`TauriBackend` maps `invoke` to `@tauri-apps/api/core` `invoke` and `subscribe` to `session_subscribe` with a `Channel`. `MockBackend` is section 6.

## 2. Types

```ts
type SessionId = string; type ConversationId = string; type RunId = string;
type TerminalId = string; type ProposalId = string; type TaskId = string; type TurnId = string;
type Mode = "plan" | "build" | "run";
type KernelState = "stopped" | "starting" | "ready" | "error" | "offline";

interface AppInfo { version: string; home_abs: string; platform: "macos" | "linux" | "windows"; dev_mode: boolean }

interface SessionSummary {
  id: SessionId; name: string;            // name = repo directory name unless renamed
  repo_abs: string; target: string;       // target name, "local" by default
  model: string | null;                   // "provider/id"
  created_at: string; last_opened_at: string; open: boolean;
}
interface PolicyView {
  edits: "review" | "auto-new-files" | "auto"; runs: "auto" | "ask";
  packages: "ask" | "auto" | "deny"; shell: "ask" | "auto" | "deny";
  network: "deny" | "ask" | "allow"; git: "ask" | "auto" | "deny";
  protected: string[]; max_steps: number;
}
interface ConversationSummary { id: ConversationId; title: string; model: string | null; mode: Mode; created_at: string; turns: number; busy: boolean }
interface SessionInfo extends SessionSummary {
  policy: PolicyView; kernel: KernelState; sandbox: string | null; python_abs: string | null;
  conversations: ConversationSummary[]; active_conversation: ConversationId;
  target_kind: "local" | "ssh"; target_host: string | null;
}

interface FileEntry { path: string; name: string; kind: "file" | "dir"; size: number | null; children: FileEntry[] | null }  // children null = not expanded
interface FileContent { path: string; content: string; hash: string }      // hash = sha256 hex of bytes
interface WriteResult { hash: string }
interface SearchHit { path: string; line: number; col: number; text: string }

interface IndexResult { scripts: ScriptInfo[]; library: LibFunction[] }

interface RunView {
  meta: RunMeta; target: string; artifacts: Artifact[];
  stdout_tail: string; stderr_tail: string;     // last 4 KiB each; full text via run_get
}
interface FigureView {
  run_id: RunId; script: string; path: string;  // repo-relative, e.g. out/fit_decay/<run>/figures/residuals.png
  name: string; format: "png" | "svg" | "pdf"; sidecar: FigureSidecar | null; created_at: string; size: number;
}
interface FigureGroup { run: RunView; figures: FigureView[] }
interface FigureData { mime: string; base64: string }

interface EqCheck {
  status: "agree" | "mismatch" | "unavailable"; trials: number;
  max_abs_err: number | null; max_rel_err: number | null;
  counterexample: { inputs: Record<string, number>; expected: number; actual: number } | null; reason: string | null;
}
interface PackageInfo { name: string; version: string; locked: boolean }
interface ModelEntry { name: string; provider: string; id: string; kind: "openai-compatible" | "anthropic"; local: boolean;
                       capabilities: { context_window: number; tools: "native" | "text-protocol" | "none"; vision: boolean } | null }
interface TargetInfo { name: string; kind: "local" | "ssh"; host: string | null; remote_root: string | null; sandbox: string; data_dirs: string[]; bootstrapped: boolean | null }

interface ProposalView {
  id: ProposalId; conversation: ConversationId;
  kind: "edit" | "create" | "delete" | "package" | "shell" | "commit";
  path: string | null; summary: string;
  original: string | null; proposed: string | null;   // full file contents for edit/create (original null for create)
  diff: string | null;                                // unified diff for display
  command: string | null; packages: string[] | null; message: string | null;
  in_scope: boolean; reason: "review" | "ask" | "out_of_scope" | "protected";
}
type ProposalDecision =
  | { decision: "accept"; content?: string }   // content: the file as it should be written after per-hunk review; omitted = proposed as is
  | { decision: "reject"; reason?: string };

type Attachment =
  | { kind: "file"; path: string }
  | { kind: "selection"; path: string; start_line: number; end_line: number; text: string }
  | { kind: "figure"; path: string };

// Transcript events are PROTOCOL.md section 5, re-exported as TranscriptEvent.
```

## 3. Events

```ts
type SessionEvent =
  | { type: "agent_text"; conversation: ConversationId; turn: TurnId; delta: string }
  | { type: "agent_tool_call"; conversation: ConversationId; turn: TurnId; id: string; name: string; arguments: unknown }
  | { type: "agent_tool_result"; conversation: ConversationId; turn: TurnId; id: string; ok: boolean; summary: string }
  | { type: "proposal"; proposal: ProposalView }
  | { type: "proposal_resolved"; proposal_id: ProposalId; decision: "accept" | "reject" | "auto"; by: "user" | "policy" }
  | { type: "turn_started"; conversation: ConversationId; turn: TurnId }
  | { type: "turn_done"; conversation: ConversationId; turn: TurnId; summary: { text: string; files_changed: string[]; runs: RunId[]; figures: string[] } }
  | { type: "turn_error"; conversation: ConversationId; turn: TurnId; message: string }
  | { type: "usage"; conversation: ConversationId; input_tokens: number; output_tokens: number; context_window: number; context_used: number }
  | { type: "run_started"; run: RunView }
  | { type: "run_output"; run_id: RunId; stream: "stdout" | "stderr"; chunk: string }
  | { type: "run_artifact"; run_id: RunId; artifact: Artifact; figure: FigureView | null }
  | { type: "run_done"; run: RunView }
  | { type: "diagnostics"; paths: string[]; diagnostics: Diagnostic[] }     // replaces diagnostics for the listed paths
  | { type: "files_changed"; paths: string[] }                               // created, modified, or deleted on disk by anyone
  | { type: "index_changed" }
  | { type: "terminal_data"; terminal_id: TerminalId; data: string }        // UTF-8 text; invalid sequences replaced
  | { type: "terminal_exit"; terminal_id: TerminalId; code: number | null }
  | { type: "kernel_state"; state: KernelState; message: string | null }
  | { type: "packages_changed" }
  | { type: "task_log"; task_id: TaskId; line: string }
  | { type: "task_done"; task_id: TaskId; ok: boolean; message: string | null }
  | { type: "session_changed"; info: SessionInfo };                         // after target, model, mode, or policy changes
```

## 4. Commands

Argument names are exact. `session` is always a `SessionId`.

| command | args | result | notes |
|---|---|---|---|
| `app_info` | `{}` | `AppInfo` | |
| `sessions_list` | `{}` | `SessionSummary[]` | newest first |
| `session_create` | `{ repo_abs, target?, model?, init? }` | `SessionInfo` | `init: true` runs the `analyst init` template on an empty directory; opens the session |
| `session_open` | `{ session }` | `SessionInfo` | starts the kernel lazily; idempotent |
| `session_close` | `{ session }` | `null` | suspends; kernel shut down; terminals closed |
| `session_info` | `{ session }` | `SessionInfo` | |
| `session_rename` | `{ session, name }` | `SessionSummary` | |
| `session_subscribe` | `{ session, channel }` | `string` | subscription id; events flow until `session_unsubscribe` or window close |
| `session_unsubscribe` | `{ subscription }` | `null` | |
| `session_set_target` | `{ session, target }` | `TaskId` | re-binds env, syncs; `session_changed` on completion |
| `session_set_policy` | `{ session, policy: Partial<PolicyView> }` | `PolicyView` | session override, not written to project.toml |
| `layout_get` | `{ session }` | `string \| null` | dockview JSON |
| `layout_save` | `{ session, layout }` | `null` | |
| `fs_tree` | `{ session, path?, depth? }` | `FileEntry[]` | respects .gitignore; `out/` shown but collapsed |
| `fs_read` | `{ session, path }` | `FileContent` | text files only; binary -> `invalid` |
| `fs_read_binary` | `{ session, path }` | `FigureData` | any file under the repo, base64 |
| `fs_write` | `{ session, path, content, expected_hash? }` | `WriteResult` | `conflict` if the file's hash differs from `expected_hash` |
| `fs_create` | `{ session, path, is_dir }` | `null` | |
| `fs_delete` | `{ session, path }` | `null` | moves to the OS trash when available |
| `fs_rename` | `{ session, from, to }` | `null` | |
| `fs_search` | `{ session, pattern, glob?, max_results? }` | `SearchHit[]` | ripgrep |
| `index_get` | `{ session }` | `IndexResult` | cached; refreshed on `files_changed` of .py files |
| `check_run` | `{ session, paths? }` | `Diagnostic[]` | contract + lint; also emits `diagnostics` |
| `run_start` | `{ session, script, args, network? }` | `RunView` | syncs mirror on remote targets first |
| `run_cancel` | `{ session, run_id }` | `null` | |
| `runs_list` | `{ session, limit? }` | `RunView[]` | newest first |
| `run_get` | `{ session, run_id }` | `RunView & { stdout: string; stderr: string }` | |
| `out_list` | `{ session, path?, run_id? }` | kernel `out.list` result | |
| `out_read` | `{ session, path, max_bytes? }` | kernel `out.read` result | |
| `figures_list` | `{ session, limit? }` | `FigureGroup[]` | newest run first, includes terminal runs found under out/ |
| `figure_data` | `{ session, path }` | `FigureData` | pulls from a remote target on first access |
| `eq_extract` | `{ session, path }` | `Equation[]` | |
| `eq_check` | `{ session, path, function }` | `EqCheck` | |
| `env_packages` | `{ session }` | `PackageInfo[]` | |
| `env_add` | `{ session, packages }` | `TaskId` | `packages_changed` on completion |
| `env_remove` | `{ session, packages }` | `TaskId` | |
| `env_sync` | `{ session }` | `TaskId` | |
| `conversation_new` | `{ session, title? }` | `ConversationSummary` | becomes active |
| `conversation_select` | `{ session, conversation }` | `null` | |
| `conversation_transcript` | `{ session, conversation }` | `TranscriptEvent[]` | |
| `chat_send` | `{ session, conversation, text, attachments? }` | `TurnId` | `invalid` if the conversation is busy |
| `chat_cancel` | `{ session, conversation }` | `null` | pending proposals are rejected |
| `chat_set_mode` | `{ session, conversation, mode }` | `null` | |
| `chat_set_model` | `{ session, conversation, model }` | `null` | |
| `proposal_respond` | `{ session, proposal_id, decision }` | `null` | `not_found` if already resolved |
| `models_list` | `{}` | `ModelEntry[]` | from global config plus cached capabilities |
| `model_probe` | `{ model }` | `ModelEntry` | runs the capability probe |
| `targets_list` | `{}` | `TargetInfo[]` | |
| `target_bootstrap` | `{ target }` | `TaskId` | |
| `terminal_open` | `{ session, cols, rows }` | `TerminalId` | local: `$SHELL` in the repo with the venv on PATH and the figure-capture env; remote: `ssh -t` into the mirror with the same |
| `terminal_write` | `{ terminal_id, data }` | `null` | |
| `terminal_resize` | `{ terminal_id, cols, rows }` | `null` | |
| `terminal_close` | `{ terminal_id }` | `null` | |
| `dialog_pick_directory` | `{ title? }` | `string \| null` | native folder picker (absolute path) |

## 5. Behavior that both sides rely on

- **Editor saves are the user's.** `fs_write` writes immediately; no proposal. The agent sees user changes through the working-tree diff before its next turn.
- **Proposals.** In `review` mode the agent's turn is paused when a `proposal` event arrives and resumes on `proposal_respond`. The renderer shows an edit proposal in a merge view (original vs proposed) with per-hunk accept and reject; on accept it sends the resulting file content. The backend writes that content, reports to the model whether the user modified it, and continues. A `create` proposal has `original: null`. Out-of-scope or protected proposals carry `reason` so the card can say why approval is being asked even in auto mode.
- **Runs.** A run started from anywhere (chat, run form, terminal) produces `run_started`, `run_output`, `run_artifact`, `run_done`. Terminal runs are discovered through the `out/` watcher and appear with `run_started` and `run_done` together once their `run.json` is seen.
- **Figures.** `figure_data` returns bytes; the renderer renders PNG and SVG as `<img>` with a data URL. Figures from remote targets are pulled on first access and cached under the session's `artifacts/`.
- **Diagnostics.** The backend runs `check_run` automatically after an accepted proposal and after a `.py` file is saved from the editor, and pushes `diagnostics` for the affected paths.
- **Kernel lifecycle.** The kernel starts on `session_open` and on demand; `kernel_state` events report `starting`, `ready`, `offline` (remote unreachable), `error` (with message). Commands that need the kernel fail with `kernel_offline` or `target_offline` while it is not ready; the renderer disables the run form and run tools accordingly and keeps editing and reading working.
- **Layout.** The renderer saves dockview layout on change (debounced) and restores it on open; absence means the default layout.

## 6. MockBackend (renderer-only development)

Lives in `src/ipc/mock.ts` and is selected automatically when the Tauri API is not present. Requirements:

- An in-memory repo with: `.analyst/project.toml`, `analysis/__init__.py`, `analysis/models.py` (a `single_exp` with a `Math:` section), `scripts/decay_sim.py` (argparse with five options), `data/sample.csv`, `notes.md`.
- `index_get` returns the parsed shape of those scripts; `check_run` returns one warning and one error diagnostic on a deliberately flawed `scripts/bad.py`.
- `run_start` streams several `run_output` chunks over about one second, then a `run_artifact` with an SVG figure (inline generated), then `run_done` with exit 0; `args` containing `--fail` makes it exit 1 with a traceback on stderr.
- `chat_send` streams `agent_text`, then in build mode emits a `proposal` of kind `edit` on `analysis/models.py` (adds a function) and waits for `proposal_respond`; after accept it emits a `run_started`/`run_done` pair with a figure and a `turn_done`; in run mode it runs the script twice and finishes; in plan mode it streams text only. A text containing `error` produces `turn_error`.
- `terminal_open` echoes input and answers `ls` and `python --version` plausibly.
- `models_list` returns two entries (one local, one remote); `targets_list` returns `local` and one `ssh` target.
- Latency is simulated at 50 to 300 ms so loading states are visible.
