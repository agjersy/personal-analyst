// Command table from docs/IPC.md section 4. Argument names are exact (snake_case).
import type {
  AppInfo,
  Attachment,
  ConversationId,
  ConversationSummary,
  Diagnostic,
  EqCheck,
  Equation,
  FigureData,
  FigureGroup,
  FileContent,
  FileEntry,
  IndexResult,
  Mode,
  ModelEntry,
  OutListResult,
  OutReadResult,
  PackageInfo,
  PolicyView,
  ProposalDecision,
  ProposalId,
  RunId,
  RunView,
  SearchHit,
  SessionId,
  SessionInfo,
  SessionSummary,
  TargetInfo,
  TaskId,
  TerminalId,
  TranscriptEvent,
  TurnId,
  WriteResult,
} from "./types";

type S = { session: SessionId };

export interface Commands {
  app_info: { args: Record<string, never>; result: AppInfo };
  sessions_list: { args: Record<string, never>; result: SessionSummary[] };
  session_create: { args: { repo_abs: string; target?: string; model?: string; init?: boolean }; result: SessionInfo };
  session_open: { args: S; result: SessionInfo };
  session_close: { args: S; result: null };
  session_info: { args: S; result: SessionInfo };
  session_rename: { args: S & { name: string }; result: SessionSummary };
  session_subscribe: { args: S & { channel: unknown }; result: string };
  session_unsubscribe: { args: { subscription: string }; result: null };
  session_set_target: { args: S & { target: string }; result: TaskId };
  session_set_policy: { args: S & { policy: Partial<PolicyView> }; result: PolicyView };
  layout_get: { args: S; result: string | null };
  layout_save: { args: S & { layout: string }; result: null };
  fs_tree: { args: S & { path?: string; depth?: number }; result: FileEntry[] };
  fs_read: { args: S & { path: string }; result: FileContent };
  fs_read_binary: { args: S & { path: string }; result: FigureData };
  fs_write: { args: S & { path: string; content: string; expected_hash?: string }; result: WriteResult };
  fs_create: { args: S & { path: string; is_dir: boolean }; result: null };
  fs_delete: { args: S & { path: string }; result: null };
  fs_rename: { args: S & { from: string; to: string }; result: null };
  fs_search: { args: S & { pattern: string; glob?: string; max_results?: number }; result: SearchHit[] };
  index_get: { args: S; result: IndexResult };
  check_run: { args: S & { paths?: string[] }; result: Diagnostic[] };
  run_start: { args: S & { script: string; args: string[]; network?: boolean }; result: RunView };
  run_cancel: { args: S & { run_id: RunId }; result: null };
  runs_list: { args: S & { limit?: number }; result: RunView[] };
  run_get: { args: S & { run_id: RunId }; result: RunView & { stdout: string; stderr: string } };
  out_list: { args: S & { path?: string; run_id?: RunId }; result: OutListResult };
  out_read: { args: S & { path: string; max_bytes?: number }; result: OutReadResult };
  figures_list: { args: S & { limit?: number }; result: FigureGroup[] };
  figure_data: { args: S & { path: string }; result: FigureData };
  eq_extract: { args: S & { path: string }; result: Equation[] };
  eq_check: { args: S & { path: string; function: string }; result: EqCheck };
  env_packages: { args: S; result: PackageInfo[] };
  env_add: { args: S & { packages: string[] }; result: TaskId };
  env_remove: { args: S & { packages: string[] }; result: TaskId };
  env_sync: { args: S; result: TaskId };
  conversation_new: { args: S & { title?: string }; result: ConversationSummary };
  conversation_select: { args: S & { conversation: ConversationId }; result: null };
  conversation_transcript: { args: S & { conversation: ConversationId }; result: TranscriptEvent[] };
  chat_send: { args: S & { conversation: ConversationId; text: string; attachments?: Attachment[] }; result: TurnId };
  chat_cancel: { args: S & { conversation: ConversationId }; result: null };
  chat_set_mode: { args: S & { conversation: ConversationId; mode: Mode }; result: null };
  chat_set_model: { args: S & { conversation: ConversationId; model: string }; result: null };
  proposal_respond: { args: S & { proposal_id: ProposalId; decision: ProposalDecision }; result: null };
  models_list: { args: Record<string, never>; result: ModelEntry[] };
  model_probe: { args: { model: string }; result: ModelEntry };
  targets_list: { args: Record<string, never>; result: TargetInfo[] };
  target_bootstrap: { args: { target: string }; result: TaskId };
  terminal_open: { args: S & { cols: number; rows: number }; result: TerminalId };
  terminal_write: { args: { terminal_id: TerminalId; data: string }; result: null };
  terminal_resize: { args: { terminal_id: TerminalId; cols: number; rows: number }; result: null };
  terminal_close: { args: { terminal_id: TerminalId }; result: null };
  dialog_pick_directory: { args: { title?: string }; result: string | null };
}

export type CommandName = keyof Commands;
