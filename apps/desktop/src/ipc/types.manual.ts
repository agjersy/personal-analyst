// Reference shapes written from docs/IPC.md sections 2 and 3 and the kernel types in
// docs/PROTOCOL.md. Not used at runtime: types.compat.test.ts proves these stay
// interchangeable with the ts-rs export in ./generated, which src/ipc/types.ts re-exports.

export type SessionId = string;
export type ConversationId = string;
export type RunId = string;
export type TerminalId = string;
export type ProposalId = string;
export type TaskId = string;
export type TurnId = string;
export type Mode = "plan" | "build" | "run";
export type KernelState = "stopped" | "starting" | "ready" | "error" | "offline";

// ---- kernel-level types (PROTOCOL.md, re-exported unchanged) ----

export interface OptionInfo {
  flags: string[];
  dest: string;
  type: string | null;
  default: unknown;
  help: string | null;
  required: boolean;
  choices: unknown[] | null;
  action: string | null;
}

export interface ScriptInfo {
  path: string;
  name: string;
  description: string | null;
  has_main: boolean;
  has_main_guard: boolean;
  options: OptionInfo[];
}

export interface LibFunction {
  module: string;
  qualname: string;
  path: string;
  line: number;
  signature: string;
  doc_summary: string | null;
  has_math: boolean;
  callers: { path: string; line: number }[];
}

export type RunStatus = "running" | "done" | "cancelled" | "timeout" | "failed";

export interface RunMeta {
  run_id: string;
  script: string;
  args: string[];
  status: RunStatus;
  exit_code: number | null;
  started_at: string;
  finished_at: string | null;
  out_dir: string;
  sandbox: string;
  tree_hash: string | null;
  pid: number | null;
  host: string;
}

export interface FigureSidecar {
  run_id: string;
  script: string;
  args: string[];
  title: string | null;
  axes: { title: string | null; xlabel: string | null; ylabel: string | null }[];
  created_at: string;
  width_in: number;
  height_in: number;
  dpi: number;
}

export interface Artifact {
  kind: "figure" | "table" | "array" | "json" | "text" | "file";
  path: string;
  size: number;
  created_at: string;
  sidecar?: FigureSidecar;
  rendered: boolean;
}

export interface Equation {
  function: string;
  qualname: string;
  line: number;
  tier: "derived" | "declared" | "latex" | "none";
  latex: string | null;
  source_expr: string | null;
  status: "ok" | "unsupported" | "error";
  reason: string | null;
}

export interface Diagnostic {
  path: string;
  line: number;
  col: number;
  rule: string;
  severity: "error" | "warn";
  message: string;
  hint: string | null;
  related: { path: string; line: number }[];
}

export interface SimilarPair {
  kind: "duplicate" | "near-duplicate" | "unused-twin";
  score: number;
  a: { path: string; qualname: string; line: number };
  b: { path: string; qualname: string; line: number };
}

export interface OutListResult {
  entries: { path: string; size: number; kind: Artifact["kind"]; mtime: string }[];
}

export interface OutReadResult {
  kind: Artifact["kind"];
  preview: string;
  truncated: boolean;
  summary: object | null;
}

export type TranscriptEvent =
  | { ts: string; type: "user"; text: string; attachments?: string[] }
  | { ts: string; type: "assistant"; text: string }
  | { ts: string; type: "tool_call"; id: string; name: string; arguments: unknown }
  | { ts: string; type: "tool_result"; id: string; ok: boolean; result: unknown; truncated?: boolean }
  | {
      ts: string;
      type: "proposal";
      id: string;
      kind: "edit" | "create" | "delete" | "package" | "shell" | "commit";
      summary: string;
      diff?: string;
    }
  | { ts: string; type: "decision"; proposal_id: string; decision: "accept" | "reject" | "auto"; by: "user" | "policy" }
  | { ts: string; type: "run"; run_id: string; script: string; args: string[]; target: string }
  | { ts: string; type: "checkpoint"; ref: string }
  | { ts: string; type: "summary"; text: string; files_changed: string[]; runs: string[]; figures: string[] }
  | { ts: string; type: "usage"; input_tokens: number; output_tokens: number; model: string };

// ---- IPC.md section 2 ----

export interface AppInfo {
  version: string;
  home_abs: string;
  platform: "macos" | "linux" | "windows";
  dev_mode: boolean;
}

export interface SessionSummary {
  id: SessionId;
  name: string;
  repo_abs: string;
  target: string;
  model: string | null;
  created_at: string;
  last_opened_at: string;
  open: boolean;
}

export interface PolicyView {
  edits: "review" | "auto-new-files" | "auto";
  runs: "auto" | "ask";
  packages: "ask" | "auto" | "deny";
  shell: "ask" | "auto" | "deny";
  network: "deny" | "ask" | "allow";
  git: "ask" | "auto" | "deny";
  protected: string[];
  max_steps: number;
}

export interface ConversationSummary {
  id: ConversationId;
  title: string;
  model: string | null;
  mode: Mode;
  created_at: string;
  turns: number;
  busy: boolean;
}

export interface SessionInfo extends SessionSummary {
  policy: PolicyView;
  kernel: KernelState;
  sandbox: string | null;
  python_abs: string | null;
  conversations: ConversationSummary[];
  active_conversation: ConversationId;
  target_kind: "local" | "ssh";
  target_host: string | null;
}

export interface FileEntry {
  path: string;
  name: string;
  kind: "file" | "dir";
  size: number | null;
  children: FileEntry[] | null;
}

export interface FileContent {
  path: string;
  content: string;
  hash: string;
}

export interface WriteResult {
  hash: string;
}

export interface SearchHit {
  path: string;
  line: number;
  col: number;
  text: string;
}

export interface IndexResult {
  scripts: ScriptInfo[];
  library: LibFunction[];
}

export interface RunView {
  meta: RunMeta;
  target: string;
  artifacts: Artifact[];
  stdout_tail: string;
  stderr_tail: string;
}

export interface FigureView {
  run_id: RunId;
  script: string;
  path: string;
  name: string;
  format: "png" | "svg" | "pdf";
  sidecar: FigureSidecar | null;
  created_at: string;
  size: number;
}

export interface FigureGroup {
  run: RunView;
  figures: FigureView[];
}

export interface FigureData {
  mime: string;
  base64: string;
}

export interface EqCheck {
  status: "agree" | "mismatch" | "unavailable";
  trials: number;
  max_abs_err: number | null;
  max_rel_err: number | null;
  counterexample: { inputs: Record<string, number>; expected: number; actual: number } | null;
  reason: string | null;
}

export interface PackageInfo {
  name: string;
  version: string;
  locked: boolean;
}

export interface ModelEntry {
  name: string;
  provider: string;
  id: string;
  kind: "openai-compatible" | "anthropic";
  local: boolean;
  capabilities: { context_window: number; tools: "native" | "text-protocol" | "none"; vision: boolean } | null;
}

export interface TargetInfo {
  name: string;
  kind: "local" | "ssh";
  host: string | null;
  remote_root: string | null;
  sandbox: string;
  data_dirs: string[];
  bootstrapped: boolean | null;
}

export interface ProposalView {
  id: ProposalId;
  conversation: ConversationId;
  kind: "edit" | "create" | "delete" | "package" | "shell" | "commit";
  path: string | null;
  summary: string;
  original: string | null;
  proposed: string | null;
  diff: string | null;
  command: string | null;
  packages: string[] | null;
  message: string | null;
  in_scope: boolean;
  reason: "review" | "ask" | "out_of_scope" | "protected";
}

export type ProposalDecision =
  | { decision: "accept"; content?: string }
  | { decision: "reject"; reason?: string };

export type Attachment =
  | { kind: "file"; path: string }
  | { kind: "selection"; path: string; start_line: number; end_line: number; text: string }
  | { kind: "figure"; path: string };

export interface IpcError {
  code: "not_found" | "conflict" | "policy_denied" | "kernel_offline" | "target_offline" | "invalid" | "io" | "internal";
  message: string;
  detail?: unknown;
}

// ---- IPC.md section 3 ----

export interface TurnSummary {
  text: string;
  files_changed: string[];
  runs: RunId[];
  figures: string[];
}

export type SessionEvent =
  | { type: "agent_text"; conversation: ConversationId; turn: TurnId; delta: string }
  | { type: "agent_tool_call"; conversation: ConversationId; turn: TurnId; id: string; name: string; arguments: unknown }
  | { type: "agent_tool_result"; conversation: ConversationId; turn: TurnId; id: string; ok: boolean; summary: string }
  | { type: "proposal"; proposal: ProposalView }
  | { type: "proposal_resolved"; proposal_id: ProposalId; decision: "accept" | "reject" | "auto"; by: "user" | "policy" }
  | { type: "turn_started"; conversation: ConversationId; turn: TurnId }
  | { type: "turn_done"; conversation: ConversationId; turn: TurnId; summary: TurnSummary }
  | { type: "turn_error"; conversation: ConversationId; turn: TurnId; message: string }
  | {
      type: "usage";
      conversation: ConversationId;
      input_tokens: number;
      output_tokens: number;
      context_window: number;
      context_used: number;
    }
  | { type: "run_started"; run: RunView }
  | { type: "run_output"; run_id: RunId; stream: "stdout" | "stderr"; chunk: string }
  | { type: "run_artifact"; run_id: RunId; artifact: Artifact; figure: FigureView | null }
  | { type: "run_done"; run: RunView }
  | { type: "diagnostics"; paths: string[]; diagnostics: Diagnostic[] }
  | { type: "files_changed"; paths: string[] }
  | { type: "index_changed" }
  | { type: "terminal_data"; terminal_id: TerminalId; data: string }
  | { type: "terminal_exit"; terminal_id: TerminalId; code: number | null }
  | { type: "kernel_state"; state: KernelState; message: string | null }
  | { type: "packages_changed" }
  | { type: "task_log"; task_id: TaskId; line: string }
  | { type: "task_done"; task_id: TaskId; ok: boolean; message: string | null }
  | { type: "session_changed"; info: SessionInfo };
